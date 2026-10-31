use super::borrow_set::{BorrowSet, TwoPhaseActivation};
use super::dataflow::{Borrowck, Borrows, RegionScope};
use super::path_utils::is_active;
use crate::mir::dataflow::{iterate_to_fixpoint, BitSet};
use crate::mir::move_paths::MoveData;
use crate::mir::*;
use crate::program::{DefId, Program};
use crate::tcx::{Tcx, TypingEnv};
use crate::types::{RegionId, TyId, TyKind};

fn local(i: u32) -> Place { Place::local(Local(i)) }
fn place(i: u32, projection: Vec<PlaceElem>) -> Place { Place { local: Local(i), projection } }
fn at(block: u32, statement_index: usize) -> Location { Location { block: BasicBlock(block), statement_index } }
fn block(statements: Vec<StatementKind>, kind: TerminatorKind) -> BasicBlockData {
    BasicBlockData {
        statements: statements.into_iter().map(|kind| Statement { kind, source_info: SourceInfo::default() }).collect(),
        terminator: Some(Terminator { kind, source_info: SourceInfo::default() }), is_cleanup: false,
    }
}
fn body(types: &[TyId], basic_blocks: Vec<BasicBlockData>) -> Body {
    Body {
        phase: MirPhase::Analysis(AnalysisPhase::Initial), source: MirSource { owner: DefId(0), kind: BodyKind::Fn },
        local_decls: types.iter().enumerate().map(|(i, &ty)| LocalDecl {
            ty, mutability: Mutability::Mut, user_ty: Vec::new(), source_info: SourceInfo::default(),
            kind: if i == 0 { LocalKind::Return } else { LocalKind::Temp }, local_info: LocalInfo::Boring,
        }).collect(),
        basic_blocks, source_scopes: vec![SourceScopeData { span: Span::default(), parent_scope: None }],
        user_type_annotations: Vec::new(), span: Span::default(), upvars: Vec::new(), allocs: AllocMap::default(), var_debug_info: Vec::new(), arg_count: 0,
        spread_arg: None, promoted: Vec::new(), coroutine: None,
    }
}
fn reference(tcx: &Tcx, ty: TyId) -> TyId { tcx.tys.intern(TyKind::Ref(true, ty, RegionId::ERASED)) }
fn borrow(dest: u32, region: u32, source: Place) -> StatementKind {
    StatementKind::Assign(local(dest), Rvalue::Ref(Region(region), BorrowKind::Shared, source))
}
fn assign(dest: Place, source: Place) -> StatementKind { StatementKind::Assign(dest, Rvalue::Use(Operand::Copy(source))) }
fn goto(target: u32) -> TerminatorKind { TerminatorKind::Goto { target: BasicBlock(target) } }
fn switch(a: u32, b: u32) -> TerminatorKind {
    TerminatorKind::SwitchInt {
        discr: Operand::Copy(local(1)),
        targets: SwitchTargets { values: vec![0], targets: vec![BasicBlock(a)], otherwise: BasicBlock(b) },
    }
}
fn gather(tcx: &Tcx, body: &Body, invalidated: bool) -> (MoveData, BorrowSet) {
    let session = std::sync::Arc::new(crate::Session::default());
    let prog = Program::new(&session);
    let moves = MoveData::gather_moves(body, &prog, tcx, TypingEnv::fully_monomorphized(&prog), &|_| true);
    let borrows = BorrowSet::build(tcx, body, invalidated, &moves);
    (moves, borrows)
}
fn bits(actual: &BitSet, expected: &[usize]) { assert_eq!(actual.iter().collect::<Vec<_>>(), expected); }

// Hand-built solved region values: a region is a set of MIR points, not a
// lexical interval. Deliberately permit holes and different branch endpoints.
struct RegionValues(Vec<Vec<Location>>);
impl RegionValues {
    fn all(body: &Body, count: usize) -> Self {
        let points: Vec<_> = body.basic_blocks.iter().enumerate().flat_map(|(b, data)| {
            (0..=data.statements.len()).map(move |s| at(b as u32, s))
        }).collect();
        Self(vec![points; count])
    }
    fn remove(&mut self, region: usize, point: Location) { self.0[region].retain(|&p| p != point); }
}
impl RegionScope for RegionValues {
    fn contains_point(&self, region: Region, location: Location) -> bool { self.0[region.0 as usize].contains(&location) }
}

