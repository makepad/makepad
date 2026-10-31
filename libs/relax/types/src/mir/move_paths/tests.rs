use super::*;
use crate::mir::dataflow::*;
use crate::mir::drop_flag_effects::*;
use crate::types::TyKind;

fn place(i: u32) -> Place { Place::local(Local(i)) }
fn project(i: u32, projection: Vec<PlaceElem>) -> Place { Place { local: Local(i), projection } }
fn field(i: u32, f: u32, ty: TyId) -> Place { project(i, vec![PlaceElem::Field(FieldIdx(f), ty)]) }
fn copy(i: u32) -> Operand { Operand::Copy(place(i)) }
fn moved(i: u32) -> Operand { Operand::Move(place(i)) }
fn constant(ty: TyId) -> Operand {
    Operand::Constant(Box::new(ConstOperand { span: Span::default(), user_ty: None, const_: Const::Val(ConstValue::ZeroSized, ty) }))
}
fn assign(i: u32, op: Operand) -> StatementKind { StatementKind::Assign(place(i), Rvalue::Use(op)) }
fn block(stmts: Vec<StatementKind>, kind: TerminatorKind) -> BasicBlockData {
    let mut statements = Vec::new();
    for kind in stmts { statements.push(Statement { source_info: SourceInfo::default(), kind }); }
    BasicBlockData { statements, terminator: Some(Terminator { source_info: SourceInfo::default(), kind }), is_cleanup: false }
}
fn body(types: &[TyId], args: u32, blocks: Vec<BasicBlockData>) -> Body {
    let mut local_decls = Vec::new();
    for (i, &ty) in types.iter().enumerate() {
        local_decls.push(LocalDecl { ty, mutability: Mutability::Mut, user_ty: Vec::new(), source_info: SourceInfo::default(),
            kind: if i == 0 { LocalKind::Return } else if i <= args as usize { LocalKind::Arg } else { LocalKind::Temp }, local_info: LocalInfo::Boring });
    }
    Body { phase: MirPhase::Built, source: MirSource { owner: DefId(0), kind: BodyKind::Fn }, basic_blocks: blocks, local_decls,
        source_scopes: vec![SourceScopeData { span: Span::default(), parent_scope: None }],
        user_type_annotations: Vec::new(), span: Span::default(), upvars: Vec::new(), allocs: AllocMap::default(), var_debug_info: Vec::new(), arg_count: args,
        spread_arg: None, promoted: Vec::new(), coroutine: None }
}
fn goto(i: u32) -> TerminatorKind { TerminatorKind::Goto { target: BasicBlock(i) } }
fn switch(discr: Operand, target: u32, otherwise: u32) -> TerminatorKind {
    TerminatorKind::SwitchInt { discr, targets: SwitchTargets { values: vec![0], targets: vec![BasicBlock(target)], otherwise: BasicBlock(otherwise) } }
}
fn call(func: Operand, args: Vec<Operand>, destination: Place, target: Option<u32>, cleanup: u32) -> TerminatorKind {
    TerminatorKind::Call { func, arg_spans: vec![Span::default(); args.len()], args, destination,
        target: target.map(BasicBlock), unwind: UnwindAction::Cleanup(BasicBlock(cleanup)), call_source: CallSource::Normal, fn_span: Span::default() }
}
fn at(block: u32, statement_index: usize) -> Location { Location { block: BasicBlock(block), statement_index } }
fn gather(b: &Body, prog: &Program, tcx: &Tcx) -> MoveData { MoveData::gather_moves(b, prog, tcx, crate::tcx::TypingEnv::fully_monomorphized(&prog), &|_| true) }
fn exact(data: &MoveData, place: &Place) -> usize {
    let LookupResult::Exact(index) = data.rev_lookup.find(place) else { panic!("untracked {place:?}"); };
    index
}
fn bits(state: &BitSet, expected: &[usize]) {
    let mut expected = expected.to_vec(); expected.sort_unstable();
    assert_eq!(state.iter().collect::<Vec<_>>(), expected);
}
fn after(results: &Results<'_>, loc: Location, expected: &[usize]) {
    let mut cursor = results.cursor(); cursor.seek_after_primary_effect(loc); bits(cursor.get(), expected);
}
fn collect(source: &str) -> (Program, Tcx) {
    let sess = &std::sync::Arc::new(crate::Session::default());
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let index = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let file = std::env::temp_dir().join(format!("relax-move-paths-{}-{index}.rs", std::process::id()));
    std::fs::write(&file, source).unwrap();
    let mut prog = Program::new(sess);
    prog.load_crate("custom", file.to_str().unwrap(), 2021, crate::cfg::CfgSet::host(sess, &[]), Vec::new()).unwrap();
    std::fs::remove_file(file).unwrap();
    prog.resolve_imports();
    let mut tcx = Tcx::new(); tcx.collect(&prog);
    assert!(tcx.errors.is_empty(), "{:?}", tcx.errors);
    (prog, tcx)
}
fn named(prog: &Program, name: &str) -> DefId {
    for i in 0..prog.defs.len() { let def = DefId(i as u32); if prog.name(def) == name { return def; } }
    panic!("missing definition {name}")
}
fn adt(prog: &Program, tcx: &Tcx, name: &str, args: Vec<TyId>) -> TyId { tcx.tys.intern(TyKind::Adt(named(prog, name), tcx.tys.mk_list(&args), crate::types::RegionArgsId::EMPTY)) }

