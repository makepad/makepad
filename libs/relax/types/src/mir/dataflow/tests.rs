use super::*;

fn place(i: u32) -> Place { Place::local(Local(i)) }
fn copy(i: u32) -> Operand { Operand::Copy(place(i)) }
fn constant() -> Operand {
    Operand::Constant(Box::new(ConstOperand {
        span: Span::default(), user_ty: None, const_: Const::Val(ConstValue::ZeroSized, TyId(0)),
    }))
}
fn stmt(kind: StatementKind) -> Statement { Statement { source_info: SourceInfo::default(), kind } }
fn block(statements: Vec<StatementKind>, kind: TerminatorKind) -> BasicBlockData {
    let mut stmts = Vec::new();
    for kind in statements { stmts.push(stmt(kind)); }
    BasicBlockData { statements: stmts, terminator: Some(Terminator { source_info: SourceInfo::default(), kind }), is_cleanup: false }
}
fn body(locals: u32, blocks: Vec<BasicBlockData>) -> Body {
    let mut decls = Vec::new();
    for i in 0..locals {
        decls.push(LocalDecl {
            ty: TyId(0), mutability: Mutability::Mut, user_ty: Vec::new(), source_info: SourceInfo::default(),
            kind: if i == 0 { LocalKind::Return } else { LocalKind::Temp }, local_info: LocalInfo::Boring,
        });
    }
    Body {
        phase: MirPhase::Built, source: MirSource { owner: DefId(0), kind: BodyKind::Fn },
        basic_blocks: blocks, local_decls: decls,
        source_scopes: vec![SourceScopeData { span: Span::default(), parent_scope: None }],
        user_type_annotations: Vec::new(), var_debug_info: Vec::new(), arg_count: 0,
        spread_arg: None, promoted: Vec::new(), coroutine: None,
    }
}
fn goto(target: u32) -> TerminatorKind { TerminatorKind::Goto { target: BasicBlock(target) } }
fn switch(target: u32, otherwise: u32) -> TerminatorKind {
    TerminatorKind::SwitchInt {
        discr: constant(), targets: SwitchTargets { values: vec![0], targets: vec![BasicBlock(target)], otherwise: BasicBlock(otherwise) },
    }
}
fn call(destination: Place, target: Option<u32>, unwind: UnwindAction) -> TerminatorKind {
    TerminatorKind::Call {
        func: constant(), args: vec![Operand::Move(place(1))], destination, target: target.map(BasicBlock), unwind,
        call_source: CallSource::Normal, fn_span: Span::default(), arg_spans: vec![Span::default()],
    }
}
fn at(block: u32, statement_index: usize) -> Location { Location { block: BasicBlock(block), statement_index } }
fn set(bits: &BitSet, expected: &[usize]) { assert_eq!(bits.iter().collect::<Vec<_>>(), expected); }
fn entries(results: &Results<'_>, expected: &[&[usize]]) {
    for (i, bits) in expected.iter().enumerate() { set(results.entry_state(BasicBlock(i as u32)), bits); }
}

#[test]
fn bitsets_word_boundaries_and_lattice_operations() {
    for size in [0, 1, 63, 64, 65, 127, 128, 129, 2049] {
        let filled = BitSet::new_filled(size);
        assert_eq!(filled.count(), size);
        assert_eq!(filled.iter().collect::<Vec<_>>(), (0..size).collect::<Vec<_>>());
        let mut a = BitSet::new_empty(size);
        assert!(a.is_empty());
        for i in (0..size).step_by(2) { assert!(a.insert(i)); assert!(!a.insert(i)); }
        let mut b = filled.clone();
        assert_eq!(b.subtract(&a), size != 0);
        assert!(!b.subtract(&a));
        for i in 0..size { assert_eq!(b.contains(i), i % 2 == 1); }
        let mut union = a.clone();
        assert_eq!(union.union(&b), size > 1);
        assert_eq!(union, filled);
        assert!(!union.union(&b));
        assert_eq!(union.intersect(&a), size > 1);
        assert_eq!(union, a);
        assert!(!union.intersect(&a));
        for i in (0..size).step_by(2) { assert!(a.remove(i)); assert!(!a.remove(i)); }
        assert!(a.is_empty());
        b.insert_all();
        assert_eq!(b, filled);
        b.clear();
        assert!(b.is_empty());
    }
}

