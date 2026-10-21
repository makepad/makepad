use super::visit::{MutVisitor, NonMutatingUseContext as R, PlaceContext, Visitor};
use super::*;
use crate::program::Program;
use crate::tcx::Tcx;
use crate::types::TyKind;

fn local(i: u32) -> Place {
    Place::local(Local(i))
}
fn copy(i: u32) -> Operand {
    Operand::Copy(local(i))
}
fn val(ty: TyId, value: ConstValue) -> Operand {
    Operand::Constant(Box::new(ConstOperand { span: Span::default(), user_ty: None, const_: Const::Val(value, ty) }))
}
fn integer(ty: TyId) -> Operand {
    val(ty, ConstValue::Scalar(Scalar::Int { bits: 1, size: 4 }))
}
fn stmt(kind: StatementKind) -> Statement {
    Statement { source_info: SourceInfo::default(), kind }
}
fn block(kind: TerminatorKind, cleanup: bool) -> BasicBlockData {
    BasicBlockData {
        statements: Vec::new(),
        terminator: Some(Terminator { source_info: SourceInfo::default(), kind }),
        is_cleanup: cleanup,
    }
}
fn body(tcx: &Tcx) -> Body {
    let mut locals = Vec::new();
    for kind in [LocalKind::Return, LocalKind::Arg, LocalKind::Temp, LocalKind::UserVar] {
        locals.push(LocalDecl {
            ty: tcx.tys.i32_,
            mutability: Mutability::Mut,
            user_ty: Vec::new(),
            source_info: SourceInfo::default(),
            kind,
            local_info: LocalInfo::Boring,
        });
    }
    Body {
        phase: MirPhase::Built,
        source: MirSource { owner: DefId(0), kind: BodyKind::Fn },
        basic_blocks: vec![block(TerminatorKind::Return, false), block(TerminatorKind::Return, false)],
        local_decls: locals,
        source_scopes: vec![SourceScopeData { span: Span::default(), parent_scope: None }],
        user_type_annotations: vec![CanonicalUserTypeAnnotation::for_test(TyId(0), Span::default())],
        span: Span::default(),
        upvars: Vec::new(),
        allocs: AllocMap::default(),
        var_debug_info: Vec::new(),
        arg_count: 1,
        spread_arg: None,
        promoted: Vec::new(),
        coroutine: None,
    }
}
fn term(body: &mut Body, kind: TerminatorKind) {
    body.basic_blocks[0].terminator.as_mut().unwrap().kind = kind;
}
fn good(body: &Body, tcx: &Tcx) {
    assert_eq!(validate::validate(body, tcx), Vec::new());
}
fn bad(body: &Body, tcx: &Tcx, expected: &str) {
    let errors = validate::validate(body, tcx);
    assert!(errors.iter().any(|e| e.message.contains(expected)), "expected {expected}, got {errors:?}");
}
fn call(tcx: &Tcx, unwind: UnwindAction) -> TerminatorKind {
    let ty = tcx.tys.intern(TyKind::FnDef(DefId(0), crate::types::List::EMPTY, crate::types::RegionArgsId::EMPTY));
    TerminatorKind::Call {
        func: val(ty, ConstValue::ZeroSized),
        args: vec![copy(1)],
        destination: local(0),
        target: Some(BasicBlock(1)),
        unwind,
        call_source: CallSource::Normal,
        fn_span: Span::default(),
        arg_spans: vec![Span::default()],
    }
}
fn cleanup_body(tcx: &Tcx) -> Body {
    let mut b = body(tcx);
    b.basic_blocks.truncate(1);
    term(&mut b, call(tcx, UnwindAction::Cleanup(BasicBlock(2))));
    b.basic_blocks.push(block(TerminatorKind::Return, false));
    b.basic_blocks.push(block(TerminatorKind::UnwindResume, true));
    b
}

#[test]
fn cfg_cleanup_call_and_drop() {
    let tcx = Tcx::new();
    let mut b = cleanup_body(&tcx);
    good(&b, &tcx);
    b.basic_blocks[1].terminator.as_mut().unwrap().kind =
        TerminatorKind::Drop { place: local(2), target: BasicBlock(3), unwind: UnwindAction::Cleanup(BasicBlock(2)), replace: true };
    b.basic_blocks.push(block(TerminatorKind::Return, false));
    good(&b, &tcx);
    for phase in [RuntimePhase::Initial, RuntimePhase::PostCleanup, RuntimePhase::Optimized] {
        b.phase = MirPhase::Runtime(phase);
        good(&b, &tcx);
    }
    b.basic_blocks[2].is_cleanup = false;
    bad(&b, &tcx, "cleanup invariants");
}