#[test]
fn move_reinit_loop_and_storage_dead_history() {
    let sess = &std::sync::Arc::new(crate::Session::default());
    let prog = Program::new(sess); let tcx = Tcx::new(); let t = tcx.tys.unit;
    let b = body(&[t, t, t], 1, vec![
        block(vec![], goto(1)),
        block(vec![assign(2, moved(1)), assign(1, constant(t)), StatementKind::StorageDead(Local(2))], switch(constant(t), 1, 2)),
        block(vec![], TerminatorKind::Return),
    ]);
    let data = gather(&b, &prog, &tcx);
    assert_eq!(data.inits.len(), 3); assert_eq!(data.moves.len(), 2);
    assert_eq!(data.loc_map[at(1, 0)], vec![0]); assert_eq!(data.init_loc_map[at(1, 1)], vec![2]);
    let init = MaybeInitializedPlaces::new(&prog, &tcx, &b, &data); let i = iterate_to_fixpoint(&b, &init);
    bits(i.entry_state(BasicBlock(1)), &[1]); after(&i, at(1, 0), &[2]); after(&i, at(1, 1), &[1, 2]); after(&i, at(1, 2), &[1]);
    let uninit = MaybeUninitializedPlaces::new(&prog, &tcx, &b, &data); let u = iterate_to_fixpoint(&b, &uninit);
    bits(u.entry_state(BasicBlock(1)), &[0, 2]); after(&u, at(1, 0), &[0, 1]); after(&u, at(1, 1), &[0]);
    let ever = EverInitializedPlaces::new(&b, &data); let e = iterate_to_fixpoint(&b, &ever);
    bits(e.entry_state(BasicBlock(1)), &[0, 2]); after(&e, at(1, 0), &[0, 1, 2]); after(&e, at(1, 2), &[0, 2]);
}

