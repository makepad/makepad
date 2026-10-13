//! Hand-built MIR plus an independent symbolic walker. Drop invokes the generated
//! shim; only destructor calls and dyn's virtual dispatch are primitive events.
use crate::types::AliasTyKind;
use crate::types::{AliasTy};
use super::*;
use super::elaborate_drops::elaborate_drops;
use super::elaborate_drop::{integer, project};
use crate::mir::shim::build_drop_shim;
use crate::types::{TyKind};
use std::collections::{HashMap, HashSet};

struct Provider { prog: Program, tcx: Tcx, fail: Option<TyId>, copy: HashSet<TyId> }
impl DropProvider for Provider {
    fn tcx(&self) -> &Tcx { &self.tcx }
    fn program(&self) -> &Program { &self.prog }
    fn typing_env(&self) -> crate::tcx::TypingEnv { crate::tcx::TypingEnv::fully_monomorphized(&self.prog) }
    // rustc_ty_utils/src/needs_drop.rs:108,193; ty/util.rs:1493.
    // This is the authorized test query, not a new production needs_drop path.
    fn needs_drop(&self, ty: TyId) -> Result<bool, DropError> {
        if self.fail == Some(ty) { return Err(DropError::Query(ty)); }
        if self.copy.contains(&ty) { return Ok(false); }
        match self.tcx.tys.kind(ty) {
            TyKind::Adt(def, _, _) => {
                let adt = &self.tcx.adts[def];
                if self.adt_destructor(*def).is_some() { return Ok(true); }
                if adt.is_union() || adt.is_manually_drop() { return Ok(false); }
                for (v, variant) in adt.variants.iter().enumerate() {
                    for i in 0..variant.fields.len() {
                        let ty = self.field_ty(ty, if adt.is_enum() { Some(VariantIdx(v as u32)) } else { None }, FieldIdx(i as u32))?;
                        if self.needs_drop(ty)? { return Ok(true); }
                    }
                }
                Ok(false)
            }
            TyKind::Tuple(fields) => { for &field in self.tcx.tys.list(*fields) { if self.needs_drop(field)? { return Ok(true); } } Ok(false) }
            TyKind::Closure(_, _, crate::types::ClosureArgs { tupled_upvars_ty: captures, .. }) => self.needs_drop(*captures),
            TyKind::Array(elem, len) => {
                let zero = len.try_to_target_usize(&self.tcx.tys) == Some(0);
                if zero { Ok(false) } else { self.needs_drop(*elem) }
            }
            TyKind::Slice(elem) => self.needs_drop(*elem),
            TyKind::Param(_) | TyKind::Alias(_, AliasTy { kind: AliasTyKind::Projection { .. }, .. }) | TyKind::Alias(_, AliasTy { kind: AliasTyKind::Opaque { .. }, .. }) | TyKind::Dyn(..) => Ok(true),
            _ => Ok(false),
        }
    }
}
impl Provider {
    fn new() -> Self {
        let sess = &std::sync::Arc::new(crate::Session::default());
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let file = std::env::temp_dir().join(format!("relax-drop-{}-{id}.rs", std::process::id()));
        std::fs::write(&file, r#"
            #[lang = "drop"] trait Cleanup { fn release(&mut self); }
            struct D; impl Cleanup for D { fn release(&mut self) {} }
            struct E; impl Cleanup for E { fn release(&mut self) {} }
            struct Pair(D, E);
            struct WithDrop(D, E); impl Cleanup for WithDrop { fn release(&mut self) {} }
            #[repr(i8)] enum Choice { A(D, E) = -3, B(E) = 7, Empty = 20 }
            struct NonNull<T: ?Sized>(*mut T);
            #[lang = "owned_box"] struct Heap<T: ?Sized>(NonNull<T>);
            // The symbolic heap below supplies this toy deallocation primitive.
            extern "C" { fn deallocate(ptr: *mut u8); }
            impl<T: ?Sized> Cleanup for Heap<T> { fn release(&mut self) { unsafe { deallocate(self.0.0 as *mut u8); } } }
            struct VecLike; impl Cleanup for VecLike { fn release(&mut self) {} }
            struct Tail<T: ?Sized>(D, T); impl<T: ?Sized> Cleanup for Tail<T> { fn release(&mut self) {} }
            union Manual<T> { value: T }
            #[lang = "manually_drop"] struct Suppress<T>(T);
            trait Object {}
            fn glue<T: ?Sized>(ptr: *mut T) {}
            fn produce() -> D { D }
        "#).unwrap();
        let mut prog = Program::new(sess);
        prog.load_crate("drop_test", file.to_str().unwrap(), 2021, crate::cfg::CfgSet::host(sess, &[]), Vec::new()).unwrap();
        std::fs::remove_file(file).unwrap(); prog.resolve_imports();
        let mut tcx = Tcx::new(); tcx.collect(&prog);
        assert!(tcx.errors.is_empty(), "{:?}", tcx.errors);
        Self { prog, tcx, fail: None, copy: HashSet::new() }
    }
    fn def(&self, name: &str) -> DefId {
        for i in 0..self.prog.defs.len() { let d = DefId(i as u32); if self.prog.name(d) == name { return d; } }
        panic!("missing test definition {name}")
    }
    fn adt(&self, name: &str, args: Vec<TyId>) -> TyId { self.tcx.tys.intern(TyKind::Adt(self.def(name), self.tcx.tys.mk_list(&args), crate::types::RegionArgsId::EMPTY)) }
    fn d(&self) -> TyId { self.adt("D", vec![]) }
    fn e(&self) -> TyId { self.adt("E", vec![]) }
    fn shim(&self, ty: TyId) -> Body {
        let b = build_drop_shim(self, self.def("glue"), ty).unwrap(); valid(self, &b); b
    }
}
fn valid(p: &Provider, b: &Body) {
    assert_eq!(b.phase, MirPhase::Runtime(RuntimePhase::Initial));
    assert!(validate::validate(b, &p.tcx).is_empty(), "{:?}\n{}", validate::validate(b, &p.tcx), pretty::print_body(&p.prog, &p.tcx, "test", b));
}
fn local(n: u32) -> Place { Place::local(Local(n)) }
fn field(place: &Place, i: u32, ty: TyId) -> Place { project(place, PlaceElem::Field(FieldIdx(i), ty)) }
fn stmt(lhs: Place, value: Rvalue) -> Statement { Statement { source_info: SourceInfo::default(), kind: StatementKind::Assign(lhs, value) } }
fn block(statements: Vec<Statement>, kind: TerminatorKind) -> BasicBlockData {
    BasicBlockData { statements, is_cleanup: false, terminator: Some(Terminator { source_info: SourceInfo::default(), kind }) }
}
fn body(p: &Provider, types: &[TyId], args: u32, blocks: Vec<BasicBlockData>) -> Body {
    let mut b = p.shim(p.tcx.tys.unit);
    b.phase = MirPhase::Built; b.source.kind = BodyKind::Fn; b.local_decls.clear(); b.arg_count = args; b.basic_blocks = blocks;
    for (i, &ty) in types.iter().enumerate() {
        b.local_decls.push(LocalDecl { ty, source_info: SourceInfo::default(), mutability: Mutability::Mut,
            user_ty: Vec::new(), local_info: LocalInfo::Boring,
            kind: if i == 0 { LocalKind::Return } else if i <= args as usize { LocalKind::Arg } else { LocalKind::Temp } });
    }
    b
}
fn drop(place: Place, target: u32) -> TerminatorKind {
    TerminatorKind::Drop { place, target: BasicBlock(target), unwind: UnwindAction::Continue, replace: false }
}
fn go(target: u32) -> TerminatorKind { TerminatorKind::Goto { target: BasicBlock(target) } }
fn branch(discr: Operand, no: u32, yes: u32) -> TerminatorKind {
    TerminatorKind::SwitchInt { discr, targets: SwitchTargets { values: vec![0], targets: vec![BasicBlock(no)], otherwise: BasicBlock(yes) } }
}
fn elaborate(p: &Provider, b: &mut Body) { elaborate_drops(b, p).unwrap(); valid(p, b); }

#[derive(Clone, Debug, PartialEq, Eq)]
struct Address { name: String, meta: u64, start: u64 }
impl Address { fn new(name: &str, meta: u64) -> Self { Self { name: name.into(), meta, start: 0 } } }
#[derive(Clone, Debug)]
enum Value { Int(u128), Ptr(Address), Object(Address), Uninit }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Exit { Return, Panic, Abort }
#[derive(Clone, Debug, PartialEq, Eq)]
struct Event { ty: TyId, at: Address }
struct Walker<'a> {
    p: &'a Provider, memory: HashMap<String, Value>, events: Vec<Event>, panic_at: Vec<usize>,
    dynamic: HashMap<u64, TyId>, virtual_calls: usize, steps: usize, frames: usize,
    produce_panics: bool, live_allocations: HashSet<String>, frees: Vec<String>,
}
impl<'a> Walker<'a> {
    fn new(p: &'a Provider) -> Self {
        Self { p, memory: HashMap::new(), events: Vec::new(), panic_at: Vec::new(), dynamic: HashMap::new(),
            virtual_calls: 0, steps: 0, frames: 0, produce_panics: false, live_allocations: HashSet::new(), frees: Vec::new() }
    }
    fn heap(&mut self, name: &str, inner: &str, meta: u64) {
        assert!(self.live_allocations.insert(inner.into()));
        self.memory.insert(format!("{name}.0"), Value::Ptr(Address::new(inner, meta)));
    }
    fn address(&self, place: &Place, locals: &[Value], frame: usize) -> Address {
        let mut addr = match &locals[place.local.0 as usize] {
            Value::Object(a) => a.clone(), _ => Address::new(&format!("f{frame}_{}", place.local.0), 0),
        };
        for (i, elem) in place.projection.iter().enumerate() {
            match elem {
                PlaceElem::Deref => {
                    let value = if i == 0 { locals[place.local.0 as usize].clone() } else { self.read(&addr) };
                    addr = match value {
                        Value::Ptr(a) => a,
                        Value::Object(a) => {
                            let Value::Ptr(pointee) = self.memory[&format!("{}.0", a.name)].clone() else { panic!("Box pointer") };
                            pointee
                        }
                        other => panic!("deref {other:?}"),
                    };
                }
                PlaceElem::Field(f, _) => { addr.name.push_str(&format!(".{}", f.0)); addr.start = 0; }
                PlaceElem::Downcast(_, v) => { addr.name.push_str(&format!(".v{}", v.0)); }
                PlaceElem::Index(l) => {
                    let Value::Int(n) = locals[l.0 as usize] else { panic!("index not int"); };
                    addr.name.push_str(&format!("[{}]", addr.start + n as u64)); addr.start = 0;
                }
                PlaceElem::ConstantIndex { offset, from_end: false, .. } => {
                    addr.name.push_str(&format!("[{}]", addr.start + offset)); addr.start = 0;
                }
                PlaceElem::Subslice { from, to, from_end: false } => { addr.start += from; addr.meta = to - from; }
                PlaceElem::OpaqueCast(_) | PlaceElem::Subtype(_) => (),
                _ => panic!("unsupported walker projection {elem:?}"),
            }
        }
        addr
    }
    fn read(&self, addr: &Address) -> Value { self.memory.get(&addr.name).cloned().unwrap_or(Value::Object(addr.clone())) }
    fn op(&self, op: &Operand, locals: &[Value], frame: usize) -> Value {
        match op {
            Operand::Copy(p) | Operand::Move(p) => {
                if p.projection.is_empty() { locals[p.local.0 as usize].clone() } else { self.read(&self.address(p, locals, frame)) }
            }
            Operand::Constant(c) => match &c.const_ {
                Const::Val(ConstValue::Scalar(Scalar::Int { bits, .. }), _) => Value::Int(*bits),
                Const::Val(ConstValue::ZeroSized, _) => Value::Int(0),
                _ => panic!("unsupported walker constant"),
            },
        }
    }
    fn run(&mut self, b: &Body, args: Vec<Value>) -> Exit {
        valid(self.p, b);
        self.frames += 1; let frame = self.frames;
        let mut locals = vec![Value::Uninit; b.local_decls.len()];
        for (i, arg) in args.into_iter().enumerate() { locals[i + 1] = arg; }
        let mut bb = BasicBlock(0);
        loop {
            self.steps += 1; assert!(self.steps < 20000, "walker loop");
            let block = &b.basic_blocks[bb.0 as usize];
            for s in &block.statements {
                let StatementKind::Assign(dest, rv) = &s.kind else { continue; };
                let value = match rv {
                    Rvalue::Use(op) => self.op(op, &locals, frame),
                    Rvalue::Ref(_, _, p) | Rvalue::RawPtr(_, p) => Value::Ptr(self.address(p, &locals, frame)),
                    Rvalue::Cast(kind, op, _) => {
                        let mut val = self.op(op, &locals, frame);
                        if matches!(kind, CastKind::PointerCoercion(PointerCoercion::Unsize, _)) {
                            let (Operand::Move(p) | Operand::Copy(p)) = op else { panic!() };
                            let t = p.ty(b, &self.p.tcx).unwrap().ty;
                            let (TyKind::Ptr(_, array) | TyKind::Ref(_, array, _)) = self.p.tcx.tys.kind(t) else { panic!() };
                            let TyKind::Array(_, len) = self.p.tcx.tys.kind(*array) else { panic!() };
                            let Value::Ptr(ref mut a) = val else { panic!() }; a.meta = len.try_to_target_usize(&self.p.tcx.tys).unwrap();
                        }
                        val
                    }
                    Rvalue::UnaryOp(UnOp::PtrMetadata, op) => {
                        let Value::Ptr(a) = self.op(op, &locals, frame) else { panic!() }; Value::Int(a.meta as u128)
                    }
                    Rvalue::BinaryOp(op, operands) => {
                        let Value::Int(a) = self.op(&operands.0, &locals, frame) else { panic!() };
                        let Value::Int(c) = self.op(&operands.1, &locals, frame) else { panic!() };
                        Value::Int(match op { BinOp::Add => a + c, BinOp::Eq => (a == c) as u128, _ => panic!() })
                    }
                    Rvalue::Discriminant(p) => {
                        let mut a = self.address(p, &locals, frame); a.name.push_str(".disc"); self.read(&a)
                    }
                    _ => panic!("unsupported walker rvalue {rv:?}"),
                };
                if dest.projection.is_empty() { locals[dest.local.0 as usize] = value; }
                else { self.memory.insert(self.address(dest, &locals, frame).name, value); }
            }
            let (result, target, unwind) = match &block.terminator.as_ref().unwrap().kind {
                TerminatorKind::Goto { target } => { bb = *target; continue; }
                TerminatorKind::SwitchInt { discr, targets } => {
                    let Value::Int(n) = self.op(discr, &locals, frame) else { panic!("uninitialized flag") };
                    bb = targets.otherwise;
                    for (i, &v) in targets.values.iter().enumerate() { if v == n { bb = targets.targets[i]; break; } }
                    continue;
                }
                TerminatorKind::Return => return Exit::Return,
                TerminatorKind::UnwindResume => return Exit::Panic,
                TerminatorKind::UnwindTerminate(_) => return Exit::Abort,
                TerminatorKind::Drop { place, target, unwind, .. } => {
                    let ty = place.ty(b, &self.p.tcx).unwrap().ty;
                    let addr = self.address(place, &locals, frame);
                    (self.drop_value(ty, addr), Some(*target), *unwind)
                }
                TerminatorKind::Call { func, args, target, unwind, destination, .. } => {
                    let Operand::Constant(c) = func else { panic!() };
                    let Const::Val(_, ty) = c.const_ else { panic!() };
                    let TyKind::FnDef(def, tys, _) = self.p.tcx.tys.kind(ty) else { panic!() };
                    let tys = self.p.tcx.tys.list(*tys);
                    let result = if *def == self.p.def("produce") {
                        if self.produce_panics { Exit::Panic } else {
                            locals[destination.local.0 as usize] = Value::Object(Address::new("produced", 0)); Exit::Return
                        }
                    } else {
                        let Value::Ptr(addr) = self.op(&args[0], &locals, frame) else { panic!() };
                        if *def == self.p.def("glue") { self.drop_value(tys[0], addr) }
                        else {
                            // A Drop trait call, resolved by Self rather than impl parameter order.
                            assert_eq!(self.p.prog.def(*def).parent, self.p.tcx.lang_items.get(crate::lang_items::LangItem::Drop).unwrap());
                            if let TyKind::Adt(def, _, _) = self.p.tcx.tys.kind(tys[0]) {
                                if self.p.is_box(*def) {
                                    let Value::Ptr(pointee) = &self.memory[&format!("{}.0", addr.name)] else { panic!() };
                                    assert!(self.live_allocations.remove(&pointee.name), "double deallocation");
                                    self.frees.push(pointee.name.clone());
                                }
                            }
                            self.events.push(Event { ty: tys[0], at: addr });
                            if self.panic_at.contains(&(self.events.len() - 1)) { Exit::Panic } else { Exit::Return }
                        }
                    };
                    (result, *target, *unwind)
                }
                other => panic!("unexpected walker terminator {other:?}"),
            };
            match result {
                Exit::Return => bb = target.unwrap(),
                Exit::Abort => return Exit::Abort,
                Exit::Panic => match unwind {
                    UnwindAction::Cleanup(target) => bb = target,
                    UnwindAction::Continue => return Exit::Panic,
                    UnwindAction::Terminate(_) => return Exit::Abort,
                    UnwindAction::Unreachable => panic!("unreachable unwind taken"),
                },
            }
        }
    }
    fn drop_value(&mut self, ty: TyId, addr: Address) -> Exit {
        for free in &self.frees {
            assert!(addr.name != *free && !addr.name.starts_with(&format!("{free}."))
                && !addr.name.starts_with(&format!("{free}[")), "drop after deallocation");
        }
        if matches!(self.p.tcx.tys.kind(ty), TyKind::Dyn(..)) {
            self.virtual_calls += 1;
            let concrete = self.dynamic[&addr.meta];
            return self.drop_value(concrete, addr);
        }
        let b = self.p.shim(ty); self.run(&b, vec![Value::Ptr(addr)])
    }
    fn names(&self) -> Vec<&str> { self.events.iter().map(|e| e.at.name.as_str()).collect() }
}

