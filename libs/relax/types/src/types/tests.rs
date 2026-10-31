use crate::types::AliasTyKind;
use crate::types::{AliasTy};
use super::*;
use super::fold::BoundVarReplacer;

fn bound(tys: &Types, depth: u32, var: u32) -> TyId {
    tys.intern(TyKind::Bound(depth, BoundTy { var: BoundVar(var), kind: BoundTyKind::Anon }))
}

fn region(tys: &Types, depth: u32, var: u32) -> RegionId {
    tys.intern_region(RegionKind::ReBound(depth, BoundVar(var)))
}

fn reference(tys: &Types, depth: u32, var: u32) -> TyId {
    tys.intern(TyKind::Ref(false, tys.u32_, region(tys, depth, var)))
}

fn placeholder(tys: &Types, universe: u32, var: u32) -> TyId {
    tys.intern(TyKind::Placeholder(PlaceholderType { universe: UniverseIndex(universe), bound: BoundVar(var) }))
}

#[test]
fn bound_and_placeholder_identity_and_erasure() {
    let tys = Types::new();
    let b = bound(&tys, 0, 0);
    assert_eq!(b, bound(&tys, 0, 0));
    for other in [bound(&tys, 1, 0), bound(&tys, 0, 1),
        tys.intern(TyKind::Bound(0, BoundTy { var: BoundVar(0), kind: BoundTyKind::Param(DefId(5)) })),
        placeholder(&tys, 0, 0), tys.intern(TyKind::Param(0)), tys.intern(TyKind::Infer(0))] {
        assert_ne!(b, other);
    }
    let p = placeholder(&tys, 2, 0);
    assert_eq!(p, placeholder(&tys, 2, 0));
    assert_ne!(p, placeholder(&tys, 3, 0));
    assert_ne!(p, placeholder(&tys, 2, 1));
    for t in [b, p] {
        assert_eq!(tys.erase_regions(t), t);
        assert_eq!(tys.erase_all_regions(t), t);
        assert!(!tys.is_concrete(t));
        assert!(!tys.has_infer(t));
        assert!(!tys.has_error(t));
        assert!(!tys.may_project(t));
        assert_eq!(tys.subst(t, &[tys.u32_]), t);
    }
    assert!(UniverseIndex(2).can_name(UniverseIndex::ROOT));
    assert!(!UniverseIndex::ROOT.can_name(UniverseIndex(2)));
}

#[test]
fn flags_distinguish_bound_from_escaping_and_placeholder() {
    let tys = Types::new();
    let b = bound(&tys, 0, 0);
    assert!(tys.flags(b).contains(TypeFlags::HAS_TY_BOUND));
    assert_eq!(tys.outer_exclusive_binder(b), 1);
    assert!(tys.has_escaping_bound_vars(b));
    let r = reference(&tys, 0, 1);
    let captured = tys.intern(TyKind::FnPtr(tys.mk_list(&[b, r]), tys.unit, FnHeader::RUST, 1));
    assert!(tys.flags(captured).contains(TypeFlags::HAS_TY_BOUND | TypeFlags::HAS_RE_BOUND));
    assert!(!tys.has_escaping_bound_vars(captured));
    let escaping = tys.intern(TyKind::FnPtr(tys.mk_list(&[bound(&tys, 2, 0)]), captured, FnHeader::RUST, 0));
    assert_eq!(tys.outer_exclusive_binder(escaping), 2);
    assert!(tys.has_vars_bound_at_or_above(escaping, 1));
    assert!(!tys.has_vars_bound_at_or_above(escaping, 2));
    let p = placeholder(&tys, 1, 0);
    let rp = tys.intern_region(RegionKind::RePlaceholder { universe: UniverseIndex(1), var: BoundVar(1) });
    let mixed = tys.intern(TyKind::FnPtr(tys.mk_list(&[tys.intern(TyKind::Ref(false, p, rp))]), captured, FnHeader::RUST, 0));
    assert!(tys.has_placeholders(mixed));
    assert!(tys.flags(mixed).contains(TypeFlags::HAS_TY_PLACEHOLDER | TypeFlags::HAS_RE_PLACEHOLDER));
    assert!(!tys.has_escaping_bound_vars(mixed));
    assert!(!tys.flags(tys.erase_regions(mixed)).intersects(TypeFlags::HAS_RE_PLACEHOLDER));
    assert!(tys.has_placeholders(tys.erase_regions(mixed)));
    for (r, flag) in [
        (RegionKind::ReEarlyParam(0), TypeFlags::HAS_RE_PARAM),
        (RegionKind::ReVar(0), TypeFlags::HAS_RE_INFER),
        (RegionKind::ReError, TypeFlags::HAS_RE_ERROR),
        (RegionKind::ReErased, TypeFlags::HAS_RE_ERASED),
    ] {
        let ty = tys.intern(TyKind::Ref(false, tys.u32_, tys.intern_region(r)));
        assert!(tys.flags(ty).contains(flag));
        assert!(tys.flags(ty).intersects(TypeFlags::HAS_REGIONS));
    }
}

