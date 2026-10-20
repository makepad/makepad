use super::check::{mir_borrowck, MirBorrowckResult};
use super::constraints::{ConstraintCategory, OutlivesConstraint, OutlivesConstraintSet};
use super::mir_nll::{compute_regions, LocationMap, RegionInput};
use super::region_infer::{NllRegionVariableOrigin as Origin, RegionDefinition};
use super::region_infer::values::LivenessValues;
use super::type_check::free_region_relations::UniversalRegionRelations;
use super::universal_regions::{UniversalRegions, RegionClassification as Class};
use crate::mir::*;
use crate::mir::move_paths::MoveData;
use crate::program::{DefId, Program};
use crate::tcx::{Tcx, TypingEnv};
use crate::types::{RegionId, TyId, TyKind, UniverseIndex};

fn p(i: u32) -> Place { Place::local(Local(i)) }
fn at(b: u32, s: usize) -> Location { Location { block: BasicBlock(b), statement_index: s } }
fn span(line: u32) -> Span { Span { file: 7, lo: line, hi: line + 1 } }
fn block(stmts: Vec<StatementKind>, term: TerminatorKind) -> BasicBlockData {
    BasicBlockData { statements: stmts.into_iter().enumerate().map(|(i, kind)| Statement { kind, source_info: SourceInfo { span: span(i as u32 + 1), ..SourceInfo::default() } }).collect(),
        terminator: Some(Terminator { kind: term, source_info: SourceInfo { span: span(99), ..SourceInfo::default() } }), is_cleanup: false }
}
fn body(tys: &[TyId], blocks: Vec<BasicBlockData>) -> Body {
    Body { phase: MirPhase::Analysis(AnalysisPhase::Initial), source: MirSource { owner: DefId(0), kind: BodyKind::Fn },
        basic_blocks: blocks, local_decls: tys.iter().enumerate().map(|(i, &ty)| LocalDecl { ty,
            mutability: Mutability::Mut, source_info: SourceInfo::default(), user_ty: vec![], local_info: LocalInfo::Boring,
            kind: if i == 0 { LocalKind::Return } else { LocalKind::UserVar } }).collect(),
        source_scopes: vec![SourceScopeData { span: Span::default(), parent_scope: None }], user_type_annotations: vec![], span: Span::default(), upvars: Vec::new(), allocs: AllocMap::default(),
        var_debug_info: vec![], arg_count: 1, spread_arg: None, promoted: vec![], coroutine: None }
}
fn reference(tcx: &Tcx, t: TyId, mutable: bool) -> TyId { tcx.tys.intern(TyKind::Ref(mutable, t, RegionId::ERASED)) }
fn val(ty: TyId) -> Operand { Operand::Constant(Box::new(ConstOperand { span: Span::default(), user_ty: None, const_: Const::Val(ConstValue::ZeroSized, ty) })) }
fn assign(dst: u32, op: Operand) -> StatementKind { StatementKind::Assign(p(dst), Rvalue::Use(op)) }
fn borrow(dst: u32, source: Place, region: u32, kind: BorrowKind) -> StatementKind { StatementKind::Assign(p(dst), Rvalue::Ref(Region(region), kind, source)) }
fn mutable() -> BorrowKind { BorrowKind::Mut { kind: MutBorrowKind::Default } }
fn two_phase() -> BorrowKind { BorrowKind::Mut { kind: MutBorrowKind::TwoPhaseBorrow } }
fn goto(b: u32) -> TerminatorKind { TerminatorKind::Goto { target: BasicBlock(b) } }
fn input(body: &Body, scopes: &[Vec<Location>]) -> RegionInput {
    let map = LocationMap::new(body);
    let mut definitions = vec![RegionDefinition { origin: Origin::FreeRegion, universe: UniverseIndex(0), external_name: None }];
    definitions.extend(scopes.iter().map(|_| RegionDefinition { origin: Origin::Existential, universe: UniverseIndex(0), external_name: None }));
    let mut liveness = LivenessValues::new(definitions.len(), map.num_points());
    liveness.add_all_points(0);
    for (r, points) in scopes.iter().enumerate() { for &at in points { liveness.add_point(r + 1, map.point(at)); } }
    RegionInput { universe_causes: vec![], polonius: Default::default(), tainted_by_errors:false, relations: UniversalRegionRelations::create(UniversalRegions::new(0, None, vec![(0, Class::Global)]), definitions.len(), &[]),
        definitions, outlives: OutlivesConstraintSet::default(), liveness, type_tests: vec![] }
}
fn gather(tcx: &Tcx, body: &Body) -> MoveData {
    let session = std::sync::Arc::new(crate::Session::default());
    MoveData::gather_moves(body, &Program::new(&session), tcx, TypingEnv::fully_monomorphized(&Program::new(&session)), &|_| true)
}
fn check(tcx: &Tcx, body: &Body, scopes: &[Vec<Location>]) -> MirBorrowckResult {
    let mut regions = compute_regions(tcx, &test_prog(), body, input(body, scopes), false);
    mir_borrowck(tcx, &test_prog(), body, &mut regions, &gather(tcx, body))
}
fn points(end: usize) -> Vec<Location> { (0..end).map(|s| at(0, s)).collect() }
fn codes(result: &MirBorrowckResult) -> Vec<&'static str> {
    assert!(result.unsupported.is_empty(), "{:?}", result.unsupported);
    assert!(result.region_errors.is_empty(), "{:?}", result.region_errors);
    result.errors.iter().map(|e| e.code()).collect()
}
#[test]
fn classic_borrow_conflicts_and_primary_spans() {
    let tcx = Tcx::new(); let t = tcx.tys.i32_; let r = reference(&tcx, t, true);
    for (first, second, expected) in [(mutable(), mutable(), "E0499"), (BorrowKind::Shared, mutable(), "E0502"), (mutable(), BorrowKind::Shared, "E0502")] {
        let b = body(&[tcx.tys.unit, t, r, r], vec![block(vec![borrow(2, p(1), 1, first), borrow(3, p(1), 2, second)], TerminatorKind::Return)]);
        let result = check(&tcx, &b, &[points(2), points(2)]);
        assert_eq!(codes(&result), [expected]); assert_eq!(result.errors[0].span, span(2));
    }
}
#[test]
fn move_assign_and_use_while_borrowed() {
    let tcx = Tcx::new(); let t = tcx.tys.i32_; let r = reference(&tcx, t, true);
    for (kind, stmt, expected) in [
        (BorrowKind::Shared, assign(3, Operand::Move(p(1))), "E0505"),
        (BorrowKind::Shared, assign(1, val(t)), "E0506"),
        (mutable(), assign(3, Operand::Copy(p(1))), "E0503"),
    ] {
        let b = body(&[tcx.tys.unit, t, r, t], vec![block(vec![borrow(2, p(1), 1, kind), stmt], TerminatorKind::Return)]);
        assert_eq!(codes(&check(&tcx, &b, &[points(2)])), [expected]);
    }
}
#[test]
fn moved_uninit_and_immutable_errors() {
    let tcx = Tcx::new(); let t = tcx.tys.i32_; let r = reference(&tcx, t, true);
    let b = body(&[tcx.tys.unit, t, t, t], vec![block(vec![assign(2, Operand::Move(p(1))), assign(3, Operand::Copy(p(1)))], TerminatorKind::Return)]);
    assert_eq!(codes(&check(&tcx, &b, &[])), ["E0382"]);
    let b = body(&[tcx.tys.unit, t, t], vec![block(vec![assign(1, Operand::Copy(p(2)))], TerminatorKind::Return)]);
    assert_eq!(codes(&check(&tcx, &b, &[])), ["E0381"]);
    let mut b = body(&[tcx.tys.unit, t, r], vec![block(vec![assign(1, val(t))], TerminatorKind::Return)]);
    b.local_decls[1].mutability = Mutability::Not;
    assert_eq!(codes(&check(&tcx, &b, &[])), ["E0384"]);
    b.basic_blocks[0].statements[0].kind = borrow(2, p(1), 1, mutable());
    assert_eq!(codes(&check(&tcx, &b, &[points(1)])), ["E0596"]);
}
#[test]
fn storage_dead_drop_exit_and_temporary_lifetimes() {
    let tcx = Tcx::new(); let t = tcx.tys.i32_; let r = reference(&tcx, t, true);
    let mut b = body(&[tcx.tys.unit, t, r], vec![block(vec![borrow(2, p(1), 1, BorrowKind::Shared), StatementKind::StorageDead(Local(1))], TerminatorKind::Return)]);
    let result = check(&tcx, &b, &[points(2)]);
    assert_eq!(codes(&result), ["E0597"]); assert_eq!(result.errors[0].span, span(1));
    b.local_decls[1].kind = LocalKind::Temp;
    assert_eq!(codes(&check(&tcx, &b, &[points(2)])), ["E0716"]);
    b.local_decls[1].kind = LocalKind::UserVar;
    b.basic_blocks[0].statements.pop();
    assert_eq!(codes(&check(&tcx, &b, &[points(2)])), ["E0597"]);
    b.basic_blocks[0].terminator.as_mut().unwrap().kind = TerminatorKind::Drop { place: p(1), target: BasicBlock(1), unwind: UnwindAction::Continue, replace: false };
    b.basic_blocks.push(block(vec![], TerminatorKind::Return));
    assert_eq!(codes(&check(&tcx, &b, &[points(2)])), ["E0597"]);
    if let TerminatorKind::Drop { replace, .. } = &mut b.basic_blocks[0].terminator.as_mut().unwrap().kind { *replace = true; }
    assert_eq!(codes(&check(&tcx, &b, &[points(2)])), ["E0506"]);
}
#[test]
fn nll_early_end_and_initial_assignment_are_accepted() {
    let tcx = Tcx::new(); let t = tcx.tys.i32_; let r = reference(&tcx, t, true);
    let mut b = body(&[tcx.tys.unit, t, r], vec![block(vec![borrow(2, p(1), 1, BorrowKind::Shared), assign(1, val(t))], TerminatorKind::Return)]);
    let result = check(&tcx, &b, &[points(1)]); assert!(result.accepted(), "{result:?}"); assert!(result.used_mut.contains(1));
    b.basic_blocks[0].statements.remove(0); b.arg_count = 0; b.local_decls[1].mutability = Mutability::Not;
    assert!(check(&tcx, &b, &[]).accepted());
}
fn push_body(tcx: &Tcx) -> Body {
    let t = tcx.tys.i32_; let r = reference(tcx, t, true);
    // vec.push(vec.len()): reserve receiver, shared len call, activate receiver.
    body(&[tcx.tys.unit, t, r, reference(tcx, t, false), t], vec![
        block(vec![borrow(2, p(1), 1, two_phase()), borrow(3, p(1), 2, BorrowKind::Shared)],
            TerminatorKind::Call { func: val(tcx.tys.unit), args: vec![Operand::Copy(p(3))], destination: p(4), target: Some(BasicBlock(1)),
                unwind: UnwindAction::Continue, call_source: CallSource::Normal, fn_span: span(80), arg_spans: vec![] }),
        block(vec![], TerminatorKind::Call { func: val(tcx.tys.unit), args: vec![Operand::Move(p(2)), Operand::Copy(p(4))], destination: p(0), target: Some(BasicBlock(2)),
            unwind: UnwindAction::Continue, call_source: CallSource::Normal, fn_span: span(81), arg_spans: vec![] }),
        block(vec![], TerminatorKind::Return),
    ])
}
#[test]
fn two_phase_vec_push_vec_len_is_accepted() {
    let tcx = Tcx::new(); let b = push_body(&tcx);
    let mut receiver = points(3); receiver.push(at(1, 0));
    let result = check(&tcx, &b, &[receiver, points(3)]);
    assert!(result.accepted(), "{result:?}");
}
#[test]
fn activation_conflicts_with_still_live_shared_borrow() {
    let tcx = Tcx::new(); let b = push_body(&tcx);
    let mut live = points(3); live.push(at(1, 0));
    assert_eq!(codes(&check(&tcx, &b, &[live.clone(), live])), ["E0502"]);
}
#[test]
fn invalid_reservation_does_not_report_activation_again() {
    let tcx = Tcx::new(); let mut b = push_body(&tcx);
    b.basic_blocks[0].statements[1].kind = borrow(3, p(1), 2, mutable());
    b.basic_blocks[0].statements.swap(0, 1);
    let mut live = points(3); live.push(at(1, 0));
    let result = check(&tcx, &b, &[live.clone(), live]);
    assert_eq!(codes(&result), ["E0499"]);
}
#[test]
fn partial_move_field_reinitialization_and_full_move_rejection() {
    let tcx = Tcx::new(); let t = tcx.tys.i32_; let tuple = tcx.tys.intern(TyKind::Tuple(tcx.tys.mk_list(&[t, t])));
    let f = |n| Place { local: Local(1), projection: vec![PlaceElem::Field(FieldIdx(n), t)] };
    let mut b = body(&[tcx.tys.unit, tuple, t], vec![block(vec![assign(2, Operand::Move(f(0))),
        StatementKind::Assign(f(0), Rvalue::Use(val(t))), assign(2, Operand::Copy(f(1)))], TerminatorKind::Return)]);
    assert!(check(&tcx, &b, &[]).accepted());
    b.basic_blocks[0].statements[0].kind = assign(0, Operand::Move(p(1)));
    // One E0382 per set of move-outs: the later use of `_1.1` (not a prefix of the
    // already reported `_1`) replaces the partial-assignment error (buffer_move_error).
    let result = check(&tcx, &b, &[]);
    assert_eq!(codes(&result), ["E0382"]); assert_eq!(result.errors[0].span, span(3));
    b.basic_blocks[0].statements.remove(0); b.arg_count = 0;
    assert_eq!(codes(&check(&tcx, &b, &[])), ["E0381"]);
}
#[test]
fn mutable_reference_does_not_require_mutable_binding() {
    let tcx = Tcx::new(); let t = tcx.tys.i32_;
    let mut b = body(&[tcx.tys.unit, reference(&tcx, t, true)], vec![block(vec![StatementKind::Assign(
        Place { local: Local(1), projection: vec![PlaceElem::Deref] }, Rvalue::Use(val(t)))], TerminatorKind::Return)]);
    b.local_decls[1].mutability = Mutability::Not;
    assert!(check(&tcx, &b, &[]).accepted());
    b.local_decls[1].ty = reference(&tcx, t, false);
    assert_eq!(codes(&check(&tcx, &b, &[])), ["E0594"]);
}
#[test]
fn storage_dead_resets_immutable_initialization_history() {
    let tcx = Tcx::new(); let t = tcx.tys.i32_;
    let mut b = body(&[tcx.tys.unit, t], vec![block(vec![StatementKind::StorageDead(Local(1)), StatementKind::StorageLive(Local(1)), assign(1, val(t))], TerminatorKind::Return)]);
    b.local_decls[1].mutability = Mutability::Not;
    assert!(check(&tcx, &b, &[]).accepted());
    b.basic_blocks[0].statements.remove(0);
    assert_eq!(codes(&check(&tcx, &b, &[])), ["E0384"]);
}
#[test]
fn call_cleanup_does_not_initialize_destination() {
    let tcx = Tcx::new(); let t = tcx.tys.i32_;
    let b = body(&[tcx.tys.unit, t, t], vec![
        block(vec![], TerminatorKind::Call { func: val(tcx.tys.unit), args: vec![], destination: p(2), target: Some(BasicBlock(1)), unwind: UnwindAction::Cleanup(BasicBlock(2)),
            call_source: CallSource::Normal, fn_span: span(80), arg_spans: vec![] }),
        block(vec![assign(1, Operand::Copy(p(2)))], TerminatorKind::Return),
        block(vec![assign(1, Operand::Copy(p(2)))], TerminatorKind::UnwindResume),
    ]);
    assert_eq!(codes(&check(&tcx, &b, &[])), ["E0381"]);
}
#[test]
fn unreachable_conflict_is_not_checked() {
    let tcx = Tcx::new(); let t = tcx.tys.i32_;
    let b = body(&[tcx.tys.unit, t, t], vec![block(vec![], TerminatorKind::Return), block(vec![assign(1, Operand::Copy(p(2)))], TerminatorKind::Return)]);
    assert!(check(&tcx, &b, &[]).accepted());
}
#[test]
fn unsupported_metadata_is_not_an_acceptance() {
    let tcx = Tcx::new(); let t = tcx.tys.i32_;
    let mut b = body(&[tcx.tys.unit, t], vec![block(vec![StatementKind::PlaceMention(Place { local: Local(1), projection: vec![PlaceElem::Subtype(t)] })], TerminatorKind::Return)]);
    let result = check(&tcx, &b, &[]); assert!(!result.accepted()); assert!(result.unsupported[0].contains("Subtype"));
    // Build MoveData before introducing unsupported asm; preflight precedes BorrowSet/dataflow.
    b.basic_blocks[0].statements.clear(); let moves = gather(&tcx, &b);
    b.basic_blocks[0].terminator.as_mut().unwrap().kind = TerminatorKind::InlineAsm(InlineAsmUnsupported);
    let mut regions = compute_regions(&tcx, &test_prog(), &b, input(&b, &[]), false);
    let result = mir_borrowck(&tcx, &test_prog(), &b, &mut regions, &moves); assert!(!result.accepted()); assert!(result.unsupported[0].contains("inline-asm"));
}
#[test]
fn loop_move_and_initially_uninitialized_loop_have_distinct_errors() {
    let tcx = Tcx::new(); let t = tcx.tys.i32_;
    let switch = || TerminatorKind::SwitchInt { discr: val(t), targets: SwitchTargets { values: vec![0], targets: vec![BasicBlock(1)], otherwise: BasicBlock(2) } };
    let mut b = body(&[tcx.tys.unit, t, t], vec![block(vec![], goto(1)),
        block(vec![assign(2, Operand::Copy(p(1))), assign(2, Operand::Move(p(1)))], switch()), block(vec![], TerminatorKind::Return)]);
    assert!(codes(&check(&tcx, &b, &[])).iter().all(|&code| code == "E0382"));
    assert!(!check(&tcx, &b, &[]).accepted());
    b.arg_count = 0;
    assert_eq!(codes(&check(&tcx, &b, &[])), ["E0381"]);
}
#[test]
fn diamond_reassignment_and_two_phase_reservation_dominance() {
    let tcx = Tcx::new(); let t = tcx.tys.i32_; let r = reference(&tcx, t, true);
    let mut b = body(&[tcx.tys.unit, t, r, t], vec![
        block(vec![borrow(2, p(1), 1, two_phase())], TerminatorKind::SwitchInt { discr: val(t), targets: SwitchTargets { values: vec![0], targets: vec![BasicBlock(2)], otherwise: BasicBlock(1) } }),
        block(vec![assign(3, Operand::Copy(p(1)))], goto(3)),
        block(vec![assign(3, Operand::Copy(p(1)))], goto(3)),
        block(vec![], TerminatorKind::Call { func: val(tcx.tys.unit), args: vec![Operand::Move(p(2))], destination: p(0), target: Some(BasicBlock(4)), unwind: UnwindAction::Continue,
            call_source: CallSource::Normal, fn_span: span(80), arg_spans: vec![] }),
        block(vec![], TerminatorKind::Return),
    ]);
    let live = vec![at(0,0), at(0,1), at(1,0), at(1,1), at(2,0), at(2,1), at(3,0)];
    assert!(check(&tcx, &b, &[live]).accepted());
    b.basic_blocks[0].statements.clear();
    b.basic_blocks[3].terminator.as_mut().unwrap().kind = TerminatorKind::Return;
    b.basic_blocks[1].statements[0].kind = assign(3, val(t));
    b.basic_blocks[2].statements.clear();
    b.basic_blocks[3].statements.push(Statement { kind: assign(3, val(t)), source_info: SourceInfo { span: span(88), ..SourceInfo::default() } });
    b.local_decls[3].mutability = Mutability::Not;
    assert_eq!(codes(&check(&tcx, &b, &[])), ["E0384"]);
}
#[test]
fn nll_branch_frontier_expires_only_on_the_dead_branch() {
    let tcx = Tcx::new(); let t = tcx.tys.i32_; let r = reference(&tcx, t, true);
    let b = body(&[tcx.tys.unit, t, r], vec![block(vec![borrow(2, p(1), 1, BorrowKind::Shared)],
        TerminatorKind::SwitchInt { discr: val(t), targets: SwitchTargets { values: vec![0], targets: vec![BasicBlock(1)], otherwise: BasicBlock(2) } }),
        block(vec![assign(1, val(t))], TerminatorKind::Return), block(vec![assign(1, val(t))], TerminatorKind::Return)]);
    let result = check(&tcx, &b, &[vec![at(0,0), at(0,1), at(2,0)]]);
    assert_eq!(codes(&result), ["E0506"]);
}
#[test]
fn projected_and_promoted_illegal_moves() {
    let tcx = Tcx::new(); let t = tcx.tys.i32_;
    let deref = Place { local: Local(1), projection: vec![PlaceElem::Deref] };
    let b = body(&[tcx.tys.unit, reference(&tcx,t,true), t], vec![block(vec![assign(2, Operand::Move(deref))], TerminatorKind::Return)]);
    assert_eq!(codes(&check(&tcx, &b, &[])), ["E0507"]);
    let array = tcx.tys.intern(TyKind::Array(t, tcx.tys.const_usize(3)));
    let mut promoted = body(&[tcx.tys.unit, array, t, t], vec![block(vec![assign(3, Operand::Move(Place { local: Local(1), projection: vec![PlaceElem::Index(Local(2))] }))], TerminatorKind::Return)]);
    promoted.source.kind = BodyKind::Promoted(DefId(0), PromotedId(0));
    let mut parent = body(&[tcx.tys.unit], vec![block(vec![], TerminatorKind::Return)]); parent.arg_count = 0;
    parent.promoted.push(promoted);
    assert_eq!(codes(&check(&tcx, &parent, &[])), ["E0508"]);
}
#[test]
fn closure_capture_kinds_and_fake_guard_kinds() {
    let tcx = Tcx::new(); let t = tcx.tys.i32_; let r = reference(&tcx,t,true);
    let capture = BorrowKind::Mut { kind: MutBorrowKind::ClosureCapture };
    for (old, new, code) in [(capture, capture, "E0524"), (BorrowKind::Shared,capture,"E0500"), (capture,BorrowKind::Shared,"E0501"), (capture,mutable(),"E0501")] {
        let b = body(&[tcx.tys.unit,t,r,r],vec![block(vec![borrow(2,p(1),1,old),borrow(3,p(1),2,new)],TerminatorKind::Return)]);
        assert_eq!(codes(&check(&tcx,&b,&[points(2),points(2)])),[code]);
    }
    let mut b = body(&[tcx.tys.unit,t,r],vec![block(vec![borrow(2,p(1),1,BorrowKind::Fake(FakeBorrowKind::Shallow)),
        assign(1,val(t)), StatementKind::FakeRead(FakeReadCause::ForMatchGuard,p(2))],TerminatorKind::Return)]);
    assert_eq!(codes(&check(&tcx,&b,&[points(3)])),["E0510"]);
    b.basic_blocks[0].statements.pop();
    assert_eq!(codes(&check(&tcx,&b,&[points(2)])),["E0506"]);
}
#[test]
fn never_initialized_mut_and_promoted_locals_are_isolated() {
    let tcx=Tcx::new(); let t=tcx.tys.i32_;
    let mut b=body(&[tcx.tys.unit,t,t],vec![block(vec![],TerminatorKind::Return)]);
    b.local_decls[1].kind=LocalKind::Arg;
    let result=check(&tcx,&b,&[]); assert!(result.accepted());
    assert!(!result.used_mut.contains(1)); assert!(result.used_mut.contains(2));
    let mut promoted=body(&[tcx.tys.unit,t,t],vec![block(vec![assign(2,val(t))],TerminatorKind::Return)]);
    promoted.source.kind=BodyKind::Promoted(DefId(0),PromotedId(0)); b.promoted.push(promoted);
    assert!(check(&tcx,&b,&[]).used_mut.contains(2));
}
#[test]
fn solved_outlives_constraints_extend_borrow_regions() {
    let tcx=Tcx::new(); let t=tcx.tys.i32_; let r=reference(&tcx,t,true);
    let b=body(&[tcx.tys.unit,t,r],vec![block(vec![borrow(2,p(1),1,BorrowKind::Shared),assign(1,val(t))],TerminatorKind::Return)]);
    let mut regions=input(&b,&[points(1),points(2)]);
    regions.outlives.push(OutlivesConstraint { sup:1,sub:2,location:Some(0),span:Span {file:7,lo:10,hi:11},category:ConstraintCategory::Assignment,from_closure:false });
    let mut solved=compute_regions(&tcx, &test_prog(), &b,regions,false);
    assert_eq!(codes(&mir_borrowck(&tcx, &test_prog(), &b,&mut solved,&gather(&tcx,&b))),["E0506"]);
}
fn external_input(b: &Body) -> RegionInput {
    let definitions=vec![RegionDefinition {origin:Origin::FreeRegion,universe:UniverseIndex(0),external_name:None};3];
    RegionInput { universe_causes: vec![], polonius: Default::default(), tainted_by_errors:false, definitions, outlives:OutlivesConstraintSet::default(),
        relations:UniversalRegionRelations::create(UniversalRegions::new(0,None,vec![(0,Class::Global),(1,Class::External),(2,Class::External)]),3,&[]),
        liveness:LivenessValues::new(3,LocationMap::new(b).num_points()),type_tests:vec![] }
}
/// `_1: i32` borrowed as `_2 = &_1` (region living across the yield), then a yield: a coroutine body.
fn yielding(tcx: &Tcx, movability: Movability, borrowed: Place) -> Body {
    let t = tcx.tys.i32_; let r = reference(tcx, t, false);
    let mut b = body(&[tcx.tys.unit, t, r], vec![block(vec![assign(1, val(t)), borrow(2, borrowed, 1, BorrowKind::Shared)],
        TerminatorKind::Yield { value: val(tcx.tys.unit), resume: BasicBlock(1), resume_arg: p(0), drop: None }),
        block(vec![assign(0, Operand::Copy(Place { local: Local(2), projection: vec![PlaceElem::Deref] }))], TerminatorKind::Return)]);
    b.arg_count = 0;
    b.coroutine = Some(CoroutineInfo { yield_ty: tcx.tys.unit, resume_ty: tcx.tys.unit, kind: CoroutineKind::Coroutine(movability) });
    b
}
#[test]
fn borrow_of_local_data_across_a_movable_coroutine_yield_is_e0626() {
    let tcx = Tcx::new();
    // `let r = &x; yield; *r`: the coroutine may be moved at the yield, so `x` cannot be borrowed across it.
    let b = yielding(&tcx, Movability::Movable, p(1));
    let result = check(&tcx, &b, &[points(3), points(3)]);
    assert_eq!(codes(&result), ["E0626"]);
    // A `static` (immovable) coroutine may keep the borrow.
    let b = yielding(&tcx, Movability::Static, p(1));
    assert!(check(&tcx, &b, &[points(3), points(3)]).accepted());
}