#[test]
fn destructor_before_fields_and_remaining_fields_on_unwind() {
    let p = Provider::new(); let ty = p.adt("WithDrop", vec![]); let b = p.shim(ty);
    for panic in [None, Some(0), Some(1), Some(2)] {
        let mut w = Walker::new(&p); if let Some(n) = panic { w.panic_at.push(n); }
        assert_eq!(w.run(&b, vec![Value::Ptr(Address::new("s", 0))]), if panic.is_some() { Exit::Panic } else { Exit::Return });
        assert_eq!(w.names(), ["s", "s.0", "s.1"]);
        assert_eq!(w.events[0].ty, ty); assert_eq!(w.events[1].ty, p.d()); assert_eq!(w.events[2].ty, p.e());
    }
    let mut w = Walker::new(&p); w.panic_at = vec![0, 1];
    assert_eq!(w.run(&b, vec![Value::Ptr(Address::new("s", 0))]), Exit::Abort);
    assert_eq!(w.names(), ["s", "s.0"]);
}

#[test]
fn partial_struct_move_drops_only_remaining_field() {
    let p = Provider::new(); let ty = p.adt("Pair", vec![]);
    let mut b = body(&p, &[p.tcx.tys.unit, ty, p.d()], 1, vec![
        block(vec![stmt(local(2), Rvalue::Use(Operand::Move(field(&local(1), 0, p.d()))))], drop(local(1), 1)),
        block(vec![], TerminatorKind::Return),
    ]);
    elaborate(&p, &mut b);
    let mut w = Walker::new(&p); assert_eq!(w.run(&b, vec![Value::Object(Address::new("s", 0))]), Exit::Return);
    assert_eq!(w.names(), ["s.1"]);
}