#[test]
fn flags_and_shifts_cover_all_structural_positions() {
    let tys = Types::new();
    let b = bound(&tys, 1, 0);
    let p = placeholder(&tys, 1, 1);
    let rs = tys.intern_region_args(&[region(&tys, 1, 2)]);
    let empty = RegionArgsId::EMPTY;
    let mut shapes = vec![
        tys.intern(TyKind::Tuple(tys.mk_list(&[b, p]))),
        tys.intern(TyKind::Slice(b)),
        tys.intern(TyKind::Ptr(true, b)),
        tys.intern(TyKind::Ref(true, b, RegionId::ERASED)),
        tys.intern(TyKind::Array(b, tys.const_usize(3))),
        tys.mk_array(b, tys.intern_const(crate::consts::ConstKind::Bound(1, crate::consts::BoundConst { var: BoundVar(0) }))),
        tys.intern(TyKind::Adt(DefId(0), tys.mk_list(&[b]), empty)),
        tys.intern(TyKind::FnDef(DefId(0), tys.mk_list(&[b]), empty)),
        tys.intern(TyKind::Alias(crate::consts::IsRigid::No, AliasTy { kind: AliasTyKind::Projection { def_id: DefId(0) }, args: tys.mk_list(&[b]), regions: empty })),
        tys.intern(TyKind::Alias(crate::consts::IsRigid::No, AliasTy { kind: AliasTyKind::Opaque { def_id: DefId(0) }, args: tys.mk_list(&[b]), regions: empty })),
        tys.intern(TyKind::FnPtr(tys.mk_list(&[b]), b, FnHeader::RUST, 0)),
        tys.intern(TyKind::Closure(0, 0, crate::types::ClosureArgs { parent_args: b, closure_kind_ty: tys.prim(crate::program::Prim::I8), closure_sig_as_fn_ptr_ty: tys.unit, tupled_upvars_ty: tys.unit })),
        tys.intern(TyKind::Closure(0, 0, crate::types::ClosureArgs { parent_args: tys.unit, closure_kind_ty: tys.prim(crate::program::Prim::I8), closure_sig_as_fn_ptr_ty: b, tupled_upvars_ty: tys.unit })),
        tys.intern(TyKind::Closure(0, 0, crate::types::ClosureArgs { parent_args: tys.unit, closure_kind_ty: tys.prim(crate::program::Prim::I8), closure_sig_as_fn_ptr_ty: tys.unit, tupled_upvars_ty: b })),
        tys.intern(TyKind::Dyn(DefId(0), tys.mk_list(&[b]), crate::types::List::EMPTY, empty, RegionId::ERASED, 0, crate::types::List::EMPTY)),
        tys.intern(TyKind::Dyn(DefId(0), crate::types::List::EMPTY, tys.mk_list(&[(DefId(1), b, 0)]), empty, RegionId::ERASED, 0, crate::types::List::EMPTY)),
    ];
    for t in &shapes { assert!(tys.flags(*t).contains(TypeFlags::HAS_TY_BOUND)); }
    shapes.extend([
        tys.intern(TyKind::Adt(DefId(0), tys.mk_list(&[]), rs)),
        tys.intern(TyKind::FnDef(DefId(0), tys.mk_list(&[]), rs)),
        tys.intern(TyKind::Alias(crate::consts::IsRigid::No, AliasTy { kind: AliasTyKind::Projection { def_id: DefId(0) }, args: tys.mk_list(&[]), regions: rs })),
        tys.intern(TyKind::Alias(crate::consts::IsRigid::No, AliasTy { kind: AliasTyKind::Opaque { def_id: DefId(0) }, args: tys.mk_list(&[]), regions: rs })),
        tys.intern(TyKind::Dyn(DefId(0), crate::types::List::EMPTY, crate::types::List::EMPTY, rs, RegionId::ERASED, 0, crate::types::List::EMPTY)),
        tys.intern(TyKind::Dyn(DefId(0), crate::types::List::EMPTY, crate::types::List::EMPTY, empty, region(&tys, 1, 2), 0, crate::types::List::EMPTY)),
    ]);
    for t in shapes {
        assert!(tys.has_escaping_bound_vars(t));
        let shifted = tys.shift_in(t, 2);
        assert_ne!(shifted, t);
        assert_eq!(tys.outer_exclusive_binder(shifted), tys.outer_exclusive_binder(t) + 2);
        assert_eq!(tys.shift_out(shifted, 2), t);
        assert_eq!(tys.shift_vars(t, 0), t);
    }
    // New flags must propagate where some legacy query bits deliberately stop.
    for t in [tys.mk_array(p, tys.intern_const(crate::consts::ConstKind::Placeholder(crate::consts::PlaceholderConst { universe: UniverseIndex(1), bound: crate::consts::BoundConst { var: BoundVar(1) } }))), tys.intern(TyKind::Closure(0, 0, crate::types::ClosureArgs { parent_args: p, closure_kind_ty: tys.prim(crate::program::Prim::I8), closure_sig_as_fn_ptr_ty: tys.unit, tupled_upvars_ty: tys.unit }))] {
        assert!(tys.has_placeholders(t));
        assert_eq!(tys.shift_in(t, 2), t);
    }
}