#[test]
fn matrix_rows_are_independent_and_union_reports_changes() {
    let mut matrix = BitMatrix::new(3, 129);
    for bit in [0, 63, 64, 128] { assert!(matrix.insert(0, bit)); assert!(!matrix.insert(0, bit)); }
    matrix.insert(1, 65);
    assert!(matrix.union_rows(0, 1));
    assert!(!matrix.union_rows(0, 1));
    assert!(!matrix.union_rows(1, 1));
    assert_eq!(matrix.iter(1).collect::<Vec<_>>(), [0, 63, 64, 65, 128]);
    assert!(!matrix.contains(0, 65));
    assert!(matrix.iter(2).next().is_none());
    let mut extra = BitSet::new_empty(129);
    extra.insert(127);
    assert!(matrix.union_row_with(&extra, 2));
    assert!(!matrix.union_row_with(&extra, 2));
    assert!(matrix.contains(2, 127));
    let mut empty = BitMatrix::new(2, 0);
    assert!(!empty.union_rows(0, 1));
    assert!(empty.iter(1).next().is_none());
}

#[test]
#[should_panic]
fn bitset_checks_partial_word_bounds() { BitSet::new_empty(65).insert(65); }

#[test]
#[should_panic]
fn bitset_checks_domain_compatibility() { BitSet::new_empty(63).union(&BitSet::new_empty(64)); }

#[test]
#[should_panic]
fn matrix_checks_row_bounds() { BitMatrix::new(1, 65).insert(1, 0); }

struct OrderedEffects(Direction);
impl Analysis for OrderedEffects {
    fn direction(&self) -> Direction { self.0 }
    fn bottom_value(&self, _: &Body) -> BitSet { BitSet::new_empty(8) }
    fn apply_early_statement_effect(&self, state: &mut BitSet, _: &Statement, at: Location) { state.insert(at.statement_index * 2); }
    fn apply_primary_statement_effect(&self, state: &mut BitSet, _: &Statement, at: Location) { state.insert(at.statement_index * 2 + 1); }
    fn apply_early_terminator_effect(&self, state: &mut BitSet, _: &Terminator, at: Location) { state.insert(at.statement_index * 2); }
    fn apply_primary_terminator_effect<'a>(&self, state: &mut BitSet, term: &'a Terminator, at: Location) -> TerminatorEdges<'a> {
        state.insert(at.statement_index * 2 + 1);
        TerminatorEdges::for_terminator(term)
    }
}

#[test]
fn forward_cursor_early_primary_rewind_and_block_switch() {
    let b = body(1, vec![block(vec![StatementKind::Nop, StatementKind::Nop], goto(1)), block(vec![], TerminatorKind::Return)]);
    let analysis = OrderedEffects(Direction::Forward);
    let r = iterate_to_fixpoint(&b, &analysis);
    entries(&r, &[&[], &[0, 1, 2, 3, 4, 5]]);
    let mut c = r.cursor();
    c.seek_before_primary_effect(at(0, 1)); set(c.get(), &[0, 1, 2]);
    c.seek_after_primary_effect(at(0, 1)); set(c.get(), &[0, 1, 2, 3]);
    c.seek_after_primary_effect(at(0, 1)); set(c.get(), &[0, 1, 2, 3]);
    c.seek_before_primary_effect(at(0, 0)); set(c.get(), &[0]);
    c.seek_before_primary_effect(at(0, 2)); set(c.get(), &[0, 1, 2, 3, 4]);
    c.seek_to_block_end(BasicBlock(0)); set(c.get(), &[0, 1, 2, 3, 4, 5]);
    c.seek_to_block_start(BasicBlock(1)); set(c.get(), &[0, 1, 2, 3, 4, 5]);
    c.seek_to_block_start(BasicBlock(0)); set(c.get(), &[]);
}

