use super::*;
use crate::tcx::Tcx;
use crate::consts::{ParamConst, AliasConstKind, IsRigid};

#[test]
fn response_replay_preserves_native_const_alias_and_external_constraints() {
    use super::response::{canonicalize_query_response, instantiate_and_apply_query_response, ResponseRelating};
    use crate::infer::relate::TypeRelation;
    let tcx = Tcx::new();
    let tys = &tcx.tys;
    let prog = crate::program::Program::new(&std::sync::Arc::new(crate::Session::default()));
    let caller = InferCtxt::new(&tcx, TypingMode::PostAnalysis, Default::default());
    let original = [GenericArg::Type(caller.next_ty_var()), GenericArg::Const(caller.next_const_var()), GenericArg::Lifetime(caller.next_region_var())];
    let query = InferCtxt::new(&tcx, TypingMode::PostAnalysis, Default::default());
    let region = tys.intern_region(RegionKind::RePlaceholder { universe: UniverseIndex::ROOT, var: BoundVar(2) });
    let alias = tys.intern_const(ConstKind::Alias(IsRigid::No, AliasConst {
        kind: AliasConstKind::Anon { def_id: DefId(1) }, args: vec![GenericArg::Lifetime(region)].into(),
    }));
    let ty = tys.mk_array(tys.u8_, alias);
    let opaque = OpaqueTypeKey { def_id: DefId(2), args: vec![GenericArg::Lifetime(region)].into() };
    let response = Response { certainty: Certainty::Yes, var_values: vec![GenericArg::Type(ty), GenericArg::Const(alias), GenericArg::Lifetime(region)],
        external_constraints: ExternalConstraintsData {
            region_constraints: vec![(RegionConstraint::RegionOutlives(tys.intern_region(RegionKind::ReStatic), region), true), (RegionConstraint::TypeOutlives(ty, region), true)],
            opaque_types: vec![(opaque, ty)], normalization_nested_goals: Vec::new(),
        },
    };
    let canonical = canonicalize_query_response(&query, UniverseIndex::ROOT, &response);
    let snapshot = caller.start_snapshot();
    let (_, certainty) = instantiate_and_apply_query_response(&caller, &prog, &original, &canonical);
    assert_eq!(certainty, Certainty::Yes);
    let GenericArg::Type(original_ty) = original[0] else { panic!() };
    let GenericArg::Const(original_ct) = original[1] else { panic!() };
    let GenericArg::Lifetime(original_region) = original[2] else { panic!() };
    let resolved_ct = caller.shallow_resolve_const(original_ct);
    let ConstKind::Alias(IsRigid::No, alias) = tys.const_kind(resolved_ct) else { panic!() };
    assert_eq!(alias.args[0], GenericArg::Lifetime(original_region));
    assert_eq!(caller.shallow_resolve(original_ty), tys.mk_array(tys.u8_, resolved_ct));
    assert_eq!(caller.inner.borrow().region_obligations().len(), 1);
    assert!(caller.inner.borrow_mut().unwrap_region_constraints().constraints().iter().all(|c| !c.visible_for_leak_check));
    let mut relation = ResponseRelating { infcx: &caller, prog: &prog };
    assert!(relation.tys(caller.shallow_resolve(original_ty), tys.mk_array(tys.u8_, resolved_ct)).is_ok());
    caller.rollback_to(snapshot);
    assert_eq!(caller.shallow_resolve(original_ty), original_ty);
    assert_eq!(caller.shallow_resolve_const(original_ct), original_ct);
    assert!(caller.inner.borrow().region_obligations().is_empty());
    assert!(caller.inner.borrow_mut().opaque_types().is_empty());
}

