use crate::types::AliasTyKind;
use crate::types::{AliasTy};
use super::*;
use crate::types::{BoundVar, RegionKind, Types};
use type_variable::TypeVariableValue;
use opaque_types::{OpaqueTypeKey, ProvisionalHiddenType};
use crate::program::DefId;

fn placeholder(tys: &Types, var: u32) -> RegionId {
    tys.intern_region(RegionKind::RePlaceholder { universe: UniverseIndex(1), var: BoundVar(var) })
}

#[test]
fn placeholder_outliving_outer_region_leaks_but_reverse_does_not() {
    let tcx = Tcx::new();
    let tys = &tcx.tys;
    let infcx = InferCtxt::new(&tcx, TypingMode::PostAnalysis, Default::default());
    let outer = infcx.inner.borrow_mut().unwrap_region_constraints().new_region_var(tys, UniverseIndex::ROOT, 0);
    let p = placeholder(tys, 0);
    let snapshot = infcx.start_snapshot();
    infcx.inner.borrow_mut().unwrap_region_constraints().make_subregion(tys, crate::infer::region_constraints::SubregionOrigin::Misc, p, outer, true);
    assert!(infcx.inner.borrow_mut().unwrap_region_constraints().leak_check(tys, UniverseIndex::ROOT, UniverseIndex(1), Some(&snapshot)).is_ok());
    infcx.rollback_to(snapshot);
    let snapshot = infcx.start_snapshot();
    infcx.inner.borrow_mut().unwrap_region_constraints().make_subregion(tys, crate::infer::region_constraints::SubregionOrigin::Misc, outer, p, true);
    assert!(infcx.inner.borrow_mut().unwrap_region_constraints().leak_check(tys, UniverseIndex::ROOT, UniverseIndex(1), Some(&snapshot)).is_err());
    infcx.rollback_to(snapshot);
}

#[test]
fn distinct_placeholders_cannot_outlive_each_other() {
    let tcx = Tcx::new();
    let tys = &tcx.tys;
    let infcx = InferCtxt::new(&tcx, TypingMode::PostAnalysis, Default::default());
    let p = placeholder(tys, 0);
    let q = placeholder(tys, 1);
    infcx.inner.borrow_mut().unwrap_region_constraints().make_subregion(tys, crate::infer::region_constraints::SubregionOrigin::Misc, p, q, true);
    assert!(infcx.inner.borrow_mut().unwrap_region_constraints().leak_check(tys, UniverseIndex::ROOT, UniverseIndex(1), None).is_err());
}

#[test]
fn static_equality_is_visible_and_snapshot_scoped() {
    let tcx = Tcx::new();
    let tys = &tcx.tys;
    let infcx = InferCtxt::new(&tcx, TypingMode::PostAnalysis, Default::default());
    let p = placeholder(tys, 0);
    let static_ = tys.intern_region(RegionKind::ReStatic);
    infcx.inner.borrow_mut().unwrap_region_constraints().make_eqregion(tys, crate::infer::region_constraints::SubregionOrigin::Misc, p, static_, true);
    assert!(infcx.inner.borrow_mut().unwrap_region_constraints().leak_check(tys, UniverseIndex::ROOT, UniverseIndex(1), None).is_err());
    let snapshot = infcx.start_snapshot();
    assert!(infcx.inner.borrow_mut().unwrap_region_constraints().leak_check(tys, UniverseIndex::ROOT, UniverseIndex(1), Some(&snapshot)).is_ok());
    infcx.rollback_to(snapshot);
}

#[test]
fn failed_opportunistic_unification_retains_leak_constraint() {
    let tcx = Tcx::new();
    let tys = &tcx.tys;
    let infcx = InferCtxt::new(&tcx, TypingMode::PostAnalysis, Default::default());
    let outer = infcx.inner.borrow_mut().unwrap_region_constraints().new_region_var(tys, UniverseIndex::ROOT, 0);
    let snapshot = infcx.start_snapshot();
    infcx.inner.borrow_mut().unwrap_region_constraints().make_eqregion(tys, crate::infer::region_constraints::SubregionOrigin::Misc, outer, placeholder(tys, 0), true);
    assert_eq!(infcx.inner.borrow_mut().unwrap_region_constraints().shallow_resolve_region_var(tys, 0), outer);
    assert!(infcx.inner.borrow_mut().unwrap_region_constraints().leak_check(tys, UniverseIndex::ROOT, UniverseIndex(1), None).is_err());
    infcx.rollback_to(snapshot);
    assert!(infcx.inner.borrow_mut().unwrap_region_constraints().constraints().is_empty());
    assert!(infcx.inner.borrow_mut().unwrap_region_constraints().leak_check(tys, UniverseIndex::ROOT, UniverseIndex(1), None).is_ok());
}