#[test]
fn backward_cursor_early_primary_rewind_and_empty_block() {
    let b = body(1, vec![block(vec![], goto(1)), block(vec![StatementKind::Nop, StatementKind::Nop], TerminatorKind::Return)]);
    let analysis = OrderedEffects(Direction::Backward);
    let r = iterate_to_fixpoint(&b, &analysis);
    entries(&r, &[&[0, 1, 2, 3, 4, 5], &[]]);
    let mut c = r.cursor();
    c.seek_to_block_end(BasicBlock(1)); set(c.get(), &[]);
    c.seek_before_primary_effect(at(1, 2)); set(c.get(), &[4]);
    c.seek_after_primary_effect(at(1, 2)); set(c.get(), &[4, 5]);
    c.seek_before_primary_effect(at(1, 1)); set(c.get(), &[2, 4, 5]);
    c.seek_after_primary_effect(at(1, 1)); set(c.get(), &[2, 3, 4, 5]);
    c.seek_before_primary_effect(at(1, 2)); set(c.get(), &[4]);
    c.seek_to_block_start(BasicBlock(1)); set(c.get(), &[0, 1, 2, 3, 4, 5]);
    c.seek_to_block_start(BasicBlock(0)); set(c.get(), &[0, 1, 2, 3, 4, 5]);
}

struct Recorder(Vec<(char, usize, Vec<usize>)>);
impl ResultsVisitor for Recorder {
    fn visit_block_start(&mut self, _: BasicBlock, state: &BitSet) { self.0.push(('S', 0, state.iter().collect())); }
    fn visit_block_end(&mut self, _: BasicBlock, state: &BitSet) { self.0.push(('E', 0, state.iter().collect())); }
    fn visit_after_early_statement_effect(&mut self, _: &dyn Analysis, state: &BitSet, _: &Statement, at: Location) { self.0.push(('e', at.statement_index, state.iter().collect())); }
    fn visit_after_primary_statement_effect(&mut self, _: &dyn Analysis, state: &BitSet, _: &Statement, at: Location) { self.0.push(('p', at.statement_index, state.iter().collect())); }
    fn visit_after_early_terminator_effect(&mut self, _: &dyn Analysis, state: &BitSet, _: &Terminator, at: Location) { self.0.push(('t', at.statement_index, state.iter().collect())); }
    fn visit_after_primary_terminator_effect(&mut self, _: &dyn Analysis, state: &BitSet, _: &Terminator, at: Location) { self.0.push(('T', at.statement_index, state.iter().collect())); }
}

#[test]
fn visitors_observe_every_effect_in_both_directions() {
    let b = body(1, vec![block(vec![StatementKind::Nop], TerminatorKind::Return), block(vec![], TerminatorKind::Return)]);
    for direction in [Direction::Forward, Direction::Backward] {
        let analysis = OrderedEffects(direction);
        let r = iterate_to_fixpoint(&b, &analysis);
        let mut record = Recorder(Vec::new());
        visit_reachable_results(&r, &mut record);
        let expected = match direction {
            Direction::Forward => vec![('S', 0, vec![]), ('e', 0, vec![0]), ('p', 0, vec![0, 1]), ('t', 1, vec![0, 1, 2]), ('T', 1, vec![0, 1, 2, 3]), ('E', 0, vec![0, 1, 2, 3])],
            Direction::Backward => vec![('E', 0, vec![]), ('t', 1, vec![2]), ('T', 1, vec![2, 3]), ('e', 0, vec![0, 2, 3]), ('p', 0, vec![0, 1, 2, 3]), ('S', 0, vec![0, 1, 2, 3])],
        };
        assert_eq!(record.0, expected);
        let mut selected = Recorder(Vec::new());
        visit_results(&r, &[BasicBlock(0)], &mut selected);
        assert_eq!(selected.0, expected);
        assert!(!r.is_reachable(BasicBlock(1)));
    }
}