#[test]
fn conditional_move_flag_is_initialized_cleared_and_tested() {
    let p = Provider::new(); let ty = p.adt("Pair", vec![]);
    let mut b = body(&p, &[p.tcx.tys.unit, ty, p.tcx.tys.bool_, p.d()], 2, vec![
        block(vec![], branch(Operand::Copy(local(2)), 2, 1)),
        block(vec![stmt(local(3), Rvalue::Use(Operand::Move(field(&local(1), 0, p.d()))))], go(2)),
        block(vec![], drop(local(1), 3)), block(vec![], TerminatorKind::Return),
    ]);
    elaborate(&p, &mut b);
    assert_eq!(b.local_decls.len(), 5, "one ambiguous field needs one flag");
    for moved in [0, 1] {
        let mut w = Walker::new(&p);
        assert_eq!(w.run(&b, vec![Value::Object(Address::new("s", 0)), Value::Int(moved)]), Exit::Return);
        assert_eq!(w.names(), if moved == 0 { vec!["s.0", "s.1"] } else { vec!["s.1"] });
    }
}

#[test]
fn enum_signed_discriminants_select_the_active_ladder() {
    let p = Provider::new(); let b = p.shim(p.adt("Choice", vec![]));
    for (disc, expected) in [(253, vec!["e.v0.0", "e.v0.1"]), (7, vec!["e.v1.0"]), (20, vec![])] {
        for panic in [false, true] {
            let mut w = Walker::new(&p); w.memory.insert("e.disc".into(), Value::Int(disc)); if panic { w.panic_at.push(0); }
            let out = w.run(&b, vec![Value::Ptr(Address::new("e", 0))]);
            assert_eq!(out, if panic && !expected.is_empty() { Exit::Panic } else { Exit::Return });
            assert_eq!(w.names(), expected);
        }
    }
}