#[test]
fn cfg_target_range_and_missing_terminator() {
    let tcx = Tcx::new();
    let mut b = body(&tcx);
    term(&mut b, TerminatorKind::Goto { target: BasicBlock(1) });
    good(&b, &tcx);
    term(&mut b, TerminatorKind::Goto { target: BasicBlock(10) });
    bad(&b, &tcx, "target block");
    b.basic_blocks[0].terminator = None;
    bad(&b, &tcx, "no terminator");
}

#[test]
fn cleanup_edges_preserve_status() {
    let tcx = Tcx::new();
    let mut b = cleanup_body(&tcx);
    b.basic_blocks.push(block(TerminatorKind::UnwindResume, true));
    b.basic_blocks[2].terminator.as_mut().unwrap().kind = TerminatorKind::Goto { target: BasicBlock(3) };
    good(&b, &tcx);
    term(&mut b, TerminatorKind::Goto { target: BasicBlock(2) });
    bad(&b, &tcx, "cleanup invariants");
    term(&mut b, call(&tcx, UnwindAction::Cleanup(BasicBlock(2))));
    b.basic_blocks[2].terminator.as_mut().unwrap().kind = TerminatorKind::Goto { target: BasicBlock(1) };
    bad(&b, &tcx, "cleanup invariants");
}

#[test]
fn cleanup_unwind_actions() {
    let tcx = Tcx::new();
    let mut b = cleanup_body(&tcx);
    for unwind in [
        UnwindAction::Unreachable,
        UnwindAction::Terminate(UnwindTerminateReason::InCleanup),
        UnwindAction::Terminate(UnwindTerminateReason::Abi),
    ] {
        b.basic_blocks[2].terminator.as_mut().unwrap().kind =
            TerminatorKind::Drop { place: local(2), target: BasicBlock(2), unwind, replace: false };
        good(&b, &tcx);
    }
    for (unwind, message) in [(UnwindAction::Continue, "unwind continue"), (UnwindAction::Cleanup(BasicBlock(2)), "cleanup invariants")] {
        b.basic_blocks[2].terminator.as_mut().unwrap().kind =
            TerminatorKind::Drop { place: local(2), target: BasicBlock(2), unwind, replace: false };
        bad(&b, &tcx, message);
    }
    let mut b = cleanup_body(&tcx);
    term(&mut b, call(&tcx, UnwindAction::Continue));
    good(&b, &tcx);
    term(&mut b, call(&tcx, UnwindAction::Terminate(UnwindTerminateReason::InCleanup)));
    bad(&b, &tcx, "outside cleanup");
}

#[test]
fn cleanup_terminal_rules() {
    let tcx = Tcx::new();
    let mut b = cleanup_body(&tcx);
    good(&b, &tcx);
    b.basic_blocks[2].terminator.as_mut().unwrap().kind = TerminatorKind::UnwindTerminate(UnwindTerminateReason::InCleanup);
    good(&b, &tcx);
    b.basic_blocks[2].terminator.as_mut().unwrap().kind = TerminatorKind::Return;
    bad(&b, &tcx, "return from cleanup");
    for kind in [TerminatorKind::UnwindResume, TerminatorKind::UnwindTerminate(UnwindTerminateReason::Abi)] {
        let mut b = body(&tcx);
        term(&mut b, kind);
        bad(&b, &tcx, "outside cleanup");
    }
}

#[test]
fn match_switch_unique_values_and_edges() {
    let tcx = Tcx::new();
    let mut b = body(&tcx);
    let mut targets = SwitchTargets { values: vec![0, 1], targets: vec![BasicBlock(1), BasicBlock(1)], otherwise: BasicBlock(1) };
    term(&mut b, TerminatorKind::SwitchInt { discr: copy(1), targets: targets.clone() });
    good(&b, &tcx);
    targets.values[1] = 0;
    term(&mut b, TerminatorKind::SwitchInt { discr: copy(1), targets: targets.clone() });
    bad(&b, &tcx, "duplicate switch");
    targets.values.pop();
    term(&mut b, TerminatorKind::SwitchInt { discr: copy(1), targets });
    bad(&b, &tcx, "count mismatch");
    term(
        &mut b,
        TerminatorKind::SwitchInt { discr: copy(1), targets: SwitchTargets { values: vec![], targets: vec![], otherwise: BasicBlock(4) } },
    );
    bad(&b, &tcx, "target block");
}

