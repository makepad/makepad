// Hand-reduced MIR semantics from rust-tests-src/tests/ui/nll/polonius.
use super::*;
use crate::borrowck::check::{check_polonius, check_with_scopes};
use crate::borrowck::polonius::{ConstraintDirection as D, record_live_region_variance};
use crate::types::RegionKind;

fn rty(tcx: &Tcx, r: u32) -> TyId {
    tcx.tys.intern(TyKind::Ref(true, tcx.tys.i32_, tcx.tys.intern_region(RegionKind::ReVar(r))))
}
fn deref(i: u32) -> Place { Place { local: Local(i), projection: vec![PlaceElem::Deref] } }
fn branch(tcx: &Tcx, yes: u32, no: u32) -> TerminatorKind {
    TerminatorKind::SwitchInt { discr: val(tcx.tys.bool_), targets: SwitchTargets { values: vec![1], targets: vec![BasicBlock(yes)], otherwise: BasicBlock(no) } }
}
fn typed_input(b: &Body, n: usize) -> RegionInput {
    let mut i = input(b, &vec![vec![]; n - 1]);
    i.definitions[1].origin = Origin::FreeRegion;
    i.relations = UniversalRegionRelations::create(UniversalRegions::new(0, None, vec![(0, Class::Global), (1, Class::External)]), n, &[]);
    i
}
fn edge(i: &mut RegionInput, b: &Body, sup: usize, sub: usize, at: Option<Location>) {
    i.outlives.push(OutlivesConstraint { sup, sub, location: at.map(|p| LocationMap::new(b).point(p)),
        span: Default::default(), category: ConstraintCategory::Assignment, from_closure: false });
}
fn generate(tcx: &Tcx, b: &Body, moves: &MoveData, i: &mut RegionInput, drops: &[Vec<usize>]) {
    crate::borrowck::type_check::liveness::generate(tcx, &test_prog(), b, moves, i, drops);
}

