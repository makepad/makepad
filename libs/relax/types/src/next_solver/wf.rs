// ported from N:rustc_trait_selection/src/traits/wf.rs @8d1a764
use super::*;
use crate::infer::InferCtxt;
use crate::program::Program;
use crate::types::{Types, TyKind, RegionKind, AliasTyKind};
use crate::types::fold::{TypeFolder, super_fold_ty, super_fold_const};
use crate::consts::{ConstKind, AliasConstKind};

pub fn unnormalized_obligations(infcx: &InferCtxt<'_>, prog: &Program, env: &ParamEnvId, term: Term) -> Result<Option<Vec<Goal>>, NoSolution> {
    let term = match term { Term::Ty(t) => Term::Ty(infcx.deeply_resolve_ignoring_regions(t)), Term::Const(c) => Term::Const(crate::infer::resolve::DeepResolverIgnoringRegions::new(infcx).fold_const(&infcx.tcx.tys, c, 0)) };
    match term {
        Term::Ty(t) if matches!(infcx.tcx.tys.kind(t), TyKind::Infer(_)) => return Ok(None),
        Term::Const(c) if matches!(infcx.tcx.tys.const_kind(c), ConstKind::Infer(_)) => return Ok(None),
        _ => {}
    }
    let mut wf = WfPredicates { infcx, prog, env, out: Vec::new(), visited_tys: crate::fxhash::HashSet::default(), failed: false };
    match term { Term::Ty(t) => { wf.fold_ty(&infcx.tcx.tys, t, 0); } Term::Const(c) => { wf.fold_const(&infcx.tcx.tys, c, 0); } }
    if wf.failed { Err(NoSolution) } else { Ok(Some(wf.out)) }
}
struct WfPredicates<'a, 'tcx> {
    infcx: &'a InferCtxt<'tcx>, prog: &'a Program, env: &'a ParamEnvId, out: Vec<Goal>, visited_tys: crate::fxhash::HashSet<TyId>, failed: bool,
}
impl WfPredicates<'_, '_> {
    fn push(&mut self, kind: PredicateKind) { self.out.push(Goal { param_env: self.env.clone(), predicate: Predicate::dummy(kind) }); }
    fn require_sized(&mut self, ty: TyId) {
        let tcx = self.infcx.tcx;
        if !tcx.tys.has_vars_bound_at_or_above(ty, 0) {
            if let Some(def) = tcx.lang_items.get(crate::lang_items::LangItem::Sized) { self.out.push(Goal { param_env: self.env.clone(), predicate: collect::trait_predicate(def, vec![GenericArg::Type(ty)]) }); }
        }
    }
    fn nominal_obligations(&mut self, def: DefId, args: &[GenericArg]) {
        let tcx = self.infcx.tcx;
        if tcx.lang_items.get(crate::lang_items::LangItem::Sized) == Some(def) { return; }
        for p in tcx.clauses_of(self.prog, def) {
            let p = tcx.instantiate_predicate(self.prog, def, args, &p);
            if !predicate_has_escaping_bound_vars(&tcx.tys, &p) { self.out.push(Goal { param_env: self.env.clone(), predicate: p }); }
        }
        // rustc `predicates_of` of an ADT includes its `inferred_outlives_of` (RFC 2093)
        if tcx.adts.contains_key(&def) {
            for p in crate::borrowck::implied_bounds::ImpliedBounds::inferred_outlives(tcx, self.prog, def) {
                let kind = match p {
                    crate::tcx::outlives::OutlivesPredicate::Region(a, b) => ClauseKind::RegionOutlives(a, b),
                    crate::tcx::outlives::OutlivesPredicate::Type(t, r) => ClauseKind::TypeOutlives(t, r),
                };
                let p = tcx.instantiate_predicate(self.prog, def, args, &Predicate::dummy(PredicateKind::Clause(kind)));
                if !predicate_has_escaping_bound_vars(&tcx.tys, &p) { self.out.push(Goal { param_env: self.env.clone(), predicate: p }); }
            }
        }
    }
    fn add_wf_preds_for_inherent_projection(&mut self, def: DefId, args: &[GenericArg], impl_format: bool) {
        let tcx = self.infcx.tcx;
        let self_ty = if impl_format { None } else { let GenericArg::Type(t) = args[0] else { unreachable!() }; Some(t) };
        if self_ty.is_none_or(|t| !tcx.tys.has_vars_bound_at_or_above(t, 0)) {
            let mut full = args.to_vec();
            if let Some(self_ty) = self_ty {
                let imp = self.prog.def(def).parent;
                full = self.infcx.fresh_args_for_item(self.prog, imp);
                let impl_ty = tcx.instantiate_ty(self.prog, imp, &full, tcx.impl_self[&imp]);
                match self.infcx.at(self.prog, self.env.clone()).eq(self_ty, impl_ty) {
                    Ok(goals) => self.out.extend(goals),
                    Err(_) => { self.failed = true; }
                }
                full.extend_from_slice(&args[1..]);
            }
            self.nominal_obligations(def, &full);
        }
        for &a in args { fold::fold_arg(&tcx.tys, a, self, 0); }
    }
}
impl TypeFolder for WfPredicates<'_, '_> {
    fn fold_ty(&mut self, tys: &Types, t: TyId, depth: u32) -> TyId {
        if !self.visited_tys.insert(t) { return t; }
        let tcx = self.infcx.tcx;
        match tys.kind(t) {
            TyKind::Slice(e) => self.require_sized(*e),
            TyKind::Array(e, n) => { self.require_sized(*e); self.push(PredicateKind::Clause(ClauseKind::ConstArgHasType(*n, tys.usize_))); }
            TyKind::Tuple(ts) => { let ts = tys.list(*ts); if !ts.is_empty() { for &t in &ts[..ts.len()-1] { self.require_sized(t); } } }
            TyKind::Ref(_, ty, region) => {
                if !tys.has_vars_bound_at_or_above(*ty, 0) && !matches!(tys.region_kind(*region), RegionKind::ReBound(..)) { self.push(PredicateKind::Clause(ClauseKind::TypeOutlives(*ty, *region))); }
            }
            TyKind::Adt(d, args, rs) | TyKind::FnDef(d, args, rs) => {
                let args = tcx.complete_args(self.prog, *d, tys.list(*args), *rs);
                if matches!(tys.kind(t), TyKind::FnDef(..)) {
                    let sig = tcx.fn_item_sig(self.prog, *d).expect("function signature");
                    let ret = tcx.instantiate_ty(self.prog, *d, &args, sig.ret);
                    self.fold_ty(tys, ret, depth);
                }
                self.nominal_obligations(*d, &args);
            }
            TyKind::Alias(_, a) => {
                let args = tcx.complete_args(self.prog, a.kind.def_id(), tys.list(a.args), a.regions);
                if matches!(a.kind, AliasTyKind::Inherent { .. }) { self.add_wf_preds_for_inherent_projection(a.kind.def_id(), &args, false); return t; }
                self.nominal_obligations(a.kind.def_id(), &args);
            }
            TyKind::Closure(_, _, crate::types::ClosureArgs { parent_args: owner, tupled_upvars_ty: upvars, .. }) => {
                if let TyKind::FnDef(d, args, rs) = tys.kind(*owner) { self.nominal_obligations(*d, &tcx.complete_args(self.prog, *d, tys.list(*args), *rs)); }
                if !tcx.is_coroutine(t) { self.fold_ty(tys, *upvars, depth); return t; }
            }
            TyKind::Dyn(principal, ..) => {
                let predicates = tcx.object_predicates(self.prog, t);
                let TyKind::Dyn(_, _, _, _, region, _, _) = tys.kind(t) else { unreachable!() };
                if !tys.has_vars_bound_at_or_above(t, 0) {
                    for p in &predicates { for p in tcx.supertraits(self.prog, p.clone()) {
                        if let PredicateKind::Clause(ClauseKind::TypeOutlives(subject, bound)) = p.kind {
                            if subject == t && !matches!(tys.region_kind(bound), RegionKind::ReBound(..)) { self.push(PredicateKind::Clause(ClauseKind::RegionOutlives(*region, bound))); }
                        }
                    } }
                }
                if *principal != crate::program::NO_DEF {
                    self.push(PredicateKind::DynCompatible(*principal));
                    let p = &predicates[0];
                    assert!(matches!(p.kind, PredicateKind::Clause(ClauseKind::Trait(_))));
                    for clause in tcx.clauses_of(self.prog, *principal) {
                        let clause = tcx.instantiate_supertrait(self.prog, &clause, p);
                        if let PredicateKind::Clause(ClauseKind::ConstArgHasType(ct, _)) = clause.kind {
                            if matches!(tys.const_kind(ct), ConstKind::Param(_) | ConstKind::Placeholder(_)) && !predicate_has_escaping_bound_vars(tys, &clause) { self.out.push(Goal { param_env: self.env.clone(), predicate: clause }); }
                        }
                    }

                }
            }
            TyKind::Infer(_) => self.push(PredicateKind::Clause(ClauseKind::WellFormed(Term::Ty(t)))),
            _ => {}
        }
        super_fold_ty(tys, t, self, depth); t
    }
    fn fold_const(&mut self, tys: &Types, ct: Const, depth: u32) -> Const {
        match tys.const_kind(ct) {
            ConstKind::Alias(_, a) if !tys.has_vars_bound_at_or_above(tys.intern(TyKind::Const(ct)), 0) => {
                if matches!(a.kind, AliasConstKind::Anon { .. }) { self.push(PredicateKind::Clause(ClauseKind::ConstEvaluatable(ct))); }
                if matches!(a.kind, AliasConstKind::InherentSelf { .. } | AliasConstKind::InherentImpl { .. }) {
                    self.add_wf_preds_for_inherent_projection(a.kind.def_id(), &a.args, matches!(a.kind, AliasConstKind::InherentImpl { .. })); return ct;
                }
                self.nominal_obligations(a.kind.def_id(), &a.args);
            }
            ConstKind::Infer(_) => self.push(PredicateKind::Clause(ClauseKind::WellFormed(Term::Const(ct)))),
            _ => {}
        }
        super_fold_const(tys, ct, self, depth); ct
    }
}
pub fn predicate_has_escaping_bound_vars(tys: &Types, p: &Predicate) -> bool {
    struct Finder(bool);
    impl TypeFolder for Finder {
        fn fold_ty(&mut self, tys: &Types, t: TyId, depth: u32) -> TyId { self.0 |= tys.has_vars_bound_at_or_above(t, depth); t }
        fn fold_const(&mut self, tys: &Types, c: Const, depth: u32) -> Const { self.0 |= tys.has_vars_bound_at_or_above(tys.intern(TyKind::Const(c)), depth); c }
        fn fold_region(&mut self, tys: &Types, r: RegionId, depth: u32) -> RegionId { self.0 |= matches!(tys.region_kind(r), RegionKind::ReBound(d, _) if d >= depth); r }
    }
    let mut f = Finder(false); fold::fold_predicate(tys, p, &mut f); f.0
}
impl eval_ctxt::EvalCtxt<'_, '_> {
    pub(super) fn compute_well_formed_goal(&mut self, env: ParamEnvId, term: Term) -> QueryResultOrRerunNonErased {
        match unnormalized_obligations(self.delegate, self.solver.prog, &env, term)? {
            Some(goals) => { self.add_goals(GoalSource::Misc, goals)?; self.evaluate_added_goals_and_make_canonical_response(Certainty::Yes) }
            None => self.evaluate_added_goals_and_make_canonical_response(Certainty::AMBIGUOUS),
        }
    }
}
