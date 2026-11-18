// ported from N:rustc_type_ir/src/elaborate.rs @8d1a764
// ported from N:rustc_middle/src/ty/predicate.rs @8d1a764
use super::*;
use crate::infer::fresh_args::ArgFolder;
use crate::program::Program;
use crate::tcx::Tcx;
use crate::types::{Types, RegionKind, RegionId, BoundVar, BoundTy, TyKind, TyId, UniverseIndex};
use crate::types::fold::{TypeFolder, super_fold_ty, super_fold_const};
use crate::consts::ConstKind;
use crate::borrowck::implied_bounds::{ImpliedBounds, instantiate};
use crate::infer::outlives::components::{push_outlives_components, Component};
use crate::tcx::outlives::OutlivesPredicate;
use crate::fxhash::HashSet;

struct BoundVarShifter { offset: u32 }
impl TypeFolder for BoundVarShifter {
    fn fold_region(&mut self, tys: &Types, r: RegionId, depth: u32) -> RegionId {
        match tys.region_kind(r) {
            RegionKind::ReBound(d, var) if d == depth => tys.intern_region(RegionKind::ReBound(d, BoundVar(var.0 + self.offset))),
            _ => r,
        }
    }
    fn fold_ty(&mut self, tys: &Types, ty: TyId, depth: u32) -> TyId {
        if let TyKind::Bound(d, b) = *tys.kind(ty) {
            if d == depth { return tys.intern(TyKind::Bound(d, BoundTy { var: BoundVar(b.var.0 + self.offset), kind: b.kind })); }
        }
        super_fold_ty(tys, ty, self, depth)
    }
    fn fold_const(&mut self, tys: &Types, ct: Const, depth: u32) -> Const {
        if let crate::consts::ConstKind::Bound(d, b) = tys.const_kind(ct) {
            if d == depth { return tys.intern_const(crate::consts::ConstKind::Bound(d, crate::consts::BoundConst { var: BoundVar(b.var.0 + self.offset) })); }
        }
        super_fold_const(tys, ct, self, depth)
    }
}

impl Predicate {
    /// rustc anonymize_bound_vars: the kind with the binder's region names erased, the key
    /// two predicates are deduplicated by.
    pub fn anonymized(&self) -> (PredicateKind, Vec<BoundVariableKind>) {
        let vars = self.bound_vars.iter().map(|var| match var {
            BoundVariableKind::Region(_) => BoundVariableKind::Region(crate::types::BoundRegionKind::Anon),
            other => other.clone(),
        }).collect();
        (self.kind.clone(), vars)
    }
}

// Inspect without rebuilding: the ordinary folder allocates for dyn bindings and consts.
struct RegionFinder<F> { matches: F, found: bool }
impl<F: FnMut(RegionId, u32) -> bool> TypeFolder for RegionFinder<F> {
    fn fold_region(&mut self, _: &Types, r: RegionId, depth: u32) -> RegionId {
        self.found |= (self.matches)(r, depth);
        r
    }
    fn fold_ty(&mut self, tys: &Types, t: TyId, depth: u32) -> TyId {
        if self.found { return t; }
        if let TyKind::Dyn(_, args, bindings, regions, r, _, _) = tys.kind(t) {
            self.fold_region(tys, *r, depth);
            for &t in tys.list(*args) { self.fold_ty(tys, t, depth + 1); }
            for &(_, t, _) in tys.list(*bindings) { self.fold_ty(tys, t, depth + 1); }
            for &r in tys.region_args(*regions).iter() { self.fold_region(tys, r, depth + 1); }
        } else { super_fold_ty(tys, t, self, depth); }
        t
    }
    fn fold_const(&mut self, tys: &Types, ct: Const, depth: u32) -> Const {
        if self.found { return ct; }
        match tys.const_kind(ct) {
            ConstKind::Alias(_, a) => { for &arg in &*a.args { fold::fold_arg(tys, arg, self, depth); } }
            ConstKind::Value(v) => {
                self.fold_ty(tys, v.ty, depth);
                if let Some(branches) = v.valtree.try_to_branch() { for &ct in branches { self.fold_const(tys, ct, depth); } }
            }
            _ => {}
        }
        ct
    }
}

fn arg_has_region(tys: &Types, arg: GenericArg, matches: impl FnMut(RegionId, u32) -> bool) -> bool {
    let mut finder = RegionFinder { matches, found: false };
    fold::fold_arg(tys, arg, &mut finder, 0);
    finder.found
}