// #21906, from_the_rfc/get_default: returning a borrow on Some must not
// prevent mutation + a fresh borrow on None. The pointee abstracts the map.
fn conditional_return(tcx: &Tcx, filtering: bool) -> (Body, RegionInput) {
    let b = body(&[rty(tcx, 1), rty(tcx, 1), rty(tcx, 3), rty(tcx, 6)], vec![
        block(vec![borrow(2, deref(1), 2, mutable())], branch(tcx, 1, 2)),
        block(vec![assign(0, Operand::Move(p(2)))], TerminatorKind::Return),
        if filtering { block(vec![], goto(0)) } else { block(vec![
            StatementKind::Assign(deref(1), Rvalue::Use(val(tcx.tys.i32_))),
            borrow(3, deref(1), 4, mutable()), assign(0, Operand::Move(p(3)))], TerminatorKind::Return) },
    ]);
    let mut i = typed_input(&b, 7);
    edge(&mut i, &b, 2, 3, Some(at(0, 0)));
    edge(&mut i, &b, 3, 1, Some(at(1, 0)));
    if !filtering {
        edge(&mut i, &b, 4, 6, Some(at(2, 1)));
        edge(&mut i, &b, 6, 1, Some(at(2, 2)));
    }
    (b, i)
}
#[test]
fn get_default_and_filtering_lending_iterator_use_final_polonius_verdict() {
    // #92985 / #51526 reduce to the same return-or-loop graph. GAT/type
    // normalization is intentionally outside these MIR tests.
    for filtering in [false, true] {
        let tcx = Tcx::new(); let (b, mut i) = conditional_return(&tcx, filtering);
        let moves = gather(&tcx, &b);
        generate(&tcx, &b, &moves, &mut i, &vec![vec![]; b.local_decls.len()]);
        let mut regions = compute_regions(&tcx, &test_prog(), &b, i, false);
        let nll = check_with_scopes(&test_prog(), &tcx, &b, &regions, &moves);
        assert!(!nll.accepted(), "fixture must expose NLL's imprecision");
        assert!(nll.errors.iter().any(|e| matches!(e.code(), "E0499" | "E0506")), "{nll:?}");
        let result = mir_borrowck(&tcx, &test_prog(), &b, &mut regions, &moves);
        assert!(result.accepted(), "filtering={filtering}: {result:?}");
        assert!(check_polonius(&tcx, &test_prog(), &b, &regions, &moves).accepted());
    }
}
#[test]
fn lending_items_still_reject_overlapping_use_and_drop_liveness() {
    // lending-iterator-sanity-checks.polonius.stderr: E0499 for use_live
    // and drop_live; normalized dropck regions are provided by the fixture.
    for drop_live in [false, true] {
        let tcx = Tcx::new();
        let b = body(&[tcx.tys.unit, rty(&tcx, 1), rty(&tcx, 3), rty(&tcx, 5), tcx.tys.i32_], vec![
            block(vec![borrow(2, deref(1), 2, mutable()), borrow(3, deref(1), 4, mutable())],
                if drop_live { TerminatorKind::Drop { place: p(2), target: BasicBlock(1), unwind: UnwindAction::Continue, replace: false } }
                else { goto(1) }),
            block(if drop_live { vec![] } else { vec![assign(4, Operand::Copy(deref(2)))] }, TerminatorKind::Return),
        ]);
        let mut i = typed_input(&b, 6);
        edge(&mut i, &b, 2, 3, Some(at(0, 0))); edge(&mut i, &b, 4, 5, Some(at(0, 1)));
        let moves = gather(&tcx, &b);
        let mut drops = vec![vec![]; b.local_decls.len()];
        if drop_live { drops[2] = vec![3]; }
        generate(&tcx, &b, &moves, &mut i, &drops);
        let mut regions = compute_regions(&tcx, &test_prog(), &b, i, false);
        assert_eq!(codes(&mir_borrowck(&tcx, &test_prog(), &b, &mut regions, &moves)), ["E0499"]);
        assert_eq!(codes(&check_polonius(&tcx, &test_prog(), &b, &regions, &moves)), ["E0499"]);
    }
}
#[test]
fn conditional_return_does_not_hide_move_mutability_or_region_errors() {
    let tcx = Tcx::new();
    let (mut b, mut i) = conditional_return(&tcx, false);
    b.local_decls[1].ty = tcx.tys.intern(TyKind::Ref(false, tcx.tys.i32_, tcx.tys.intern_region(RegionKind::ReVar(1))));
    let moves = gather(&tcx, &b);
    generate(&tcx, &b, &moves, &mut i, &vec![vec![]; b.local_decls.len()]);
    let mut regions = compute_regions(&tcx, &test_prog(), &b, i, false);
    assert!(mir_borrowck(&tcx, &test_prog(), &b, &mut regions, &moves).errors.iter().any(|e| e.code() == "E0596"));

    // flow-sensitive-invariance.polonius.stderr retains universal region errors.
    let b = body(&[tcx.tys.unit, tcx.tys.i32_], vec![block(vec![], TerminatorKind::Return)]);
    let mut i = external_input(&b);
    edge(&mut i, &b, 1, 2, Some(at(0, 0))); edge(&mut i, &b, 2, 1, Some(at(0, 0)));
    let mut regions = compute_regions(&tcx, &test_prog(), &b, i, false);
    let result = mir_borrowck(&tcx, &test_prog(), &b, &mut regions, &gather(&tcx, &b));
    assert!(!result.accepted()); assert_eq!(result.region_errors.len(), 2);
}
#[test]
fn variance_records_covariant_contravariant_invariant_and_missing_regions() {
    let tcx = Tcx::new();
    let u = UniversalRegions::new(0, None, vec![(0, Class::Global)]);
    let region = |r| tcx.tys.intern_region(RegionKind::ReVar(r));
    let r = |n| tcx.tys.intern(TyKind::Ref(false, tcx.tys.i32_, region(n)));
    let mut dirs = vec![];
    record_live_region_variance(&tcx, &test_prog(), &mut dirs, &u, r(1));
    let fp = tcx.tys.intern(TyKind::FnPtr(tcx.tys.mk_list(&[r(2)]), r(3), crate::types::FnHeader::RUST, 0));
    record_live_region_variance(&tcx, &test_prog(), &mut dirs, &u, fp);
    let inv = tcx.tys.intern(TyKind::Ref(true, r(4), region(5)));
    record_live_region_variance(&tcx, &test_prog(), &mut dirs, &u, inv);
    assert_eq!(dirs, [None, Some(D::Forward), Some(D::Backward), Some(D::Forward), Some(D::Bidirectional), Some(D::Forward)]);
    record_live_region_variance(&tcx, &test_prog(), &mut dirs, &u, r(2));
    assert_eq!(dirs[2], Some(D::Bidirectional));
}