#[test]
fn response_replay_preserves_subtype_components_and_returns_input_placeholders() {
    use super::response::{canonicalize_query_response, instantiate_and_apply_query_response};
    let tcx = Tcx::new();
    let prog = crate::program::Program::new(&std::sync::Arc::new(crate::Session::default()));
    let caller = InferCtxt::new(&tcx, TypingMode::PostAnalysis, Default::default());
    let a = caller.next_ty_var();
    let b = caller.next_ty_var();
    let param = tcx.tys.intern(TyKind::Param(3));
    let query = InferCtxt::new(&tcx, TypingMode::PostAnalysis, Default::default());
    let values = instantiate_canonical_vars(&query, UniverseIndex::ROOT, &[
        CanonicalVarKind::Ty { ui: UniverseIndex::ROOT, sub_root: BoundVar(0) },
        CanonicalVarKind::Ty { ui: UniverseIndex::ROOT, sub_root: BoundVar(0) },
        CanonicalVarKind::PlaceholderTy(PlaceholderType { universe: UniverseIndex::ROOT, bound: BoundVar(2) }),
    ]);
    let response = canonicalize_query_response(&query, UniverseIndex::ROOT, &Response {
        certainty: Certainty::AMBIGUOUS, var_values: values, external_constraints: ExternalConstraintsData::default(),
    });
    instantiate_and_apply_query_response(&caller, &prog, &[GenericArg::Type(a), GenericArg::Type(b), GenericArg::Type(param)], &response);
    let (TyKind::Infer(av), TyKind::Infer(bv)) = (tcx.tys.kind(a), tcx.tys.kind(b)) else { panic!() };
    let mut inner = caller.inner.borrow_mut();
    let mut table = inner.type_variables();
    assert_ne!(table.root_var(*av), table.root_var(*bv));
    assert_eq!(table.sub_unification_table_root_var(*av), table.sub_unification_table_root_var(*bv));
}

#[test]
fn input_round_trip_keeps_lifetime_type_const_and_nested_binder_identity() {
    let tcx = Tcx::new();
    let tys = &tcx.tys;
    let infcx = InferCtxt::new(&tcx, TypingMode::PostAnalysis, Default::default());
    let outer = infcx.next_region_var();
    let local = tys.intern_region(RegionKind::ReBound(0, BoundVar(0)));
    let ty = infcx.next_ty_var();
    let ct = infcx.next_const_var();
    let sig = tys.intern(TyKind::FnPtr(tys.mk_list(&[tys.intern(TyKind::Ref(false, ty, local))]), tys.intern(TyKind::Ref(false, ty, outer)), FnHeader::RUST, 1));
    let alias = AliasConst { kind: AliasConstKind::Anon { def_id: DefId(9) }, args: vec![GenericArg::Lifetime(outer), GenericArg::Type(sig), GenericArg::Const(ct)].into() };
    let alias = tys.intern_const(ConstKind::Alias(IsRigid::No, alias));
    let goal = Goal { param_env: ParamEnvId::EMPTY, predicate: Predicate::dummy(PredicateKind::Clause(ClauseKind::Trait(TraitClause { trait_ref: TraitRef { def_id: DefId(1), args: vec![GenericArg::Type(sig), GenericArg::Const(alias)] }, polarity: ClausePolarity::Positive }))) };
    let (values, canonical) = canonicalize_goal(&infcx, &goal, &[], TypingMode::ErasedNotCoherence);
    assert_eq!(canonical.canonical.max_universe, UniverseIndex::ROOT);
    assert_eq!(values.len(), 3);
    let input = instantiate_query_input(&tcx, &canonical.canonical.value, &values);
    assert_eq!(input.goal, goal);
    assert!(infcx.canonicalizer_state.borrow().variables.is_empty());
    assert_eq!(canonicalize_goal(&infcx, &goal, &[], TypingMode::ErasedNotCoherence).1, canonical);
    assert!(infcx.canonicalizer_state.borrow().variables.is_empty());
}

#[test]
fn input_parameters_and_free_regions_become_root_placeholders() {
    let tcx = Tcx::new();
    let tys = &tcx.tys;
    let infcx = InferCtxt::new(&tcx, TypingMode::PostAnalysis, Default::default());
    let universe = infcx.create_next_universe();
    let ty = tys.intern(TyKind::Param(2));
    let ct = tys.intern_const(ConstKind::Param(ParamConst { index: 3 }));
    let region = tys.intern_region(RegionKind::RePlaceholder { universe, var: BoundVar(7) });
    let goal = Goal { param_env: ParamEnvId::EMPTY, predicate: Predicate::dummy(PredicateKind::Clause(ClauseKind::Trait(TraitClause { trait_ref: TraitRef { def_id: DefId(1), args: vec![GenericArg::Type(ty), GenericArg::Lifetime(region), GenericArg::Const(ct)] }, polarity: ClausePolarity::Positive }))) };
    let (_, canonical) = canonicalize_goal(&infcx, &goal, &[], TypingMode::ErasedNotCoherence);
    assert_eq!(canonical.canonical.var_kinds, vec![
        CanonicalVarKind::PlaceholderTy(PlaceholderType { universe: UniverseIndex::ROOT, bound: BoundVar(0) }),
        CanonicalVarKind::PlaceholderRegion { universe: UniverseIndex::ROOT, var: BoundVar(1) },
        CanonicalVarKind::PlaceholderConst(PlaceholderConst { universe: UniverseIndex::ROOT, bound: BoundConst { var: BoundVar(2) } }),
    ]);
}