#[test]
fn forward_storage_loop_reaches_both_may_states() {
    let mut b = body(4, vec![
        block(vec![StatementKind::StorageLive(Local(2))], goto(1)),
        block(vec![], switch(2, 3)),
        block(vec![StatementKind::StorageDead(Local(2))], goto(1)),
        block(vec![], TerminatorKind::Return),
    ]);
    b.arg_count = 1;
    let live = MaybeStorageLive::new(&b);
    let dead = MaybeStorageDead::new(&b);
    let live_r = iterate_to_fixpoint(&b, &live);
    let dead_r = iterate_to_fixpoint(&b, &dead);
    entries(&live_r, &[&[0, 1, 3], &[0, 1, 2, 3], &[0, 1, 2, 3], &[0, 1, 2, 3]]);
    entries(&dead_r, &[&[2], &[2], &[2], &[2]]);
    let mut c = dead_r.cursor();
    c.seek_after_primary_effect(at(0, 0)); set(c.get(), &[]);
    c.seek_after_primary_effect(at(2, 0)); set(c.get(), &[2]);
}

#[test]
fn storage_arguments_and_unreachable_annotations() {
    let mut b = body(4, vec![
        block(vec![StatementKind::StorageDead(Local(1))], TerminatorKind::Return),
        block(vec![StatementKind::StorageLive(Local(2))], TerminatorKind::Unreachable),
    ]);
    b.arg_count = 1;
    set(&always_storage_live_locals(&b), &[0, 3]);
    let live = MaybeStorageLive::new(&b);
    let dead = MaybeStorageDead::new(&b);
    let l = iterate_to_fixpoint(&b, &live);
    let d = iterate_to_fixpoint(&b, &dead);
    set(l.entry_state(BasicBlock(0)), &[0, 1, 3]);
    set(d.entry_state(BasicBlock(0)), &[2]);
    let mut c = l.cursor(); c.seek_to_block_end(BasicBlock(0)); set(c.get(), &[0, 3]);
    let mut c = d.cursor(); c.seek_to_block_end(BasicBlock(0)); set(c.get(), &[1, 2]);
    assert!(!l.is_reachable(BasicBlock(1)));
}

struct ToySwitch(Direction);
impl Analysis for ToySwitch {
    fn direction(&self) -> Direction { self.0 }
    fn bottom_value(&self, _: &Body) -> BitSet { BitSet::new_empty(4) }
    fn has_switch_int_edge_effect(&self, _: BasicBlock, _: &SwitchTargets, _: &Operand) -> bool { true }
    fn apply_switch_int_edge_effect(&self, state: &mut BitSet, _: BasicBlock, _: &SwitchTargets, _: &Operand, index: SwitchTargetIndex) {
        state.insert(match index { SwitchTargetIndex::Normal(i) => i, SwitchTargetIndex::Otherwise => 3 });
    }
}

#[test]
fn switch_edge_effects_keep_duplicate_targets_and_otherwise_separate() {
    let b = body(1, vec![
        block(vec![], TerminatorKind::SwitchInt { discr: constant(), targets: SwitchTargets {
            values: vec![10, 20, 30], targets: vec![BasicBlock(1), BasicBlock(1), BasicBlock(2)], otherwise: BasicBlock(3),
        }}),
        block(vec![], goto(4)), block(vec![], goto(4)), block(vec![], goto(4)), block(vec![], TerminatorKind::Return),
    ]);
    let analysis = ToySwitch(Direction::Forward);
    let r = iterate_to_fixpoint(&b, &analysis);
    entries(&r, &[&[], &[0, 1], &[2], &[3], &[0, 1, 2, 3]]);
    let mut c = r.cursor(); c.seek_to_block_end(BasicBlock(0)); set(c.get(), &[]);
}

