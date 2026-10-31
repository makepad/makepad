use super::borrow_set::{BorrowData, BorrowSet, LocalsStateAtExit, TwoPhaseActivation};
use super::def_use::{categorize, DefUse};
use super::path_utils::{borrow_of_local_data, each_borrow_involving_path, is_active};
use super::place_ext::ignore_borrow;
use super::places_conflict::{borrow_conflicts_with_place, places_conflict, AccessDepth, ArtificialField, PlaceConflictBias};
use super::prefixes::{prefixes, PrefixSet};
use crate::mir::move_paths::MoveData;
use crate::mir::visit::{MutatingUseContext as M, NonMutatingUseContext as N, NonUseContext as U, PlaceContext, Visitor};
use crate::mir::*;
use crate::program::{DefId, Program};
use crate::tcx::{Tcx, TypingEnv};
use crate::types::{RegionArgsId, RegionId, TyId, TyKind};
use std::ops::ControlFlow;

fn place(local: u32, projection: Vec<PlaceElem>) -> Place {
    Place { local: Local(local), projection }
}
fn local(i: u32) -> Place {
    Place::local(Local(i))
}
fn at(block: u32, statement_index: usize) -> Location {
    Location { block: BasicBlock(block), statement_index }
}
fn block(statements: Vec<StatementKind>, kind: TerminatorKind) -> BasicBlockData {
    BasicBlockData {
        statements: statements.into_iter().map(|kind| Statement { kind, source_info: SourceInfo::default() }).collect(),
        terminator: Some(Terminator { kind, source_info: SourceInfo::default() }),
        is_cleanup: false,
    }
}
fn body(types: &[TyId], basic_blocks: Vec<BasicBlockData>) -> Body {
    Body {
        phase: MirPhase::Analysis(AnalysisPhase::Initial),
        source: MirSource { owner: DefId(0), kind: BodyKind::Fn },
        local_decls: types
            .iter()
            .enumerate()
            .map(|(i, &ty)| LocalDecl {
                ty,
                mutability: Mutability::Mut,
                user_ty: Vec::new(),
                source_info: SourceInfo::default(),
                kind: if i == 0 { LocalKind::Return } else { LocalKind::Temp },
                local_info: LocalInfo::Boring,
            })
            .collect(),
        basic_blocks,
        source_scopes: vec![SourceScopeData { span: Span::default(), parent_scope: None }],
        user_type_annotations: Vec::new(),
        span: Span::default(), upvars: Vec::new(), allocs: AllocMap::default(),
        var_debug_info: Vec::new(),
        arg_count: 0,
        spread_arg: None,
        promoted: Vec::new(),
        coroutine: None,
    }
}
fn empty_body(types: &[TyId]) -> Body {
    body(types, vec![block(vec![], TerminatorKind::Return)])
}
fn borrow(dest: u32, source: Place, kind: BorrowKind) -> StatementKind {
    StatementKind::Assign(local(dest), Rvalue::Ref(Region(7), kind, source))
}
fn gather(tcx: &Tcx, b: &Body, invalidated: bool) -> BorrowSet {
    let session = std::sync::Arc::new(crate::Session::default());
    let prog = Program::new(&session);
    let moves = MoveData::gather_moves(b, &prog, tcx, TypingEnv::fully_monomorphized(&prog), &|_| true);
    BorrowSet::build(tcx, b, invalidated, &moves)
}
fn reference(tcx: &Tcx, mutable: bool, ty: TyId) -> TyId {
    tcx.tys.intern(TyKind::Ref(mutable, ty, RegionId::ERASED))
}
const MUT: BorrowKind = BorrowKind::Mut { kind: MutBorrowKind::Default };
const TWO_PHASE: BorrowKind = BorrowKind::Mut { kind: MutBorrowKind::TwoPhaseBorrow };
fn conflict(tcx: &Tcx, b: &Body, borrow: &Place, access: &Place) -> bool {
    places_conflict(tcx, b, borrow, access, PlaceConflictBias::Overlap)
}
fn depth(tcx: &Tcx, b: &Body, borrow: &Place, access: &Place, depth: AccessDepth) -> bool {
    borrow_conflicts_with_place(tcx, b, borrow, MUT, access, depth, PlaceConflictBias::Overlap)
}
fn collect(source: &str) -> (Program, Tcx) {
    let session = std::sync::Arc::new(crate::Session::default());
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let index = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let file = std::env::temp_dir().join(format!("relax-cnll2-{}-{index}.rs", std::process::id()));
    std::fs::write(&file, source).unwrap();
    let mut prog = Program::new(&session);
    prog.load_crate("custom", file.to_str().unwrap(), 2021, crate::cfg::CfgSet::host(&session, &[]), Vec::new()).unwrap();
    std::fs::remove_file(file).unwrap();
    prog.resolve_imports();
    let mut tcx = Tcx::new();
    tcx.collect(&prog);
    assert!(tcx.errors.is_empty(), "{:?}", tcx.errors);
    (prog, tcx)
}
fn adt(prog: &Program, tcx: &Tcx, name: &str, args: Vec<TyId>) -> TyId {
    let def = (0..prog.defs.len()).map(|i| DefId(i as u32)).find(|&d| prog.name(d) == name).unwrap();
    tcx.tys.intern(TyKind::Adt(def, tcx.tys.mk_list(&args), RegionArgsId::EMPTY))
}