#[test]
fn local_range_in_operands_and_projection_indices() {
    let tcx = Tcx::new();
    let mut b = body(&tcx);
    b.local_decls[1].ty = tcx.tys.usize_;
    b.local_decls[2].ty = tcx.tys.intern(TyKind::Array(tcx.tys.i32_, tcx.tys.const_usize(4)));
    let mut p = local(2);
    p.projection.push(PlaceElem::Index(Local(1)));
    b.basic_blocks[0].statements.push(stmt(StatementKind::PlaceMention(p.clone())));
    good(&b, &tcx);
    p.projection[0] = PlaceElem::Index(Local(10));
    b.basic_blocks[0].statements[0] = stmt(StatementKind::PlaceMention(p));
    bad(&b, &tcx, "local is out of range");
    b.basic_blocks[0].statements[0] = stmt(StatementKind::Assign(local(2), Rvalue::Use(copy(10))));
    bad(&b, &tcx, "local is out of range");
}

#[test]
fn storage_live_dead_only_non_arguments() {
    let tcx = Tcx::new();
    for is_live in [true, false] {
        for i in [0, 1, 2, 3] {
            let mut b = body(&tcx);
            b.basic_blocks[0].statements.push(stmt(if is_live {
                StatementKind::StorageLive(Local(i))
            } else {
                StatementKind::StorageDead(Local(i))
            }));
            if i <= 1 {
                bad(&b, &tcx, "storage marker");
            } else {
                good(&b, &tcx);
            }
        }
    }
}

#[test]
fn analysis_statements_rejected_in_all_runtime_phases() {
    let tcx = Tcx::new();
    for kind in [
        StatementKind::FakeRead(FakeReadCause::ForMatchGuard, local(1)),
        StatementKind::AscribeUserType(
            local(1),
            UserTypeProjection { base: UserTypeAnnotationIndex(0), projection: vec![] },
            Variance::Covariant,
        ),
    ] {
        let mut b = body(&tcx);
        b.basic_blocks[0].statements.push(stmt(kind));
        good(&b, &tcx);
        b.phase = MirPhase::Analysis(AnalysisPhase::Initial);
        good(&b, &tcx);
        for phase in [RuntimePhase::Initial, RuntimePhase::PostCleanup, RuntimePhase::Optimized] {
            b.phase = MirPhase::Runtime(phase);
            bad(&b, &tcx, "analysis statement");
        }
    }
}

#[test]
fn place_mention_validated_in_all_runtime_phases() {
    let tcx = Tcx::new();
    for phase in [RuntimePhase::Initial, RuntimePhase::PostCleanup, RuntimePhase::Optimized] {
        let mut b = body(&tcx);
        b.phase = MirPhase::Runtime(phase);
        b.local_decls[1].ty = tcx.tys.usize_;
        b.local_decls[2].ty = tcx.tys.intern(TyKind::Array(tcx.tys.i32_, tcx.tys.const_usize(4)));
        let mut p = local(2);
        p.projection.push(PlaceElem::Index(Local(1)));
        b.basic_blocks[0].statements.push(stmt(StatementKind::PlaceMention(p.clone())));
        good(&b, &tcx);
        p.projection[0] = PlaceElem::Index(Local(10));
        b.basic_blocks[0].statements[0] = stmt(StatementKind::PlaceMention(p));
        bad(&b, &tcx, "local is out of range");
    }
}

#[test]
fn false_edges_phase_rules() {
    let tcx = Tcx::new();
    for kind in [
        TerminatorKind::FalseEdge { real_target: BasicBlock(1), imaginary_target: BasicBlock(1) },
        TerminatorKind::FalseUnwind { real_target: BasicBlock(1), unwind: UnwindAction::Continue },
    ] {
        let mut b = body(&tcx);
        term(&mut b, kind);
        good(&b, &tcx);
        for phase in [RuntimePhase::Initial, RuntimePhase::PostCleanup, RuntimePhase::Optimized] {
            b.phase = MirPhase::Runtime(phase);
            bad(&b, &tcx, "false edge");
        }
    }
}