#[test]
fn environment_static_is_preserved_without_poisoning_predicate_cache() {
    let tcx = Tcx::new();
    let tys = &tcx.tys;
    let infcx = InferCtxt::new(&tcx, TypingMode::PostAnalysis, Default::default());
    let static_ = tys.intern_region(RegionKind::ReStatic);
    let ty = tys.intern(TyKind::Ref(false, tys.u8_, static_));
    let predicate = Predicate::dummy(PredicateKind::Clause(ClauseKind::WellFormed(Term::Ty(ty))));
    let goal = Goal { param_env: tcx.intern_param_env(ParamEnv { clauses: vec![predicate.clone()].into() }), predicate: predicate.clone() };
    let (values, canonical) = canonicalize_goal(&infcx, &goal, &[], TypingMode::PostAnalysis);
    assert_eq!(tcx.param_env_by_id(canonical.canonical.value.goal.param_env).clauses[0], predicate);
    assert_ne!(canonical.canonical.value.goal.predicate, predicate);
    assert_eq!(values, vec![GenericArg::Lifetime(static_)]);
    assert_eq!(instantiate_query_input(&tcx, &canonical.canonical.value, &values).goal, goal);
}

#[test]
fn fresh_instantiation_restores_subtype_components_without_equating_them() {
    let tcx = Tcx::new();
    let infcx = InferCtxt::new(&tcx, TypingMode::PostAnalysis, Default::default());
    let a = infcx.next_ty_var();
    let b = infcx.next_ty_var();
    let (TyKind::Infer(av), TyKind::Infer(bv)) = (tcx.tys.kind(a), tcx.tys.kind(b)) else { panic!() };
    infcx.inner.borrow_mut().type_variables().sub_unify(*av, *bv);
    let goal = Goal { param_env: ParamEnvId::EMPTY, predicate: Predicate::dummy(PredicateKind::Subtype(SubtypePredicate { a_is_expected: true, a, b })) };
    let (_, canonical) = canonicalize_goal(&infcx, &goal, &[], TypingMode::PostAnalysis);
    let fresh = InferCtxt::new(&tcx, TypingMode::PostAnalysis, Default::default());
    let values = instantiate_canonical_vars(&fresh, canonical.canonical.max_universe, &canonical.canonical.var_kinds);
    assert_ne!(values[0], values[1]);
    let GenericArg::Type(a) = values[0] else { panic!() };
    let GenericArg::Type(b) = values[1] else { panic!() };
    let (TyKind::Infer(av), TyKind::Infer(bv)) = (tcx.tys.kind(a), tcx.tys.kind(b)) else { panic!() };
    let mut inner = fresh.inner.borrow_mut();
    let mut table = inner.type_variables();
    assert_eq!(table.sub_unification_table_root_var(*av), table.sub_unification_table_root_var(*bv));
    assert_ne!(table.root_var(*av), table.root_var(*bv));
}

#[test]
fn response_universes_retain_only_universes_created_inside_query() {
    let tcx = Tcx::new();
    let infcx = InferCtxt::new(&tcx, TypingMode::PostAnalysis, Default::default());
    let outer = infcx.create_next_universe();
    let a = infcx.next_ty_var();
    let inner = infcx.create_next_universe();
    let b = infcx.next_ty_var();
    let p = tcx.tys.intern_const(ConstKind::Placeholder(PlaceholderConst { universe: inner, bound: BoundConst { var: BoundVar(8) } }));
    let response = canonicalize_response(&infcx, outer, &[GenericArg::Type(a), GenericArg::Type(b), GenericArg::Const(p)]);
    assert_eq!(response.max_universe, UniverseIndex(1));
    assert_eq!(response.var_kinds[0].universe(), UniverseIndex::ROOT);
    assert_eq!(response.var_kinds[1].universe(), UniverseIndex(1));
    assert_eq!(response.var_kinds[2].universe(), UniverseIndex(1));
}