#[test]
fn partial_struct_move_and_whole_drop_children() {
    let (prog, tcx) = collect("struct Pair { a: u8, b: u8 }"); let t = tcx.tys.u8_; let pair = adt(&prog, &tcx, "Pair", vec![]);
    let a = field(1, 0, t); let c = field(1, 1, t);
    let b = body(&[tcx.tys.unit, pair, t], 1, vec![
        block(vec![StatementKind::FakeRead(FakeReadCause::ForMatchedPlace(None), c.clone()), assign(2, Operand::Move(a.clone()))],
            TerminatorKind::Drop { place: place(1), target: BasicBlock(1), unwind: UnwindAction::Cleanup(BasicBlock(2)), replace: false }),
        block(vec![], TerminatorKind::Return), block(vec![], TerminatorKind::UnwindResume),
    ]);
    let data = gather(&b, &prog, &tcx); let pa = exact(&data, &a); let pc = exact(&data, &c);
    let init = MaybeInitializedPlaces::new(&prog, &tcx, &b, &data); let i = iterate_to_fixpoint(&b, &init);
    bits(i.entry_state(BasicBlock(0)), &[1, pa, pc]); after(&i, at(0, 1), &[1, 2, pc]);
    bits(i.entry_state(BasicBlock(1)), &[2]); bits(i.entry_state(BasicBlock(2)), &[2]);
    let mut children = Vec::new(); on_all_children_bits(&data, 1, &mut |p| children.push(p));
    assert_eq!(children, vec![1, pa, pc]);
    assert_eq!(move_path_children_matching(&data, 1, &mut |e| matches!(e, PlaceElem::Field(FieldIdx(1), _))), Some(pc));
    assert_eq!(data.moves.len(), 1, "Drop is an effect, not MoveOut");
    let uninit = MaybeUninitializedPlaces::new(&prog, &tcx, &b, &data); let u = iterate_to_fixpoint(&b, &uninit);
    bits(u.entry_state(BasicBlock(1)), &[0, 1, pa, pc]);
}

#[test]
fn maybe_join_of_moved_and_unmoved_branch() {
    let sess = &std::sync::Arc::new(crate::Session::default());
    let prog = Program::new(sess); let tcx = Tcx::new(); let t = tcx.tys.unit;
    let b = body(&[t, t, t], 1, vec![block(vec![], switch(constant(t), 1, 2)),
        block(vec![assign(2, moved(1))], goto(3)), block(vec![], goto(3)), block(vec![], TerminatorKind::Return)]);
    let data = gather(&b, &prog, &tcx);
    let init = MaybeInitializedPlaces::new(&prog, &tcx, &b, &data); let i = iterate_to_fixpoint(&b, &init); bits(i.entry_state(BasicBlock(3)), &[1, 2]);
    let uninit = MaybeUninitializedPlaces::new(&prog, &tcx, &b, &data); let u = iterate_to_fixpoint(&b, &uninit); bits(u.entry_state(BasicBlock(3)), &[0, 1, 2]);
    let ever = EverInitializedPlaces::new(&b, &data); let e = iterate_to_fixpoint(&b, &ever); bits(e.entry_state(BasicBlock(3)), &[0, 1]);
}

#[test]
fn call_moves_before_cleanup_and_inits_only_on_return() {
    let sess = &std::sync::Arc::new(crate::Session::default());
    let prog = Program::new(sess); let tcx = Tcx::new(); let t = tcx.tys.unit;
    let b = body(&[t, t, t, t], 2, vec![block(vec![], call(moved(1), vec![moved(2)], place(3), Some(1), 2)),
        block(vec![], TerminatorKind::Return), block(vec![], TerminatorKind::UnwindResume)]);
    let data = gather(&b, &prog, &tcx); assert_eq!(data.inits[2].kind, InitKind::NonPanicPathOnly);
    let init = MaybeInitializedPlaces::new(&prog, &tcx, &b, &data); let i = iterate_to_fixpoint(&b, &init);
    after(&i, at(0, 0), &[]); bits(i.entry_state(BasicBlock(1)), &[3]); bits(i.entry_state(BasicBlock(2)), &[]);
    let uninit = MaybeUninitializedPlaces::new(&prog, &tcx, &b, &data); let u = iterate_to_fixpoint(&b, &uninit);
    bits(u.entry_state(BasicBlock(1)), &[0, 1, 2]); bits(u.entry_state(BasicBlock(2)), &[0, 1, 2, 3]);
    let ever = EverInitializedPlaces::new(&b, &data); let e = iterate_to_fixpoint(&b, &ever);
    bits(e.entry_state(BasicBlock(1)), &[0, 1, 2]); bits(e.entry_state(BasicBlock(2)), &[0, 1]);
    let mut diverging = b.clone();
    if let TerminatorKind::Call { target, .. } = &mut diverging.basic_blocks[0].terminator.as_mut().unwrap().kind { *target = None; }
    let data = gather(&diverging, &prog, &tcx); assert_eq!(data.inits.len(), 2); assert_eq!(data.moves.len(), 2);
}