/// A closure body `|| { (*_1).0 = 1 }` over a one-field environment.
fn closure_assigning_upvar(tcx: &Tcx, mutability: Mutability, env_mutable: bool) -> Body {
    let t = tcx.tys.i32_;
    let closure = tcx.tys.intern(TyKind::Closure(7, 1, crate::types::ClosureArgs { parent_args: tcx.tys.unit, closure_kind_ty: tcx.tys.prim(crate::program::Prim::I8),
        closure_sig_as_fn_ptr_ty: tcx.tys.intern(TyKind::FnPtr(tcx.tys.mk_list(&[]), tcx.tys.unit, crate::types::FnHeader::RUST, 0)),
        tupled_upvars_ty: tcx.tys.intern(TyKind::Tuple(tcx.tys.mk_list(&[t]))) }));
    let env = tcx.tys.intern(TyKind::Ref(env_mutable, closure, RegionId::ERASED));
    let field = Place { local: Local(1), projection: vec![PlaceElem::Deref, PlaceElem::Field(FieldIdx(0), t)] };
    let mut b = body(&[tcx.tys.unit, env], vec![block(vec![StatementKind::Assign(field, Rvalue::Use(val(t)))], TerminatorKind::Return)]);
    b.local_decls[1].kind = LocalKind::Arg; b.source.kind = BodyKind::Closure(1);
    b.upvars = vec![CapturedPlace { name: "x".into(), var: 0, root_ty: t, projection: vec![], ty: t, capture: UpvarCapture::ByValue, mutability,
        path_span: Span::default(), kind_span: Span::default() }];
    b
}
#[test]
fn assigning_a_mutable_upvar_is_reported_back_to_the_creator() {
    let tcx = Tcx::new();
    let b = closure_assigning_upvar(&tcx, Mutability::Mut, true);
    let result = check(&tcx, &b, &[]);
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(result.used_mut_upvars, [FieldIdx(0)]);
    // An immutable binding cannot be assigned through the closure (E0594).
    let b = closure_assigning_upvar(&tcx, Mutability::Not, true);
    assert_eq!(codes(&check(&tcx, &b, &[])), ["E0594"]);
    // And a mutable upvar of an `Fn` closure (shared environment) neither.
    let b = closure_assigning_upvar(&tcx, Mutability::Mut, false);
    assert_eq!(codes(&check(&tcx, &b, &[])), ["E0594"]);
}
#[test]
fn a_variable_mutated_by_the_closure_it_is_moved_into_is_used_mut() {
    use super::check::{mir_borrowck_nested, NestedUsedMut};
    let tcx = Tcx::new(); let t = tcx.tys.i32_;
    let id = ClosureId { file: 7, expr: 1 };
    let closure = tcx.tys.intern(TyKind::Closure(7, 1, crate::types::ClosureArgs { parent_args: tcx.tys.unit, closure_kind_ty: tcx.tys.prim(crate::program::Prim::I8),
        closure_sig_as_fn_ptr_ty: tcx.tys.intern(TyKind::FnPtr(tcx.tys.mk_list(&[]), tcx.tys.unit, crate::types::FnHeader::RUST, 0)),
        tupled_upvars_ty: tcx.tys.intern(TyKind::Tuple(tcx.tys.mk_list(&[t]))) }));
    // `let mut x = 0; let c = move || x += 1;` : `_2 = Closure(move _1)`.
    let mut b = body(&[tcx.tys.unit, t, closure], vec![block(vec![assign(1, val(t)),
        StatementKind::Assign(p(2), Rvalue::Aggregate(Box::new(AggregateKind::Closure(id, vec![])), vec![Operand::Move(p(1))]))], TerminatorKind::Return)]);
    b.arg_count = 0;
    b.local_decls[1].local_info = LocalInfo::User(BindingForm::Var(VarBindingForm { binding_mode: BindingMode(ByRef::No, Mutability::Mut), opt_ty_info: None,
        opt_match_place: None, pat_span: Span::default(), introductions: vec![] }));
    let moves = gather(&tcx, &b);
    let run = |nested: NestedUsedMut| {
        let mut regions = compute_regions(&tcx, &test_prog(), &b, input(&b, &[]), false);
        mir_borrowck_nested(&tcx, &test_prog(), &b, &mut regions, &moves, None, &nested)
    };
    assert!(run(NestedUsedMut(vec![(id, vec![FieldIdx(0)])])).used_mut.contains(1));
    assert!(!run(NestedUsedMut(vec![(id, vec![])])).used_mut.contains(1));
    // Without the nested body's result the aggregate is an explicit stop.
    assert!(!run(NestedUsedMut::default()).unsupported.is_empty());
}
#[test]
fn subslice_reads_only_overlapping_moved_elements() {
    let tcx = Tcx::new(); let t = tcx.tys.i32_; let array = tcx.tys.intern(TyKind::Array(t, tcx.tys.const_usize(3)));
    let elem = |offset| Place { local: Local(1), projection: vec![PlaceElem::ConstantIndex { offset, min_length: 3, from_end: false }] };
    let slice = |from, to| Place { local: Local(1), projection: vec![PlaceElem::Subslice { from, to, from_end: false }] };
    let mut b = body(&[tcx.tys.unit, array, t, array], vec![block(vec![assign(2, Operand::Move(elem(0))), assign(3, Operand::Move(slice(1, 3)))], TerminatorKind::Return)]);
    assert!(check(&tcx, &b, &[]).accepted());
    b.basic_blocks[0].statements[1].kind = assign(3, Operand::Move(slice(0, 2)));
    assert_eq!(codes(&check(&tcx, &b, &[])), ["E0382"]);
}
#[test]
fn temporary_primary_span_is_creation_and_const_exit_preserves_storage() {
    let tcx = Tcx::new(); let t = tcx.tys.i32_; let r = reference(&tcx, t, false);
    let mut b = body(&[tcx.tys.unit, t, r], vec![block(vec![borrow(2, p(1), 1, BorrowKind::Shared), StatementKind::StorageDead(Local(1))], TerminatorKind::Return)]);
    b.local_decls[1].kind = LocalKind::Temp; b.local_decls[1].source_info.span = span(55);
    let result = check(&tcx, &b, &[points(2)]);
    assert_eq!(codes(&result), ["E0716"]); assert_eq!(result.errors[0].span, span(55));
    b.basic_blocks[0].statements.pop(); b.source.kind = BodyKind::Const;
    assert!(check(&tcx, &b, &[points(2)]).accepted());
}
#[test]
fn used_mut_temporary_propagates_to_moved_user_local() {
    let tcx = Tcx::new(); let t = tcx.tys.i32_;
    let mut b = body(&[tcx.tys.unit, t, t], vec![block(vec![assign(2, Operand::Move(p(1)))], TerminatorKind::Return)]);
    b.local_decls[2].kind = LocalKind::Temp;
    let moves = gather(&tcx, &b); let mut used = crate::mir::dataflow::BitSet::new_empty(3); used.insert(2);
    super::used_muts::gather_used_muts(&b, &moves, &mut used); assert!(used.contains(1));
}
#[test]
fn destructor_prefix_borrow_and_illegal_field_move() {
    let session = std::sync::Arc::new(crate::Session::default());
    let file = std::env::temp_dir().join(format!("relax-cnll4-drop-{}.rs", std::process::id()));
    std::fs::write(&file, "#![no_std]\n#[lang = \"drop\"] trait Drop { fn drop(&mut self); }\nstruct S { value: i32 }\nimpl Drop for S { fn drop(&mut self) {} }").unwrap();
    let mut prog = Program::new(&session);
    prog.load_crate("custom",file.to_str().unwrap(),2021,crate::cfg::CfgSet::host(&session,&[]),vec![]).unwrap();
    std::fs::remove_file(file).unwrap(); prog.resolve_imports(); let mut tcx = Tcx::new(); tcx.collect(&prog); assert!(tcx.errors.is_empty());
    let def = (0..prog.defs.len()).map(|i|DefId(i as u32)).find(|&d|prog.name(d)=="S").unwrap();
    assert!(tcx.adt_destructor(def).is_some());
    let ty = tcx.tys.intern(TyKind::Adt(def,tcx.tys.mk_list(&[]),crate::types::RegionArgsId::EMPTY)); let t=tcx.tys.i32_;
    let field = Place {local:Local(1),projection:vec![PlaceElem::Field(FieldIdx(0),t)]};
    let mut b = body(&[tcx.tys.unit,ty,reference(&tcx,t,false),t],vec![block(vec![borrow(2,field.clone(),1,BorrowKind::Shared)],
        TerminatorKind::Drop {place:p(1),target:BasicBlock(1),unwind:UnwindAction::Continue,replace:false}),block(vec![],TerminatorKind::Return)]);
    let moves=MoveData::gather_moves(&b,&prog,&tcx,TypingEnv::fully_monomorphized(&prog),&|_|true);
    let mut regions=compute_regions(&tcx, &test_prog(), &b,input(&b,&[points(2)]),false);
    assert_eq!(codes(&mir_borrowck(&tcx, &test_prog(), &b,&mut regions,&moves)),["E0713"]);
    b.basic_blocks[0].statements[0].kind=assign(3,Operand::Move(field));
    let moves=MoveData::gather_moves(&b,&prog,&tcx,TypingEnv::fully_monomorphized(&prog),&|_|true);
    let mut regions=compute_regions(&tcx, &test_prog(), &b,input(&b,&[]),false);
    assert_eq!(codes(&mir_borrowck(&tcx, &test_prog(), &b,&mut regions,&moves)),["E0509"]);
}