#[test]
fn a_borrow_used_on_the_other_branch_still_conflicts() {
    let tcx = Tcx::new(); let (mut b, mut i) = conditional_return(&tcx, false);
    // Keep the first result alive on None as well, after the mutation.
    b.basic_blocks[2].statements[2].kind = assign(0, Operand::Move(p(2)));
    edge(&mut i, &b, 3, 1, Some(at(2, 2)));
    let moves = gather(&tcx, &b);
    generate(&tcx, &b, &moves, &mut i, &vec![vec![]; b.local_decls.len()]);
    let mut regions = compute_regions(&tcx, &test_prog(), &b, i, false);
    let result = mir_borrowck(&tcx, &test_prog(), &b, &mut regions, &moves);
    assert!(result.errors.iter().any(|e| e.code() == "E0506"), "{result:?}");
}

#[test]
fn backward_edges_reach_earlier_constraints_without_requiring_predecessor_liveness() {
    let tcx = Tcx::new(); let t = tcx.tys.i32_; let r = reference(&tcx, t, false);
    let b = body(&[tcx.tys.unit, t, r, t], vec![block(vec![assign(3, val(t)), assign(3, val(t)),
        borrow(2, p(1), 1, BorrowKind::Shared), assign(1, val(t))], TerminatorKind::Return)]);
    for (direction, rejects) in [(Some(D::Forward), false), (Some(D::Backward), true), (Some(D::Bidirectional), true), (None, true)] {
        let mut i = input(&b, &[vec![at(0, 2)], vec![at(0, 1), at(0, 2)], vec![at(0, 1), at(0, 2), at(0, 3)]]);
        edge(&mut i, &b, 1, 2, Some(at(0, 2)));
        edge(&mut i, &b, 2, 3, Some(at(0, 0)));
        i.polonius.live_region_variances = vec![None, Some(D::Forward), direction, Some(D::Forward)];
        let regions = compute_regions(&tcx, &test_prog(), &b, i, false);
        let result = check_polonius(&tcx, &test_prog(), &b, &regions, &gather(&tcx, &b));
        assert_eq!(result.errors.iter().any(|e| e.code() == "E0506"), rejects, "{direction:?}: {result:?}");
    }
}

#[test]
fn logical_edges_apply_at_every_point_and_universals_only_flow_forward() {
    let tcx = Tcx::new(); let t = tcx.tys.i32_; let r = reference(&tcx, t, false);
    let b = body(&[tcx.tys.unit, t, r], vec![block(vec![borrow(2, p(1), 1, BorrowKind::Shared), assign(1, val(t))], TerminatorKind::Return)]);
    let mut i = input(&b, &[vec![at(0, 0)], vec![at(0, 1)]]);
    edge(&mut i, &b, 1, 2, None);
    let regions = compute_regions(&tcx, &test_prog(), &b, i, false);
    assert_eq!(codes(&check_polonius(&tcx, &test_prog(), &b, &regions, &gather(&tcx, &b))), ["E0506"]);

    // Even an explicitly invariant universal must never time-travel from a
    // loan's later issue point to an earlier localized outlives edge.
    let b = body(&[tcx.tys.unit, t, r, t], vec![block(vec![assign(3, val(t)), borrow(2, p(1), 1, BorrowKind::Shared), assign(1, val(t))], TerminatorKind::Return)]);
    let mut i = input(&b, &[vec![at(0, 1)], vec![at(0, 1), at(0, 2)]]);
    edge(&mut i, &b, 1, 0, Some(at(0, 1)));
    edge(&mut i, &b, 0, 2, Some(at(0, 0)));
    i.polonius.live_region_variances = vec![Some(D::Bidirectional), Some(D::Forward), Some(D::Forward)];
    let regions = compute_regions(&tcx, &test_prog(), &b, i, false);
    let borrows = crate::borrowck::borrow_set::BorrowSet::build(&tcx, &b, true, &gather(&tcx, &b));
    let live = regions.polonius.compute_loan_liveness(&tcx, &test_prog(), &b, &gather(&tcx, &b), &regions.locations, &borrows, &regions.regioncx);
    assert!(!live.live_loans().unwrap().contains(0, 0));
}