#[test]
fn two_phase_and_fake_borrows() {
    let tcx = Tcx::new();
    let mut b = body(&tcx);
    for kind in [
        BorrowKind::Shared,
        BorrowKind::Mut { kind: MutBorrowKind::TwoPhaseBorrow },
        BorrowKind::Mut { kind: MutBorrowKind::ClosureCapture },
        BorrowKind::Fake(FakeBorrowKind::Deep),
        BorrowKind::Fake(FakeBorrowKind::Shallow),
    ] {
        b.local_decls[2].ty = tcx.tys.intern(TyKind::Ref(matches!(kind, BorrowKind::Mut { .. }), tcx.tys.i32_, crate::types::RegionId::ERASED));
        b.phase = MirPhase::Built;
        b.basic_blocks[0].statements = vec![stmt(StatementKind::Assign(local(2), Rvalue::Ref(Region(0), kind, local(1))))];
        good(&b, &tcx);
        b.phase = MirPhase::Runtime(RuntimePhase::Initial);
        if matches!(kind, BorrowKind::Fake(_)) {
            bad(&b, &tcx, "fake borrow");
        } else {
            good(&b, &tcx);
        }
    }
}

#[test]
fn yield_and_coroutine_drop_phase_rules() {
    let tcx = Tcx::new();
    for kind in [
        TerminatorKind::Yield { value: copy(1), resume: BasicBlock(1), resume_arg: local(2), drop: Some(BasicBlock(1)) },
        TerminatorKind::CoroutineDrop,
    ] {
        let mut b = body(&tcx);
        term(&mut b, kind);
        bad(&b, &tcx, "outside coroutine");
        b.coroutine = Some(CoroutineInfo { yield_ty: tcx.tys.i32_, resume_ty: tcx.tys.i32_, kind: CoroutineKind::Coroutine(Movability::Movable) });
        good(&b, &tcx);
        for phase in [RuntimePhase::Initial, RuntimePhase::PostCleanup, RuntimePhase::Optimized] {
            b.phase = MirPhase::Runtime(phase);
            bad(&b, &tcx, "after coroutine lowering");
        }
    }
}

#[test]
fn primitive_assignment_types() {
    let tcx = Tcx::new();
    let mut b = body(&tcx);
    for ty in [tcx.tys.i32_, tcx.tys.bool_, tcx.tys.char_, tcx.tys.f64_] {
        b.local_decls[2].ty = ty;
        b.local_decls[1].ty = ty;
        b.basic_blocks[0].statements = vec![stmt(StatementKind::Assign(local(2), Rvalue::Use(copy(1))))];
        good(&b, &tcx);
        b.local_decls[2].ty = if ty == tcx.tys.i32_ { tcx.tys.bool_ } else { tcx.tys.i32_ };
        bad(&b, &tcx, "incompatible primitive");
    }
    b.local_decls[1].ty = tcx.tys.i32_;
    b.local_decls[2].ty = tcx.tys.bool_;
    b.basic_blocks[0].statements = vec![stmt(StatementKind::Assign(local(2), Rvalue::BinaryOp(BinOp::Eq, Box::new((copy(1), copy(1))))))];
    good(&b, &tcx);
    b.local_decls[2].ty = tcx.tys.i32_;
    bad(&b, &tcx, "incompatible primitive");
    b.basic_blocks[0].statements = vec![stmt(StatementKind::Assign(local(2), Rvalue::Cast(CastKind::IntToInt, copy(1), tcx.tys.i32_)))];
    good(&b, &tcx);
}

#[test]
fn constants_and_allocation_provenance() {
    let sess = &std::sync::Arc::new(crate::Session::default());
    let tcx = Tcx::new();
    let mut allocations = AllocMap::default();
    let id =
        allocations.insert(Allocation { init_mask: vec![true; 16], deallocated: false, provenance_fragments: Vec::new(), bytes: vec![0; 16], align: 8, provenance: vec![(8, CtfeProvenance::new(AllocId(0)))], mutability: Mutability::Not });
    assert_eq!(allocations.get(id).unwrap().provenance, vec![(8, CtfeProvenance::new(id))]);
    assert!(allocations.get(AllocId(1)).is_none());
    let ptr = tcx.tys.intern(TyKind::Ptr(false, tcx.tys.u8_));
    let slice = tcx.tys.intern(TyKind::Slice(tcx.tys.u8_));
    let slice_ref = tcx.tys.intern(TyKind::Ref(false, slice, crate::types::RegionId::ERASED));
    let wide = tcx.tys.prim(crate::program::Prim::U128);
    let values = [
        (ConstValue::Scalar(Scalar::Int { bits: u128::MAX, size: 16 }), wide),
        (ConstValue::Scalar(Scalar::Ptr { provenance: CtfeProvenance::new(id), offset: 8 }), ptr),
        (ConstValue::ZeroSized, tcx.tys.unit),
        (ConstValue::Slice { alloc: id, meta: 16 }, slice_ref),
        (ConstValue::Indirect { alloc: id, offset: 4 }, tcx.tys.i32_),
    ];
    let mut b = body(&tcx);
    b.allocs = allocations;
    for (value, ty) in values {
        let target = Local(b.local_decls.len() as u32);
        let mut decl = b.local_decls[2].clone();
        decl.ty = ty;
        b.local_decls.push(decl);
        b.basic_blocks[0].statements.push(stmt(StatementKind::Assign(Place::local(target), Rvalue::Use(val(ty, value)))));
    }
    good(&b, &tcx);
    let dump = pretty::print_body(&Program::new(sess), &tcx, "constants", &b);
    for text in ["0xffffffffffffffffffffffffffffffff:16B", "alloc0+8", "zst", "slice(alloc0, 16)", "indirect(alloc0+4)"] {
        assert!(dump.contains(text), "{dump}");
    }
}