#[test]
fn invisible_constraints_and_outer_placeholders_are_not_leaks() {
    let tcx = Tcx::new();
    let tys = &tcx.tys;
    let infcx = InferCtxt::new(&tcx, TypingMode::PostAnalysis, Default::default());
    infcx.inner.borrow_mut().unwrap_region_constraints().make_eqregion(tys, crate::infer::region_constraints::SubregionOrigin::Misc, placeholder(tys, 0), placeholder(tys, 1), false);
    assert!(infcx.inner.borrow_mut().unwrap_region_constraints().leak_check(tys, UniverseIndex::ROOT, UniverseIndex(1), None).is_ok());
    infcx.inner.borrow_mut().unwrap_region_constraints().make_eqregion(tys, crate::infer::region_constraints::SubregionOrigin::Misc, placeholder(tys, 0), placeholder(tys, 1), true);
    assert!(infcx.inner.borrow_mut().unwrap_region_constraints().leak_check(tys, UniverseIndex(1), UniverseIndex(2), None).is_ok());
}

#[test]
fn subtype_components_do_not_equate_variables() {
    let tcx = Tcx::new();
    let infcx = InferCtxt::new(&tcx, TypingMode::PostAnalysis, Default::default());
    let mut inner = infcx.inner.borrow_mut();
    let mut vars = inner.type_variables();
    let a = vars.new_var(UniverseIndex::ROOT);
    let b = vars.new_var(UniverseIndex(1));
    vars.sub_unify(a, b);
    assert_eq!(vars.sub_unification_table_root_var(a), vars.sub_unification_table_root_var(b));
    assert_ne!(vars.root_var(a), vars.root_var(b));
    vars.instantiate(a, tcx.tys.bool_);
    assert_eq!(vars.probe(b), TypeVariableValue::Unknown { universe: UniverseIndex(1) });
}

#[test]
fn combined_nested_commit_rolls_back_with_outer_snapshot() {
    let tcx = Tcx::new();
    let infcx = InferCtxt::new(&tcx, TypingMode::PostAnalysis, Default::default());
    let a = infcx.inner.borrow_mut().type_variables().new_var(UniverseIndex(2));
    let key = OpaqueTypeKey { def_id: DefId(0), args: [].into() };
    let original = ProvisionalHiddenType::new(tcx.tys.bool_);
    infcx.inner.borrow_mut().opaque_types().register(key.clone(), original);
    infcx.probe(|outer| {
        let b = infcx.inner.borrow_mut().type_variables().new_var(UniverseIndex::ROOT);
        infcx.inner.borrow_mut().type_variables().equate(a, b);
        assert_eq!(infcx.inner.borrow_mut().type_variables().probe(a), TypeVariableValue::Unknown { universe: UniverseIndex::ROOT });
        let _: Result<(), ()> = infcx.commit_if_ok(|_| {
            infcx.inner.borrow_mut().type_variables().instantiate(b, tcx.tys.char_);
            let region = infcx.next_region_var();
            let universe = infcx.create_next_universe();
            let p = tcx.tys.intern_region(RegionKind::RePlaceholder { universe, var: BoundVar(0) });
            infcx.inner.borrow_mut().unwrap_region_constraints().make_eqregion(&tcx.tys, crate::infer::region_constraints::SubregionOrigin::Misc, region, p, true);
            assert_eq!(infcx.inner.borrow_mut().opaque_types().register(key.clone(), ProvisionalHiddenType::new(tcx.tys.char_)), Some(tcx.tys.bool_));
            infcx.inner.borrow_mut().opaque_types().add_duplicate(key.clone(), original);
            Ok(())
        });
        assert!(infcx.region_constraints_added_in_snapshot(outer));
        assert!(infcx.opaque_types_added_in_snapshot(outer));
        assert_eq!(infcx.inner.borrow_mut().type_variables().probe(a).known(), Some(tcx.tys.char_));
        assert_eq!(infcx.num_open_snapshots(), 1);
    });
    assert!(!infcx.in_snapshot());
    assert_eq!(infcx.universe(), UniverseIndex::ROOT);
    let mut inner = infcx.inner.borrow_mut();
    assert_eq!(inner.type_variables().num_vars(), 1);
    assert_eq!(inner.type_variables().probe(a), TypeVariableValue::Unknown { universe: UniverseIndex(2) });
    assert_eq!(inner.type_variables().sub_unification_table_root_var(a), a);
    assert_eq!(inner.unwrap_region_constraints().num_region_vars(), 0);
    assert!(inner.unwrap_region_constraints().constraints().is_empty());
    assert_eq!(inner.opaque_types().iter_lookup_table(), &[(key, original)]);
    assert!(inner.opaque_types().iter_duplicate_entries().is_empty());
}

#[test]
fn failed_commit_restores_new_variables_and_opaque_keys() {
    let tcx = Tcx::new();
    let infcx = InferCtxt::new(&tcx, TypingMode::PostAnalysis, Default::default());
    let result = infcx.commit_if_ok(|_| {
        infcx.next_ty_var();
        infcx.next_region_var();
        infcx.create_next_universe();
        infcx.inner.borrow_mut().opaque_types().register(OpaqueTypeKey { def_id: DefId(1), args: [].into() }, ProvisionalHiddenType::new(tcx.tys.bool_));
        Err::<(), _>(7)
    });
    assert_eq!(result, Err(7));
    assert!(!infcx.in_snapshot());
    assert_eq!(infcx.universe(), UniverseIndex::ROOT);
    assert_eq!(infcx.inner.borrow_mut().type_variables().num_vars(), 0);
    assert_eq!(infcx.inner.borrow_mut().unwrap_region_constraints().num_region_vars(), 0);
    assert!(infcx.inner.borrow_mut().opaque_types().is_empty());
}

