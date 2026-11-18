//! The binder of a fn signature (rustc issues 25860 and 129005). A binder opened with placeholders may assume
//! the outlives bounds that well-formedness of its inputs and output implies, the bounds a fn item with that
//! signature assumes; a binder opened with fresh variables must prove its instantiated inputs and output well
//! formed. `S <: for<..> fn(I) -> O` then holds iff `fn shim<..>(a: I) -> O { s(a) }` would borrow-check.
//!
//! Region inference knows no assumptions, so the binder's own regions are eliminated before its constraints
//! reach it. An existential stands for the regions it must outlive (paths through existentials end at a
//! placeholder or an outside region); a relation between a placeholder and another region is dropped when the
//! assumptions give it, rewritten when exactly one assumption bounds the placeholder, and otherwise kept as it
//! is, for region inference's placeholder rules to reject. A type-outlives assumption is used only for its exact
//! subject. The elimination can only make the check stricter than the rule, never looser.
//!
//! An alias obligation `A: r` raised while a scope is open, whose alias matches an assumption's subject regions and
//! rigidity aside, is not decided when it is raised: rustc's alias rule leaves its unique-bound shortcut when the
//! environment has a bound for the alias, and the shim's environment has the assumption. The innermost open scope
//! holds it, and at its close discharges each target that is its own placeholder when an assumption `A': q` covers
//! that placeholder and `A'` is the same alias, its region arguments proven equal by the constraints at close;
//! every other target goes back through the ordinary rule.
//!
//! A relation nested in the signature that opens no scope of its own (a `dyn`'s higher-ranked principal) makes
//! placeholders and variables of its own. Grouped by the scope's raw constraints, a component of the scope's regions
//! other than its own placeholders that no constraint, type test or held obligation ties to any other region, and
//! that holds such a later placeholder, is the nested relation's alone: it is set aside untouched, never walked
//! through, and pushed back as it was when the scope is done. Only a later placeholder still named by the rest
//! keeps the scope's constraints as they are.
use super::*;
use crate::borrowck::implied_bounds::ImpliedBounds;
use crate::fxhash::{HashMap, HashSet};
use crate::types::{TypeFlags, Types};
use std::sync::atomic::{AtomicU64, Ordering};

/// Scope closes by outcome, for the check's summary: eliminated; kept as they are for a later placeholder; kept as
/// they are for a type test naming the scope's regions; and nested components set aside.
static SCOPE_CLOSES: [AtomicU64; 4] = [AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0)];
const ELIMINATED: usize = 0;
const KEPT_LATER: usize = 1;
const KEPT_TEST: usize = 2;
const SET_ASIDE: usize = 3;
fn count(outcome: usize, n: u64) { SCOPE_CLOSES[outcome].fetch_add(n, Ordering::Relaxed); }
/// The scope closes of this process by outcome: eliminated, kept for a later placeholder, kept for a type test, and
/// nested components set aside.
pub fn scope_closes() -> [u64; 4] { std::array::from_fn(|i| SCOPE_CLOSES[i].load(Ordering::Relaxed)) }

/// What a binder's constraints are separated from: the lengths of the region variables, outlives constraints,
/// type tests and placeholders when it was opened, and the assumptions of its placeholder side.
pub(super) struct BinderScope {
    vars: usize,
    outlives: usize,
    type_tests: usize,
    placeholders: usize,
    /// The end of this binder's own placeholders in the placeholder list (None: none opened).
    own_end: Option<usize>,
    /// `a: b` that the placeholder side's well-formedness implies.
    regions: Vec<(RegionVid, RegionVid)>,
    /// `T: b` that the placeholder side's well-formedness implies.
    types: Vec<(TyId, RegionVid)>,
    /// Whether it pushed a frame (`assume_wf`), which its close pops.
    frame: bool,
}

/// An alias obligation `ty: region` that an open binder scope holds until it closes.
#[derive(Clone, Debug)]
pub(crate) struct HeldAlias {
    pub(crate) ty: TyId,
    pub(crate) region: RegionId,
    /// The category of the operation that raised it, and its own.
    pub(crate) outer: ConstraintCategory,
    pub(crate) category: ConstraintCategory,
    pub(crate) locations: Locations,
    pub(crate) span: Span,
}