#[test]
fn owned_box_contents_then_deallocation_including_unwind() {
    let p = Provider::new(); let inner = p.adt("Pair", vec![]); let heap = p.adt("Heap", vec![inner]);
    let b = p.shim(heap);
    for panic in [None, Some(0), Some(1)] {
        let mut w = Walker::new(&p); w.heap("b", "p", 0); if let Some(n) = panic { w.panic_at.push(n); }
        assert_eq!(w.run(&b, vec![Value::Ptr(Address::new("b", 0))]), if panic.is_some() { Exit::Panic } else { Exit::Return });
        assert_eq!(w.names(), ["p.0", "p.1", "b"]); assert_eq!(w.events[2].ty, heap);
        assert_eq!(w.frees, ["p"]); assert!(w.live_allocations.is_empty());
    }
}

#[test]
fn moved_box_pointee_retains_exactly_one_deallocation() {
    let p = Provider::new(); let inner = p.adt("Pair", vec![]); let heap = p.adt("Heap", vec![inner]);
    for whole in [false, true] {
        let pointee = project(&local(1), PlaceElem::Deref);
        let moved = if whole { pointee } else { field(&pointee, 0, p.d()) };
        let mut b = body(&p, &[p.tcx.tys.unit, heap, if whole { inner } else { p.d() }], 1, vec![
            block(vec![stmt(local(2), Rvalue::Use(Operand::Move(moved)))], drop(local(1), 1)),
            block(vec![], TerminatorKind::Return),
        ]);
        elaborate(&p, &mut b);
        for panic in [false, true] {
            let mut w = Walker::new(&p); w.heap("b", "p", 0); if panic && !whole { w.panic_at.push(0); }
            assert_eq!(w.run(&b, vec![Value::Object(Address::new("b", 0))]), if panic && !whole { Exit::Panic } else { Exit::Return });
            assert_eq!(w.names(), if whole { vec!["b"] } else { vec!["p.1", "b"] });
            assert_eq!(w.frees, ["p"]); assert!(w.live_allocations.is_empty());
        }
    }
}