#[test]
fn numeric_variables_keep_their_class_across_canonical_instantiation() {
    let tcx = Tcx::new();
    let infcx = InferCtxt::new(&tcx, TypingMode::PostAnalysis, Default::default());
    infcx.create_next_universe();
    let a = infcx.next_int_var();
    let b = infcx.next_float_var();
    let goal = Goal { param_env: ParamEnvId::EMPTY, predicate: Predicate::dummy(PredicateKind::Clause(ClauseKind::Trait(TraitClause { trait_ref: TraitRef { def_id: DefId(1), args: vec![GenericArg::Type(a), GenericArg::Type(b)] }, polarity: ClausePolarity::Positive }))) };
    let (_, canonical) = canonicalize_goal(&infcx, &goal, &[], TypingMode::ErasedNotCoherence);
    assert_eq!(canonical.canonical.var_kinds, vec![CanonicalVarKind::Int, CanonicalVarKind::Float]);
    let values = instantiate_canonical_vars(&infcx, UniverseIndex::ROOT, &canonical.canonical.var_kinds);
    let GenericArg::Type(a) = values[0] else { panic!() };
    let GenericArg::Type(b) = values[1] else { panic!() };
    assert!(matches!(tcx.tys.kind(a), TyKind::IntVar(_)));
    assert!(matches!(tcx.tys.kind(b), TyKind::FloatVar(_)));
}

#[test]
fn parameter_environment_cache_excludes_non_region_inference_and_reuses_rigid_envs() {
    let tcx = Tcx::new();
    let infcx = InferCtxt::new(&tcx, TypingMode::PostAnalysis, Default::default());
    let param = tcx.tys.intern(TyKind::Param(0));
    let clause = Predicate::dummy(PredicateKind::Clause(ClauseKind::WellFormed(Term::Ty(param))));
    let goal = Goal { param_env: tcx.intern_param_env(ParamEnv { clauses: vec![clause.clone()].into() }), predicate: clause };
    let first = canonicalize_goal(&infcx, &goal, &[], TypingMode::PostAnalysis);
    let second = canonicalize_goal(&infcx, &goal, &[], TypingMode::PostAnalysis);
    assert_eq!(first, second);
    assert!(tcx.canonical_param_env(goal.param_env).get().is_some());
    let var = infcx.next_ty_var();
    let clause = Predicate::dummy(PredicateKind::Clause(ClauseKind::WellFormed(Term::Ty(var))));
    let goal = Goal { param_env: tcx.intern_param_env(ParamEnv { clauses: vec![clause.clone()].into() }), predicate: clause };
    canonicalize_goal(&infcx, &goal, &[], TypingMode::PostAnalysis);
    assert!(tcx.canonical_param_env(goal.param_env).get().is_none());
    infcx.at(&crate::program::Program::new(&std::sync::Arc::new(crate::Session::default())), Default::default()).eq(var, tcx.tys.u8_).unwrap();
    let (_, resolved) = canonicalize_goal(&infcx, &goal, &[], TypingMode::PostAnalysis);
    // WF(u8) has no canonical variables, so it is returned directly without filling a cell.
    let env = resolved.canonical.value.goal.param_env;
    assert_eq!(tcx.param_env_by_id(env).clauses[0],
        Predicate::dummy(PredicateKind::Clause(ClauseKind::WellFormed(Term::Ty(tcx.tys.u8_)))));
    assert!(tcx.canonical_param_env(env).get().is_none());
}

#[test]
fn native_array_lengths_and_nominal_const_slots_round_trip() {
    let tcx = Tcx::new();
    let tys = &tcx.tys;
    let infcx = InferCtxt::new(&tcx, TypingMode::PostAnalysis, Default::default());
    let ct = infcx.next_const_var();
    let array = tys.mk_array(tys.u8_, ct);
    let nominal = tys.intern(TyKind::Adt(DefId(1), tys.mk_list(&[tys.intern(TyKind::Const(ct)), array]), crate::types::RegionArgsId::EMPTY));
    let goal = Goal { param_env: ParamEnvId::EMPTY, predicate: Predicate::dummy(PredicateKind::Clause(ClauseKind::WellFormed(Term::Ty(nominal)))) };
    let (values, canonical) = canonicalize_goal(&infcx, &goal, &[], TypingMode::ErasedNotCoherence);
    assert_eq!(values, vec![GenericArg::Const(ct)]);
    assert_eq!(instantiate_query_input(&tcx, &canonical.canonical.value, &values).goal, goal);
    let other = InferCtxt::new(&tcx, TypingMode::PostAnalysis, Default::default());
    let fresh = instantiate_canonical_vars(&other, canonical.canonical.max_universe, &canonical.canonical.var_kinds);
    let input = instantiate_query_input(&tcx, &canonical.canonical.value, &fresh);
    let PredicateKind::Clause(ClauseKind::WellFormed(Term::Ty(ty))) = input.goal.predicate.kind else { panic!() };
    assert!(tys.has_infer(ty));
}