/// An open scope that assumed its signature well formed: the subjects of its alias assumptions (regions and
/// rigidity aside) and the alias obligations it holds.
#[derive(Clone, Debug)]
pub(crate) struct BinderFrame { subjects: Vec<TyId>, held: Vec<HeldAlias> }
impl BinderFrame {
    /// The frame of a scope whose assumptions are `types` (`T: b`, only the alias subjects count).
    pub(crate) fn new(tys: &Types, types: &[(TyId, RegionVid)]) -> Self {
        let subjects = types.iter().filter(|&&(t, _)| matches!(tys.kind(t), TyKind::Alias(..))).map(|&(t, _)| erased(tys, t)).collect();
        BinderFrame { subjects, held: Vec::new() }
    }
}

/// The type as an environment bound is matched with it (`approx_declared_bounds_from_env`): regions erased,
/// rigidity forgotten.
fn erased(tys: &Types, t: TyId) -> TyId { crate::infer::outlives::verify::forget_rigidity(tys, tys.erase_regions(t)) }

/// Constraint conversion's hook into the alias rule: the innermost open scope holds an alias obligation when its
/// alias matches the subject of an assumption of any open scope; anything else takes the ordinary rule.
pub(crate) fn hold_alias(frames: &mut [BinderFrame], tys: &Types, held: HeldAlias) -> bool {
    if frames.is_empty() { return false; }
    let key = erased(tys, held.ty);
    if !frames.iter().any(|f| f.subjects.contains(&key)) { return false; }
    frames.last_mut().expect("an open frame").held.push(held);
    true
}

/// A closing scope's verdict on the alias obligations it held: `(held index, target)` for each target the ordinary
/// rule must still decide (`None`: the obligation's own region, which names no region variable). The targets of
/// `A: rho` are `rho`, or what it reaches through the scope's existentials (`is_e`) in its constraints (`raw`). A
/// target that is one of the scope's own placeholders is discharged when an assumption `A': q` has `q` at or above
/// it and `A'` is the same alias: equal regions and rigidity aside, and each pair of region arguments mutually
/// reachable in the constraints at close (`all`); one direction is not equality.
#[allow(clippy::too_many_arguments)]
pub(crate) fn held_targets(tys: &Types, held: &[HeldAlias], vid: &dyn Fn(RegionId) -> Option<RegionVid>, own: &[RegionVid],
    is_e: &dyn Fn(RegionVid) -> bool, raw: &[OutlivesConstraint], all: &[OutlivesConstraint], regions: &[(RegionVid, RegionVid)],
    types: &[(TyId, RegionVid)], static_vid: RegionVid) -> Vec<(usize, Option<RegionVid>)> {
    let mut out = Vec::new();
    let mut graph: Option<HashMap<RegionVid, Vec<RegionVid>>> = None;
    for (i, h) in held.iter().enumerate() {
        let Some(rho) = vid(h.region) else { out.push((i, None)); continue };
        let targets = if is_e(rho) { reach_in(raw, is_e, rho) } else { vec![rho] };
        for t in targets {
            let discharged = own.contains(&t)
                && types.iter().any(|&(a, q)| assumed(regions, own, static_vid, q, t) && same_alias(tys, h.ty, a, vid, all, &mut graph));
            if !discharged { out.push((i, Some(t))); }
        }
    }
    out
}

/// The regions outside the existentials that `start` reaches through them in `raw`.
fn reach_in(raw: &[OutlivesConstraint], is_e: &dyn Fn(RegionVid) -> bool, start: RegionVid) -> Vec<RegionVid> {
    let mut out = Vec::new();
    let mut seen: HashSet<RegionVid> = HashSet::default();
    seen.insert(start);
    let mut stack = vec![start];
    while let Some(x) = stack.pop() {
        for c in raw.iter().filter(|c| c.sup == x) {
            if is_e(c.sub) { if seen.insert(c.sub) { stack.push(c.sub); } } else if !out.contains(&c.sub) { out.push(c.sub); }
        }
    }
    out
}