fn term_arg(term: Term) -> GenericArg {
    match term { Term::Ty(t) => GenericArg::Type(t), Term::Const(c) => GenericArg::Const(c) }
}

fn clause_args_any(tys: &Types, clause: &ClauseKind, mut f: impl FnMut(GenericArg) -> bool) -> bool {
    match clause {
        ClauseKind::Trait(tr) => tr.trait_ref.args.iter().copied().any(f),
        ClauseKind::Projection(p) => {
            let found = match &p.projection_term {
                AliasTerm::Ty(t) => {
                    let TyKind::Alias(_, a) = tys.kind(*t) else { unreachable!("projection alias") };
                    tys.list(a.args).iter().any(|&t| f(match tys.kind(t) {
                        TyKind::Const(c) => GenericArg::Const(*c), _ => GenericArg::Type(t),
                    })) || tys.region_args(a.regions).iter().any(|&r| f(GenericArg::Lifetime(r)))
                }
                AliasTerm::Const(a) => a.args.iter().copied().any(&mut f),
            };
            found || f(term_arg(p.term))
        }
        ClauseKind::RegionOutlives(a, b) => f(GenericArg::Lifetime(*a)) || f(GenericArg::Lifetime(*b)),
        ClauseKind::TypeOutlives(t, r) => f(GenericArg::Type(*t)) || f(GenericArg::Lifetime(*r)),
        ClauseKind::WellFormed(t) => f(term_arg(*t)),
        ClauseKind::ConstEvaluatable(c) => f(GenericArg::Const(*c)),
        ClauseKind::ConstArgHasType(c, t) => f(GenericArg::Const(*c)) || f(GenericArg::Type(*t)),
    }
}

// Give the outer binder distinct free identities; the component collector skips bound regions.
struct OpenRegions { universe: UniverseIndex }
impl TypeFolder for OpenRegions {
    fn fold_region(&mut self, tys: &Types, r: RegionId, depth: u32) -> RegionId {
        match tys.region_kind(r) {
            RegionKind::ReBound(d, var) if d == depth => tys.intern_region(RegionKind::RePlaceholder { universe: self.universe, var }),
            _ => r,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum OutlivesAtom { Region(RegionId), Type(TyId) }
type OutlivesFacts = HashSet<(OutlivesAtom, RegionId)>;

struct ImpliedOutlives<'a> {
    tcx: &'a Tcx,
    prog: &'a Program,
    parent_regions: Option<(UniverseIndex, usize)>,
    unknown: bool,
    facts: OutlivesFacts,
    visited: HashSet<TyId>,
}
impl ImpliedOutlives<'_> {
    fn alias(&mut self, t: TyId) {
        if let Some((universe, count)) = self.parent_regions {
            let tys = &self.tcx.tys;
            self.unknown |= arg_has_region(tys, GenericArg::Type(t), |r, _| {
                matches!(tys.region_kind(r), RegionKind::RePlaceholder { universe: u, var } if u == universe && (var.0 as usize) < count)
            });
        }
    }
    fn insert(&mut self, atom: OutlivesAtom, r: RegionId) {
        if matches!(self.tcx.tys.region_kind(r), RegionKind::ReBound(..)) { return; }
        if let OutlivesAtom::Region(a) = atom {
            if matches!(self.tcx.tys.region_kind(a), RegionKind::ReBound(..)) { return; }
        }
        self.facts.insert((atom, r));
    }
    fn components(&mut self, t: TyId, r: RegionId) {
        let mut parts = Vec::new();
        push_outlives_components(self.tcx, self.prog, t, &mut parts);
        for part in parts {
            let atom = match part {
                Component::Region(r) => OutlivesAtom::Region(r),
                Component::Param(t) | Component::Placeholder(t) => OutlivesAtom::Type(t),
                Component::Alias(t) => { self.alias(t); OutlivesAtom::Type(t) }
                Component::EscapingAlias(_) | Component::UnresolvedInferenceVariable(_) => continue,
            };
            self.insert(atom, r);
        }
    }
}
impl TypeFolder for ImpliedOutlives<'_> {
    fn fold_ty(&mut self, tys: &Types, t: TyId, depth: u32) -> TyId {
        if self.unknown || !self.visited.insert(t) { return t; }
        match tys.kind(t) {
            TyKind::Alias(..) => { self.alias(t); return t; }
            TyKind::Ref(_, inner, r) => self.components(*inner, *r),
            TyKind::Adt(def, args, regions) => if let Some(adt) = self.tcx.adts.get(def) {
                for p in ImpliedBounds::inferred_outlives(self.tcx, self.prog, *def) {
                    match instantiate(self.tcx, p, tys.list(*args), &tys.region_args(adt.region_params), &tys.region_args(*regions)) {
                        OutlivesPredicate::Region(a, b) => self.insert(OutlivesAtom::Region(a), b),
                        OutlivesPredicate::Type(t, r) => self.components(t, r),
                    }
                }
            }
            _ => {}
        }
        super_fold_ty(tys, t, self, depth);
        t
    }
}