#[test]
fn nll_scope_ends_before_statement_and_terminator() {
    let tcx = Tcx::new();
    let t = tcx.tys.i32_;
    let b = body(&[tcx.tys.unit, t, reference(&tcx, t), reference(&tcx, t)], vec![block(vec![
        borrow(2, 0, local(1)), borrow(3, 1, local(1)), StatementKind::Nop,
    ], TerminatorKind::Return)]);
    let (_, set) = gather(&tcx, &b, true);
    let regions = RegionValues(vec![vec![at(0, 0), at(0, 1)], vec![at(0, 1), at(0, 2)]]);
    let analysis = Borrows::new(&tcx, &b, &regions, &set);
    let results = iterate_to_fixpoint(&b, &analysis);
    let mut cursor = results.cursor();
    cursor.seek_before_primary_effect(at(0, 0)); bits(cursor.get(), &[]);
    cursor.seek_after_primary_effect(at(0, 1)); bits(cursor.get(), &[0, 1]);
    cursor.seek_before_primary_effect(at(0, 2)); bits(cursor.get(), &[1]);
    cursor.seek_before_primary_effect(at(0, 3)); bits(cursor.get(), &[]);
    cursor.seek_after_primary_effect(at(0, 0)); bits(cursor.get(), &[0]); // cursor rewind
}

#[test]
fn reservation_generation_follows_scope_kill_and_precedes_assignment_kill() {
    let tcx = Tcx::new();
    let r = reference(&tcx, tcx.tys.i32_);
    let b = body(&[tcx.tys.unit, r, r], vec![block(vec![
        borrow(2, 0, local(1)),
        // The reborrow is generated, then killed by overwriting its root.
        borrow(1, 1, place(1, vec![PlaceElem::Deref])),
    ], TerminatorKind::Return)]);
    let (_, set) = gather(&tcx, &b, true);
    let regions = RegionValues(vec![vec![], vec![at(0, 1), at(0, 2)]]);
    let analysis = Borrows::new(&tcx, &b, &regions, &set);
    let results = iterate_to_fixpoint(&b, &analysis);
    let mut cursor = results.cursor();
    cursor.seek_after_primary_effect(at(0, 0)); bits(cursor.get(), &[0]);
    cursor.seek_after_primary_effect(at(0, 1)); bits(cursor.get(), &[]);
}

#[test]
fn loop_backedge_scans_before_reservation_and_regenerates() {
    let tcx = Tcx::new();
    let t = tcx.tys.i32_;
    let b = body(&[tcx.tys.unit, t, reference(&tcx, t)], vec![block(vec![
        StatementKind::Nop, borrow(2, 0, local(1)),
    ], goto(0))]);
    let (_, set) = gather(&tcx, &b, true);
    let regions = RegionValues(vec![vec![at(0, 1), at(0, 2)]]);
    let analysis = Borrows::new(&tcx, &b, &regions, &set);
    let results = iterate_to_fixpoint(&b, &analysis);
    bits(results.entry_state(BasicBlock(0)), &[0]);
    let mut cursor = results.cursor();
    cursor.seek_before_primary_effect(at(0, 0)); bits(cursor.get(), &[]);
    cursor.seek_after_primary_effect(at(0, 1)); bits(cursor.get(), &[0]);
}

#[test]
fn branch_frontiers_stop_dead_paths_reset_visited_and_join_live_paths() {
    let tcx = Tcx::new();
    let t = tcx.tys.i32_;
    let r = reference(&tcx, t);
    let b = body(&[tcx.tys.unit, t, r, r], vec![
        block(vec![borrow(2, 0, local(1)), borrow(3, 1, local(1))], switch(1, 2)),
        block(vec![], goto(3)), block(vec![], goto(3)),
        block(vec![], TerminatorKind::Return),
        block(vec![borrow(2, 0, local(1))], TerminatorKind::Return), // unreachable
    ]);
    let (_, set) = gather(&tcx, &b, true);
    assert_eq!(set.len(), 2);
    let mut regions = RegionValues::all(&b, 2);
    regions.remove(0, at(1, 0));
    regions.remove(1, at(2, 0));
    let analysis = Borrows::new(&tcx, &b, &regions, &set);
    let results = iterate_to_fixpoint(&b, &analysis);
    assert!(!results.is_reachable(BasicBlock(4)));
    let mut cursor = results.cursor();
    cursor.seek_before_primary_effect(at(1, 0)); bits(cursor.get(), &[1]);
    cursor.seek_before_primary_effect(at(2, 0)); bits(cursor.get(), &[0]);
    cursor.seek_before_primary_effect(at(3, 0)); bits(cursor.get(), &[0, 1]);
}

