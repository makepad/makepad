use super::*;
use crate::infer::free_regions::FreeRegionMap;
use crate::next_solver::{ParamEnv, Predicate, PredicateKind, ClauseKind};
use crate::tcx::{Tcx, TypingMode};
use crate::types::{RegionKind, TyKind, BoundVar};
use std::sync::Arc;
fn program() -> Program { Program::new(&Arc::new(crate::Session::default())) }
fn environment(tcx: &Tcx, predicates: Vec<Predicate>) -> env::OutlivesEnvironment {
    let mut known = Vec::new();
    for p in &predicates { if matches!(p.kind, PredicateKind::Clause(ClauseKind::TypeOutlives(..))) { known.push(p.clone()); } }
    env::OutlivesEnvironment::from_normalized_bounds(&tcx.tys, ParamEnv { clauses: predicates.into() }, known, vec![])
}
#[test]
fn rpitit_free_lifetime_outlives_requires_declared_bound() {
    // compare_impl_item.rs:704: returning &'b u32 for `impl Copy + 'a`.
    let tcx = Tcx::new(); let prog = program();
    let a = tcx.tys.intern_region(RegionKind::ReEarlyParam(0));
    let b = tcx.tys.intern_region(RegionKind::ReEarlyParam(1));
    let ty = tcx.tys.intern(TyKind::Ref(false, tcx.tys.u32_, b));
    for positive in [false, true] {
        let infcx = InferCtxt::new(&tcx, TypingMode::PostAnalysis, Default::default());
        infcx.register_ty_outlives(ty, a);
        let predicates = if positive { vec![Predicate::dummy(PredicateKind::Clause(ClauseKind::RegionOutlives(b, a)))] } else { vec![] };
        let errors = infcx.resolve_regions_with_outlives_env(&prog, &environment(&tcx, predicates));
        if positive { assert!(errors.is_empty(), "{errors:?}"); }
        else { assert_eq!(errors, vec![RegionResolutionError::ConcreteFailure(a, b)]); }
    }
}
#[test]
fn region_graph_resolves_nested_type_after_lower_bound_propagation() {
    let tcx = Tcx::new(); let prog = program();
    let infcx = InferCtxt::new(&tcx, TypingMode::PostAnalysis, Default::default());
    let a = tcx.tys.intern_region(RegionKind::ReEarlyParam(0));
    let x = infcx.next_region_var(); let y = infcx.next_region_var();
    {
        let mut inner = infcx.inner.borrow_mut(); let mut c = inner.unwrap_region_constraints();
        c.make_subregion(&tcx.tys, x, y, false); c.make_subregion(&tcx.tys, a, x, false); c.make_subregion(&tcx.tys, y, a, false);
    }
    let ty = tcx.tys.intern(TyKind::Ref(false, tcx.tys.bool_, y));
    assert!(infcx.resolve_regions_with_outlives_env(&prog, &environment(&tcx, vec![])).is_empty());
    assert_eq!(infcx.deeply_resolve_via_region_graph(ty).unwrap(), tcx.tys.intern(TyKind::Ref(false, tcx.tys.bool_, a)));
}
#[test]
fn incompatible_upper_bound_is_rejected_even_without_type_obligation() {
    let tcx = Tcx::new(); let prog = program();
    let infcx = InferCtxt::new(&tcx, TypingMode::PostAnalysis, Default::default());
    let a = tcx.tys.intern_region(RegionKind::ReEarlyParam(0)); let b = tcx.tys.intern_region(RegionKind::ReEarlyParam(1));
    let x = infcx.next_region_var();
    { let mut inner = infcx.inner.borrow_mut(); let mut c = inner.unwrap_region_constraints(); c.make_subregion(&tcx.tys, a, x, false); c.make_subregion(&tcx.tys, x, b, false); }
    assert_eq!(infcx.resolve_regions_with_outlives_env(&prog, &environment(&tcx, vec![])), vec![RegionResolutionError::SubSupConflict(0, a, b)]);
}
#[test]
fn generic_outlives_uses_disjunction_and_higher_ranked_bounds() {
    let tcx = Tcx::new(); let prog = program();
    let t = tcx.tys.intern(TyKind::Param(0));
    let a = tcx.tys.intern_region(RegionKind::ReEarlyParam(0)); let b = tcx.tys.intern_region(RegionKind::ReEarlyParam(1));
    let bounds = vec![Predicate::dummy(PredicateKind::Clause(ClauseKind::TypeOutlives(t, a))), Predicate::dummy(PredicateKind::Clause(ClauseKind::TypeOutlives(t, b)))];
    let infcx = InferCtxt::new(&tcx, TypingMode::PostAnalysis, Default::default());
    infcx.register_ty_outlives(t, b);
    assert!(infcx.resolve_regions_with_outlives_env(&prog, &environment(&tcx, bounds)).is_empty());
    let infcx = InferCtxt::new(&tcx, TypingMode::PostAnalysis, Default::default());
    infcx.register_ty_outlives(t, b);
    assert_eq!(infcx.resolve_regions_with_outlives_env(&prog, &environment(&tcx, vec![])), vec![RegionResolutionError::GenericBoundFailure(t, b, 0)]);
    let infcx = InferCtxt::new(&tcx, TypingMode::PostAnalysis, Default::default());
    infcx.register_ty_outlives(t, tcx.tys.intern_region(RegionKind::ReStatic));
    let bound = Predicate { kind: PredicateKind::Clause(ClauseKind::TypeOutlives(t, tcx.tys.intern_region(RegionKind::ReBound(0, BoundVar(0))))), bound_vars: vec![crate::next_solver::BoundVariableKind::Region(crate::types::BoundRegionKind::Anon)].into() };
    assert!(infcx.resolve_regions_with_outlives_env(&prog, &environment(&tcx, vec![bound])).is_empty());
}
#[test]
fn free_region_diamond_uses_postdominator_not_arbitrary_minimum() {
    let tcx = Tcx::new(); let mut r = Vec::new();
    for i in 0..5 { r.push(tcx.tys.intern_region(RegionKind::ReEarlyParam(i))); }
    let map = FreeRegionMap::new(&[(r[0],r[2]), (r[0],r[3]), (r[1],r[2]), (r[1],r[3]), (r[2],r[4]), (r[3],r[4])]);
    assert_eq!(map.lub_param_regions(&tcx.tys, r[0],r[1]), r[4]);
    assert!(map.sub_free_regions(&tcx.tys, r[0], r[2]));
    assert!(!map.sub_free_regions(&tcx.tys, r[2], r[3]));
}

#[test]
fn full_resolution_rejects_unconstrained_types_and_consts() {
    let tcx = Tcx::new(); let prog = program();
    let infcx = InferCtxt::new(&tcx, TypingMode::PostAnalysis, Default::default());
    let ty = infcx.next_ty_var(); let ct = infcx.next_const_var();
    let array = tcx.tys.intern(TyKind::Array(tcx.tys.u8_, ct));
    assert!(infcx.resolve_regions_with_outlives_env(&prog, &environment(&tcx, vec![])).is_empty());
    assert_eq!(infcx.deeply_resolve_via_region_graph(ty).unwrap_err().unresolved, crate::next_solver::Term::Ty(ty));
    assert_eq!(infcx.deeply_resolve_via_region_graph(array).unwrap_err().unresolved, crate::next_solver::Term::Const(ct));
}