#[test]
fn gather_all_borrow_kinds_and_maps() {
    let tcx = Tcx::new();
    let t = tcx.tys.i32_;
    let kinds = [
        BorrowKind::Shared,
        MUT,
        BorrowKind::Mut { kind: MutBorrowKind::ClosureCapture },
        BorrowKind::Fake(FakeBorrowKind::Shallow),
        BorrowKind::Fake(FakeBorrowKind::Deep),
    ];
    let b = body(
        &[tcx.tys.unit, t, reference(&tcx, true, t)],
        vec![
            block(kinds.iter().map(|&kind| borrow(2, local(1), kind)).collect(), TerminatorKind::Return),
            block(vec![borrow(2, local(1), MUT)], TerminatorKind::Return), // unreachable
        ],
    );
    let set = gather(&tcx, &b, true);
    assert_eq!(set.len(), kinds.len());
    assert_eq!(set.iter().count(), kinds.len());
    assert_eq!(set.borrows_on_local(Local(1)), Some(&[0, 1, 2, 3, 4][..]));
    assert_eq!(set.borrows_on_local(Local(2)), None);
    assert_eq!(set.borrows_at_location(&at(1, 0)), None);
    for (i, data) in set.iter_enumerated() {
        assert_eq!(data.kind, kinds[i]);
        assert_eq!(data.region, Region(7));
        assert_eq!(data.reserve_location, at(0, i));
        assert_eq!(data.activation_location, TwoPhaseActivation::NotTwoPhase);
        assert_eq!(data.borrowed_place, local(1));
        assert_eq!(data.assigned_place, local(2));
        assert_eq!(set.borrows_at_location(&at(0, i)), Some(&[i][..]));
        assert!(set.activations_at_location(&at(0, i)).is_empty());
    }
}

#[test]
fn two_phase_preorder_activation_and_nonuses() {
    let tcx = Tcx::new();
    let t = tcx.tys.i32_;
    let r = reference(&tcx, true, t);
    // Block numbering deliberately places the activation before its reservation.
    let b = body(
        &[tcx.tys.unit, t, r, r],
        vec![
            block(vec![], TerminatorKind::Goto { target: BasicBlock(2) }),
            block(vec![StatementKind::Assign(local(3), Rvalue::Use(Operand::Move(local(2))))], TerminatorKind::Return),
            block(
                vec![borrow(2, local(1), TWO_PHASE), StatementKind::StorageLive(Local(2)), StatementKind::StorageDead(Local(2))],
                TerminatorKind::Goto { target: BasicBlock(1) },
            ),
        ],
    );
    let set = gather(&tcx, &b, true);
    assert_eq!(set.len(), 1);
    assert_eq!(set[0].reserve_location, at(2, 0));
    assert_eq!(set[0].activation_location, TwoPhaseActivation::ActivatedAt(at(1, 0)));
    assert_eq!(set.activations_at_location(&at(1, 0)), &[0]);
    assert!(set.activations_at_location(&at(2, 0)).is_empty());
}