#[test]
fn const_and_numeric_tables_follow_combined_snapshots() {
    use crate::consts::{Const, ConstKind, ConstVid, InferConst, ScalarInt, ValTree};
    use crate::program::Prim;
    use unify_key::{ConstVariableValue, FloatVarValue, IntVarValue};
    let tcx = Tcx::new();
    let infcx = InferCtxt::new(&tcx, TypingMode::PostAnalysis, Default::default());
    let ct = infcx.next_const_var_in_universe(UniverseIndex(2));
    let ConstKind::Infer(InferConst::Var(ConstVid(a))) = tcx.tys.const_kind(ct) else { panic!() };
    let int = infcx.inner.borrow_mut().int_unification_table().new_key(IntVarValue::Unknown);
    let float = infcx.inner.borrow_mut().float_unification_table().new_key(FloatVarValue::Unknown);
    let value = Const::new_value(&tcx.tys, ValTree::from_scalar_int(&tcx.tys,
        ScalarInt::try_from_uint(1u128 << 127, 16).unwrap()), tcx.tys.intern(TyKind::Int(Prim::U128)));
    infcx.probe(|_| {
        let b = infcx.inner.borrow_mut().const_unification_table().new_key(ConstVariableValue::Unknown { universe: UniverseIndex::ROOT });
        infcx.inner.borrow_mut().const_unification_table().union(a, b);
        assert_eq!(infcx.inner.borrow_mut().const_unification_table().probe_value(a), ConstVariableValue::Unknown { universe: UniverseIndex::ROOT });
        infcx.inner.borrow_mut().const_unification_table().union_value(b, ConstVariableValue::Known { value });
        infcx.inner.borrow_mut().int_unification_table().union_value(int, IntVarValue::UintType(Prim::U128));
        infcx.inner.borrow_mut().float_unification_table().union_value(float, FloatVarValue::Known(Prim::F64));
        assert_eq!(infcx.shallow_resolve_const(ct), value);
        let _: Result<(), ()> = infcx.commit_if_ok(|_| {
            infcx.inner.borrow_mut().int_unification_table().new_key(IntVarValue::Unknown);
            infcx.inner.borrow_mut().float_unification_table().new_key(FloatVarValue::Unknown);
            Ok(())
        });
    });
    assert_eq!(infcx.shallow_resolve_const(ct), ct);
    let mut inner = infcx.inner.borrow_mut();
    assert_eq!(inner.const_unification_table().len(), 1);
    assert_eq!(inner.const_unification_table().probe_value(a), ConstVariableValue::Unknown { universe: UniverseIndex(2) });
    assert_eq!(inner.int_unification_table().len(), 1);
    assert_eq!(inner.float_unification_table().len(), 1);
    assert_eq!(inner.int_unification_table().probe_value(int), IntVarValue::Unknown);
    assert_eq!(inner.float_unification_table().probe_value(float), FloatVarValue::Unknown);
}

#[test]
fn solver_relating_preserves_alias_obligations_and_subtype_direction() {
    let prog = crate::program::Program::new(&std::sync::Arc::new(crate::Session::default()));
    use crate::next_solver::{PredicateKind, ClauseKind, Term};
    use crate::types::RegionArgsId;
    let tcx = Tcx::new();
    let infcx = InferCtxt::new(&tcx, TypingMode::PostAnalysis, Default::default());
    let var = infcx.next_ty_var();
    let alias = tcx.tys.intern(TyKind::Alias(crate::consts::IsRigid::No, AliasTy { kind: AliasTyKind::Projection { def_id: DefId(55) }, args: tcx.tys.mk_list(&[var]), regions: RegionArgsId::EMPTY }));
    let goals = infcx.at(&prog, Default::default()).eq(var, alias).unwrap();
    assert_eq!(goals.len(), 1);
    let PredicateKind::Clause(ClauseKind::Projection(projection)) = &goals[0].predicate.kind else { panic!() };
    let Term::Ty(output) = projection.term else { panic!() };
    assert_eq!(infcx.shallow_resolve(output), infcx.shallow_resolve(var));
    let other = infcx.next_ty_var();
    let goals = infcx.at(&prog, Default::default()).sub(var, other).unwrap();
    assert_eq!(goals.len(), 1);
    let PredicateKind::Subtype(subtype) = &goals[0].predicate.kind else { panic!() };
    assert_eq!(subtype.a, infcx.shallow_resolve(var));
    assert_eq!(subtype.b, other);
    assert!(subtype.a_is_expected);
    assert_ne!(infcx.shallow_resolve(var), infcx.shallow_resolve(other));
}