#[test]
#[should_panic(expected = "backward switch edge effects are unsupported")]
fn backward_switch_effect_rejected_like_rustc() {
    let b = body(1, vec![block(vec![], switch(1, 1)), block(vec![], TerminatorKind::Return)]);
    iterate_to_fixpoint(&b, &ToySwitch(Direction::Backward));
}

struct ToyCalls;
impl Analysis for ToyCalls {
    fn bottom_value(&self, body: &Body) -> BitSet { BitSet::new_empty(body.local_decls.len()) }
    fn initialize_start_block(&self, _: &Body, state: &mut BitSet) { state.insert(1); }
    fn apply_primary_terminator_effect<'a>(&self, state: &mut BitSet, term: &'a Terminator, _: Location) -> TerminatorEdges<'a> {
        if let TerminatorKind::Call { args, .. } = &term.kind {
            for arg in args {
                if let Operand::Move(place) = arg { state.remove(place.local.0 as usize); }
            }
        }
        TerminatorEdges::for_terminator(term)
    }
    fn apply_call_return_effect(&self, state: &mut BitSet, _: BasicBlock, places: CallReturnPlaces<'_>) {
        let place = match places { CallReturnPlaces::Call(p) | CallReturnPlaces::Yield(p) => p };
        state.insert(place.local.0 as usize);
    }
}

#[test]
fn call_return_assignment_does_not_reach_cleanup_or_cursor_end() {
    let mut b = body(3, vec![
        block(vec![], call(place(2), Some(1), UnwindAction::Cleanup(BasicBlock(2)))),
        block(vec![], TerminatorKind::Return), block(vec![], TerminatorKind::UnwindResume),
    ]);
    b.basic_blocks[2].is_cleanup = true;
    let r = iterate_to_fixpoint(&b, &ToyCalls);
    entries(&r, &[&[1], &[2], &[]]);
    let mut c = r.cursor(); c.seek_to_block_end(BasicBlock(0)); set(c.get(), &[]);
    let mut v = Recorder(Vec::new()); visit_results(&r, &[BasicBlock(0)], &mut v);
    assert_eq!(v.0.last().unwrap(), &('E', 0, vec![]));
    b.basic_blocks[0].terminator.as_mut().unwrap().kind = call(place(2), None, UnwindAction::Cleanup(BasicBlock(2)));
    let r = iterate_to_fixpoint(&b, &ToyCalls);
    assert!(!r.is_reachable(BasicBlock(1)));
    set(r.entry_state(BasicBlock(2)), &[]);
}

#[test]
fn backward_liveness_keeps_call_destination_on_cleanup_only() {
    let b = body(3, vec![
        block(vec![], call(place(2), Some(1), UnwindAction::Cleanup(BasicBlock(2)))),
        block(vec![StatementKind::PlaceMention(place(2))], TerminatorKind::Return),
        block(vec![StatementKind::PlaceMention(place(2))], TerminatorKind::UnwindResume),
    ]);
    let r = iterate_to_fixpoint(&b, &MaybeLiveLocals);
    entries(&r, &[&[0, 2], &[], &[]]);
    let mut c = r.cursor(); c.seek_to_block_start(BasicBlock(0)); set(c.get(), &[0, 1, 2]);
    let mut b = b;
    b.basic_blocks[0].terminator.as_mut().unwrap().kind = call(place(2), Some(1), UnwindAction::Continue);
    let r = iterate_to_fixpoint(&b, &MaybeLiveLocals);
    set(r.entry_state(BasicBlock(0)), &[0]);
    let mut c = r.cursor(); c.seek_to_block_start(BasicBlock(0)); set(c.get(), &[0, 1]);
}