#[test]
fn two_phase_call_activates_multiple_borrows() {
    let tcx = Tcx::new();
    let t = tcx.tys.i32_;
    let r = reference(&tcx, true, t);
    let b = body(
        &[tcx.tys.unit, t, t, r, r],
        vec![block(
            vec![borrow(3, local(1), TWO_PHASE), borrow(4, local(2), TWO_PHASE)],
            TerminatorKind::Call {
                func: Operand::Constant(Box::new(ConstOperand {
                    span: Span::default(),
                    user_ty: None,
                    const_: Const::Val(ConstValue::ZeroSized, tcx.tys.intern(TyKind::FnDef(DefId(0), tcx.tys.mk_list(&[]), RegionArgsId::EMPTY))),
                })),
                args: vec![Operand::Move(local(3)), Operand::Move(local(4))],
                destination: local(0),
                target: None,
                unwind: UnwindAction::Continue,
                call_source: CallSource::Normal,
                fn_span: Span::default(),
                arg_spans: vec![],
            },
        )],
    );
    let set = gather(&tcx, &b, true);
    assert_eq!(set.activations_at_location(&at(0, 2)), &[0, 1]);
    assert!(set.iter().all(|b| b.activation_location == TwoPhaseActivation::ActivatedAt(at(0, 2))));
}

#[test]
fn unactivated_two_phase_borrow() {
    let tcx = Tcx::new();
    let t = tcx.tys.i32_;
    let b = body(&[tcx.tys.unit, t, reference(&tcx, true, t)], vec![block(vec![borrow(2, local(1), TWO_PHASE)], TerminatorKind::Return)]);
    let set = gather(&tcx, &b, true);
    assert_eq!(set[0].activation_location, TwoPhaseActivation::NotActivated);
    assert!(!is_active(|_, _| true, &set[0], at(0, 1)));
}

#[test]
#[should_panic(expected = "multiple uses of two-phase temporary")]
fn two_phase_rejects_multiple_activations() {
    let tcx = Tcx::new();
    let t = tcx.tys.i32_;
    let b = body(
        &[tcx.tys.unit, t, reference(&tcx, true, t)],
        vec![block(
            vec![borrow(2, local(1), TWO_PHASE), StatementKind::PlaceMention(local(2)), StatementKind::PlaceMention(local(2))],
            TerminatorKind::Return,
        )],
    );
    gather(&tcx, &b, true);
}

#[test]
fn ignore_shared_raw_and_thread_local_exception() {
    let tcx = Tcx::new();
    let t = tcx.tys.i32_;
    let shared = reference(&tcx, false, t);
    let raw = tcx.tys.intern(TyKind::Ptr(true, t));
    let mutable = reference(&tcx, true, t);
    let mut b = body(
        &[tcx.tys.unit, shared, raw, mutable, shared, mutable],
        vec![block((1..=4).map(|i| borrow(5, place(i, vec![PlaceElem::Deref]), MUT)).collect(), TerminatorKind::Return)],
    );
    b.local_decls[4].local_info = LocalInfo::StaticRef { def_id: DefId(0), is_thread_local: true };
    let set = gather(&tcx, &b, true);
    assert_eq!(set.iter().map(|d| d.borrowed_place.local).collect::<Vec<_>>(), vec![Local(3), Local(4)]);
    b.local_decls[4].local_info = LocalInfo::StaticRef { def_id: DefId(0), is_thread_local: false };
    assert_eq!(gather(&tcx, &b, true).len(), 1);
    let nested = tcx.tys.intern(TyKind::Tuple(tcx.tys.mk_list(&[shared])));
    b.local_decls[4].ty = nested;
    b.local_decls[4].local_info = LocalInfo::StaticRef { def_id: DefId(0), is_thread_local: true };
    assert!(ignore_borrow(
        &place(4, vec![PlaceElem::Field(FieldIdx(0), shared), PlaceElem::Deref]),
        &tcx,
        &b,
        &LocalsStateAtExit::AllAreInvalidated
    ));
}