#[test]
fn array_shim_delegates_to_slice_and_slice_cleanup_advances() {
    let p = Provider::new(); let array = p.tcx.tys.intern(TyKind::Array(p.d(), p.tcx.tys.const_usize(3)));
    let b = p.shim(array);
    assert_eq!(b.basic_blocks.len(), 2);
    assert!(matches!(b.basic_blocks[0].terminator.as_ref().unwrap().kind, TerminatorKind::Call { .. }));
    for ty in [array, p.tcx.tys.intern(TyKind::Slice(p.d()))] {
        for panic in [None, Some(0), Some(1), Some(2)] {
            let mut w = Walker::new(&p); if let Some(i) = panic { w.panic_at.push(i); }
            assert_eq!(w.drop_value(ty, Address::new("a", 3)), if panic.is_some() { Exit::Panic } else { Exit::Return });
            assert_eq!(w.names(), ["a[0]", "a[1]", "a[2]"]);
        }
    }
    let slice = p.tcx.tys.intern(TyKind::Slice(p.d()));
    let mut w = Walker::new(&p); assert_eq!(w.drop_value(slice, Address::new("a", 0)), Exit::Return); assert!(w.events.is_empty());
}

#[test]
fn box_slice_dyn_and_unsized_tail_keep_metadata_and_cleanup() {
    let p = Provider::new();
    let slice = p.tcx.tys.intern(TyKind::Slice(p.d()));
    let dyn_ty = p.tcx.tys.intern(TyKind::Dyn(p.def("Object"), Default::default(), Default::default(), crate::types::RegionArgsId::EMPTY, crate::types::RegionId::ERASED, 0, Default::default()));
    for inner in [slice, dyn_ty, p.adt("Tail", vec![slice]), p.adt("Tail", vec![dyn_ty])] {
        let tail = matches!(p.tcx.tys.kind(inner), TyKind::Adt(..));
        let is_dyn = inner == dyn_ty || inner == p.adt("Tail", vec![dyn_ty]);
        let meta = if is_dyn { 42 } else { 3 };
        let heap = p.adt("Heap", vec![inner]); let b = p.shim(heap);
        for panic in [false, true] {
            let mut w = Walker::new(&p); w.dynamic.insert(42, p.e()); w.heap("b", "p", meta);
            if panic { w.panic_at.push(if tail { 1 } else { 0 }); }
            assert_eq!(w.run(&b, vec![Value::Ptr(Address::new("b", 0))]), if panic { Exit::Panic } else { Exit::Return });
            let expected = match (tail, is_dyn) {
                (false, false) => vec!["p[0]", "p[1]", "p[2]", "b"],
                (false, true) => vec!["p", "b"],
                (true, false) => vec!["p", "p.0", "p.1[0]", "p.1[1]", "p.1[2]", "b"],
                (true, true) => vec!["p", "p.0", "p.1", "b"],
            };
            assert_eq!(w.names(), expected); assert_eq!(w.events.last().unwrap().ty, heap);
            assert_eq!(w.frees, ["p"]); assert!(w.live_allocations.is_empty());
            if tail { assert_eq!(w.events[0].at.meta, meta, "DST destructor receives metadata"); }
            assert_eq!(w.virtual_calls, if is_dyn { 1 } else { 0 });
        }
    }
    let b = p.shim(dyn_ty);
    assert!(b.basic_blocks.iter().any(|b| matches!(b.terminator.as_ref().unwrap().kind, TerminatorKind::Drop { .. })));
}