/// `a: b` follows from the scope's region assumptions, through its own placeholders only.
fn assumed(regions: &[(RegionVid, RegionVid)], own: &[RegionVid], static_vid: RegionVid, a: RegionVid, b: RegionVid) -> bool {
    if a == b || a == static_vid { return true; }
    let mut seen: HashSet<RegionVid> = HashSet::default();
    let mut stack = vec![a];
    while let Some(x) = stack.pop() {
        for &(sup, sub) in regions {
            if sup != x { continue; }
            if sub == b { return true; }
            if own.contains(&sub) && seen.insert(sub) { stack.push(sub); }
        }
    }
    false
}

/// `a` and `b` are one alias: equal regions and rigidity aside, each pair of region arguments the same region or
/// mutually reachable in `all` (the graph is built once, when first needed).
fn same_alias(tys: &Types, a: TyId, b: TyId, vid: &dyn Fn(RegionId) -> Option<RegionVid>, all: &[OutlivesConstraint],
    graph: &mut Option<HashMap<RegionVid, Vec<RegionVid>>>) -> bool {
    if !matches!(tys.kind(a), TyKind::Alias(..)) || erased(tys, a) != erased(tys, b) { return false; }
    let regions_in = |t: TyId| {
        let out = std::cell::RefCell::new(Vec::new());
        tys.map_regions(t, &|r, _| { out.borrow_mut().push(r); r }, 0);
        out.into_inner()
    };
    let (ra, rb) = (regions_in(a), regions_in(b));
    ra.len() == rb.len() && ra.iter().zip(&rb).all(|(&x, &y)| {
        if x == y { return true; }
        let (Some(x), Some(y)) = (vid(x), vid(y)) else { return false };
        if x == y { return true; }
        let g = graph.get_or_insert_with(|| {
            let mut g: HashMap<RegionVid, Vec<RegionVid>> = HashMap::default();
            for c in all { g.entry(c.sup).or_default().push(c.sub); }
            g
        });
        reaches(g, x, y) && reaches(g, y, x)
    })
}

/// Each held obligation against its own region (a scope that keeps its constraints as they are).
/// Which of a scope's raw constraints to set aside, and how many components that is: `free` are the scope's regions
/// other than its own placeholders, grouped by the raw constraints relating two of them. A component is open when a
/// raw constraint ties one of its regions to any other region, or `tied` (the regions the scope's type tests and held
/// obligations name) holds one; a closed component that holds one of the `later` placeholders is set aside.
pub(crate) fn set_aside(raw: &[OutlivesConstraint], free: &dyn Fn(RegionVid) -> bool, later: &[RegionVid], tied: &[RegionVid]) -> (Vec<bool>, usize) {
    fn root(parent: &mut HashMap<RegionVid, RegionVid>, v: RegionVid) -> RegionVid {
        let mut r = v;
        while let Some(&up) = parent.get(&r) { r = up; }
        let mut x = v;
        while x != r { let up = parent[&x]; parent.insert(x, r); x = up; }
        r
    }
    let mut parent: HashMap<RegionVid, RegionVid> = HashMap::default();
    for c in raw {
        if free(c.sup) && free(c.sub) {
            let (a, b) = (root(&mut parent, c.sup), root(&mut parent, c.sub));
            if a != b { parent.insert(a, b); }
        }
    }
    let mut open: HashSet<RegionVid> = HashSet::default();
    for &v in tied { if free(v) { open.insert(root(&mut parent, v)); } }
    for c in raw {
        match (free(c.sup), free(c.sub)) {
            (true, false) => { open.insert(root(&mut parent, c.sup)); }
            (false, true) => { open.insert(root(&mut parent, c.sub)); }
            _ => {}
        }
    }
    let mut closed: HashSet<RegionVid> = HashSet::default();
    for &l in later { if free(l) { let r = root(&mut parent, l); if !open.contains(&r) { closed.insert(r); } } }
    let mut components: HashSet<RegionVid> = HashSet::default();
    let mask = raw.iter().map(|c| {
        let aside = free(c.sup) && closed.contains(&root(&mut parent, c.sup));
        if aside { components.insert(root(&mut parent, c.sup)); }
        aside
    }).collect();
    (mask, components.len())
}