#[test]
fn promoted_body_has_independent_indices() {
    let sess = &std::sync::Arc::new(crate::Session::default());
    let tcx = Tcx::new();
    let mut b = body(&tcx);
    let mut promoted = body(&tcx);
    promoted.source.kind = BodyKind::Promoted(DefId(0), PromotedId(0));
    promoted.arg_count = 0;
    promoted.local_decls.truncate(1);
    b.promoted.push(promoted);
    b.basic_blocks[0].statements.push(stmt(StatementKind::Assign(
        local(2),
        Rvalue::Use(Operand::Constant(Box::new(ConstOperand {
            span: Span::default(),
            user_ty: Some(UserTypeAnnotationIndex(0)),
            const_: Const::Unevaluated { def: DefId(0), args: vec![crate::consts::GenericArg::Type(tcx.tys.i32_)], promoted: Some(PromotedId(0)), ty: tcx.tys.i32_ },
        }))),
    )));
    good(&b, &tcx);
    let dump = pretty::print_body(&Program::new(sess), &tcx, "promotions", &b);
    assert!(dump.contains("const def#0::<i32>::promoted[0]"));
    assert!(dump.contains("fn promotions::promoted[0]() -> i32"));
    b.promoted[0].basic_blocks[0].statements.push(stmt(StatementKind::PlaceMention(local(1))));
    let errors = validate::validate(&b, &tcx);
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].promoted, vec![PromotedId(0)]);
    assert_eq!(errors[0].bb, Some(BasicBlock(0)));
    assert_eq!(errors[0].statement_index, Some(0));
}

#[test]
fn body_metadata_rules() {
    let tcx = Tcx::new();
    let base = body(&tcx);
    good(&base, &tcx);
    let mut b = base.clone();
    b.basic_blocks.clear();
    bad(&b, &tcx, "no entry");
    let mut b = base.clone();
    b.basic_blocks[0].is_cleanup = true;
    bad(&b, &tcx, "entry block");
    let mut b = base.clone();
    b.local_decls.clear();
    bad(&b, &tcx, "no return local");
    let mut b = base.clone();
    b.arg_count = 4;
    bad(&b, &tcx, "argument count");
    let mut b = base.clone();
    b.local_decls[1].kind = LocalKind::Temp;
    bad(&b, &tcx, "local kind");
    let mut b = base.clone();
    b.spread_arg = Some(Local(1));
    good(&b, &tcx);
    b.spread_arg = Some(Local(0));
    bad(&b, &tcx, "spread argument");
    let mut b = base.clone();
    b.source_scopes.clear();
    bad(&b, &tcx, "no source scope");
    let mut b = base.clone();
    b.source_scopes.push(SourceScopeData { span: Span::default(), parent_scope: Some(SourceScope(0)) });
    good(&b, &tcx);
    b.source_scopes[0].parent_scope = Some(SourceScope(1));
    bad(&b, &tcx, "scope cycle");
    b.source_scopes[0].parent_scope = Some(SourceScope(8));
    bad(&b, &tcx, "parent is out of range");
    let mut b = base.clone();
    b.local_decls[0].source_info.scope = SourceScope(8);
    bad(&b, &tcx, "scope is out of range");
    let mut b = base.clone();
    b.local_decls[0].ty = TyId(u32::MAX);
    bad(&b, &tcx, "type is out of range");
    let mut b = base.clone();
    b.local_decls[0].user_ty.push(UserTypeProjection { base: UserTypeAnnotationIndex(0), projection: vec![] });
    good(&b, &tcx);
    b.user_type_annotations.clear();
    bad(&b, &tcx, "user type annotation");
}