#[test]
fn tuple_box_and_vec_shim_and_closure_capture_storage() {
    let p = Provider::new(); let heap = p.adt("Heap", vec![p.d()]); let vec_ty = p.adt("VecLike", vec![]);
    let tuple = p.tcx.tys.intern(TyKind::Tuple(p.tcx.tys.mk_list(&[heap, vec_ty])));
    let sig = p.tcx.tys.intern(TyKind::FnPtr(p.tcx.tys.mk_list(&[]), p.tcx.tys.unit, crate::types::FnHeader::RUST, 0));
    let owner = p.tcx.tys.intern(TyKind::FnDef(p.def("produce"), p.tcx.tys.mk_list(&[]), crate::types::RegionArgsId::EMPTY));
    let closure = p.tcx.tys.intern(TyKind::Closure(0, 123, crate::types::ClosureArgs { parent_args: owner, closure_kind_ty: p.tcx.tys.prim(crate::program::Prim::I8), closure_sig_as_fn_ptr_ty: sig, tupled_upvars_ty: tuple }));
    for ty in [tuple, closure] {
        let mut w = Walker::new(&p); w.heap("t.0", "p", 0);
        assert_eq!(w.drop_value(ty, Address::new("t", 0)), Exit::Return);
        assert_eq!(w.names(), ["p", "t.0", "t.1"]); assert_eq!(w.events[2].ty, vec_ty);
    }
}

#[test]
fn query_semantics_empty_glue_and_failure_leave_body_unchanged() {
    let mut p = Provider::new();
    for ty in [p.tcx.tys.unit, p.tcx.tys.intern(TyKind::Array(p.d(), p.tcx.tys.const_usize(0))), p.adt("Manual", vec![p.d()]), p.adt("Suppress", vec![p.d()])] {
        let b = p.shim(ty); assert_eq!(b.basic_blocks.len(), 1); assert!(matches!(b.basic_blocks[0].terminator.as_ref().unwrap().kind, TerminatorKind::Return));
    }
    let param = p.tcx.tys.intern(TyKind::Param(0));
    assert!(p.needs_drop(param).unwrap()); p.copy.insert(param); assert!(!p.needs_drop(param).unwrap());
    assert!(p.needs_drop(p.tcx.tys.intern(TyKind::Slice(p.d()))).unwrap());
    assert!(p.needs_drop(p.tcx.tys.intern(TyKind::Array(p.d(), p.tcx.tys.const_usize(3)))).unwrap(), "ZST element with Drop still drops");
    let mut b = body(&p, &[p.tcx.tys.unit, p.d()], 1, vec![block(vec![], drop(local(1), 1)), block(vec![], TerminatorKind::Return)]);
    let original = format!("{b:?}"); p.fail = Some(p.d());
    assert_eq!(elaborate_drops(&mut b, &p), Err(DropError::Query(p.d())));
    assert_eq!(format!("{b:?}"), original);
    assert!(matches!(build_drop_shim(&p, p.def("glue"), p.d()), Err(DropError::Query(_))));
}

#[test]
fn conditional_whole_move_and_reinitialization_update_one_flag() {
    let p = Provider::new();
    for reinit in [false, true] {
        let mut statements = vec![stmt(local(3), Rvalue::Use(Operand::Move(local(1))))];
        if reinit { statements.push(stmt(local(1), Rvalue::Use(Operand::Move(local(3))))); }
        let mut b = body(&p, &[p.tcx.tys.unit, p.d(), p.tcx.tys.bool_, p.d()], 2, vec![
            block(vec![], branch(Operand::Copy(local(2)), 2, 1)), block(statements, go(2)),
            block(vec![], drop(local(1), 3)), block(vec![], TerminatorKind::Return),
        ]);
        b.phase = MirPhase::Analysis(AnalysisPhase::PostCleanup);
        elaborate(&p, &mut b);
        assert_eq!(b.local_decls.len(), if reinit { 4 } else { 5 });
        for moved in [0, 1] {
            let mut w = Walker::new(&p);
            assert_eq!(w.run(&b, vec![Value::Object(Address::new("d", 0)), Value::Int(moved)]), Exit::Return);
            assert_eq!(w.names(), if reinit || moved == 0 { vec!["d"] } else { vec![] });
        }
    }
}