#[test]
fn assignment_prefix_and_storage_dead_kill_only_their_loans() {
    let tcx = Tcx::new();
    let t = tcx.tys.i32_;
    let pair = tcx.tys.intern(TyKind::Tuple(tcx.tys.mk_list(&[t, t])));
    let nested = tcx.tys.intern(TyKind::Tuple(tcx.tys.mk_list(&[pair, t])));
    let field = |n, ty| PlaceElem::Field(FieldIdx(n), ty);
    let b = body(&[tcx.tys.unit, nested, reference(&tcx, t), reference(&tcx, t), pair], vec![block(vec![
        borrow(2, 0, place(1, vec![field(0, pair), field(1, t)])),
        borrow(3, 0, place(1, vec![field(1, t)])),
        assign(place(1, vec![field(0, pair)]), local(4)),
        StatementKind::StorageLive(Local(1)), StatementKind::StorageDead(Local(1)),
    ], TerminatorKind::Return)]);
    let (_, set) = gather(&tcx, &b, true);
    let analysis = Borrows::new(&tcx, &b, &RegionValues::all(&b, 1), &set);
    let results = iterate_to_fixpoint(&b, &analysis);
    let mut cursor = results.cursor();
    cursor.seek_after_primary_effect(at(0, 1)); bits(cursor.get(), &[0, 1]);
    cursor.seek_after_primary_effect(at(0, 2)); bits(cursor.get(), &[1]);
    cursor.seek_after_primary_effect(at(0, 3)); bits(cursor.get(), &[1]);
    cursor.seek_before_primary_effect(at(0, 4)); bits(cursor.get(), &[1]);
    cursor.seek_after_primary_effect(at(0, 4)); bits(cursor.get(), &[]);
}

#[test]
fn runtime_indices_do_not_definitely_conflict_but_constant_indices_do() {
    let tcx = Tcx::new();
    let t = tcx.tys.i32_;
    let arr = tcx.tys.intern(TyKind::Array(t, tcx.tys.const_usize(4)));
    let index = |offset| PlaceElem::ConstantIndex { offset, min_length: 4, from_end: false };
    let b = body(&[tcx.tys.unit, arr, reference(&tcx, t), tcx.tys.usize_, t], vec![block(vec![
        borrow(2, 0, place(1, vec![index(1)])),
        assign(place(1, vec![PlaceElem::Index(Local(3))]), local(4)),
        assign(place(1, vec![index(2)]), local(4)),
        assign(place(1, vec![index(1)]), local(4)),
    ], TerminatorKind::Return)]);
    let (_, set) = gather(&tcx, &b, true);
    let analysis = Borrows::new(&tcx, &b, &RegionValues::all(&b, 1), &set);
    let results = iterate_to_fixpoint(&b, &analysis);
    let mut cursor = results.cursor();
    cursor.seek_after_primary_effect(at(0, 1)); bits(cursor.get(), &[0]);
    cursor.seek_after_primary_effect(at(0, 2)); bits(cursor.get(), &[0]);
    cursor.seek_after_primary_effect(at(0, 3)); bits(cursor.get(), &[]);
}

#[test]
fn static_ref_root_assignment_and_storage_dead_preserve_borrows() {
    let tcx = Tcx::new();
    let t = tcx.tys.i32_;
    let r = reference(&tcx, t);
    let mut b = body(&[tcx.tys.unit, r, r, r, t], vec![block(vec![
        borrow(2, 0, place(1, vec![PlaceElem::Deref])),
        assign(local(1), local(3)), StatementKind::StorageDead(Local(1)),
        assign(place(1, vec![PlaceElem::Deref]), local(4)),
    ], TerminatorKind::Return)]);
    b.local_decls[1].local_info = LocalInfo::StaticRef { def_id: DefId(1), is_thread_local: true };
    let (_, set) = gather(&tcx, &b, true);
    let analysis = Borrows::new(&tcx, &b, &RegionValues::all(&b, 1), &set);
    let results = iterate_to_fixpoint(&b, &analysis);
    let mut cursor = results.cursor();
    cursor.seek_after_primary_effect(at(0, 1)); bits(cursor.get(), &[0]);
    cursor.seek_after_primary_effect(at(0, 2)); bits(cursor.get(), &[0]);
    cursor.seek_after_primary_effect(at(0, 3)); bits(cursor.get(), &[]);
}

#[test]
fn ignored_borrow_assignment_returns_before_destination_kill() {
    let tcx = Tcx::new();
    let t = tcx.tys.i32_;
    let r = reference(&tcx, t);
    let rr = reference(&tcx, r);
    let mut b = body(&[tcx.tys.unit, t, r, rr], vec![block(vec![
        borrow(3, 0, local(2)), borrow(2, 0, local(1)),
    ], TerminatorKind::Return)]);
    b.local_decls[1].mutability = Mutability::Not;
    let (_, set) = gather(&tcx, &b, false);
    assert_eq!(set.len(), 1);
    let analysis = Borrows::new(&tcx, &b, &RegionValues::all(&b, 1), &set);
    let results = iterate_to_fixpoint(&b, &analysis);
    let mut cursor = results.cursor();
    cursor.seek_after_primary_effect(at(0, 1)); bits(cursor.get(), &[0]);
}