#[test]
fn call_spans_and_unsupported_asm() {
    let tcx = Tcx::new();
    let mut b = cleanup_body(&tcx);
    good(&b, &tcx);
    if let TerminatorKind::Call { arg_spans, .. } = &mut b.basic_blocks[0].terminator.as_mut().unwrap().kind {
        arg_spans.push(Span::default());
    }
    bad(&b, &tcx, "argument span count");
    term(&mut b, TerminatorKind::InlineAsm(InlineAsmUnsupported));
    bad(&b, &tcx, "assembly is unsupported");
}

#[test]
fn printer_golden() {
    let sess = &std::sync::Arc::new(crate::Session::default());
    let tcx = Tcx::new();
    let mut b = body(&tcx);
    b.local_decls.truncate(3);
    b.basic_blocks.truncate(1);
    b.user_type_annotations.clear();
    b.basic_blocks[0].statements = vec![
        stmt(StatementKind::StorageLive(Local(2))),
        stmt(StatementKind::Assign(local(2), Rvalue::BinaryOp(BinOp::Add, Box::new((copy(1), integer(tcx.tys.i32_)))))),
        stmt(StatementKind::Assign(local(0), Rvalue::Use(Operand::Move(local(2))))),
    ];
    let expected = "fn add_one(_1: i32) -> i32 {\n    let mut _0: i32;\n    let mut _2: i32;\n\n    bb0: {\n        StorageLive(_2);\n        _2 = Add(copy _1, const 0x1:4B: i32);\n        _0 = move _2;\n        return;\n    }\n}\n";
    let prog = Program::new(sess);
    assert_eq!(pretty::print_body(&prog, &tcx, "add_one", &b), expected);
    assert_eq!(pretty::print_body(&prog, &tcx, "add_one", &b), expected);
}

#[test]
fn visitors_preserve_place_contexts_and_rewrite_nested_data() {
    struct ReadVisitor {
        locals: Vec<(Local, PlaceContext)>,
        tys: usize,
        regions: usize,
        allocs: usize,
    }
    impl Visitor for ReadVisitor {
        fn visit_local(&mut self, l: &Local, c: PlaceContext, _: Location) {
            self.locals.push((*l, c));
        }
        fn visit_ty(&mut self, _: &TyId) {
            self.tys += 1;
        }
        fn visit_region(&mut self, _: &Region) {
            self.regions += 1;
        }
        fn visit_alloc_id(&mut self, _: &AllocId) {
            self.allocs += 1;
        }
    }
    struct Rewrite;
    impl MutVisitor for Rewrite {
        fn visit_local(&mut self, l: &mut Local, _: PlaceContext, _: Location) {
            if l.0 != 0 {
                l.0 += 10;
            }
        }
        fn visit_region(&mut self, r: &mut Region) {
            r.0 += 1;
        }
        fn visit_alloc_id(&mut self, a: &mut AllocId) {
            a.0 += 1;
        }
        fn visit_target(&mut self, target: &mut BasicBlock, _: bool, _: Location) {
            target.0 += 5;
        }
    }
    let tcx = Tcx::new();
    let mut b = cleanup_body(&tcx);
    let place = Place { local: Local(1), projection: vec![PlaceElem::Index(Local(2)), PlaceElem::Field(FieldIdx(0), tcx.tys.i32_)] };
    b.basic_blocks[0].statements.push(stmt(StatementKind::Assign(
        local(2),
        Rvalue::Ref(Region(0), BorrowKind::Mut { kind: MutBorrowKind::TwoPhaseBorrow }, place.clone()),
    )));
    b.basic_blocks[0]
        .statements
        .push(stmt(StatementKind::Assign(local(2), Rvalue::Use(val(tcx.tys.i32_, ConstValue::Slice { alloc: AllocId(0), meta: 4 })))));
    b.var_debug_info.push(VarDebugInfo { name: "x".into(), source_info: SourceInfo::default(), place });
    let mut reader = ReadVisitor { locals: vec![], tys: 0, regions: 0, allocs: 0 };
    reader.visit_body(&b);
    assert!(reader.locals.contains(&(Local(2), PlaceContext::NonMutatingUse(R::Copy))));
    assert!(reader.locals.contains(&(Local(1), PlaceContext::MutatingUse(visit::MutatingUseContext::Projection))));
    assert!(reader.locals.contains(&(Local(2), PlaceContext::NonUse(visit::NonUseContext::VarDebugInfo))));
    assert!(reader.locals.contains(&(Local(0), PlaceContext::NonMutatingUse(R::Move))));
    assert_eq!((reader.regions, reader.allocs), (1, 1));
    assert!(reader.tys >= 7);
    Rewrite.visit_body(&mut b);
    let mut reader = ReadVisitor { locals: vec![], tys: 0, regions: 0, allocs: 0 };
    reader.visit_body(&b);
    assert!(reader.locals.iter().all(|(local, _)| local.0 == 0 || local.0 >= 10));
    match &b.basic_blocks[0].statements[0].kind {
        StatementKind::Assign(_, Rvalue::Ref(region, _, _)) => assert_eq!(*region, Region(1)),
        _ => panic!(),
    }
    match &b.basic_blocks[0].terminator.as_ref().unwrap().kind {
        TerminatorKind::Call { target, unwind, .. } => {
            assert_eq!(*target, Some(BasicBlock(6)));
            assert_eq!(*unwind, UnwindAction::Cleanup(BasicBlock(7)));
        }
        _ => panic!(),
    }
}