#[test]
fn backward_liveness_propagates_exit_use_around_loop() {
    let b = body(3, vec![
        block(vec![StatementKind::Assign(place(1), Rvalue::Use(copy(2)))], goto(1)),
        block(vec![], switch(1, 2)),
        block(vec![StatementKind::Assign(place(0), Rvalue::Use(copy(1)))], TerminatorKind::Return),
    ]);
    let r = iterate_to_fixpoint(&b, &MaybeLiveLocals);
    entries(&r, &[&[1], &[1], &[]]);
    let mut c = r.cursor();
    c.seek_to_block_start(BasicBlock(0)); set(c.get(), &[2]);
    c.seek_to_block_start(BasicBlock(1)); set(c.get(), &[1]);
    c.seek_to_block_start(BasicBlock(2)); set(c.get(), &[1]);
}

#[test]
fn liveness_projected_writes_indices_self_assign_and_nonuses() {
    let indexed = Place { local: Local(1), projection: vec![PlaceElem::Index(Local(2))] };
    let indirect = Place { local: Local(3), projection: vec![PlaceElem::Deref, PlaceElem::Field(FieldIdx(0), TyId(0))] };
    let b = body(6, vec![block(vec![
        StatementKind::Assign(place(4), Rvalue::Use(copy(4))),
        StatementKind::Assign(indexed, Rvalue::Use(constant())),
        StatementKind::Assign(indirect, Rvalue::Use(constant())),
        StatementKind::SetDiscriminant { place: place(5), variant_index: VariantIdx(0) },
        StatementKind::StorageDead(Local(2)),
        StatementKind::AscribeUserType(place(5), UserTypeProjection { base: UserTypeAnnotationIndex(0), projection: vec![PlaceElem::Index(Local(5))] }, Variance::Invariant),
    ], TerminatorKind::Unreachable)]);
    let r = iterate_to_fixpoint(&b, &MaybeLiveLocals);
    let mut c = r.cursor(); c.seek_to_block_start(BasicBlock(0)); set(c.get(), &[2, 3, 4]);
}

#[test]
fn borrowed_raw_pointers_fake_borrows_storage_dead_and_drop() {
    let indirect = Place { local: Local(2), projection: vec![PlaceElem::Deref] };
    let field = Place { local: Local(5), projection: vec![PlaceElem::Field(FieldIdx(0), TyId(0))] };
    let b = body(6, vec![
        block(vec![
            StatementKind::Assign(place(2), Rvalue::RawPtr(RawPtrKind::Mut, place(1))),
            StatementKind::Assign(place(3), Rvalue::Ref(Region::ERASED, BorrowKind::Shared, indirect)),
            StatementKind::Assign(place(3), Rvalue::Ref(Region::ERASED, BorrowKind::Fake(FakeBorrowKind::Deep), place(4))),
            StatementKind::Assign(place(3), Rvalue::RawPtr(RawPtrKind::FakeForPtrMetadata, place(4))),
            StatementKind::Assign(place(3), Rvalue::Ref(Region::ERASED, BorrowKind::Mut { kind: MutBorrowKind::TwoPhaseBorrow }, field)),
            StatementKind::StorageDead(Local(1)),
        ], TerminatorKind::Drop { place: place(3), target: BasicBlock(1), unwind: UnwindAction::Cleanup(BasicBlock(2)), replace: false }),
        block(vec![], TerminatorKind::Return), block(vec![], TerminatorKind::UnwindResume),
    ]);
    let r = iterate_to_fixpoint(&b, &MaybeBorrowedLocals);
    entries(&r, &[&[], &[3, 4, 5], &[3, 4, 5]]);
    let mut c = r.cursor();
    c.seek_after_primary_effect(at(0, 0)); set(c.get(), &[1]);
    c.seek_after_primary_effect(at(0, 2)); set(c.get(), &[1]);
    c.seek_after_primary_effect(at(0, 4)); set(c.get(), &[1, 4, 5]);
    c.seek_before_primary_effect(at(0, 6)); set(c.get(), &[4, 5]);
}