#[test]
fn immutable_exit_state_storage_dead_move_and_promoted_isolation() {
    let tcx = Tcx::new();
    let t = tcx.tys.i32_;
    let r = reference(&tcx, false, t);
    let mut b = body(
        &[tcx.tys.unit, t, t, t, t, r],
        vec![block(
            vec![
                borrow(5, local(1), BorrowKind::Shared),
                borrow(5, local(2), BorrowKind::Shared),
                borrow(5, local(3), BorrowKind::Shared),
                StatementKind::StorageDead(Local(2)),
                StatementKind::Assign(local(4), Rvalue::Use(Operand::Move(local(3)))),
            ],
            TerminatorKind::Return,
        )],
    );
    for decl in &mut b.local_decls[1..4] {
        decl.mutability = Mutability::Not;
    }
    b.promoted.push(body(&[tcx.tys.unit, t], vec![block(vec![StatementKind::StorageDead(Local(1))], TerminatorKind::Return)]));
    let set = gather(&tcx, &b, false);
    assert_eq!(set.iter().map(|d| d.borrowed_place.local).collect::<Vec<_>>(), vec![Local(2), Local(3)]);
    let LocalsStateAtExit::SomeAreInvalidated { has_storage_dead_or_moved } = set.locals_state_at_exit() else { panic!() };
    assert!(!has_storage_dead_or_moved.contains(1));
    assert!(has_storage_dead_or_moved.contains(2));
    assert!(has_storage_dead_or_moved.contains(3));
    assert_eq!(gather(&tcx, &b, true).len(), 3);
}

#[test]
fn two_phase_active_diamond_and_loop() {
    let mut data = BorrowData {
        reserve_location: at(1, 2),
        activation_location: TwoPhaseActivation::ActivatedAt(at(4, 1)),
        kind: TWO_PHASE,
        region: Region(0),
        borrowed_place: local(1),
        assigned_place: local(2),
    };
    // 0 -> R(1) -> {2,3} -> A(4) -> 5; 5 may loop to R.
    let dominates = |a: BasicBlock, b: BasicBlock| a == b || a.0 == 0 || a.0 == 1 && b.0 >= 2 || a.0 == 4 && b.0 == 5;
    for loc in [at(0, 0), at(1, 2), at(4, 1), at(4, 2), at(5, 0)] {
        assert!(is_active(dominates, &data, loc), "{loc:?}");
    }
    for loc in [at(1, 3), at(2, 0), at(3, 0), at(4, 0)] {
        assert!(!is_active(dominates, &data, loc), "{loc:?}");
    }
    data.activation_location = TwoPhaseActivation::NotTwoPhase;
    assert!(is_active(dominates, &data, at(2, 0)));
    data.activation_location = TwoPhaseActivation::NotActivated;
    assert!(!is_active(dominates, &data, at(5, 0)));
}

#[test]
fn fields_variants_union_and_prefixes_conflict() {
    let (prog, tcx) = collect("struct Pair { a: i32, b: i32 } union U { a: (i32, i32), b: (i32, i32) } enum E { A(i32), B(i32) }");
    let t = tcx.tys.i32_;
    let pair = adt(&prog, &tcx, "Pair", vec![]);
    let u = adt(&prog, &tcx, "U", vec![]);
    let e = adt(&prog, &tcx, "E", vec![]);
    let tuple = tcx.tys.intern(TyKind::Tuple(tcx.tys.mk_list(&[t, t])));
    let b = empty_body(&[tcx.tys.unit, pair, u, e]);
    let f = |i, n| place(i, vec![PlaceElem::Field(FieldIdx(n), t)]);
    assert!(conflict(&tcx, &b, &local(1), &local(1)));
    assert!(!conflict(&tcx, &b, &local(1), &local(2)));
    assert!(conflict(&tcx, &b, &f(1, 0), &f(1, 0)));
    assert!(!conflict(&tcx, &b, &f(1, 0), &f(1, 1)));
    assert!(conflict(&tcx, &b, &f(1, 0), &local(1)));
    assert!(conflict(&tcx, &b, &local(1), &f(1, 1)));
    let a = place(2, vec![PlaceElem::Field(FieldIdx(0), tuple), PlaceElem::Field(FieldIdx(0), t)]);
    let c = place(2, vec![PlaceElem::Field(FieldIdx(1), tuple), PlaceElem::Field(FieldIdx(1), t)]);
    assert!(conflict(&tcx, &b, &a, &c)); // arbitrary union overlap must stop before the disjoint fields
    let v = |n| place(3, vec![PlaceElem::Downcast(None, VariantIdx(n)), PlaceElem::Field(FieldIdx(0), t)]);
    assert!(!conflict(&tcx, &b, &v(0), &v(1)));
    assert!(conflict(&tcx, &b, &v(0), &v(0)));
    let opaque = place(1, vec![PlaceElem::OpaqueCast(pair)]);
    assert!(conflict(&tcx, &b, &opaque, &opaque));
}