#[test]
fn semantic_box_deref_reference_raw_pointer_and_destructor() {
    let (prog, tcx) = collect(r#"
        #[lang = "owned_box"] struct Heap<T>(*mut T);
        #[lang = "drop"] trait Cleanup { fn release(&mut self); }
        impl<T> Cleanup for Heap<T> { fn release(&mut self) {} }
        struct Guard { value: u8 }
        impl Cleanup for Guard { fn release(&mut self) {} }
        struct Box<T>(*mut T);
    "#);
    let t = tcx.tys.u8_; let heap = adt(&prog, &tcx, "Heap", vec![t]); let guard = adt(&prog, &tcx, "Guard", vec![]);
    let reference = tcx.tys.intern(TyKind::Ref(false, t, crate::types::RegionId::ERASED)); let pointer = tcx.tys.intern(TyKind::Ptr(true, t));
    let b = body(&[tcx.tys.unit, heap, reference, pointer, guard, t], 4, vec![block(vec![
        assign(5, Operand::Move(project(1, vec![PlaceElem::Deref]))),
        assign(5, Operand::Move(project(2, vec![PlaceElem::Deref]))),
        assign(5, Operand::Move(project(3, vec![PlaceElem::Deref]))),
        assign(5, Operand::Move(field(4, 0, t))),
    ], TerminatorKind::Return)]);
    let data = gather(&b, &prog, &tcx); let path = exact(&data, &project(1, vec![PlaceElem::Deref]));
    assert_eq!(data.moves.len(), 1); assert_eq!(data.moves[0].path, path); assert_eq!(data.base_local(path), Local(1));
    assert_eq!(data.rev_lookup.find(&project(2, vec![PlaceElem::Deref])), LookupResult::Parent(Some(2)));
    assert_eq!(data.rev_lookup.find(&project(3, vec![PlaceElem::Deref])), LookupResult::Parent(Some(3)));
    assert_eq!(data.rev_lookup.find(&field(4, 0, t)), LookupResult::Parent(Some(4)));
    assert!(!tcx.adts[&named(&prog, "Box")].is_box());
    assert_eq!(project(1, vec![PlaceElem::Deref]).ty(&b, &tcx).unwrap().ty, t);
}

#[test]
fn unions_coalesce_but_check_later_dereferences() {
    let (prog, tcx) = collect("union Storage { byte: u8, pointer: &'static u8 }");
    let t = tcx.tys.u8_; let union = adt(&prog, &tcx, "Storage", vec![]); let reference = tcx.tys.intern(TyKind::Ref(false, t, crate::types::RegionId::ERASED));
    let b = body(&[tcx.tys.unit, union, t], 1, vec![block(vec![
        assign(2, Operand::Move(field(1, 0, t))),
        StatementKind::Assign(field(1, 0, t), Rvalue::Use(constant(t))),
        assign(2, Operand::Move(project(1, vec![PlaceElem::Field(FieldIdx(1), reference), PlaceElem::Deref]))),
    ], TerminatorKind::Return)]);
    let data = gather(&b, &prog, &tcx); assert_eq!(data.move_paths.len(), 3); assert_eq!(data.moves.len(), 1); assert_eq!(data.moves[0].path, 1);
    assert_eq!(data.inits[data.init_loc_map[at(0, 1)][0]].path, 1);
    assert_eq!(data.rev_lookup.find(&field(1, 0, t)), LookupResult::Parent(Some(1)));
    let init = MaybeInitializedPlaces::new(&prog, &tcx, &b, &data); let i = iterate_to_fixpoint(&b, &init);
    after(&i, at(0, 0), &[2]); after(&i, at(0, 1), &[1, 2]); after(&i, at(0, 2), &[1, 2]);
}

#[test]
fn arrays_subslice_paths_are_disjoint_and_slices_stop() {
    let sess = &std::sync::Arc::new(crate::Session::default());
    let prog = Program::new(sess); let tcx = Tcx::new(); let t = tcx.tys.u8_;
    let array = tcx.tys.intern(TyKind::Array(t, tcx.tys.const_usize(4))); let slice = tcx.tys.intern(TyKind::Slice(t));
    let ci = |offset| PlaceElem::ConstantIndex { offset, min_length: 4, from_end: false };
    let sub = project(1, vec![PlaceElem::Subslice { from: 1, to: 3, from_end: false }]);
    let b = body(&[tcx.tys.unit, array, slice, t, tcx.tys.usize_], 2, vec![block(vec![
        assign(3, Operand::Move(sub.clone())), assign(3, Operand::Move(project(1, vec![ci(1)]))),
        assign(3, Operand::Move(project(1, vec![PlaceElem::Index(Local(4))]))),
        assign(3, Operand::Move(project(1, vec![PlaceElem::ConstantIndex { offset: 1, min_length: 4, from_end: true }]))),
        assign(3, Operand::Move(project(2, vec![ci(1)]))),
        assign(3, Operand::Move(project(2, vec![PlaceElem::Subslice { from: 1, to: 1, from_end: true }]))),
    ], TerminatorKind::Return)]);
    let data = gather(&b, &prog, &tcx); let a = exact(&data, &project(1, vec![ci(1)])); let c = exact(&data, &project(1, vec![ci(2)]));
    assert_eq!(data.move_paths.len(), 7); assert_eq!(data.moves.len(), 3);
    assert_eq!(data.moves[0].path, a); assert_eq!(data.moves[1].path, c); assert_eq!(data.moves[2].path, a);
    assert_eq!(data.rev_lookup.find(&sub), LookupResult::Parent(Some(1)));
    assert_eq!(sub.ty(&b, &tcx).unwrap().ty, tcx.tys.intern(TyKind::Array(t, tcx.tys.const_usize(2))));
    let end = project(1, vec![PlaceElem::Subslice { from: 1, to: 1, from_end: true }]);
    assert_eq!(end.ty(&b, &tcx).unwrap().ty, sub.ty(&b, &tcx).unwrap().ty);
}

#[test]
fn enum_discriminant_edges_and_otherwise_options() {
    let (prog, tcx) = collect("#[repr(i8)] enum E { A(u8) = -1, B(u8) = 3, C(u8) = 5 }");
    let t = tcx.tys.u8_; let en = adt(&prog, &tcx, "E", vec![]);
    let variant = |v| project(1, vec![PlaceElem::Downcast(None, VariantIdx(v))]);
    let vf = |v| project(1, vec![PlaceElem::Downcast(None, VariantIdx(v)), PlaceElem::Field(FieldIdx(0), t)]);
    let mut stmts = Vec::new();
    for v in 0..3 { stmts.push(StatementKind::FakeRead(FakeReadCause::ForMatchedPlace(None), vf(v))); }
    stmts.push(StatementKind::Assign(place(2), Rvalue::Discriminant(place(1))));
    let b = body(&[tcx.tys.unit, en, tcx.tys.intern(TyKind::Int(crate::program::Prim::I8)), t], 1, vec![
        block(stmts, TerminatorKind::SwitchInt { discr: copy(2), targets: SwitchTargets {
            values: vec![255, 5], targets: vec![BasicBlock(1), BasicBlock(2)], otherwise: BasicBlock(3) } }),
        block(vec![], TerminatorKind::Return), block(vec![], TerminatorKind::Return), block(vec![], TerminatorKind::Return),
    ]);
    let data = gather(&b, &prog, &tcx);
    let a = exact(&data, &variant(0)); let af = exact(&data, &vf(0));
    let d = exact(&data, &variant(1)); let df = exact(&data, &vf(1));
    let c = exact(&data, &variant(2)); let cf = exact(&data, &vf(2));
    let init = MaybeInitializedPlaces::new(&prog, &tcx, &b, &data); let i = iterate_to_fixpoint(&b, &init);
    bits(i.entry_state(BasicBlock(1)), &[1, 2, a, af]); bits(i.entry_state(BasicBlock(2)), &[1, 2, c, cf]);
    bits(i.entry_state(BasicBlock(3)), &[1, 2, a, af, d, df, c, cf]);
    let init = init.exclude_inactive_in_otherwise(); let i = iterate_to_fixpoint(&b, &init);
    bits(i.entry_state(BasicBlock(3)), &[1, 2, d, df]);
    let uninit = MaybeUninitializedPlaces::new(&prog, &tcx, &b, &data); let u = iterate_to_fixpoint(&b, &uninit);
    bits(u.entry_state(BasicBlock(1)), &[0, 3]);
    let uninit = uninit.mark_inactive_variants_as_uninit(); let u = iterate_to_fixpoint(&b, &uninit);
    bits(u.entry_state(BasicBlock(1)), &[0, 3, d, df, c, cf]); bits(u.entry_state(BasicBlock(2)), &[0, 3, a, af, d, df]);
    bits(u.entry_state(BasicBlock(3)), &[0, 3]);
    let uninit = uninit.include_inactive_in_otherwise(); let u = iterate_to_fixpoint(&b, &uninit);
    bits(u.entry_state(BasicBlock(3)), &[0, 3, a, af, c, cf]);
    let init = init.precise_enum_drop_elaboration(false); let i = iterate_to_fixpoint(&b, &init);
    bits(i.entry_state(BasicBlock(1)), &[1, 2, a, af, d, df, c, cf]);
    let mut separated = b.clone(); separated.basic_blocks[0].statements.push(Statement { source_info: SourceInfo::default(), kind: StatementKind::Nop });
    let init = MaybeInitializedPlaces::new(&prog, &tcx, &separated, &data);
    assert!(!init.has_switch_int_edge_effect(BasicBlock(0), &SwitchTargets { values: vec![255], targets: vec![BasicBlock(1)], otherwise: BasicBlock(2) }, &copy(2)));
}

#[test]
fn mutable_and_raw_borrows_restore_maybe_init_children() {
    let sess = &std::sync::Arc::new(crate::Session::default());
    let prog = Program::new(sess); let tcx = Tcx::new(); let t = tcx.tys.u8_; let tuple = tcx.tys.intern(TyKind::Tuple(tcx.tys.mk_list(&[t, t])));
    for kind in 0..3 {
        let value = match kind {
            0 => Rvalue::Ref(Region::ERASED, BorrowKind::Mut { kind: MutBorrowKind::Default }, place(1)),
            1 => Rvalue::RawPtr(RawPtrKind::Const, place(1)),
            _ => Rvalue::Ref(Region::ERASED, BorrowKind::Shared, place(1)),
        };
        let b = body(&[tcx.tys.unit, tuple, t, tcx.tys.intern(TyKind::Ref(true, tuple, crate::types::RegionId::ERASED))], 1, vec![block(vec![
            assign(2, Operand::Move(field(1, 0, t))), StatementKind::Assign(place(3), value),
        ], TerminatorKind::Return)]);
        let data = gather(&b, &prog, &tcx); let f = exact(&data, &field(1, 0, t));
        let init = MaybeInitializedPlaces::new(&prog, &tcx, &b, &data); let i = iterate_to_fixpoint(&b, &init);
        if kind < 2 { after(&i, at(0, 1), &[1, 2, 3, f]); } else { after(&i, at(0, 1), &[1, 2, 3]); }
        let uninit = MaybeUninitializedPlaces::new(&prog, &tcx, &b, &data); let u = iterate_to_fixpoint(&b, &uninit); after(&u, at(0, 1), &[0, f]);
    }
}

#[test]
fn dead_drop_unwind_does_not_generate_from_unreachable_cleanup() {
    let sess = &std::sync::Arc::new(crate::Session::default());
    let prog = Program::new(sess); let tcx = Tcx::new(); let t = tcx.tys.unit;
    let b = body(&[t, t, t], 0, vec![block(vec![], TerminatorKind::Drop { place: place(1), target: BasicBlock(1),
        unwind: UnwindAction::Cleanup(BasicBlock(2)), replace: false }),
        block(vec![], goto(3)), block(vec![assign(2, constant(t))], goto(3)), block(vec![], TerminatorKind::Return)]);
    let data = gather(&b, &prog, &tcx);
    let init = MaybeInitializedPlaces::new(&prog, &tcx, &b, &data).skipping_unreachable_unwind(); let i = iterate_to_fixpoint(&b, &init);
    assert!(!i.is_reachable(BasicBlock(2))); assert!(i.is_reachable(BasicBlock(1))); bits(i.entry_state(BasicBlock(3)), &[]);
    let init = MaybeInitializedPlaces::new(&prog, &tcx, &b, &data); let i = iterate_to_fixpoint(&b, &init);
    assert!(i.is_reachable(BasicBlock(2))); bits(i.entry_state(BasicBlock(3)), &[2]);
    let mut alive = b.clone(); alive.arg_count = 1; alive.local_decls[1].kind = LocalKind::Arg;
    let data = gather(&alive, &prog, &tcx);
    let init = MaybeInitializedPlaces::new(&prog, &tcx, &alive, &data).skipping_unreachable_unwind(); let i = iterate_to_fixpoint(&alive, &init);
    assert!(i.is_reachable(BasicBlock(2)), "unwind decision must precede Drop's kill");
}

#[test]
fn deref_temporaries_recover_the_original_move_path() {
    let (prog, tcx) = collect("#[lang = \"owned_box\"] struct Heap<T>(*mut T);"); let t = tcx.tys.u8_;
    let heap = adt(&prog, &tcx, "Heap", vec![t]); let tuple = tcx.tys.intern(TyKind::Tuple(tcx.tys.mk_list(&[heap])));
    let mut b = body(&[tcx.tys.unit, tuple, heap, t], 1, vec![block(vec![
        StatementKind::Assign(place(2), Rvalue::CopyForDeref(field(1, 0, heap))),
        assign(3, Operand::Move(project(2, vec![PlaceElem::Deref]))), StatementKind::StorageDead(Local(2)),
    ], TerminatorKind::Return)]);
    b.local_decls[2].local_info = LocalInfo::DerefTemp;
    let data = gather(&b, &prog, &tcx);
    let original = project(1, vec![PlaceElem::Field(FieldIdx(0), heap), PlaceElem::Deref]);
    let path = exact(&data, &original);
    assert_eq!(exact(&data, &project(2, vec![PlaceElem::Deref])), path);
    assert_eq!(data.rev_lookup.find_local(Local(2)), data.rev_lookup.find_local(Local(1)));
    assert_eq!(data.moves.len(), 1); assert_eq!(data.moves[0].path, path);
    assert!(data.init_loc_map[at(0, 0)].is_empty()); assert!(data.loc_map[at(0, 2)].is_empty());
    assert_eq!(data.base_local(path), Local(1));
}

#[test]
fn normalized_field_type_drives_box_projection() {
    let (prog, tcx) = collect(r#"
        #[lang = "owned_box"] struct Heap<T>(*mut T);
        trait Item { type Output; }
        struct Concrete;
        impl Item for Concrete { type Output = Heap<u8>; }
        struct Record<T> { value: T }
        struct Projected { value: <Concrete as Item>::Output }
    "#);
    let t = tcx.tys.u8_; let heap = adt(&prog, &tcx, "Heap", vec![t]);
    let record = adt(&prog, &tcx, "Record", vec![heap]); let projected = adt(&prog, &tcx, "Projected", vec![]);
    for root in [record, projected] {
        let p = project(1, vec![PlaceElem::Field(FieldIdx(0), heap), PlaceElem::Deref]);
        let b = body(&[tcx.tys.unit, root, t], 1, vec![block(vec![assign(2, Operand::Move(p.clone()))], TerminatorKind::Return)]);
        let data = gather(&b, &prog, &tcx); assert_eq!(data.moves.len(), 1); assert_eq!(data.moves[0].path, exact(&data, &p));
        assert_eq!(PlaceTy::from_ty(root).field_ty(&prog, &tcx, crate::tcx::TypingEnv::fully_monomorphized(&prog), FieldIdx(0)), Some(heap));
    }
}

#[test]
fn filtered_arguments_and_untracked_writes() {
    let sess = &std::sync::Arc::new(crate::Session::default());
    let prog = Program::new(sess); let tcx = Tcx::new(); let t = tcx.tys.u8_; let r = tcx.tys.intern(TyKind::Ref(true, t, crate::types::RegionId::ERASED));
    let b = body(&[tcx.tys.unit, t, r], 2, vec![block(vec![
        StatementKind::Assign(project(2, vec![PlaceElem::Deref]), Rvalue::Use(constant(t))),
    ], TerminatorKind::Return)]);
    let data = MoveData::gather_moves(&b, &prog, &tcx, crate::tcx::TypingEnv::fully_monomorphized(&prog), &|ty| ty == r);
    assert_eq!(data.move_paths.len(), 1); assert_eq!(data.inits.len(), 1);
    assert_eq!(data.rev_lookup.find(&place(1)), LookupResult::Parent(None));
    assert!(data.init_loc_map[at(0, 0)].is_empty());
    let ever = EverInitializedPlaces::new(&b, &data); let e = iterate_to_fixpoint(&b, &ever); bits(e.entry_state(BasicBlock(0)), &[0]);
    let init = MaybeInitializedPlaces::new(&prog, &tcx, &b, &data); let i = iterate_to_fixpoint(&b, &init); after(&i, at(0, 0), &[0]);
}

#[test]
fn self_move_assignment_applies_move_before_init() {
    let sess = &std::sync::Arc::new(crate::Session::default());
    let prog = Program::new(sess); let tcx = Tcx::new(); let t = tcx.tys.unit;
    let b = body(&[t, t], 1, vec![block(vec![assign(1, moved(1))], TerminatorKind::Return)]);
    let data = gather(&b, &prog, &tcx); let mut effects = Vec::new();
    drop_flag_effects_for_location(&b, &data, at(0, 0), &mut |p, f| effects.push((p, f)));
    assert_eq!(effects, vec![(1, DropFlagState::Absent), (1, DropFlagState::Present)]);
}

#[test]
fn yield_initializations_match_reference_deep_init() {
    let sess = &std::sync::Arc::new(crate::Session::default());
    let prog = Program::new(sess); let tcx = Tcx::new(); let t = tcx.tys.unit;
    let b = body(&[t, t, t], 1, vec![block(vec![], TerminatorKind::Yield { value: moved(1), resume: BasicBlock(1),
        resume_arg: place(2), drop: Some(BasicBlock(2)) }), block(vec![], TerminatorKind::Return), block(vec![], TerminatorKind::CoroutineDrop)]);
    let data = gather(&b, &prog, &tcx); assert_eq!(data.inits[1].kind, InitKind::Deep);
    let init = MaybeInitializedPlaces::new(&prog, &tcx, &b, &data); let i = iterate_to_fixpoint(&b, &init);
    bits(i.entry_state(BasicBlock(1)), &[2]); bits(i.entry_state(BasicBlock(2)), &[2]);
}