#[test]
fn generalization_rejects_cycles_and_rolls_back_partial_bindings() {
    let prog = crate::program::Program::new(&std::sync::Arc::new(crate::Session::default()));
    let tcx = Tcx::new();
    let infcx = InferCtxt::new(&tcx, TypingMode::PostAnalysis, Default::default());
    let var = infcx.next_ty_var();
    let recursive = tcx.tys.intern(TyKind::Tuple(tcx.tys.mk_list(&[var])));
    assert!(infcx.at(&prog, Default::default()).eq(var, recursive).is_err());
    assert_eq!(infcx.shallow_resolve(var), var);
    let lhs = tcx.tys.intern(TyKind::Tuple(tcx.tys.mk_list(&[var, tcx.tys.bool_])));
    let rhs = tcx.tys.intern(TyKind::Tuple(tcx.tys.mk_list(&[tcx.tys.u8_, tcx.tys.usize_])));
    // rustc_infer/src/infer/at.rs:131-230 (round-28 D2): At does not roll back; the caller's
    // commit_if_ok boundary does.
    assert!(infcx.commit_if_ok(|_| infcx.at(&prog, Default::default()).eq(lhs, rhs)).is_err());
    assert_eq!(infcx.shallow_resolve(var), var);
    assert_eq!(infcx.num_open_snapshots(), 0);
}

#[test]
fn higher_ranked_relation_leak_check_distinguishes_static_and_general_pointer() {
    let prog = crate::program::Program::new(&std::sync::Arc::new(crate::Session::default()));
    use crate::types::FnHeader;
    let tcx = Tcx::new();
    let tys = &tcx.tys;
    let infcx = InferCtxt::new(&tcx, TypingMode::PostAnalysis, Default::default());
    let bound = tys.intern_region(RegionKind::ReBound(0, BoundVar(0)));
    let static_ = tys.intern_region(RegionKind::ReStatic);
    let general = tys.intern(TyKind::FnPtr(tys.mk_list(&[tys.intern(TyKind::Ref(false, tys.u8_, bound))]), tys.unit, FnHeader::RUST, 1));
    let specific = tys.intern(TyKind::FnPtr(tys.mk_list(&[tys.intern(TyKind::Ref(false, tys.u8_, static_))]), tys.unit, FnHeader::RUST, 0));
    let outer = infcx.universe();
    infcx.probe(|snapshot| {
        assert!(infcx.at(&prog, Default::default()).eq(general, specific).is_ok());
        assert!(infcx.leak_check(outer, Some(snapshot)).is_err());
    });
    infcx.probe(|snapshot| {
        assert!(infcx.at(&prog, Default::default()).sub(general, specific).is_ok());
        assert!(infcx.leak_check(outer, Some(snapshot)).is_ok());
    });
    infcx.probe(|snapshot| {
        assert!(infcx.at(&prog, Default::default()).sub(specific, general).is_ok());
        assert!(infcx.leak_check(outer, Some(snapshot)).is_err());
    });
    assert_eq!(infcx.universe(), outer);
}

#[test]
fn subtype_generalization_freshens_regions_and_preserves_invariance() {
    let prog = crate::program::Program::new(&std::sync::Arc::new(crate::Session::default()));
    let tcx = Tcx::new();
    let tys = &tcx.tys;
    let infcx = InferCtxt::new(&tcx, TypingMode::PostAnalysis, Default::default());
    let static_ = tys.intern_region(RegionKind::ReStatic);
    let source = tys.intern(TyKind::Ref(false, tys.u8_, static_));
    let target = infcx.next_ty_var();
    infcx.at(&prog, Default::default()).sub(target, source).unwrap();
    let TyKind::Ref(_, _, region) = tys.kind(infcx.shallow_resolve(target)) else { panic!() };
    assert!(matches!(tys.region_kind(*region), RegionKind::ReVar(_)));
    let invariant = infcx.next_ty_var();
    infcx.at(&prog, Default::default()).eq(invariant, source).unwrap();
    assert_eq!(infcx.shallow_resolve(invariant), source);
}

#[test]
fn generalization_rejects_unnameable_type_and_const_placeholders() {
    let prog = crate::program::Program::new(&std::sync::Arc::new(crate::Session::default()));
    use crate::consts::{ConstKind, PlaceholderConst, BoundConst};
    use crate::infer::relate::{TypeRelation, solver_relating::SolverRelating};
    use crate::types::{PlaceholderType, variance::V};
    let tcx = Tcx::new();
    let infcx = InferCtxt::new(&tcx, TypingMode::PostAnalysis, Default::default());
    let ty = infcx.next_ty_var();
    let ct = infcx.next_const_var();
    let universe = infcx.create_next_universe();
    let p = tcx.tys.intern(TyKind::Placeholder(PlaceholderType { universe, bound: BoundVar(0) }));
    assert!(infcx.at(&prog, Default::default()).eq(ty, p).is_err());
    let p = tcx.tys.intern_const(ConstKind::Placeholder(PlaceholderConst { universe, bound: BoundConst { var: BoundVar(0) } }));
    infcx.probe(|_| {
        let mut relation = SolverRelating::new(&infcx, &prog, V::Inv, Default::default());
        assert!(relation.consts(ct, p).is_err());
    });
    assert_eq!(infcx.shallow_resolve(ty), ty);
    assert_eq!(infcx.shallow_resolve_const(ct), ct);
}