#[test]
fn cnll4_replays_reverse_postorder_before_deduplicating_uninitialized_errors() {
    let tcx = Tcx::new(); let t = tcx.tys.i32_;
    let mut later = block(vec![assign(3,Operand::Copy(p(2)))],TerminatorKind::Return);
    later.statements[0].source_info.span = span(20);
    let mut earlier = block(vec![assign(3,Operand::Copy(p(2)))],goto(1));
    earlier.statements[0].source_info.span = span(10);
    let b = body(&[tcx.tys.unit,t,t,t],vec![block(vec![],goto(2)),later,earlier]);
    let result = check(&tcx,&b,&[]);
    assert_eq!(codes(&result),["E0381"]);
    assert_eq!(result.errors[0].span,span(10));
}
#[test]
fn cnll4_used_mut_propagates_all_non_user_locals_and_excludes_arguments() {
    let tcx = Tcx::new(); let t = tcx.tys.i32_;
    let b = body(&[t,t,t],vec![block(vec![assign(0,Operand::Move(p(1)))],TerminatorKind::Return)]);
    let moves = gather(&tcx,&b);
    let mut used = crate::mir::dataflow::BitSet::new_empty(3); used.insert(0);
    super::used_muts::gather_used_muts(&b,&moves,&mut used);
    assert!(used.contains(1));
    let mut used = crate::mir::dataflow::BitSet::new_empty(3);
    super::used_muts::gather_used_muts(&b,&moves,&mut used);
    assert!(!used.contains(1),"unused mutable arguments are not never-initialized locals");
    assert!(used.contains(2));
}