#[test]
fn dereferences_shallow_drop_box_and_destructor() {
    let (prog, tcx) = collect(
        r#"
        #[lang = "owned_box"] struct Heap<T>(*mut T);
        #[lang = "drop"] trait Cleanup { fn release(&mut self); }
        struct D { r: &'static mut i32 } impl Cleanup for D { fn release(&mut self) {} }
        struct Plain { r: &'static mut i32 }
    "#,
    );
    let t = tcx.tys.i32_;
    let r = reference(&tcx, true, t);
    let b = empty_body(&[
        tcx.tys.unit,
        r,
        reference(&tcx, false, t),
        adt(&prog, &tcx, "Heap", vec![t]),
        adt(&prog, &tcx, "D", vec![]),
        adt(&prog, &tcx, "Plain", vec![]),
    ]);
    for i in 1..=3 {
        let deref = place(i, vec![PlaceElem::Deref]);
        assert!(conflict(&tcx, &b, &deref, &deref));
        assert!(!depth(&tcx, &b, &deref, &local(i), AccessDepth::Shallow(None)));
        assert!(conflict(&tcx, &b, &local(i), &deref));
    }
    assert!(!depth(&tcx, &b, &place(1, vec![PlaceElem::Deref]), &local(1), AccessDepth::Drop));
    assert!(depth(&tcx, &b, &place(3, vec![PlaceElem::Deref]), &local(3), AccessDepth::Drop));
    assert!(!ignore_borrow(&place(3, vec![PlaceElem::Deref]), &tcx, &b, &LocalsStateAtExit::AllAreInvalidated));
    for i in [4, 5] {
        let p = place(i, vec![PlaceElem::Field(FieldIdx(0), r), PlaceElem::Deref]);
        assert_eq!(depth(&tcx, &b, &p, &local(i), AccessDepth::Drop), i == 4);
        assert!(!depth(&tcx, &b, &p, &local(i), AccessDepth::Shallow(None)));
    }
}

#[test]
#[should_panic(expected = "tracking borrow behind shared reference")]
fn shared_deref_deep_prefix_is_an_invariant_failure() {
    let tcx = Tcx::new();
    let b = empty_body(&[tcx.tys.unit, reference(&tcx, false, tcx.tys.i32_)]);
    conflict(&tcx, &b, &place(1, vec![PlaceElem::Deref]), &local(1));
}

#[test]
fn fake_borrow_and_artificial_fields() {
    let tcx = Tcx::new();
    let t = tcx.tys.i32_;
    let b = empty_body(&[tcx.tys.unit, tcx.tys.intern(TyKind::Array(t, tcx.tys.const_usize(4)))]);
    let p = place(1, vec![PlaceElem::ConstantIndex { offset: 1, min_length: 4, from_end: false }]);
    for field in [ArtificialField::ArrayLength, ArtificialField::FakeBorrow] {
        assert!(!depth(&tcx, &b, &p, &local(1), AccessDepth::Shallow(Some(field))));
        assert!(depth(&tcx, &b, &local(1), &local(1), AccessDepth::Shallow(Some(field))));
    }
    for kind in [FakeBorrowKind::Shallow, FakeBorrowKind::Deep] {
        assert_eq!(
            borrow_conflicts_with_place(&tcx, &b, &local(1), BorrowKind::Fake(kind), &p, AccessDepth::Deep, PlaceConflictBias::Overlap),
            kind == FakeBorrowKind::Deep
        );
        assert!(borrow_conflicts_with_place(
            &tcx,
            &b,
            &p,
            BorrowKind::Fake(kind),
            &local(1),
            AccessDepth::Deep,
            PlaceConflictBias::Overlap
        ));
    }
}

fn index(offset: u64, min_length: u64, from_end: bool) -> PlaceElem {
    PlaceElem::ConstantIndex { offset, min_length, from_end }
}
fn slice(from: u64, to: u64, from_end: bool) -> PlaceElem {
    PlaceElem::Subslice { from, to, from_end }
}

#[test]
fn constant_indices_and_subslice_boundaries() {
    let tcx = Tcx::new();
    let b = empty_body(&[tcx.tys.unit, tcx.tys.intern(TyKind::Slice(tcx.tys.i32_))]);
    let cases = [
        (index(1, 4, false), index(1, 5, false), true),
        (index(1, 4, false), index(2, 4, false), false),
        (index(1, 4, true), index(1, 5, true), true),
        (index(1, 4, true), index(2, 4, true), false),
        (index(1, 4, false), index(1, 4, true), false),
        (index(3, 4, false), index(1, 4, true), true),
        (index(3, 4, false), index(1, 5, true), false),
        (index(1, 4, false), slice(1, 3, false), true),
        (index(3, 4, false), slice(1, 3, false), false),
        (index(0, 4, false), slice(1, 3, false), false),
        (index(0, 4, false), slice(1, 1, true), false),
        (index(1, 4, false), slice(1, 1, true), true),
        (index(1, 4, true), slice(1, 1, true), false),
        (index(2, 4, true), slice(1, 1, true), true),
        (slice(0, 2, false), slice(2, 4, false), false),
        (slice(0, 3, false), slice(2, 4, false), true),
        (slice(0, 1, true), slice(2, 4, false), true),
        (slice(0, 2, true), slice(2, 0, true), true),
    ];
    for (left, right, expected) in cases {
        for bias in [PlaceConflictBias::Overlap, PlaceConflictBias::NoOverlap] {
            let p = place(1, vec![left.clone()]);
            let q = place(1, vec![right.clone()]);
            assert_eq!(places_conflict(&tcx, &b, &p, &q, bias), expected, "{left:?} vs {right:?}");
            assert_eq!(places_conflict(&tcx, &b, &q, &p, bias), expected, "{right:?} vs {left:?}");
        }
    }
}

#[test]
fn runtime_index_bias_and_later_disjoint_fields() {
    let tcx = Tcx::new();
    let t = tcx.tys.i32_;
    let pair = tcx.tys.intern(TyKind::Tuple(tcx.tys.mk_list(&[t, t])));
    let b = empty_body(&[tcx.tys.unit, tcx.tys.intern(TyKind::Array(pair, tcx.tys.const_usize(4))), tcx.tys.usize_]);
    let p = place(1, vec![PlaceElem::Index(Local(2))]);
    for elem in [PlaceElem::Index(Local(2)), index(0, 4, false), slice(1, 3, false)] {
        let q = place(1, vec![elem]);
        assert!(places_conflict(&tcx, &b, &p, &q, PlaceConflictBias::Overlap));
        assert!(!places_conflict(&tcx, &b, &p, &q, PlaceConflictBias::NoOverlap));
        assert!(!places_conflict(&tcx, &b, &q, &p, PlaceConflictBias::NoOverlap));
    }
    let p = place(1, vec![PlaceElem::Index(Local(2)), PlaceElem::Field(FieldIdx(0), t)]);
    let q = place(1, vec![index(0, 4, false), PlaceElem::Field(FieldIdx(1), t)]);
    assert!(!conflict(&tcx, &b, &p, &q));
}

#[test]
fn prefixes_skip_indices_casts_and_downcasts_stop_at_deref() {
    let tcx = Tcx::new();
    let t = tcx.tys.i32_;
    let p = place(
        1,
        vec![
            PlaceElem::Deref,
            PlaceElem::Field(FieldIdx(0), t),
            PlaceElem::Index(Local(2)),
            index(0, 4, false),
            slice(1, 2, false),
            PlaceElem::Downcast(None, VariantIdx(0)),
            PlaceElem::OpaqueCast(t),
            PlaceElem::Field(FieldIdx(1), t),
        ],
    );
    let all: Vec<_> = prefixes(&p, PrefixSet::All).collect();
    assert_eq!(all.iter().map(|p| p.projection.len()).collect::<Vec<_>>(), vec![8, 2, 1, 0]);
    assert!(all.iter().all(|p| p.local == Local(1)));
    assert_eq!(prefixes(&p, PrefixSet::Shallow).map(|p| p.projection.len()).collect::<Vec<_>>(), vec![8, 2, 1]);
    let root = local(1);
    let mut iter = prefixes(&root, PrefixSet::All);
    assert_eq!(iter.next(), Some(root.clone()));
    assert_eq!(iter.next(), None);
    assert_eq!(iter.next(), None);
    assert!(!borrow_of_local_data(&p));
    assert!(borrow_of_local_data(&place(1, vec![PlaceElem::Field(FieldIdx(0), t)])));
}

#[test]
fn each_borrow_filters_candidates_conflicts_and_breaks() {
    let tcx = Tcx::new();
    let t = tcx.tys.i32_;
    let pair = tcx.tys.intern(TyKind::Tuple(tcx.tys.mk_list(&[t, t])));
    let f = |n| place(1, vec![PlaceElem::Field(FieldIdx(n), t)]);
    let b = body(
        &[tcx.tys.unit, pair, t, reference(&tcx, true, t)],
        vec![block(
            vec![borrow(3, f(0), MUT), borrow(3, f(1), MUT), borrow(3, f(0), MUT), borrow(3, local(2), MUT)],
            TerminatorKind::Return,
        )],
    );
    let set = gather(&tcx, &b, true);
    let mut seen = Vec::new();
    each_borrow_involving_path(
        &tcx,
        &b,
        (AccessDepth::Deep, &f(0)),
        &set,
        |i| i != 0,
        |i, _| {
            seen.push(i);
            ControlFlow::Continue(())
        },
    );
    assert_eq!(seen, vec![2]);
    seen.clear();
    each_borrow_involving_path(
        &tcx,
        &b,
        (AccessDepth::Deep, &local(1)),
        &set,
        |_| true,
        |i, _| {
            seen.push(i);
            ControlFlow::Break(())
        },
    );
    assert_eq!(seen, vec![0]);
    each_borrow_involving_path(&tcx, &b, (AccessDepth::Deep, &local(0)), &set, |_| panic!(), |_, _| panic!());
}

#[test]
fn def_use_all_supported_contexts() {
    use PlaceContext::*;
    for ctx in [M::Store, M::Call, M::AsmOutput, M::Yield] {
        assert_eq!(categorize(MutatingUse(ctx)), Some(DefUse::Def));
    }
    for ctx in [U::StorageLive, U::StorageDead] {
        assert_eq!(categorize(NonUse(ctx)), Some(DefUse::Def));
    }
    for ctx in [M::Projection, M::Borrow, M::RawBorrow] {
        assert_eq!(categorize(MutatingUse(ctx)), Some(DefUse::Use));
    }
    for ctx in [N::Projection, N::SharedBorrow, N::FakeBorrow, N::PlaceMention, N::RawBorrow, N::Inspect, N::Copy, N::Move] {
        assert_eq!(categorize(NonMutatingUse(ctx)), Some(DefUse::Use));
    }
    assert_eq!(categorize(NonUse(U::AscribeUserTy(Variance::Invariant))), Some(DefUse::Use));
    assert_eq!(categorize(MutatingUse(M::Drop)), Some(DefUse::Drop));
    assert_eq!(categorize(NonUse(U::VarDebugInfo)), None);
}

#[test]
fn def_use_through_mir_visitor() {
    let tcx = Tcx::new();
    let t = tcx.tys.i32_;
    let b = body(
        &[tcx.tys.unit, t, t],
        vec![
            block(
                vec![
                    StatementKind::StorageLive(Local(2)),
                    StatementKind::Assign(local(2), Rvalue::Use(Operand::Copy(local(1)))),
                    StatementKind::PlaceMention(local(1)),
                    StatementKind::AscribeUserType(
                        local(1),
                        UserTypeProjection { base: UserTypeAnnotationIndex(0), projection: vec![] },
                        Variance::Invariant,
                    ),
                ],
                TerminatorKind::Drop { place: local(2), target: BasicBlock(1), unwind: UnwindAction::Continue, replace: false },
            ),
            block(vec![], TerminatorKind::Return),
        ],
    );
    struct Uses(Vec<(Local, Option<DefUse>, Location)>);
    impl Visitor for Uses {
        fn visit_local(&mut self, local: &Local, context: PlaceContext, location: Location) {
            self.0.push((*local, categorize(context), location));
        }
    }
    let mut uses = Uses(Vec::new());
    uses.visit_basic_block_data(BasicBlock(0), &b.basic_blocks[0]);
    assert_eq!(
        uses.0,
        vec![
            (Local(2), Some(DefUse::Def), at(0, 0)),
            (Local(2), Some(DefUse::Def), at(0, 1)),
            (Local(1), Some(DefUse::Use), at(0, 1)),
            (Local(1), Some(DefUse::Use), at(0, 2)),
            (Local(1), Some(DefUse::Use), at(0, 3)),
            (Local(2), Some(DefUse::Drop), at(0, 4))
        ]
    );
}

#[test]
#[should_panic(expected = "statement not allowed")]
fn def_use_rejects_set_discriminant() {
    categorize(PlaceContext::MutatingUse(M::SetDiscriminant));
}

#[test]
fn generic_array_subslice_does_not_evaluate_unused_length() {
    let tcx = Tcx::new();
    let t = tcx.tys.i32_;
    let n = tcx.tys.intern_const(crate::consts::ConstKind::Param(crate::consts::ParamConst { index: 0 }));
    let array = tcx.tys.intern(TyKind::Array(t, n));
    let b = empty_body(&[tcx.tys.unit, array]);
    let p = place(1, vec![slice(1, 1, true)]);
    assert!(conflict(&tcx, &b, &p, &p));
    assert!(!ignore_borrow(&p, &tcx, &b, &LocalsStateAtExit::AllAreInvalidated));
}

#[test]
#[should_panic(expected = "two-phase borrow must assign to a local")]
fn two_phase_destination_must_be_a_local() {
    let tcx = Tcx::new();
    let t = tcx.tys.i32_;
    let r = reference(&tcx, true, t);
    let b = body(
        &[tcx.tys.unit, t, tcx.tys.intern(TyKind::Tuple(tcx.tys.mk_list(&[r])))],
        vec![block(
            vec![StatementKind::Assign(place(2, vec![PlaceElem::Field(FieldIdx(0), r)]), Rvalue::Ref(Region(7), TWO_PHASE, local(1)))],
            TerminatorKind::Return,
        )],
    );
    gather(&tcx, &b, true);
}

#[test]
#[should_panic(expected = "M0 has no inline-asm operands")]
fn inline_asm_gathering_stops_without_operand_metadata() {
    let tcx = Tcx::new();
    let session = std::sync::Arc::new(crate::Session::default());
    let prog = Program::new(&session);
    let mut b = empty_body(&[tcx.tys.unit]);
    let moves = MoveData::gather_moves(&b, &prog, &tcx, TypingEnv::fully_monomorphized(&prog), &|_| true);
    // MoveData also refuses inline asm; isolate BorrowSet's own boundary check.
    b.basic_blocks[0].terminator.as_mut().unwrap().kind = TerminatorKind::InlineAsm(InlineAsmUnsupported);
    BorrowSet::build(&tcx, &b, true, &moves);
}

#[test]
fn preorder_visits_loop_once_and_includes_unwind() {
    let tcx = Tcx::new();
    let t = tcx.tys.i32_;
    let r = reference(&tcx, true, t);
    let b = body(
        &[tcx.tys.unit, t, r],
        vec![
            block(
                vec![borrow(2, local(1), MUT)],
                TerminatorKind::Drop {
                    place: local(1),
                    target: BasicBlock(0),
                    unwind: UnwindAction::Cleanup(BasicBlock(1)),
                    replace: false,
                },
            ),
            block(vec![borrow(2, local(1), BorrowKind::Shared)], TerminatorKind::UnwindResume),
        ],
    );
    let set = gather(&tcx, &b, true);
    assert_eq!(set.len(), 2);
    assert_eq!(set[0].reserve_location, at(0, 0));
    assert_eq!(set[1].reserve_location, at(1, 0));
}