#[test]
fn call_return_flag_is_only_set_on_success_and_not_on_a_shared_edge() {
    let p = Provider::new();
    let func = super::elaborate_drop::function(&p, p.def("produce"), vec![], Span::default());
    let mut cleanup_drop = block(vec![], TerminatorKind::Drop { place: local(2), target: BasicBlock(5),
        unwind: UnwindAction::Terminate(UnwindTerminateReason::InCleanup), replace: false }); cleanup_drop.is_cleanup = true;
    let mut resume = block(vec![], TerminatorKind::UnwindResume); resume.is_cleanup = true;
    let mut b = body(&p, &[p.tcx.tys.unit, p.tcx.tys.bool_, p.d()], 1, vec![
        block(vec![], branch(Operand::Copy(local(1)), 2, 1)),
        block(vec![], TerminatorKind::Call { func, args: vec![], destination: local(2), target: Some(BasicBlock(2)),
            unwind: UnwindAction::Cleanup(BasicBlock(4)), call_source: CallSource::Normal, fn_span: Span::default(), arg_spans: vec![] }),
        block(vec![], drop(local(2), 3)), block(vec![], TerminatorKind::Return), cleanup_drop, resume,
    ]);
    elaborate(&p, &mut b);
    let TerminatorKind::Call { target: Some(target), .. } = b.basic_blocks[1].terminator.as_ref().unwrap().kind else { panic!() };
    assert_ne!(target, BasicBlock(2), "critical return edge must be split");
    for called in [0, 1] {
        for panic in [false, true] {
            let mut w = Walker::new(&p); w.produce_panics = panic;
            assert_eq!(w.run(&b, vec![Value::Int(called)]), if called == 1 && panic { Exit::Panic } else { Exit::Return });
            assert_eq!(w.names(), if called == 1 && !panic { vec!["produced"] } else { vec![] });
        }
    }
}

#[test]
fn untracked_index_replacement_is_unconditional() {
    let p = Provider::new(); let array = p.tcx.tys.intern(TyKind::Array(p.d(), p.tcx.tys.const_usize(3)));
    let place = project(&local(1), PlaceElem::Index(Local(2)));
    let mut b = body(&p, &[p.tcx.tys.unit, array, p.tcx.tys.usize_], 2, vec![
        block(vec![], TerminatorKind::Drop { place, target: BasicBlock(1), unwind: UnwindAction::Continue, replace: true }),
        block(vec![], TerminatorKind::Return),
    ]);
    elaborate(&p, &mut b); assert_eq!(b.local_decls.len(), 3);
    let mut w = Walker::new(&p);
    assert_eq!(w.run(&b, vec![Value::Object(Address::new("a", 0)), Value::Int(1)]), Exit::Return);
    assert_eq!(w.names(), ["a[1]"]);
}

#[test]
fn partially_moved_array_uses_reference_partition_order_and_cleanup() {
    let p = Provider::new(); let array = p.tcx.tys.intern(TyKind::Array(p.d(), p.tcx.tys.const_usize(5)));
    let moved = project(&local(1), PlaceElem::ConstantIndex { offset: 2, min_length: 5, from_end: false });
    let mut b = body(&p, &[p.tcx.tys.unit, array, p.d()], 1, vec![
        block(vec![stmt(local(2), Rvalue::Use(Operand::Move(moved)))], drop(local(1), 1)),
        block(vec![], TerminatorKind::Return),
    ]);
    elaborate(&p, &mut b);
    for panic in [None, Some(0), Some(1), Some(2)] {
        let mut w = Walker::new(&p); if let Some(i) = panic { w.panic_at.push(i); }
        assert_eq!(w.run(&b, vec![Value::Object(Address::new("a", 0))]), if panic.is_some() { Exit::Panic } else { Exit::Return });
        // rustc elaborate_drop.rs:1167 reverses the tracked-index/rest-range
        // partitions; each range itself goes through forward slice glue.
        assert_eq!(w.names(), ["a[3]", "a[4]", "a[0]", "a[1]"]);
    }
}

#[test]
fn enum_body_partial_move_and_untracked_variant_fallback() {
    let p = Provider::new(); let ty = p.adt("Choice", vec![]);
    let variant = project(&local(1), PlaceElem::Downcast(None, VariantIdx(0)));
    let mut b = body(&p, &[p.tcx.tys.unit, ty, p.tcx.tys.u8_, p.d()], 1, vec![
        block(vec![stmt(local(2), Rvalue::Discriminant(local(1)))], TerminatorKind::SwitchInt {
            discr: Operand::Copy(local(2)), targets: SwitchTargets { values: vec![253], targets: vec![BasicBlock(1)], otherwise: BasicBlock(2) } }),
        block(vec![stmt(local(3), Rvalue::Use(Operand::Move(field(&variant, 0, p.d()))))], go(2)),
        block(vec![], drop(local(1), 3)), block(vec![], TerminatorKind::Return),
    ]);
    elaborate(&p, &mut b);
    for (disc, expected) in [(253, vec!["e.v0.1"]), (7, vec!["e.v1.0"]), (20, vec![])] {
        let mut w = Walker::new(&p); w.memory.insert("e.disc".into(), Value::Int(disc));
        assert_eq!(w.run(&b, vec![Value::Object(Address::new("e", 0))]), Exit::Return); assert_eq!(w.names(), expected);
    }
}