fn close_outlives(facts: &mut OutlivesFacts) {
    let mut work: Vec<_> = facts.iter().copied().collect();
    let mut i = 0;
    while i < work.len() {
        let (a, b) = work[i];
        for j in 0..work.len() {
            let (c, d) = work[j];
            if c == OutlivesAtom::Region(b) && facts.insert((a, d)) { work.push((a, d)); }
            if a == OutlivesAtom::Region(d) && facts.insert((c, b)) { work.push((c, b)); }
        }
        i += 1;
    }
}

// rustc issue 84591: elaboration must retain the implied bounds of higher-ranked arguments.
fn keeps_implied_bounds(tcx: &Tcx, prog: &Program, parent: &Predicate, clause: &Predicate) -> bool {
    let PredicateKind::Clause(ClauseKind::Trait(tr)) = &parent.kind else { unreachable!("supertrait parent") };
    let PredicateKind::Clause(clause) = &clause.kind else { unreachable!("implied clause") };
    let tys = &tcx.tys;
    let count = parent.bound_vars.len();
    let is_parent = |r, depth| matches!(tys.region_kind(r), RegionKind::ReBound(d, var) if d == depth && (var.0 as usize) < count);
    if !clause_args_any(tys, clause, |a| arg_has_region(tys, a, is_parent)) { return true; }
    if tr.trait_ref.args.iter().all(|&a| !arg_has_region(tys, a, is_parent) || clause_args_any(tys, clause, |b| a == b)) { return true; }

    let mut mentioned = HashSet::default();
    let mut universe = UniverseIndex::ROOT;
    let mut observe = |r, _| {
        if let RegionKind::RePlaceholder { universe: u, .. } = tys.region_kind(r) { universe = universe.max(u); }
        false
    };
    for &a in &tr.trait_ref.args { arg_has_region(tys, a, &mut observe); }
    clause_args_any(tys, clause, |a| arg_has_region(tys, a, &mut observe));
    clause_args_any(tys, clause, |a| arg_has_region(tys, a, |r, depth| {
        if is_parent(r, depth) { if let RegionKind::ReBound(_, var) = tys.region_kind(r) { mentioned.insert(var); } }
        false
    }));
    let universe = UniverseIndex(universe.0.checked_add(1).expect("implied bounds universe overflow"));
    let mut opener = OpenRegions { universe };
    let mut sub = ImpliedOutlives { tcx, prog, parent_regions: Some((universe, count)), unknown: false, facts: HashSet::default(), visited: HashSet::default() };
    for &a in &tr.trait_ref.args {
        if let GenericArg::Type(t) = a { sub.fold_ty(tys, opener.fold_ty(tys, t, 0), 0); }
    }
    if sub.unknown { return false; }
    let mut sup = ImpliedOutlives { tcx, prog, parent_regions: None, unknown: false, facts: HashSet::default(), visited: HashSet::default() };
    clause_args_any(tys, clause, |a| {
        if let GenericArg::Type(t) = a { sup.fold_ty(tys, opener.fold_ty(tys, t, 0), 0); }
        false
    });
    close_outlives(&mut sub.facts);
    close_outlives(&mut sup.facts);
    let relevant = |r| match tys.region_kind(r) {
        RegionKind::RePlaceholder { universe: u, var } if u == universe => mentioned.contains(&var),
        RegionKind::ReBound(..) => false,
        _ => true,
    };
    let static_region = tys.intern_region(RegionKind::ReStatic);
    sub.facts.iter().all(|&(a, b)| {
        if !relevant(b) || matches!(a, OutlivesAtom::Region(r) if !relevant(r)) { return true; }
        // Reflexivity and 'static edges need no entries in either finite closure.
        a == OutlivesAtom::Region(b) || a == OutlivesAtom::Region(static_region)
            || sup.facts.contains(&(a, b)) || sup.facts.contains(&(a, static_region))
    })
}