#[test]
fn numeric_relations_restrict_their_class_and_rollback_assignments() {
    let prog = crate::program::Program::new(&std::sync::Arc::new(crate::Session::default()));
    use crate::program::Prim;
    let tcx = Tcx::new();
    let infcx = InferCtxt::new(&tcx, TypingMode::PostAnalysis, Default::default());
    let int = infcx.next_int_var();
    let float = infcx.next_float_var();
    let f32_ = tcx.tys.intern(TyKind::Float(Prim::F32));
    let general = infcx.next_ty_var();
    infcx.at(&prog, Default::default()).eq(general, int).unwrap();
    assert!(infcx.at(&prog, Default::default()).eq(int, float).is_err());
    assert!(infcx.at(&prog, Default::default()).eq(int, f32_).is_err());
    assert!(infcx.at(&prog, Default::default()).eq(float, tcx.tys.u8_).is_err());
    infcx.probe(|_| {
        infcx.at(&prog, Default::default()).eq(int, tcx.tys.u8_).unwrap();
        infcx.at(&prog, Default::default()).eq(float, f32_).unwrap();
        assert_eq!(infcx.shallow_resolve(int), tcx.tys.u8_);
        assert_eq!(infcx.shallow_resolve(general), tcx.tys.u8_);
        assert_eq!(infcx.shallow_resolve(float), f32_);
    });
    assert_eq!(infcx.shallow_resolve(int), int);
    assert_eq!(infcx.shallow_resolve(float), float);
}

#[test]
fn binder_instantiation_preserves_all_three_kinds_and_shares_repeated_variables() {
    use crate::consts::{ConstKind, BoundConst, GenericArg};
    use crate::types::{BoundTy, BoundTyKind};
    let tcx = Tcx::new();
    let tys = &tcx.tys;
    let infcx = InferCtxt::new(&tcx, TypingMode::PostAnalysis, Default::default());
    let args = vec![
        GenericArg::Lifetime(tys.intern_region(RegionKind::ReBound(0, BoundVar(0)))),
        GenericArg::Type(tys.intern(TyKind::Bound(0, BoundTy { var: BoundVar(1), kind: BoundTyKind::Anon }))),
        GenericArg::Const(tys.intern_const(ConstKind::Bound(0, BoundConst { var: BoundVar(2) }))),
        GenericArg::Const(tys.intern_const(ConstKind::Bound(0, BoundConst { var: BoundVar(2) }))),
    ];
    let placeholders = infcx.enter_forall_args_and_leak_universe(&args);
    let universe = infcx.universe();
    assert_eq!(universe, UniverseIndex(1));
    let GenericArg::Lifetime(region) = placeholders[0] else { panic!() };
    assert_eq!(tys.region_kind(region), RegionKind::RePlaceholder { universe, var: BoundVar(0) });
    let GenericArg::Type(ty) = placeholders[1] else { panic!() };
    assert!(matches!(tys.kind(ty), TyKind::Placeholder(p) if p.universe == universe));
    let GenericArg::Const(ct) = placeholders[2] else { panic!() };
    assert!(matches!(tys.const_kind(ct), ConstKind::Placeholder(p) if p.universe == universe));
    assert_eq!(placeholders[2], placeholders[3]);
    let inferred = infcx.instantiate_binder_args_with_infer(&args);
    assert_eq!(inferred[2], inferred[3]);
    assert_ne!(inferred, placeholders);
}

#[test]
fn dynamic_projection_binders_preserve_region_constraints() {
    let prog = crate::program::Program::new(&std::sync::Arc::new(crate::Session::default()));
    use crate::types::RegionArgsId;
    let tcx = Tcx::new();
    let tys = &tcx.tys;
    let infcx = InferCtxt::new(&tcx, TypingMode::PostAnalysis, Default::default());
    let bound = tys.intern_region(RegionKind::ReBound(0, BoundVar(0)));
    let static_ = tys.intern_region(RegionKind::ReStatic);
    let general = tys.intern(TyKind::Dyn(DefId(1), crate::types::List::EMPTY, tys.mk_list(&[(DefId(2), tys.intern(TyKind::Ref(false, tys.u8_, bound)), 1)]), RegionArgsId::EMPTY, static_, 0, crate::types::List::EMPTY));
    let specific = tys.intern(TyKind::Dyn(DefId(1), crate::types::List::EMPTY, tys.mk_list(&[(DefId(2), tys.intern(TyKind::Ref(false, tys.u8_, static_)), 0)]), RegionArgsId::EMPTY, static_, 0, crate::types::List::EMPTY));
    let outer = infcx.universe();
    infcx.probe(|snapshot| {
        infcx.at(&prog, Default::default()).eq(general, specific).unwrap();
        assert!(infcx.leak_check(outer, Some(snapshot)).is_err());
    });
}

#[test]
fn const_relation_defers_nonrigid_aliases_but_checks_rigid_occurs_cycles() {
    let prog = crate::program::Program::new(&std::sync::Arc::new(crate::Session::default()));
    use crate::consts::{AliasConst, AliasConstKind, ConstKind, GenericArg, IsRigid};
    use crate::infer::relate::{TypeRelation, solver_relating::SolverRelating};
    use crate::next_solver::{PredicateKind, ClauseKind};
    use crate::types::variance::V;
    let tcx = Tcx::new();
    let infcx = InferCtxt::new(&tcx, TypingMode::PostAnalysis, Default::default());
    let var = infcx.next_const_var();
    let alias = AliasConst { kind: AliasConstKind::Anon { def_id: DefId(1) }, args: vec![GenericArg::Const(var)].into() };
    let normalizable = tcx.tys.intern_const(ConstKind::Alias(IsRigid::No, alias.clone()));
    let rigid = tcx.tys.intern_const(ConstKind::Alias(IsRigid::Yes, alias));
    let mut relation = SolverRelating::new(&infcx, &prog, V::Inv, Default::default());
    relation.consts(var, normalizable).unwrap();
    assert!(matches!(relation.goals[0].predicate.kind, PredicateKind::Clause(ClauseKind::Projection(_))));
    assert_eq!(infcx.shallow_resolve_const(var), var);
    infcx.probe(|_| { assert!(relation.consts(var, rigid).is_err()); });
}