#[test]
fn dead_drop_and_unreachable_cleanup_are_pruned_without_flags() {
    let p = Provider::new();
    let mut resume = block(vec![], TerminatorKind::UnwindResume); resume.is_cleanup = true;
    let mut cleanup = block(vec![], TerminatorKind::Drop { place: local(1), target: BasicBlock(3),
        unwind: UnwindAction::Terminate(UnwindTerminateReason::InCleanup), replace: false }); cleanup.is_cleanup = true;
    let mut b = body(&p, &[p.tcx.tys.unit, p.d()], 0, vec![
        block(vec![], TerminatorKind::Drop { place: local(1), target: BasicBlock(1), unwind: UnwindAction::Cleanup(BasicBlock(2)), replace: false }),
        block(vec![], TerminatorKind::Return), cleanup, resume,
    ]);
    elaborate(&p, &mut b); assert_eq!(b.local_decls.len(), 2);
    assert!(matches!(b.basic_blocks[0].terminator.as_ref().unwrap().kind, TerminatorKind::Goto { target: BasicBlock(1) }));
    let mut w = Walker::new(&p); assert_eq!(w.run(&b, vec![]), Exit::Return); assert!(w.events.is_empty());
}

#[test]
fn mir_patch_insertions_share_original_locations_and_source_info() {
    let p = Provider::new(); let mut b = p.shim(p.tcx.tys.unit);
    let source = SourceInfo { span: Span { file: 1, lo: 10, hi: 20 }, scope: SourceScope(0) };
    b.basic_blocks[0].terminator.as_mut().unwrap().source_info = source;
    let mut patch = super::patch::MirPatch::new(&b);
    let temp = patch.temp(p.tcx.tys.bool_, source);
    for bits in [0, 1, 0] { patch.statement(Location::default(), StatementKind::Assign(Place::local(temp),
        Rvalue::Use(integer(p.tcx.tys.bool_, bits, 1, source.span)))); }
    let resume = patch.exit(UnwindAction::Continue, source); assert_eq!(patch.exit(UnwindAction::Continue, source), resume);
    patch.apply(&mut b); valid(&p, &b);
    let mut values = Vec::new();
    for s in &b.basic_blocks[0].statements {
        assert_eq!(s.source_info, source);
        let StatementKind::Assign(_, Rvalue::Use(Operand::Constant(c))) = &s.kind else { panic!() };
        let Const::Val(ConstValue::Scalar(Scalar::Int { bits, .. }), _) = c.const_ else { panic!() }; values.push(bits);
    }
    assert_eq!(values, [0, 1, 0]);
}

#[test]
fn conditional_owned_box_partial_move_preserves_master_flag() {
    let p = Provider::new(); let inner = p.adt("Pair", vec![]); let heap = p.adt("Heap", vec![inner]);
    let mut b = body(&p, &[p.tcx.tys.unit, heap, p.tcx.tys.bool_, heap, p.d()], 2, vec![
        block(vec![], branch(Operand::Copy(local(2)), 1, 2)),
        block(vec![stmt(local(3), Rvalue::Use(Operand::Move(local(1))))], go(3)),
        block(vec![stmt(local(4), Rvalue::Use(Operand::Move(field(&project(&local(1), PlaceElem::Deref), 0, p.d()))))], go(3)),
        block(vec![], drop(local(1), 4)), block(vec![], TerminatorKind::Return),
    ]);
    elaborate(&p, &mut b);
    for retained in [0, 1] {
        let mut w = Walker::new(&p); w.heap("b", "p", 0);
        assert_eq!(w.run(&b, vec![Value::Object(Address::new("b", 0)), Value::Int(retained)]), Exit::Return);
        assert_eq!(w.names(), if retained == 0 { vec![] } else { vec!["p.1", "b"] });
        assert_eq!(w.frees, if retained == 0 { vec![] } else { vec!["p"] });
        assert_eq!(w.live_allocations.len(), if retained == 0 { 1 } else { 0 });
    }
    let heap = p.adt("Heap", vec![p.tcx.tys.u8_]); let mut w = Walker::new(&p); w.heap("b", "p", 0);
    assert_eq!(w.drop_value(heap, Address::new("b", 0)), Exit::Return);
    assert_eq!(w.names(), ["b"]); assert_eq!(w.frees, ["p"]);
}

#[test]
fn body_unwind_actions_preserve_cleanup_policy() {
    let p = Provider::new();
    for unwind in [UnwindAction::Continue, UnwindAction::Unreachable, UnwindAction::Terminate(UnwindTerminateReason::Abi)] {
        let mut b = body(&p, &[p.tcx.tys.unit, p.d()], 1, vec![
            block(vec![], TerminatorKind::Drop { place: local(1), target: BasicBlock(1), unwind, replace: false }),
            block(vec![], TerminatorKind::Return),
        ]);
        elaborate(&p, &mut b);
        let TerminatorKind::Drop { unwind: UnwindAction::Cleanup(exit), .. } = b.basic_blocks[0].terminator.as_ref().unwrap().kind else { panic!() };
        match (unwind, &b.basic_blocks[exit.0 as usize].terminator.as_ref().unwrap().kind) {
            (UnwindAction::Continue, TerminatorKind::UnwindResume) |
            (UnwindAction::Unreachable, TerminatorKind::Unreachable) |
            (UnwindAction::Terminate(UnwindTerminateReason::Abi), TerminatorKind::UnwindTerminate(UnwindTerminateReason::Abi)) => (),
            _ => panic!("wrong unwind destination"),
        }
        let mut w = Walker::new(&p); assert_eq!(w.run(&b, vec![Value::Object(Address::new("d", 0))]), Exit::Return);
        if unwind != UnwindAction::Unreachable {
            let mut w = Walker::new(&p); w.panic_at.push(0);
            assert_eq!(w.run(&b, vec![Value::Object(Address::new("d", 0))]), if unwind == UnwindAction::Continue { Exit::Panic } else { Exit::Abort });
        }
    }
}