#[test]
fn two_phase_activation_uses_reservation_bit_and_never_regenerates_dead_loan() {
    let tcx = Tcx::new();
    let t = tcx.tys.i32_;
    let r = reference(&tcx, t);
    let b = body(&[tcx.tys.unit, t, r, r], vec![block(vec![
        StatementKind::Assign(local(2), Rvalue::Ref(Region(0), BorrowKind::Mut { kind: MutBorrowKind::TwoPhaseBorrow }, local(1))),
        StatementKind::Nop, assign(local(3), local(2)),
    ], TerminatorKind::Return)]);
    let (_, set) = gather(&tcx, &b, true);
    assert_eq!(set[0].activation_location, TwoPhaseActivation::ActivatedAt(at(0, 2)));
    assert_eq!(set.activations_at_location(&at(0, 2)), &[0]);
    assert!(!is_active(|a, b| a == b, &set[0], at(0, 1)));
    assert!(is_active(|a, b| a == b, &set[0], at(0, 2)));
    for live_at_activation in [true, false] {
        let mut regions = RegionValues::all(&b, 1);
        if !live_at_activation { regions.remove(0, at(0, 2)); }
        let analysis = Borrows::new(&tcx, &b, &regions, &set);
        let results = iterate_to_fixpoint(&b, &analysis);
        let mut cursor = results.cursor();
        cursor.seek_after_primary_effect(at(0, 0)); bits(cursor.get(), &[0]);
        cursor.seek_before_primary_effect(at(0, 2));
        assert_eq!(cursor.get().contains(0), live_at_activation);
        cursor.seek_after_primary_effect(at(0, 2));
        assert_eq!(cursor.get().contains(0), live_at_activation);
    }
}

#[test]
fn combined_results_keep_init_history_moves_and_call_edges_separate() {
    let tcx = Tcx::new();
    let t = tcx.tys.i32_;
    let r = reference(&tcx, t);
    let call = TerminatorKind::Call {
        func: Operand::Copy(local(0)), args: vec![Operand::Move(local(1))], destination: local(1),
        target: Some(BasicBlock(1)), unwind: UnwindAction::Cleanup(BasicBlock(2)),
        call_source: CallSource::Normal, fn_span: Span::default(), arg_spans: vec![Span::default()],
    };
    let mut b = body(&[tcx.tys.unit, t, r], vec![
        block(vec![borrow(2, 0, local(1))], call),
        block(vec![StatementKind::StorageDead(Local(1))], TerminatorKind::Return),
        block(vec![], TerminatorKind::UnwindResume),
    ]);
    b.arg_count = 1;
    b.local_decls[1].kind = LocalKind::Arg;
    b.basic_blocks[2].is_cleanup = true;
    let (moves, set) = gather(&tcx, &b, true);
    let path = moves.rev_lookup.find_local(Local(1)).unwrap();
    let arg_init = moves.init_path_map[path][0];
    let call_init = moves.init_loc_map[at(0, 1)][0];
    let borrow_init = moves.init_loc_map[at(0, 0)][0];
    let prog = crate::program::Program::new(&std::sync::Arc::new(crate::Session::default()));
    let analysis = Borrowck::new(&prog, &tcx, &b, &RegionValues::all(&b, 1), &set, &moves);
    let results = analysis.iterate_to_fixpoint();
    let mut cursor = results.cursor();
    cursor.seek_before_primary_effect(at(0, 0));
    assert!(!cursor.get().uninits.contains(path));
    bits(cursor.get().ever_inits, &[arg_init]);
    cursor.seek_after_primary_effect(at(0, 1));
    assert!(cursor.get().uninits.contains(path));
    assert!(cursor.get().ever_inits.contains(arg_init));
    assert!(cursor.get().ever_inits.contains(borrow_init));
    assert!(!cursor.get().ever_inits.contains(call_init));
    for (block, uninit) in [(1, false), (2, true)] {
        cursor.seek_before_primary_effect(at(block, 0));
        assert_eq!(cursor.get().uninits.contains(path), uninit);
        assert_eq!(cursor.get().ever_inits.contains(call_init), !uninit);
        bits(cursor.get().borrows, &[0]); // Call destinations do not kill loans.
    }
    cursor.seek_after_primary_effect(at(1, 0));
    bits(cursor.get().borrows, &[]);
    assert!(cursor.get().uninits.contains(path));
    assert!(!cursor.get().ever_inits.contains(arg_init));
    assert!(!cursor.get().ever_inits.contains(call_init));
    assert!(cursor.get().ever_inits.contains(borrow_init));
    assert_eq!(cursor.get().uninits.domain_size(), moves.move_paths.len());
    assert_eq!(cursor.get().ever_inits.domain_size(), moves.inits.len());
}