#[test]
fn native_array_lengths_relate_and_rollback_with_elements() {
    let prog = crate::program::Program::new(&std::sync::Arc::new(crate::Session::default()));
    let tcx = Tcx::new();
    let tys = &tcx.tys;
    let infcx = InferCtxt::new(&tcx, TypingMode::PostAnalysis, Default::default());
    let element = infcx.next_ty_var();
    let length = infcx.next_const_var();
    let inferred = tys.mk_array(element, length);
    let concrete = tys.mk_array(tys.u32_, tys.const_usize(4));
    assert!(tys.has_infer(inferred));
    infcx.probe(|_| {
        assert!(infcx.at(&prog, Default::default()).eq(inferred, concrete).unwrap().is_empty());
        assert_eq!(infcx.shallow_resolve_const(length), tys.const_usize(4));
        assert_eq!(infcx.deeply_resolve_ignoring_regions(inferred), concrete);
    });
    assert_eq!(infcx.shallow_resolve_const(length), length);
    assert_eq!(infcx.shallow_resolve(element), element);
    let mismatch = tys.mk_array(tys.u8_, tys.const_usize(5));
    infcx.at(&prog, Default::default()).eq(inferred, concrete).unwrap();
    assert!(infcx.at(&prog, Default::default()).eq(inferred, mismatch).is_err());
}

#[test]
fn native_array_lengths_preserve_const_occurs_and_alias_goals() {
    let prog = crate::program::Program::new(&std::sync::Arc::new(crate::Session::default()));
    use crate::consts::{AliasConst, AliasConstKind, ConstKind, GenericArg, IsRigid};
    use crate::next_solver::{PredicateKind, ClauseKind};
    let tcx = Tcx::new();
    let tys = &tcx.tys;
    let infcx = InferCtxt::new(&tcx, TypingMode::PostAnalysis, Default::default());
    let length = infcx.next_const_var();
    let nested = tys.mk_array(tys.u8_, length);
    let alias = AliasConst { kind: AliasConstKind::Anon { def_id: DefId(1) }, args: vec![GenericArg::Type(nested)].into() };
    let rigid = tys.intern_const(ConstKind::Alias(IsRigid::Yes, alias.clone()));
    assert!(infcx.at(&prog, Default::default()).eq(nested, tys.mk_array(tys.u8_, rigid)).is_err());
    assert_eq!(infcx.shallow_resolve_const(length), length);
    let normalizable = tys.intern_const(ConstKind::Alias(IsRigid::No, alias));
    let goals = infcx.at(&prog, Default::default()).eq(nested, tys.mk_array(tys.u8_, normalizable)).unwrap();
    assert!(goals.iter().any(|g| matches!(g.predicate.kind, PredicateKind::Clause(ClauseKind::Projection(_)))));
}

#[test]
fn native_array_const_binder_instantiation_keeps_nested_depth() {
    use crate::consts::{BoundConst, ConstKind, GenericArg};
    let tcx = Tcx::new();
    let tys = &tcx.tys;
    let infcx = InferCtxt::new(&tcx, TypingMode::PostAnalysis, Default::default());
    let bound = tys.intern_const(ConstKind::Bound(0, BoundConst { var: BoundVar(0) }));
    let array = tys.mk_array(tys.u32_, bound);
    let values = infcx.instantiate_binder_args_with_infer(&[GenericArg::Type(array), GenericArg::Const(bound)]);
    let GenericArg::Type(array) = values[0] else { panic!() };
    let GenericArg::Const(ct) = values[1] else { panic!() };
    assert!(matches!(tys.kind(array), TyKind::Array(_, n) if *n == ct));
    assert!(matches!(tys.const_kind(ct), ConstKind::Infer(_)));
}