#[test]
fn cnll4_typecheck_taint_survives_region_solve() {
    let tcx = Tcx::new();
    let b = body(&[tcx.tys.unit,tcx.tys.i32_],vec![block(vec![],TerminatorKind::Return)]);
    let moves = gather(&tcx,&b);
    let mut failed = input(&b,&[]); failed.tainted_by_errors = true;
    let mut solved = compute_regions(&tcx, &test_prog(), &b,failed,false);
    let result = mir_borrowck(&tcx, &test_prog(), &b,&mut solved,&moves);
    assert!(result.errors.is_empty() && result.region_errors.is_empty());
    assert!(!result.accepted());
}

#[test]
fn cnll4_shallow_fake_borrow_fallback_precedes_closure_capture_code() {
    let tcx = Tcx::new(); let t = tcx.tys.i32_; let r = reference(&tcx,t,true);
    let b = body(&[tcx.tys.unit,t,r,r],vec![block(vec![
        borrow(2,p(1),1,BorrowKind::Fake(FakeBorrowKind::Shallow)),
        borrow(3,p(1),2,BorrowKind::Mut {kind:MutBorrowKind::ClosureCapture}),
    ],TerminatorKind::Return)]);
    assert_eq!(codes(&check(&tcx,&b,&[points(2),points(2)])),["E0502"]);
}
#[test]
fn cnll4_lifetime_diagnostic_respects_access_error_deduplication() {
    let tcx = Tcx::new(); let t = tcx.tys.i32_; let r = reference(&tcx,t,true);
    let mut b = body(&[tcx.tys.unit,t,r],vec![block(vec![borrow(2,p(1),1,mutable())],TerminatorKind::Return)]);
    b.local_decls[1].mutability = Mutability::Not;
    assert_eq!(codes(&check(&tcx,&b,&[points(2)])),["E0596"]);
}
#[test]
fn cnll4_reverse_postorder_uses_nightly_successor_order() {
    let tcx = Tcx::new(); let t = tcx.tys.i32_;
    let mut first = block(vec![assign(3,Operand::Copy(p(2)))],TerminatorKind::Return);
    first.statements[0].source_info.span = span(10);
    let mut second = block(vec![assign(3,Operand::Copy(p(2)))],TerminatorKind::Return);
    second.statements[0].source_info.span = span(20);
    let b = body(&[tcx.tys.unit,t,t,t],vec![block(vec![],TerminatorKind::SwitchInt {
        discr:val(t),targets:SwitchTargets {values:vec![0],targets:vec![BasicBlock(1)],otherwise:BasicBlock(2)},
    }),first,second]);
    let result = check(&tcx,&b,&[]);
    assert_eq!(codes(&result),["E0381"]);
    assert_eq!(result.errors[0].span,span(10));
}