#[test]
fn borrowed_join_and_indirect_drop() {
    let b = body(4, vec![
        block(vec![], switch(1, 2)),
        block(vec![StatementKind::Assign(place(2), Rvalue::RawPtr(RawPtrKind::Const, place(1)))], goto(3)),
        block(vec![], goto(3)),
        block(vec![], TerminatorKind::Drop {
            place: Place { local: Local(3), projection: vec![PlaceElem::Deref] },
            target: BasicBlock(4), unwind: UnwindAction::Continue, replace: false,
        }),
        block(vec![], TerminatorKind::Return),
    ]);
    let r = iterate_to_fixpoint(&b, &MaybeBorrowedLocals);
    entries(&r, &[&[], &[], &[], &[1], &[1]]);
}

#[test]
fn yield_resume_assignment_and_projection_are_edge_local() {
    let mut b = body(5, vec![
        block(vec![], TerminatorKind::Yield { value: copy(3), resume: BasicBlock(1), resume_arg: place(4), drop: Some(BasicBlock(2)) }),
        block(vec![], TerminatorKind::Unreachable), block(vec![], TerminatorKind::CoroutineDrop),
    ]);
    let r = iterate_to_fixpoint(&b, &ToyCalls);
    entries(&r, &[&[1], &[1, 4], &[1]]);
    let mut c = r.cursor(); c.seek_to_block_end(BasicBlock(0)); set(c.get(), &[1]);
    b.basic_blocks[0].terminator.as_mut().unwrap().kind = TerminatorKind::Yield {
        value: copy(3), resume: BasicBlock(1),
        resume_arg: Place { local: Local(1), projection: vec![PlaceElem::Deref, PlaceElem::Index(Local(2))] },
        drop: Some(BasicBlock(2)),
    };
    let r = iterate_to_fixpoint(&b, &MaybeLiveLocals);
    entries(&r, &[&[1, 2], &[], &[]]);
    let mut c = r.cursor(); c.seek_to_block_start(BasicBlock(0)); set(c.get(), &[1, 2, 3]);
    b.basic_blocks[0].terminator.as_mut().unwrap().kind = TerminatorKind::Yield {
        value: copy(3), resume: BasicBlock(1), resume_arg: place(1), drop: Some(BasicBlock(2)),
    };
    b.basic_blocks[1].statements.push(stmt(StatementKind::PlaceMention(place(1))));
    b.basic_blocks[2].statements.push(stmt(StatementKind::PlaceMention(place(1))));
    let r = iterate_to_fixpoint(&b, &MaybeLiveLocals);
    entries(&r, &[&[1], &[], &[]]);
    let mut c = r.cursor(); c.seek_to_block_start(BasicBlock(0)); set(c.get(), &[1, 3]);
}

#[test]
fn false_edges_assert_and_drop_cleanup_all_propagate() {
    let b = body(4, vec![
        block(vec![StatementKind::Assign(place(3), Rvalue::RawPtr(RawPtrKind::Mut, place(1)))],
            TerminatorKind::FalseEdge { real_target: BasicBlock(1), imaginary_target: BasicBlock(2) }),
        block(vec![], TerminatorKind::Assert { cond: constant(), expected: true, msg: AssertKind::NullPointerDereference,
            target: BasicBlock(3), unwind: UnwindAction::Cleanup(BasicBlock(4)) }),
        block(vec![], TerminatorKind::FalseUnwind { real_target: BasicBlock(3), unwind: UnwindAction::Cleanup(BasicBlock(4)) }),
        block(vec![], TerminatorKind::Drop { place: place(2), target: BasicBlock(5), unwind: UnwindAction::Cleanup(BasicBlock(4)), replace: false }),
        block(vec![], TerminatorKind::UnwindResume), block(vec![], TerminatorKind::Return),
    ]);
    let r = iterate_to_fixpoint(&b, &MaybeBorrowedLocals);
    entries(&r, &[&[], &[1], &[1], &[1], &[1, 2], &[1, 2]]);
    let r = iterate_to_fixpoint(&b, &MaybeLiveLocals);
    entries(&r, &[&[0, 2], &[0, 2], &[0, 2], &[0], &[], &[]]);
    let mut c = r.cursor(); c.seek_to_block_start(BasicBlock(0)); set(c.get(), &[0, 1, 2]);
}