#[test]
fn fudge_resolves_expectations_and_replaces_popped_variables_of_each_kind() {
    let prog = crate::program::Program::new(&std::sync::Arc::new(crate::Session::default()));
    let tcx = Tcx::new();
    let tys = &tcx.tys;
    let infcx = InferCtxt::new(&tcx, TypingMode::PostAnalysis, Default::default());
    let outer = infcx.next_ty_var();
    let result: Result<TyId, ()> = infcx.fudge_inference_if_ok(|| {
        let region = infcx.next_region_var();
        let array = tys.mk_array(infcx.next_ty_var(), infcx.next_const_var());
        let reference = tys.intern(TyKind::Ref(false, array, region));
        infcx.at(&prog, Default::default()).eq(outer, reference).unwrap();
        Ok(tys.intern(TyKind::Tuple(tys.mk_list(&[outer, infcx.next_int_var(), infcx.next_float_var()]))))
    });
    assert_eq!(infcx.shallow_resolve(outer), outer);
    let result = result.unwrap();
    let TyKind::Tuple(fields) = tys.kind(result) else { panic!() };
    let fields = tys.list(*fields);
    let reference = fields[0];
    let TyKind::Ref(_, array, region) = tys.kind(reference) else { panic!() };
    let TyKind::Array(element, length) = tys.kind(*array) else { panic!() };
    infcx.at(&prog, Default::default()).eq(*element, tys.u8_).unwrap();
    infcx.at(&prog, Default::default()).eq(*array, tys.mk_array(tys.u8_, tys.const_usize(3))).unwrap();
    infcx.at(&prog, Default::default()).eq(fields[1], tys.i32_).unwrap();
    infcx.at(&prog, Default::default()).eq(fields[2], tys.f64_).unwrap();
    assert_eq!(infcx.shallow_resolve_const(*length), tys.const_usize(3));
    let RegionKind::ReVar(v) = tys.region_kind(*region) else { panic!() };
    assert!((v as usize) < infcx.inner.borrow_mut().unwrap_region_constraints().num_region_vars());
}

#[test]
fn rigid_aliases_relate_structurally_while_all_nonrigid_kinds_emit_goals() {
    let prog = crate::program::Program::new(&std::sync::Arc::new(crate::Session::default()));
    use crate::consts::IsRigid;
    use crate::types::{AliasTy, AliasTyKind, RegionArgsId};
    let tcx = Tcx::new();
    let tys = &tcx.tys;
    let infcx = InferCtxt::new(&tcx, TypingMode::PostAnalysis, Default::default());
    for kind in [AliasTyKind::Projection { def_id: DefId(1) }, AliasTyKind::Inherent { def_id: DefId(2) }, AliasTyKind::Free { def_id: DefId(3) }] {
        let var = infcx.next_ty_var();
        let rigid = tys.intern(TyKind::Alias(IsRigid::Yes, AliasTy { kind, args: tys.mk_list(&[var]), regions: RegionArgsId::EMPTY }));
        let other = tys.intern(TyKind::Alias(IsRigid::Yes, AliasTy { kind, args: tys.mk_list(&[tys.u8_]), regions: RegionArgsId::EMPTY }));
        assert!(infcx.at(&prog, Default::default()).eq(rigid, other).unwrap().is_empty());
        assert_eq!(infcx.shallow_resolve(var), tys.u8_);
        let alias = tys.intern(TyKind::Alias(IsRigid::No, AliasTy { kind, args: tys.mk_list(&[tys.u8_]), regions: RegionArgsId::EMPTY }));
        let result = infcx.at(&prog, Default::default()).eq(alias, tys.bool_).unwrap();
        assert_eq!(result.len(), 1);
    }
}

#[test]
fn round10_leak_check_does_not_resolve_free_region_obligations() {
    use crate::types::{RegionScope, LateParamKind};
    let tcx = Tcx::new();
    let infcx = InferCtxt::new(&tcx, TypingMode::Typeck { defining_opaque_types_and_generators: tcx.intern_defining_opaques(Vec::new()) }, Default::default());
    let a = tcx.tys.intern_region(RegionKind::ReLateParam(RegionScope::Item(DefId(1)), LateParamKind::Anon(0)));
    let b = tcx.tys.intern_region(RegionKind::ReLateParam(RegionScope::Item(DefId(1)), LateParamKind::Anon(1)));
    let ty = tcx.tys.intern(TyKind::Ref(false, tcx.tys.bool_, b));
    infcx.register_ty_outlives(ty, a);
    infcx.inner.borrow_mut().unwrap_region_constraints().make_subregion(&tcx.tys, crate::infer::region_constraints::SubregionOrigin::Misc, a, b, true);
    // compare_impl_item.rs:697 needs resolve_regions_and_report_errors here.
    // A leak check only checks universe escape; these are ordinary free regions.
    assert!(infcx.leak_check(UniverseIndex::ROOT, None).is_ok());
    assert_eq!(infcx.inner.borrow().region_obligations(), &[(ty, a, 0)]);
    assert_eq!(infcx.inner.borrow_mut().unwrap_region_constraints().constraints().len(), 1);
}

#[test]
fn round28_skip_leak_check_is_explicit() {
    let tcx = Tcx::new();
    let mut infcx = InferCtxt::new(&tcx, TypingMode::Coherence, Default::default());
    let variable = infcx.next_region_var();
    let universe = infcx.create_next_universe();
    let p = tcx.tys.intern_region(RegionKind::RePlaceholder { universe, var: BoundVar(0) });
    infcx.inner.borrow_mut().unwrap_region_constraints().make_subregion(&tcx.tys, crate::infer::region_constraints::SubregionOrigin::Misc, variable, p, true);
    assert!(infcx.leak_check(UniverseIndex::ROOT, None).is_err());
    infcx.skip_leak_check = true;
    assert!(infcx.leak_check(UniverseIndex::ROOT, None).is_ok());
}