#[test]
fn visitor_finds_assert_payload_and_repeat_constant() {
    let tcx = Tcx::new();
    let mut b = body(&tcx);
    term(
        &mut b,
        TerminatorKind::Assert {
            cond: val(tcx.tys.bool_, ConstValue::Scalar(Scalar::Int { bits: 1, size: 1 })),
            expected: true,
            msg: AssertKind::BoundsCheck { len: copy(1), index: copy(8) },
            target: BasicBlock(1),
            unwind: UnwindAction::Continue,
        },
    );
    bad(&b, &tcx, "local is out of range");
    term(&mut b, TerminatorKind::Return);
    b.basic_blocks[0].statements.push(stmt(StatementKind::Assign(
        local(2),
        Rvalue::Repeat(copy(1), Const::Unevaluated { def: DefId(1), args: vec![crate::consts::GenericArg::Type(TyId(u32::MAX))], promoted: None, ty: tcx.tys.usize_ }),
    )));
    bad(&b, &tcx, "type is out of range");
}

#[test]
fn entry_block_has_no_predecessors() {
    let tcx = Tcx::new();
    let mut b = body(&tcx);
    term(&mut b, TerminatorKind::Goto { target: BasicBlock(1) });
    good(&b, &tcx);
    term(&mut b, TerminatorKind::Goto { target: BasicBlock(0) });
    bad(&b, &tcx, "must not have predecessors");
}

#[test]
#[should_panic(expected = "MutVisitor changed return's implicit local")]
fn mutable_visitor_cannot_rename_return() {
    struct RenameReturn;
    impl MutVisitor for RenameReturn {
        fn visit_local(&mut self, local: &mut Local, _: PlaceContext, _: Location) {
            local.0 += 1;
        }
    }
    let tcx = Tcx::new();
    RenameReturn.visit_body(&mut body(&tcx));
}

#[test]
fn global_allocs_name_statics_and_functions() {
    let tcx = Tcx::new();
    let mut allocs = AllocMap::default();
    let a = allocs.static_alloc(DefId(7));
    assert_eq!(allocs.static_alloc(DefId(7)), a);
    let f = allocs.insert_function(DefId(8), vec![]);
    assert_ne!(a, f);
    assert!(allocs.get(a).is_none() && allocs.get(f).is_none());
    let ptr = |alloc, offset| Const::Val(ConstValue::Scalar(Scalar::Ptr { provenance: CtfeProvenance::new(alloc), offset }), tcx.tys.u8_);
    assert_eq!(ptr(a, 0).check_static_ptr(&allocs), Some(DefId(7)));
    assert_eq!(ptr(a, 1).check_static_ptr(&allocs), None);
    assert_eq!(ptr(f, 0).check_static_ptr(&allocs), None);
    assert_eq!(Const::Ty(tcx.tys.u8_, crate::consts::Const(0)).check_static_ptr(&allocs), None);
}