impl Tcx {
    pub fn instantiate_supertrait(&self, prog: &Program, pred: &Predicate, parent: &Predicate) -> Predicate {
        let PredicateKind::Clause(ClauseKind::Trait(tr)) = &parent.kind else { panic!("supertrait parent") };
        let kind = fold::fold_predicate_kind(&self.tys, &pred.kind, &mut BoundVarShifter { offset: parent.bound_vars.len() as u32 }, 0);
        let kind = fold::fold_predicate_kind(&self.tys, &kind, &mut ArgFolder { indices: &self.generic_indices_of(prog, tr.trait_ref.def_id), args: &tr.trait_ref.args }, 0);
        let mut bound_vars = parent.bound_vars.to_vec();
        bound_vars.extend_from_slice(&pred.bound_vars);
        Predicate { kind, bound_vars: bound_vars.into() }
    }

    /// rustc_type_ir/src/elaborate.rs with filter_only_self: a trait clause adds its supertraits
    /// (explicit_super_predicates_of) only.
    pub fn supertraits(&self, prog: &Program, initial: Predicate) -> Vec<Predicate> {
        self.elaborate_with(prog, initial, false, true)
    }

    /// The full supertrait structure, including clauses that cannot be assumed universally.
    pub fn supertraits_unfiltered(&self, prog: &Program, initial: Predicate) -> Vec<Predicate> {
        self.elaborate_with(prog, initial, false, false)
    }

    /// rustc_type_ir/src/elaborate.rs (Filter::All, a param env's and an item's bounds): a trait
    /// clause adds what its trait implies (explicit_implied_predicates_of), the bounds of the
    /// supertraits' associated item constraints too.
    pub fn elaborate(&self, prog: &Program, initial: Predicate) -> Vec<Predicate> {
        self.elaborate_with(prog, initial, true, true)
    }

    /// rustc_type_ir/src/elaborate.rs:101-112 (extend_deduped): a predicate is pushed only if its
    /// kind with anonymized bound variables (anonymize_bound_vars) was not seen before; :140-249
    /// pops, elaborates and yields.
    fn elaborate_with(&self, prog: &Program, initial: Predicate, all: bool, filter: bool) -> Vec<Predicate> {
        let mut visited = crate::fxhash::HashSet::default();
        let mut stack = Vec::new();
        let mut push = |stack: &mut Vec<Predicate>, pred: Predicate| { if visited.insert(pred.anonymized()) { stack.push(pred); } };
        push(&mut stack, initial);
        let mut result = Vec::new();
        while let Some(pred) = stack.pop() {
            if let PredicateKind::Clause(ClauseKind::Trait(tr)) = &pred.kind {
                if tr.polarity == ClausePolarity::Positive {
                let implied = if all { self.explicit_implied_predicates_of(prog, tr.trait_ref.def_id) } else { self.explicit_super_predicates_of(prog, tr.trait_ref.def_id) };
                let filter = filter && pred.bound_vars.iter().any(|v| matches!(v, BoundVariableKind::Region(_)));
                for bound in implied {
                    let clause = self.instantiate_supertrait(prog, &bound, &pred);
                    if !filter || keeps_implied_bounds(self, prog, &pred, &clause) { push(&mut stack, clause); }
                }
                }
            }
            if let PredicateKind::Clause(ClauseKind::TypeOutlives(ty, region)) = pred.kind {
                if !matches!(self.tys.region_kind(region), RegionKind::ReBound(..)) {
                    let mut parts = Vec::new();
                    push_outlives_components(self, prog, ty, &mut parts);
                    for part in parts {
                        let clause = match part {
                            Component::Region(r) => ClauseKind::RegionOutlives(r, region),
                            Component::Param(t) | Component::Placeholder(t) | Component::Alias(t) => ClauseKind::TypeOutlives(t, region),
                            Component::EscapingAlias(_) | Component::UnresolvedInferenceVariable(_) => continue,
                        };
                        push(&mut stack, Predicate { kind: PredicateKind::Clause(clause), bound_vars: pred.bound_vars.clone() });
                    }
                }
            }
            result.push(pred);
        }
        result
    }
}