#[test]
fn round29_lattice_regions_variance_and_rollback() {
    use crate::infer::relate::TypeRelation;
    use crate::infer::relate::lattice::{LatticeOp, LatticeOpKind};
    let tcx = Tcx::new();
    let prog = crate::program::Program::new(&std::sync::Arc::new(crate::Session::default()));
    let infcx = InferCtxt::new(&tcx, TypingMode::PostAnalysis, Default::default());
    let a = tcx.tys.intern_region(RegionKind::ReEarlyParam(0));
    let b = tcx.tys.intern_region(RegionKind::ReEarlyParam(1));
    let at = tcx.tys.intern(TyKind::Ref(false, tcx.tys.u32_, a));
    let bt = tcx.tys.intern(TyKind::Ref(false, tcx.tys.u32_, b));
    let snapshot = infcx.start_snapshot();
    let (lub, goals) = infcx.at(&prog, Default::default()).lub(at, bt).unwrap();
    assert!(goals.is_empty());
    let TyKind::Ref(false, _, region) = *tcx.tys.kind(lub) else { panic!() };
    assert!(matches!(tcx.tys.region_kind(region), RegionKind::ReVar(_)));
    assert_eq!(infcx.num_region_vars(), 1);
    assert_eq!(infcx.at(&prog, Default::default()).lub(at, bt).unwrap().0, lub);
    assert_eq!(infcx.num_region_vars(), 1);
    let constraints = infcx.inner.borrow_mut().unwrap_region_constraints().constraints().to_vec();
    assert_eq!(constraints.len(), 2);
    assert!(constraints.iter().all(|c| c.sub == region && c.visible_for_leak_check));
    infcx.rollback_to(snapshot);
    assert_eq!(infcx.num_region_vars(), 0);
    // Rolling back also removes the combination cache entry: this allocates again.
    infcx.at(&prog, Default::default()).lub(at, bt).unwrap();
    assert_eq!(infcx.num_region_vars(), 1);
    let mut glb = LatticeOp::new(&infcx, &prog, Default::default(), LatticeOpKind::Glb);
    let glb = glb.tys(at, bt).unwrap();
    let TyKind::Ref(false, _, region) = *tcx.tys.kind(glb) else { panic!() };
    let constraints = infcx.inner.borrow_mut().unwrap_region_constraints().constraints().to_vec();
    assert_eq!(constraints.len(), 4);
    assert!(constraints[2..].iter().all(|c| c.sup == region));
    infcx.take_and_reset_region_constraints();
    infcx.at(&prog, Default::default()).lub(at, bt).unwrap();
    assert_eq!(infcx.inner.borrow_mut().unwrap_region_constraints().constraints().len(), 2);
}

#[test]
fn round29_lattice_variable_order_and_higher_ranked_binders() {
    let tcx = Tcx::new();
    let prog = crate::program::Program::new(&std::sync::Arc::new(crate::Session::default()));
    let infcx = InferCtxt::new(&tcx, TypingMode::PostAnalysis, Default::default());
    let var = infcx.next_ty_var();
    let (ty, goals) = infcx.at(&prog, Default::default()).lub(var, tcx.tys.u32_).unwrap();
    assert!(goals.is_empty());
    assert_eq!(infcx.shallow_resolve(ty), tcx.tys.u32_);
    assert_eq!(infcx.shallow_resolve(var), tcx.tys.u32_);
    let region = tcx.tys.intern_region(RegionKind::ReBound(0, BoundVar(0)));
    let arg = tcx.tys.intern(TyKind::Ref(false, tcx.tys.u32_, region));
    let hr = tcx.tys.intern(TyKind::FnPtr(tcx.tys.mk_list(&[arg]), tcx.tys.unit, crate::types::FnHeader::RUST, 1));
    let static_arg = tcx.tys.intern(TyKind::Ref(false, tcx.tys.u32_, tcx.tys.intern_region(RegionKind::ReStatic)));
    let mono = tcx.tys.intern(TyKind::FnPtr(tcx.tys.mk_list(&[static_arg]), tcx.tys.unit, crate::types::FnHeader::RUST, 0));
    let snapshot = infcx.start_snapshot();
    infcx.at(&prog, Default::default()).lub(hr, mono).unwrap();
    assert!(infcx.leak_check(UniverseIndex::ROOT, Some(&snapshot)).is_err());
    infcx.rollback_to(snapshot);
}

#[test]
fn round32_shared_session_diagnostics_and_delayed_bug() {
    use super::diagnostics::DiagCtxt;
    let cause = crate::next_solver::fulfill::ObligationCause::default();
    let dcx = DiagCtxt::default();
    let temporary = dcx.clone();
    temporary.delayed_bug(cause, "normalization recovery".into());
    temporary.report(cause, "invalid declaration".into());
    dcx.report(cause, "invalid declaration".into());
    assert_eq!(dcx.err_count(), 1);
    assert_eq!(dcx.drain_reported().0[0].message, "invalid declaration");
    assert!(temporary.drain_reported().0.is_empty());
    dcx.flush_delayed();
    let delayed = DiagCtxt::default();
    delayed.delayed_bug(cause, "unreported recovery".into());
    assert!(std::panic::catch_unwind(|| delayed.flush_delayed()).is_err());
    delayed.flush_delayed();
}