#[test]
fn shifting_protects_nested_binders_and_dyn_object_region_is_outside() {
    let tys = Types::new();
    let own = bound(&tys, 0, 0);
    let outside = bound(&tys, 1, 1);
    let nested = tys.intern(TyKind::FnPtr(tys.mk_list(&[own, outside, reference(&tys, 0, 2)]), reference(&tys, 1, 3), FnHeader::RUST, 1));
    let expected = tys.intern(TyKind::FnPtr(tys.mk_list(&[own, bound(&tys, 3, 1), reference(&tys, 0, 2)]), reference(&tys, 3, 3), FnHeader::RUST, 1));
    assert_eq!(tys.shift_in(nested, 2), expected);
    let object = tys.intern(TyKind::Dyn(DefId(0), tys.mk_list(&[own, outside]), tys.mk_list(&[(DefId(1), own, 0)]),
        tys.intern_region_args(&[region(&tys, 0, 2)]), region(&tys, 0, 3), 0, Default::default()));
    let shifted = tys.shift_in(object, 1);
    let TyKind::Dyn(_, args, bs, rs, r, _, _) = tys.kind(shifted) else { panic!("dyn") };
    assert_eq!(tys.list(*args), &[own, bound(&tys, 2, 1)]);
    assert_eq!(tys.list(*bs)[0].1, own);
    assert_eq!(tys.region_args(*rs)[0], region(&tys, 0, 2));
    assert_eq!(*r, region(&tys, 1, 3));
}

#[test]
#[should_panic(expected = "cannot shift a bound variable out through its binder")]
fn shift_out_rejects_capture_even_without_numeric_underflow() {
    let tys = Types::new();
    let t = tys.intern(TyKind::FnPtr(tys.mk_list(&[bound(&tys, 1, 0)]), tys.unit, FnHeader::RUST, 0));
    tys.shift_out(t, 1);
}

#[test]
fn instantiation_caches_replacements_and_shifts_them_under_binders() {
    let tys = Types::new();
    let b = bound(&tys, 0, 0);
    let r = reference(&tys, 0, 1);
    let own = bound(&tys, 0, 2);
    let nested = tys.intern(TyKind::FnPtr(tys.mk_list(&[bound(&tys, 1, 0), own, reference(&tys, 1, 1)]), bound(&tys, 2, 3), FnHeader::RUST, 1));
    let input = tys.intern(TyKind::Tuple(tys.mk_list(&[b, b, r, nested])));
    let replacement = tys.intern(TyKind::Tuple(tys.mk_list(&[bound(&tys, 0, 4), reference(&tys, 0, 5)])));
    let mut type_calls = 0;
    let mut region_calls = 0;
    let output = tys.instantiate_bound_vars(input, |v| {
        assert_eq!(v, BoundVar(1)); region_calls += 1; region(&tys, 0, 6)
    }, |b| {
        assert_eq!(b.var, BoundVar(0)); type_calls += 1; replacement
    });
    assert_eq!((type_calls, region_calls), (1, 1));
    let expected_nested = tys.intern(TyKind::FnPtr(tys.mk_list(&[tys.shift_in(replacement, 1), own, reference(&tys, 1, 6)]), bound(&tys, 1, 3), FnHeader::RUST, 1));
    assert_eq!(output, tys.intern(TyKind::Tuple(tys.mk_list(&[replacement, replacement, reference(&tys, 0, 6), expected_nested]))));
}