#[path = "polonius_tests.rs"]
mod polonius_tests;

// --- re-audit restorations (parity/reaudit.REPORT.md) ---
fn user(can_be_made_mutable: bool, match_place: Option<(Option<Place>, Span)>) -> LocalInfo {
    LocalInfo::user_var(BindingMode(if can_be_made_mutable { ByRef::No } else { ByRef::Yes(Mutability::Not) }, Mutability::Not), match_place, Span::default())
}
fn call(unit: TyId, args: Vec<Operand>, arg_spans: Vec<Span>, destination: Place) -> TerminatorKind {
    TerminatorKind::Call { func: val(unit), args, destination, target: Some(BasicBlock(1)),
        unwind: UnwindAction::Continue, call_source: CallSource::Normal, fn_span: span(80), arg_spans }
}
#[test]
fn reaudit_moved_error_suppressed_for_prefix_of_reported_place() {
    // nll/issue-21232-partial-init-and-use.rs test_0011: `drop(t); t.0 = ..; t.1 = ..;`
    // reports one E0382: the second use (`t`, prefix of the reported `t`) is suppressed.
    let tcx = Tcx::new(); let t = tcx.tys.i32_; let tuple = tcx.tys.intern(TyKind::Tuple(tcx.tys.mk_list(&[t, t])));
    let f = |n| Place { local: Local(1), projection: vec![PlaceElem::Field(FieldIdx(n), t)] };
    let b = body(&[tcx.tys.unit, tuple, tuple], vec![block(vec![assign(2, Operand::Move(p(1))),
        StatementKind::Assign(f(0), Rvalue::Use(val(t))), StatementKind::Assign(f(1), Rvalue::Use(val(t)))], TerminatorKind::Return)]);
    let result = check(&tcx, &b, &[]);
    assert_eq!(codes(&result), ["E0382"]); assert_eq!(result.errors[0].span, span(2));
}
#[test]
fn reaudit_descendant_search_visits_siblings_before_grandchildren() {
    // move_paths/mod.rs:106 find_descendant: the stack takes a path's first child, then
    // its next sibling, so a sibling is found before the first child's own children.
    let tcx = Tcx::new(); let t = tcx.tys.i32_;
    let inner = tcx.tys.intern(TyKind::Tuple(tcx.tys.mk_list(&[t, t]))); let outer = tcx.tys.intern(TyKind::Tuple(tcx.tys.mk_list(&[t, inner])));
    let f0 = Place { local: Local(1), projection: vec![PlaceElem::Field(FieldIdx(0), t)] };
    let f10 = Place { local: Local(1), projection: vec![PlaceElem::Field(FieldIdx(1), inner), PlaceElem::Field(FieldIdx(0), t)] };
    let b = body(&[tcx.tys.unit, outer, t, t], vec![block(vec![assign(2, Operand::Move(f0)), assign(3, Operand::Move(f10))], TerminatorKind::Return)]);
    let moves = gather(&tcx, &b);
    let root = moves.rev_lookup.find_local(Local(1)).unwrap();
    let first = moves.move_paths[root].first_child.unwrap();
    let grandchild = moves.move_paths[first].first_child.expect("the last-created child `_1.1` is the first child");
    let sibling = moves.move_paths[first].next_sibling.unwrap();
    let found = super::check::find_in_move_path_or_its_descendants(&moves, root, |p| p == grandchild || p == sibling);
    assert_eq!(found, Some(sibling));
    assert_eq!(super::check::find_in_move_path_or_its_descendants(&moves, root, |p| p == grandchild), Some(grandchild));
}
#[test]
fn reaudit_illegal_moves_of_one_match_are_grouped_at_the_match() {
    // diagnostics/move_errors.rs:118-232: `match *r { (a, b) => .. }` is one E0507 at the
    // matched place; a simple `let` points at the statement; other moves at the location.
    let tcx = Tcx::new(); let t = tcx.tys.i32_; let tuple = tcx.tys.intern(TyKind::Tuple(tcx.tys.mk_list(&[t, t])));
    let r = reference(&tcx, tuple, false);
    let deref = Place { local: Local(1), projection: vec![PlaceElem::Deref] };
    let field = |n| Place { local: Local(1), projection: vec![PlaceElem::Deref, PlaceElem::Field(FieldIdx(n), t)] };
    let mut b = body(&[tcx.tys.unit, r, t, t], vec![block(vec![assign(2, Operand::Move(field(0))), assign(3, Operand::Move(field(1)))], TerminatorKind::Return)]);
    for l in [2, 3] { b.local_decls[l].local_info = user(false, Some((Some(deref.clone()), span(60)))); }
    let result = check(&tcx, &b, &[]);
    assert_eq!(codes(&result), ["E0507"]); assert_eq!(result.errors[0].span, span(60));
    // `let a = (*r).0;`: from a simple let, the statement is the primary span.
    b.basic_blocks[0].statements.pop();
    b.local_decls[2].local_info = user(false, Some((None, span(60))));
    let result = check(&tcx, &b, &[]);
    assert_eq!(codes(&result), ["E0507"]); assert_eq!(result.errors[0].span, span(1));
    // A move out of borrowed content as a call argument: move_spans is the terminator.
    let b = body(&[tcx.tys.unit, r, t], vec![block(vec![], call(tcx.tys.unit, vec![Operand::Move(field(0))], vec![span(70)], p(2))), block(vec![], TerminatorKind::Return)]);
    let result = check(&tcx, &b, &[]);
    assert_eq!(codes(&result), ["E0507"]); assert_eq!(result.errors[0].span, span(99));
}
#[test]
fn reaudit_mutable_borrows_of_one_immutable_binding_share_one_error() {
    // diagnostics/mutability_errors.rs:231-250: the second `&mut x` of an immutable `x`
    // joins the first E0596, whose primary span becomes the binding.
    let tcx = Tcx::new(); let t = tcx.tys.i32_; let r = reference(&tcx, t, true);
    let mut b = body(&[tcx.tys.unit, t, r, r], vec![block(vec![borrow(2, p(1), 1, mutable()), borrow(3, p(1), 2, mutable())], TerminatorKind::Return)]);
    b.local_decls[1].mutability = Mutability::Not;
    b.local_decls[1].local_info = user(true, None);
    b.local_decls[1].source_info.span = span(40);
    let result = check(&tcx, &b, &[vec![at(0, 0)], vec![at(0, 1)]]);
    assert_eq!(codes(&result), ["E0596"]); assert_eq!(result.errors[0].span, span(40));
    b.basic_blocks[0].statements.pop();
    let result = check(&tcx, &b, &[vec![at(0, 0)], vec![]]);
    assert_eq!(codes(&result), ["E0596"]); assert_eq!(result.errors[0].span, span(1));
    // A binding that cannot be made mutable keeps one error per borrow.
    b.basic_blocks[0].statements.push(Statement { kind: borrow(3, p(1), 2, mutable()), source_info: SourceInfo { span: span(2), ..SourceInfo::default() } });
    b.local_decls[1].local_info = user(false, None);
    let result = check(&tcx, &b, &[vec![at(0, 0)], vec![at(0, 1)]]);
    assert_eq!(codes(&result), ["E0596", "E0596"]);
}
#[test]
fn reaudit_use_and_move_while_borrowed_take_the_location_span() {
    // conflict_errors.rs:1701,1766: E0505/E0503 at a call point at the terminator, while
    // E0382 keeps the argument span (borrow_spans fallback).
    let tcx = Tcx::new(); let t = tcx.tys.i32_;
    for (kind, arg, expected) in [(mutable(), Operand::Copy(p(1)), "E0503"), (BorrowKind::Shared, Operand::Move(p(1)), "E0505")] {
        let r = reference(&tcx, t, matches!(kind, BorrowKind::Mut { .. }));
        let b = body(&[tcx.tys.unit, t, r, t], vec![block(vec![borrow(2, p(1), 1, kind)], call(tcx.tys.unit, vec![arg], vec![span(70)], p(3))),
            block(vec![], TerminatorKind::Return)]);
        let result = check(&tcx, &b, &[points(2)]);
        assert_eq!(codes(&result), [expected]); assert_eq!(result.errors[0].span, span(99));
    }
    let b = body(&[tcx.tys.unit, t, t, t], vec![block(vec![assign(2, Operand::Move(p(1)))], call(tcx.tys.unit, vec![Operand::Move(p(1))], vec![span(70)], p(3))),
        block(vec![], TerminatorKind::Return)]);
    let result = check(&tcx, &b, &[]);
    assert_eq!(codes(&result), ["E0382"]); assert_eq!(result.errors[0].span, span(70));
}

fn test_prog() -> crate::program::Program { crate::program::Program::new(&std::sync::Arc::new(crate::Session::default())) }