#[test]
fn contract_extensions_validate_and_visit() {
    let sess = &std::sync::Arc::new(crate::Session::default());
    let tcx = Tcx::new();
    let mut b = body(&tcx);
    b.local_decls[2].local_info = LocalInfo::user_var(BindingMode(ByRef::No, Mutability::Not), None, Span::default());
    assert!(b.local_decls[2].can_be_made_mutable() && b.local_decls[2].is_nonref_binding() && b.local_decls[2].is_user_variable());
    b.local_decls[2].local_info = LocalInfo::user_var(BindingMode(ByRef::Yes(Mutability::Not), Mutability::Not), None, Span::default());
    assert!(!b.local_decls[2].can_be_made_mutable() && !b.local_decls[2].is_nonref_binding());
    b.local_decls[2].local_info = LocalInfo::User(BindingForm::RefForGuard(Local(1)));
    assert!(b.local_decls[2].is_ref_for_guard());
    b.local_decls[2].local_info = LocalInfo::StaticRef { def_id: DefId(1), is_thread_local: true };
    assert!(b.local_decls[2].is_ref_to_static() && b.local_decls[2].is_ref_to_thread_local());
    b.local_decls[2].local_info = LocalInfo::Boring;
    // The hint is a lint-only statement and visits its place as a non-use.
    b.basic_blocks[0].statements = vec![stmt(StatementKind::BackwardIncompatibleDropHint { place: local(2), reason: BackwardIncompatibleDropReason::Edition2024 })];
    good(&b, &tcx);
    let dump = pretty::print_body(&Program::new(sess), &tcx, "f", &b);
    assert!(dump.contains("BackwardIncompatibleDropHint(_2, Edition2024)"), "{dump}");
    struct Contexts(Vec<PlaceContext>);
    impl visit::Visitor for Contexts {
        fn visit_local(&mut self, _: &Local, context: PlaceContext, _: Location) { self.0.push(context); }
    }
    let mut contexts = Contexts(Vec::new());
    visit::Visitor::visit_body(&mut contexts, &b);
    assert!(contexts.0.contains(&PlaceContext::NonUse(visit::NonUseContext::BackwardIncompatibleDropHint)));
    // Upvars belong to closure bodies, and name their types.
    let upvar = CapturedPlace {
        name: "x".into(), var: 3, root_ty: tcx.tys.i32_, projection: Vec::new(), ty: tcx.tys.i32_,
        capture: UpvarCapture::ByRef(UpvarBorrowKind::Mutable), mutability: Mutability::Mut, path_span: Span::default(), kind_span: Span::default(),
    };
    assert!(upvar.is_by_ref());
    b.upvars.push(upvar);
    bad(&b, &tcx, "upvars outside a closure body");
    b.source.kind = BodyKind::Closure(0);
    good(&b, &tcx);
    let dump = pretty::print_body(&Program::new(sess), &tcx, "f", &b);
    assert!(dump.contains("upvar#0: x ByRef(Mutable)"), "{dump}");
    // Unevaluated constants carry their type; a type-system constant takes the valtree's own.
    let unevaluated = Const::Unevaluated { def: DefId(1), args: Vec::new(), promoted: None, ty: tcx.tys.bool_ };
    assert_eq!(unevaluated.ty(&tcx.tys), tcx.tys.bool_);
    assert_eq!(Const::Ty(tcx.tys.u8_, tcx.tys.const_usize(3)).ty(&tcx.tys), tcx.tys.usize_);
}

#[test]
fn canonical_user_type_identity() {
    use crate::types::user_type::{UserArgs, UserType, UserTypeKind};
    let tcx = Tcx::new();
    let canon = |kind| crate::next_solver::canonical::Canonical { value: UserType::new(kind), max_universe: crate::types::UniverseIndex::ROOT, var_kinds: Vec::new() };
    let bound = |i| tcx.tys.intern(TyKind::Bound(0, crate::types::BoundTy { var: crate::types::BoundVar(i), kind: crate::types::BoundTyKind::Anon }));
    let identity = canon(UserTypeKind::TypeOf(DefId(1), UserArgs { args: vec![crate::consts::GenericArg::Type(bound(0)), crate::consts::GenericArg::Type(bound(1))], user_self_ty: None }));
    assert!(identity.is_identity(&tcx.tys));
    let swapped = canon(UserTypeKind::TypeOf(DefId(1), UserArgs { args: vec![crate::consts::GenericArg::Type(bound(1)), crate::consts::GenericArg::Type(bound(0))], user_self_ty: None }));
    assert!(!swapped.is_identity(&tcx.tys));
    assert!(!canon(UserTypeKind::Ty(tcx.tys.i32_)).is_identity(&tcx.tys));
}