#[test]
fn instantiate_placeholders_and_regions() {
    let tys = Types::new();
    let own = bound(&tys, 0, 7);
    let input = tys.intern(TyKind::FnPtr(tys.mk_list(&[own, bound(&tys, 1, 0), reference(&tys, 1, 1)]), reference(&tys, 0, 2), FnHeader::RUST, 1));
    let output = tys.instantiate_binder_with_placeholders(input, UniverseIndex(3));
    let rp = tys.intern_region(RegionKind::RePlaceholder { universe: UniverseIndex(3), var: BoundVar(1) });
    let expected = tys.intern(TyKind::FnPtr(tys.mk_list(&[own, placeholder(&tys, 3, 0), tys.intern(TyKind::Ref(false, tys.u32_, rp))]), reference(&tys, 0, 2), FnHeader::RUST, 1));
    assert_eq!(output, expected);
    assert!(!tys.has_escaping_bound_vars(output));
    assert!(tys.has_placeholders(output));
    assert_eq!(tys.instantiate_bound_regions(reference(&tys, 0, 0), |_| RegionId::ERASED),
        tys.intern(TyKind::Ref(false, tys.u32_, RegionId::ERASED)));
    let mut replacer = BoundVarReplacer::new(|_| RegionId::ERASED, |_| tys.u32_);
    assert_eq!(replacer.instantiate(&tys, bound(&tys, 0, 0)), tys.u32_);
    assert_eq!(replacer.instantiate(&tys, input), tys.intern(TyKind::FnPtr(
        tys.mk_list(&[own, tys.u32_, tys.intern(TyKind::Ref(false, tys.u32_, RegionId::ERASED))]), reference(&tys, 0, 2), FnHeader::RUST, 1)));
}

#[test]
#[should_panic(expected = "unexpected bound type in region-only instantiation")]
fn region_only_instantiation_rejects_bound_types() {
    let tys = Types::new();
    tys.instantiate_bound_regions(bound(&tys, 0, 0), |_| RegionId::ERASED);
}

#[test]
fn substitution_shifts_bound_types_and_regions_together() {
    let tys = Types::new();
    let param = tys.intern(TyKind::Param(0));
    let input = tys.intern(TyKind::FnPtr(tys.mk_list(&[param]), param, FnHeader::RUST, 0));
    let arg = tys.intern(TyKind::Tuple(tys.mk_list(&[bound(&tys, 0, 0), reference(&tys, 0, 1)])));
    let shifted = tys.shift_in(arg, 1);
    assert_eq!(tys.subst(input, &[arg]), tys.intern(TyKind::FnPtr(tys.mk_list(&[shifted]), shifted, FnHeader::RUST, 0)));
}

#[test]
fn replacement_cache_distinguishes_the_same_type_at_different_depths() {
    let tys = Types::new();
    let shared = tys.intern(TyKind::Tuple(tys.mk_list(&[bound(&tys, 1, 0), reference(&tys, 1, 1)])));
    let nested = tys.intern(TyKind::FnPtr(tys.mk_list(&[shared, shared]), tys.unit, FnHeader::RUST, 0));
    let value = tys.intern(TyKind::Tuple(tys.mk_list(&[shared, nested])));
    let result = tys.instantiate_binder_with_placeholders(value, UniverseIndex(2));
    let rp = tys.intern_region(RegionKind::RePlaceholder { universe: UniverseIndex(2), var: BoundVar(1) });
    let replaced = tys.intern(TyKind::Tuple(tys.mk_list(&[placeholder(&tys, 2, 0), tys.intern(TyKind::Ref(false, tys.u32_, rp))])));
    let shifted = tys.intern(TyKind::Tuple(tys.mk_list(&[bound(&tys, 0, 0), reference(&tys, 0, 1)])));
    let expected = tys.intern(TyKind::Tuple(tys.mk_list(&[shifted, tys.intern(TyKind::FnPtr(tys.mk_list(&[replaced, replaced]), tys.unit, FnHeader::RUST, 0))])));
    assert_eq!(result, expected);
}