#[test]
fn issuance_exception_keeps_scanning_until_the_first_dead_successor() {
    let tcx = Tcx::new(); let t = tcx.tys.i32_; let r = reference(&tcx, t, false);
    let b = body(&[tcx.tys.unit, t, r], vec![block(vec![borrow(2, p(1), 1, BorrowKind::Shared), assign(1, val(t))], TerminatorKind::Return)]);
    let mut i = input(&b, &[vec![]]);
    i.liveness.record_live_loans(crate::borrowck::polonius::LiveLoans::new(LocationMap::new(&b).num_points(), 1));
    let regions = compute_regions(&tcx, &test_prog(), &b, i, false);
    // Killing at issuance would terminate scope precomputation too early:
    // generation then resurrects the loan, leaving no kill for statement 1.
    assert!(check_with_scopes(&test_prog(), &tcx, &b, &regions, &gather(&tcx, &b)).accepted());
}

#[test]
fn assignment_and_storage_dead_kills_prevent_live_loans_from_resurrecting() {
    for storage in [false, true] {
        let tcx = Tcx::new(); let r = rty(&tcx, 1);
        let mut statements = vec![borrow(2, deref(1), 2, mutable())];
        if storage { statements.push(StatementKind::StorageDead(Local(1))); }
        statements.push(assign(1, val(r)));
        statements.push(borrow(3, deref(1), 3, mutable()));
        let b = body(&[tcx.tys.unit, r, r, r], vec![block(statements, TerminatorKind::Return)]);
        let mut i = input(&b, &[vec![], vec![], vec![]]);
        // Scope membership alone says live everywhere; overwrites/storage
        // deaths in the existing Borrows analysis must still kill the loan.
        let map = LocationMap::new(&b);
        // Use a correctly sized matrix even for the shorter assignment case.
        i.liveness = LivenessValues::new(4, map.num_points());
        for r in 0..4 { i.liveness.add_all_points(r); }
        let regions = compute_regions(&tcx, &test_prog(), &b, i, false);
        let result = check_polonius(&tcx, &test_prog(), &b, &regions, &gather(&tcx, &b));
        assert!(!result.errors.iter().any(|e| e.code() == "E0499"), "{result:?}");
    }
}

#[test]
fn deferred_drop_liveness_is_computed_and_uninitialized_drops_are_skipped() {
    let tcx = Tcx::new();
    for initialized in [false, true] {
        let b = body(&[tcx.tys.unit, rty(&tcx, 1), rty(&tcx, 3), rty(&tcx, 5)], vec![
            block(if initialized { vec![borrow(2, deref(1), 2, mutable())] } else { vec![] }, goto(1)),
            block(vec![borrow(3, deref(1), 4, mutable())], TerminatorKind::Drop {
                place: p(2), target: BasicBlock(2), unwind: UnwindAction::Continue, replace: false }),
            block(vec![], TerminatorKind::Return),
        ]);
        let mut i = typed_input(&b, 6);
        if initialized { edge(&mut i, &b, 2, 3, Some(at(0, 0))); }
        // NLL-boring but Polonius-relevant: force the deferred drop path.
        edge(&mut i, &b, 3, 1, Some(at(2, 0)));
        edge(&mut i, &b, 4, 5, Some(at(1, 0)));
        let moves = gather(&tcx, &b);
        generate(&tcx, &b, &moves, &mut i, &[vec![], vec![], vec![3], vec![]]);
        assert!(i.liveness.point_liveness(3).is_none());
        let regions = compute_regions(&tcx, &test_prog(), &b, i, false);
        let result = check_polonius(&tcx, &test_prog(), &b, &regions, &moves);
        assert_eq!(result.errors.iter().any(|e| e.code() == "E0499"), initialized, "{result:?}");
        let prog = test_prog();
        let mut comp = crate::borrowck::type_check::liveness::LivenessComputation::new(&prog, &tcx, &b, &regions.locations, &moves);
        assert_eq!(!comp.compute(Local(2)).1.is_empty(), initialized);
    }
}