#[test]
fn backward_nonterminating_loop_generates_liveness_from_bottom() {
    let b = body(2, vec![block(vec![], goto(1)), block(vec![StatementKind::PlaceMention(place(1))], goto(1))]);
    let r = iterate_to_fixpoint(&b, &MaybeLiveLocals);
    entries(&r, &[&[1], &[1]]);
}

struct MustStorage;
impl Analysis for MustStorage {
    fn bottom_value(&self, body: &Body) -> BitSet { BitSet::new_filled(body.local_decls.len()) }
    fn initialize_start_block(&self, _: &Body, state: &mut BitSet) { state.clear(); }
    fn join(&self, state: &mut BitSet, incoming: &BitSet) -> bool { state.intersect(incoming) }
    fn apply_primary_statement_effect(&self, state: &mut BitSet, stmt: &Statement, _: Location) {
        if let StatementKind::StorageLive(local) = stmt.kind { state.insert(local.0 as usize); }
    }
}

#[test]
fn concrete_bitset_domain_supports_intersection_join() {
    let b = body(3, vec![
        block(vec![StatementKind::StorageLive(Local(1))], switch(1, 2)),
        block(vec![StatementKind::StorageLive(Local(2))], goto(3)), block(vec![], goto(3)),
        block(vec![], TerminatorKind::Return),
    ]);
    let r = iterate_to_fixpoint(&b, &MustStorage);
    entries(&r, &[&[], &[1], &[1], &[1]]);
}

struct Prune;
impl Analysis for Prune {
    fn bottom_value(&self, _: &Body) -> BitSet { BitSet::new_empty(1) }
    fn apply_primary_terminator_effect<'a>(&self, state: &mut BitSet, term: &'a Terminator, _: Location) -> TerminatorEdges<'a> {
        state.insert(0);
        match term.kind {
            TerminatorKind::SwitchInt { .. } => TerminatorEdges::Single(BasicBlock(1)),
            _ => TerminatorEdges::for_terminator(term),
        }
    }
}

#[test]
fn primary_terminator_can_filter_forward_edges() {
    let b = body(1, vec![block(vec![], switch(1, 2)), block(vec![], TerminatorKind::Return), block(vec![], TerminatorKind::Return)]);
    let r = iterate_to_fixpoint(&b, &Prune);
    entries(&r, &[&[], &[0], &[]]);
}

#[test]
#[should_panic(expected = "backward start state is unsupported")]
fn backward_initialization_rejected_like_rustc() {
    struct Invalid;
    impl Analysis for Invalid {
        fn direction(&self) -> Direction { Direction::Backward }
        fn bottom_value(&self, _: &Body) -> BitSet { BitSet::new_empty(1) }
        fn initialize_start_block(&self, _: &Body, state: &mut BitSet) { state.insert(0); }
    }
    iterate_to_fixpoint(&body(1, vec![block(vec![], TerminatorKind::Return)]), &Invalid);
}

#[test]
#[should_panic(expected = "unreachable block")]
fn cursor_rejects_unreachable_state() {
    let b = body(1, vec![block(vec![], TerminatorKind::Return), block(vec![], TerminatorKind::Return)]);
    let r = iterate_to_fixpoint(&b, &MaybeLiveLocals);
    r.cursor().seek_to_block_start(BasicBlock(1));
}