/// `tail` without the constraints `aside` marks, and those, each in its order.
pub(crate) fn split_aside(tail: Vec<OutlivesConstraint>, aside: &[bool]) -> (Vec<OutlivesConstraint>, Vec<OutlivesConstraint>) {
    let (mut rest, mut set) = (Vec::new(), Vec::new());
    for (c, &a) in tail.into_iter().zip(aside) { if a { set.push(c) } else { rest.push(c) } }
    (rest, set)
}

fn own_regions(held: &[HeldAlias]) -> Vec<(usize, Option<RegionVid>)> { (0..held.len()).map(|i| (i, None)).collect() }

fn reaches(g: &HashMap<RegionVid, Vec<RegionVid>>, a: RegionVid, b: RegionVid) -> bool {
    let mut seen: HashSet<RegionVid> = HashSet::default();
    let mut stack = vec![a];
    while let Some(x) = stack.pop() {
        for &y in g.get(&x).map(Vec::as_slice).unwrap_or(&[]) {
            if y == b { return true; }
            if seen.insert(y) { stack.push(y); }
        }
    }
    false
}

impl TypeChecker<'_> {
    pub(super) fn open_binder_scope(&mut self) -> BinderScope {
        BinderScope {
            vars: self.infcx.num_region_vars(),
            outlives: self.constraints.outlives_constraints.outlives.len(),
            type_tests: self.constraints.type_tests.len(),
            placeholders: self.constraints.placeholder_index_to_region.len(),
            own_end: None, regions: Vec::new(), types: Vec::new(), frame: false,
        }
    }

    /// The binder's placeholders are open: `tys`, its inputs and output instantiated with them, are assumed
    /// well formed (what `assumed_wf_types` gives a fn item with this signature).
    pub(super) fn assume_wf(&mut self, scope: &mut BinderScope, tys: &[TyId]) {
        scope.own_end = Some(self.constraints.placeholder_index_to_region.len());
        let implied = ImpliedBounds::create(self.tcx, self.prog, &[], tys, &[], vec![]);
        for &(a, b) in &implied.outlives { scope.regions.push((self.infcx.to_region_vid(a), self.infcx.to_region_vid(b))); }
        for &(t, r) in &implied.type_outlives { scope.types.push((t, self.infcx.to_region_vid(r))); }
        // Every such scope gets a frame, so an alias obligation is held by the innermost scope even when only an
        // enclosing one has a matching assumption (the inner scope's existentials are gone by the outer's close).
        if !scope.frame { self.constraints.binder_frames.push(BinderFrame::new(&self.tcx.tys, &scope.types)); scope.frame = true; }
    }

    /// Replaces the constraints the binder added by ones without its regions (see the module doc).
    pub(super) fn close_binder_scope(&mut self, scope: BinderScope) {
        // The alias obligations the scope held: decided below, or submitted again against their own region where
        // the scope keeps its constraints as they are.
        let held = if scope.frame { self.constraints.binder_frames.pop().expect("an open binder frame").held } else { Vec::new() };
        let Some(own_end) = scope.own_end else { return };
        // A placeholder opened after this binder's own (a nested binder's, a type op's) that a constraint this
        // binder added still names stayed: its constraints stay as they are, and so do this binder's. A nested
        // binder that was eliminated names none (a fn pointer inside a fn signature: `find`'s closure over items
        // holding fn pointers), and this binder is eliminated as if it had not been there. Type tests naming one
        // stay through the check on the binder's regions below (placeholders are region variables there).
        let later: Vec<RegionVid> = self.constraints.placeholder_index_to_region[own_end..].iter().map(|&r| self.infcx.to_region_vid(r)).collect();
        let own: Vec<RegionVid> = self.constraints.placeholder_index_to_region[scope.placeholders..own_end].iter().map(|&r| self.infcx.to_region_vid(r)).collect();
        let vars_end = self.infcx.num_region_vars();
        let names_later = |c: &OutlivesConstraint| later.contains(&c.sup) || later.contains(&c.sub);
        // A nested relation's closed components are set aside (module doc); what is left decides.
        let mut aside: Vec<OutlivesConstraint> = Vec::new();
        if self.constraints.outlives_constraints.outlives[scope.outlives..].iter().any(names_later) {
            let free = |v: RegionVid| v >= scope.vars && v < vars_end && !own.contains(&v);
            let (mask, components) = set_aside(&self.constraints.outlives_constraints.outlives[scope.outlives..], &free, &later, &self.scope_tied(&scope, &held));
            let raw = &self.constraints.outlives_constraints.outlives[scope.outlives..];
            if raw.iter().zip(&mask).any(|(c, &m)| !m && names_later(c)) {
                count(KEPT_LATER, 1);
                self.resubmit_held(&held, &own_regions(&held));
                return;
            }
            let tail = self.constraints.outlives_constraints.outlives.split_off(scope.outlives);
            let (rest, set) = split_aside(tail, &mask);
            self.constraints.outlives_constraints.outlives.extend(rest);
            aside = set;
            count(SET_ASIDE, components as u64);
        }
        let is_p = |v: RegionVid| own.contains(&v);
        let is_e = |v: RegionVid| v >= scope.vars && v < vars_end && !own.contains(&v);
        let local = |v: RegionVid| v >= scope.vars && v < vars_end;
        let tcx = self.tcx;
        let tys = &tcx.tys;
        // A type test whose subject or bound names one of the binder's regions stays as it is, with the rest.
        let mentions_local = |t: TyId| {
            if !tys.flags(t).intersects(TypeFlags::HAS_FREE_REGIONS) { return false; }
            let found = std::cell::Cell::new(false);
            tys.map_regions(t, &|r, _| { if let RegionKind::ReVar(v) = tys.region_kind(r) { if local(v as RegionVid) { found.set(true); } } r }, 0);
            found.get()
        };
        let region_local = |r: RegionId| matches!(tys.region_kind(r), RegionKind::ReVar(v) if local(v as RegionVid));
        let names_scope = |tests: &[TypeTest]| tests.iter().any(|t| mentions_local(t.generic_kind) || bound_mentions(&t.verify_bound, &local, &region_local, &mentions_local));
        if names_scope(&self.constraints.type_tests[scope.type_tests..]) {
            count(KEPT_TEST, 1);
            self.resubmit_held(&held, &own_regions(&held));
            self.constraints.outlives_constraints.outlives.extend(aside);
            return;
        }
        let static_vid = self.infcx.to_region_vid(tys.intern_region(RegionKind::ReStatic));
        if !held.is_empty() {
            let constraints = &self.constraints;
            let infcx = &*self.infcx;
            let vid = |r: RegionId| -> Option<RegionVid> {
                match tys.region_kind(r) {
                    RegionKind::ReBound(..) => None,
                    RegionKind::RePlaceholder { .. } => (0..constraints.placeholder_indices.len())
                        .find(|&i| constraints.placeholder_indices.lookup_placeholder(i) == r)
                        .map(|i| infcx.to_region_vid(constraints.placeholder_index_to_region[i])),
                    _ => Some(infcx.to_region_vid(r)),
                }
            };
            let all = &constraints.outlives_constraints.outlives;
            let owed = held_targets(tys, &held, &vid, &own, &is_e, &all[scope.outlives..], all, &scope.regions, &scope.types, static_vid);
            self.resubmit_held(&held, &owed);
            // What the rule emitted for them may be a type test naming the scope's regions: all stays as it is.
            if names_scope(&self.constraints.type_tests[scope.type_tests..]) {
                count(KEPT_TEST, 1);
                self.constraints.outlives_constraints.outlives.extend(aside);
                return;
            }
        }

        let outlives = self.constraints.outlives_constraints.outlives.split_off(scope.outlives);
        let tests = self.constraints.type_tests.split_off(scope.type_tests);
        let mut succ: HashMap<RegionVid, Vec<usize>> = HashMap::default();
        for (i, c) in outlives.iter().enumerate() { succ.entry(c.sup).or_default().push(i); }
        // The regions outside the binder's existentials that `start` reaches through them, each with the first
        // constraint of a path to it.
        let reach = |start: RegionVid| -> Vec<(RegionVid, usize)> {
            let mut out = Vec::new();
            let mut seen: HashSet<RegionVid> = HashSet::default();
            let mut stack: Vec<(RegionVid, Option<usize>)> = vec![(start, None)];
            while let Some((node, first)) = stack.pop() {
                for &i in succ.get(&node).map(Vec::as_slice).unwrap_or(&[]) {
                    let next = outlives[i].sub;
                    let first = first.unwrap_or(i);
                    if is_e(next) { if seen.insert(next) { stack.push((next, Some(first))); } } else { out.push((next, first)); }
                }
            }
            out
        };
        // `a: b` follows from the assumptions, through the binder's placeholders only.
        let holds = |a: RegionVid, b: RegionVid| assumed(&scope.regions, &own, static_vid, a, b);
        let type_holds = |t: TyId, b: RegionVid| scope.types.iter().any(|&(s, r)| s == t && holds(r, b));
        // The outside regions the assumptions put above placeholder `p` (`c: p`), or below it (`p: c`).
        let bounds_of = |p: RegionVid, above: bool| -> Vec<RegionVid> {
            let mut found: Vec<RegionVid> = scope.regions.iter().filter_map(|&(sup, sub)| {
                let (this, other) = if above { (sub, sup) } else { (sup, sub) };
                (!local(other) && (this == p || if above { holds(this, p) } else { holds(p, this) })).then_some(other)
            }).collect();
            found.sort_unstable(); found.dedup();
            found
        };

        let mut kept: Vec<OutlivesConstraint> = Vec::new();
        let mut done: HashSet<(RegionVid, RegionVid)> = HashSet::default();
        let mut push = |kept: &mut Vec<OutlivesConstraint>, sup: RegionVid, sub: RegionVid, from: &OutlivesConstraint| {
            if sup != sub && done.insert((sup, sub)) { kept.push(OutlivesConstraint { sup, sub, ..from.clone() }); }
        };
        let mut starts: Vec<RegionVid> = outlives.iter().map(|c| c.sup).filter(|&v| !is_e(v)).collect();
        starts.sort_unstable(); starts.dedup();
        let mut placeholder_pairs: Vec<(RegionVid, RegionVid, usize)> = Vec::new();
        for s in starts {
            for (t, i) in reach(s) {
                if s == t { continue; }
                match (is_p(s), is_p(t)) {
                    (false, false) => push(&mut kept, s, t, &outlives[i]),
                    _ => if !holds(s, t) { placeholder_pairs.push((s, t, i)) },
                }
            }
        }
        // A relation of a placeholder and an outside region holds for every placeholder the assumptions allow
        // when the outside region outlives one of the placeholder's upper bounds (`n: p`), or one of its lower
        // bounds outlives it (`p: t`). Known: a path of the constraints outside the binder and of the ones above.
        // Else one bound is required (a sufficient choice); with none, the relation stays for the placeholder rules.
        let mut graph: Option<HashMap<RegionVid, Vec<RegionVid>>> = None;
        for (s, t, i) in placeholder_pairs {
            let from = &outlives[i];
            if is_p(s) && is_p(t) { push(&mut kept, s, t, from); continue; }
            let (outside, candidates) = if is_p(t) { (s, bounds_of(t, true)) } else { (t, bounds_of(s, false)) };
            if candidates.is_empty() { push(&mut kept, s, t, from); continue; }
            let graph = graph.get_or_insert_with(|| {
                let mut g: HashMap<RegionVid, Vec<RegionVid>> = HashMap::default();
                for c in self.constraints.outlives_constraints.outlives.iter().chain(&kept) { g.entry(c.sup).or_default().push(c.sub); }
                g
            });
            let reaches = |a: RegionVid, b: RegionVid| -> bool {
                if a == b || a == static_vid { return true; }
                let mut seen: HashSet<RegionVid> = HashSet::default();
                let mut stack = vec![a];
                while let Some(x) = stack.pop() {
                    for &y in graph.get(&x).map(Vec::as_slice).unwrap_or(&[]) {
                        if y == b { return true; }
                        if seen.insert(y) { stack.push(y); }
                    }
                }
                false
            };
            let known = candidates.iter().any(|&c| if is_p(t) { reaches(outside, c) } else { reaches(c, outside) });
            if known { continue; }
            if is_p(t) { push(&mut kept, s, candidates[0], from) } else { push(&mut kept, candidates[0], t, from) }
        }
        let mut kept_tests: Vec<TypeTest> = Vec::new();
        for test in tests {
            let l = test.lower_bound;
            if !local(l) { kept_tests.push(test); continue; }
            let targets: Vec<RegionVid> = if is_p(l) { vec![l] } else { reach(l).into_iter().map(|(t, _)| t).collect() };
            for t in targets {
                if is_p(t) && type_holds(test.generic_kind, t) { continue; }
                kept_tests.push(TypeTest { lower_bound: t, ..test.clone() });
            }
        }
        for c in kept { self.constraints.outlives_constraints.push(c); }
        self.constraints.type_tests.extend(kept_tests);
        self.constraints.outlives_constraints.outlives.extend(aside);
        count(ELIMINATED, 1);
    }

    /// The scope's regions its type tests (lower bound, subject, bound) and held obligations (region, alias) name.
    fn scope_tied(&self, scope: &BinderScope, held: &[HeldAlias]) -> Vec<RegionVid> {
        let tys = &self.tcx.tys;
        let out = std::cell::RefCell::new(Vec::new());
        let region = |r: RegionId| { if let Some(v) = self.region_vid(r) { out.borrow_mut().push(v); } false };
        let ty = |t: TyId| { tys.map_regions(t, &|r, _| { region(r); r }, 0); false };
        let var = |v: RegionVid| { out.borrow_mut().push(v); false };
        for t in &self.constraints.type_tests[scope.type_tests..] {
            var(t.lower_bound);
            ty(t.generic_kind);
            bound_mentions(&t.verify_bound, &var, &region, &ty);
        }
        for h in held { region(h.region); ty(h.ty); }
        out.into_inner()
    }

    /// A region's variable: a placeholder's through its index; none for a bound region.
    fn region_vid(&self, r: RegionId) -> Option<RegionVid> {
        match self.tcx.tys.region_kind(r) {
            RegionKind::ReBound(..) => None,
            RegionKind::RePlaceholder { .. } => (0..self.constraints.placeholder_indices.len())
                .find(|&i| self.constraints.placeholder_indices.lookup_placeholder(i) == r)
                .map(|i| self.infcx.to_region_vid(self.constraints.placeholder_index_to_region[i])),
            _ => Some(self.infcx.to_region_vid(r)),
        }
    }

    /// Submits held alias obligations to the ordinary rule again, each `(held index, target)` of `owed` (`None`: its
    /// own region). This scope's frame is popped, so an enclosing scope may hold them.
    fn resubmit_held(&mut self, held: &[HeldAlias], owed: &[(usize, Option<RegionVid>)]) {
        let tys = &self.tcx.tys;
        for &(i, t) in owed {
            let h = &held[i];
            let region = t.map_or(h.region, |t| tys.intern_region(RegionKind::ReVar(t as u32)));
            self.infcx.type_must_outlive(&mut self.constraints, h.ty, region, h.locations, h.span, h.outer, h.category);
        }
    }
}

/// Whether a verify bound names a region of the binder.
fn bound_mentions(bound: &super::super::region_infer::VerifyBound, local: &impl Fn(RegionVid) -> bool, region: &impl Fn(RegionId) -> bool,
    mentions: &impl Fn(TyId) -> bool) -> bool {
    use super::super::region_infer::VerifyBound;
    match bound {
        VerifyBound::IfEq { ty, bound } => mentions(*ty) || region(*bound),
        VerifyBound::IsEmpty => false,
        VerifyBound::OutlivedBy(v) => local(*v),
        VerifyBound::AnyBound(bs) | VerifyBound::AllBounds(bs) => bs.iter().any(|b| bound_mentions(b, local, region, mentions)),
    }
}