#[test]
fn regular_generic_args_are_invariant_and_drop_variance_uses_the_whole_type() {
    let tcx = Tcx::new();
    let b = body(&[tcx.tys.unit, tcx.tys.i32_, rty(&tcx, 1)], vec![block(vec![assign(2,
        Operand::Constant(Box::new(ConstOperand { span: Span::default(), user_ty: None,
            const_: Const::Unevaluated { def: DefId(100), args: vec![crate::consts::GenericArg::Type(rty(&tcx, 1))], promoted: None } })))], TerminatorKind::Return)]);
    let mut i = input(&b, &[vec![]]); let moves = gather(&tcx, &b);
    generate(&tcx, &b, &moves, &mut i, &[vec![], vec![], vec![]]);
    assert_eq!(i.polonius.live_region_variances[1], Some(D::Bidirectional));

    // #160670: the outer mutable reference makes its inner region invariant,
    // even though dropck's sole live component is just that inner region.
    let nested = tcx.tys.intern(TyKind::Ref(true, rty(&tcx, 2), tcx.tys.intern_region(RegionKind::ReVar(1))));
    let b = body(&[tcx.tys.unit, nested], vec![block(vec![], TerminatorKind::Drop {
        place: p(1), target: BasicBlock(1), unwind: UnwindAction::Continue, replace: false }), block(vec![], TerminatorKind::Return)]);
    let mut i = input(&b, &[vec![], vec![]]); let moves = gather(&tcx, &b);
    generate(&tcx, &b, &moves, &mut i, &[vec![], vec![2]]);
    assert_eq!(i.polonius.live_region_variances[2], Some(D::Bidirectional));
    assert!(i.liveness.is_live_at_point(2, 0));
}
#[test]
fn reaudit_opaque_variance_follows_declared_captures() {
    // Relate<AliasTy> via opt_alias_variances: an opaque's uncaptured (bivariant)
    // lifetime records no direction; captured ones are invariant. Projections stay invariant.
    let (prog, tcx, d) = crate::borrowck::type_check_tests::opaque_fixture();
    let u = UniversalRegions::new(0, None, vec![(0, Class::Global)]);
    let region = |r| tcx.tys.intern_region(RegionKind::ReVar(r));
    let opaque = tcx.tys.intern(TyKind::Alias(crate::consts::IsRigid::Yes, crate::types::AliasTy { kind: crate::types::AliasTyKind::Opaque { def_id: d }, args: tcx.tys.mk_list(&[]), regions: tcx.tys.intern_region_args(&[region(1), region(2)]) }));
    let mut dirs = vec![];
    record_live_region_variance(&tcx, &prog, &mut dirs, &u, opaque);
    assert_eq!(dirs, [None, None, Some(D::Bidirectional)]);
    let assoc = tcx.tys.intern(TyKind::Alias(crate::consts::IsRigid::Yes, crate::types::AliasTy { kind: crate::types::AliasTyKind::Projection { def_id: d }, args: tcx.tys.mk_list(&[]), regions: tcx.tys.intern_region_args(&[region(1)]) }));
    let mut dirs = vec![];
    record_live_region_variance(&tcx, &prog, &mut dirs, &u, assoc);
    assert_eq!(dirs, [None, Some(D::Bidirectional)]);
}

fn test_prog() -> crate::program::Program { crate::program::Program::new(&std::sync::Arc::new(crate::Session::default())) }
