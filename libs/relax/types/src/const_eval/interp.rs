// ported from N:rustc_const_eval/src/interpret/{memory.rs,step.rs,operator.rs,cast.rs,discriminant.rs,intern.rs,validity.rs}
// ported from N:rustc_middle/src/mir/interpret/allocation.rs (+ provenance_map.rs, init_mask.rs) @8d1a764
//! The check-mode const-eval interpreter (design: local/agent_state/relax-campaign/suite/consteval.DESIGN.md).
//!
//! `makepad check` only, diagnostics only: a constant's value still comes from ctfe.rs / the JIT. The
//! interpreter runs the constant's MIR with rustc's CTFE memory model -- allocations with an init mask,
//! pointer provenance (pointer-sized entries plus per-byte fragments), liveness, bounds, alignment and
//! the shared-reference bit of a pointer's provenance -- and reports the undefined behaviour rustc's
//! CompileTimeMachine reports (E0080), then interns and validates the final value. An operation not
//! ported yet ends the run as `Unsupported`, which reports nothing.
//!
//! Locals live in allocations of their own (rustc keeps scalar locals as immediates; the verdicts and
//! the root-frame span are the same).

use crate::mir::{self, BasicBlock, Body, Const, ConstValue, Local, Operand, Place, PlaceElem, Rvalue, StatementKind, TerminatorKind};
use crate::program::{DefId, DefKind, Prim, Program};
use crate::tcx::Tcx;
use crate::types::{TyId, TyKind};
use std::collections::HashMap;
use std::rc::Rc;

/// What a check of one constant found.
#[derive(Debug)]
pub enum Outcome {
    Ok,
    /// rustc's error for it: (span in the constant's own body or at the item, message with its code)
    Error(mir::Span, String),
    /// an operation the interpreter does not have yet (`MAKEPAD_RUN_INTERP_TRACE=1` prints it)
    Unsupported(String),
}

enum Stop {
    /// undefined behaviour or an evaluation error: E0080 at the root frame's span
    Ub(String),
    /// a final-value error at the item (intern: no code; validity: E0080)
    Item(String),
    Unsupported(String),
    /// a constant this one reads failed (its own check reports it; rustc: "erroneous constant")
    Referenced,
}
type R<T> = Result<T, Stop>;

fn unsupported<T>(what: impl Into<String>) -> R<T> { Err(Stop::Unsupported(what.into())) }
fn ub<T>(what: impl Into<String>) -> R<T> { Err(Stop::Ub(what.into())) }

/// rustc CtfeProvenance: the allocation and whether the pointer was derived from a shared reference
/// to `Freeze` memory (a write through it is UB).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Prov { alloc: u32, shared: bool }

/// rustc Pointer<Option<Prov>>: `off` is the offset into the allocation, or the address without one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Ptr { prov: Option<Prov>, off: u64 }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind { Stack, Global, Function(DefId), /** the lazy allocation of a static (GlobalAlloc::Static) */ Static(DefId) }

/// allocation.rs Allocation (also the buffer of an operand's value: `Val`).
#[derive(Clone, Debug)]
struct Alloc {
    bytes: Vec<u8>,
    init: Vec<bool>,
    /// pointer-sized provenance at byte offsets (provenance_map.rs ptrs)
    ptrs: Vec<(u64, Prov)>,
    /// per-byte fragments of pointers copied apart (provenance_map.rs PointerFrag): (offset, provenance,
    /// index of the byte in the pointer, the raw bits of the whole pointer it is a byte of)
    frags: Vec<(u64, Prov, u8, u64)>,
    align: u64,
    mutable: bool,
    live: bool,
    kind: Kind,
    /// tcx memory (interned: another constant's or static's, or a body's): accesses go through
    /// machine.rs before_access_global
    global: bool,
    /// a static's allocation not evaluated yet (memory.rs get_global_alloc evaluates it on access)
    lazy: bool,
}
type Val = Alloc;

impl Alloc {
    fn new(size: u64, align: u64, kind: Kind) -> Alloc {
        Alloc { bytes: vec![0; size as usize], init: vec![false; size as usize], ptrs: Vec::new(), frags: Vec::new(), align: align.max(1), mutable: true, live: true, kind, global: false, lazy: false }
    }
    fn size(&self) -> u64 { self.bytes.len() as u64 }
    /// provenance_map.rs clear: the pointers overlapping `lo..hi` go, the bytes of them outside the
    /// range stay as fragments.
    fn clear_prov(&mut self, lo: u64, hi: u64, ps: u64) {
        let mut keep = Vec::new();
        for &(o, p) in &self.ptrs {
            if o + ps <= lo || o >= hi { keep.push((o, p)); continue; }
            let bits = self.ptr_bits(o, ps);
            for i in 0..ps { let b = o + i; if b < lo || b >= hi { self.frags.push((b, p, i as u8, bits)); } }
        }
        self.ptrs = keep;
        self.frags.retain(|f| f.0 < lo || f.0 >= hi);
    }
    fn ptr_bits(&self, at: u64, ps: u64) -> u64 {
        let mut bits = 0u64;
        for i in 0..ps as usize { bits |= (self.bytes[at as usize + i] as u64) << (8 * i); }
        bits
    }
    /// a sub-range as a value of its own (provenance and init mask with it)
    fn slice(&self, lo: u64, len: u64, ps: u64) -> Val {
        let (l, h) = (lo as usize, (lo + len) as usize);
        let mut v = Alloc { bytes: self.bytes[l..h].to_vec(), init: self.init[l..h].to_vec(), ptrs: Vec::new(), frags: Vec::new(), align: 1, mutable: true, live: true, kind: Kind::Stack, global: false, lazy: false };
        for &(o, p) in &self.ptrs {
            if o >= lo && o + ps <= lo + len { v.ptrs.push((o - lo, p)); }
            else if o < lo + len && o + ps > lo {
                let bits = self.ptr_bits(o, ps);
                for i in 0..ps { let b = o + i; if b >= lo && b < lo + len { v.frags.push((b - lo, p, i as u8, bits)); } }
            }
        }
        for &(o, p, i, bits) in &self.frags { if o >= lo && o < lo + len { v.frags.push((o - lo, p, i, bits)); } }
        v
    }
    /// Fragments that make up a whole pointer again -- in order, of the same provenance and the same
    /// pointer bits -- are that pointer (provenance_map.rs provenance_merge_bytes; interning sees one
    /// pointer).
    fn merge_frags(&mut self, ps: u64) {
        let mut i = 0;
        self.frags.sort_by_key(|f| f.0);
        while i < self.frags.len() {
            let (o, p, idx, bits) = self.frags[i];
            if idx == 0 && i + ps as usize <= self.frags.len() && (0..ps as usize).all(|k| { let f = self.frags[i + k]; f.0 == o + k as u64 && f.1 == p && f.2 as usize == k && f.3 == bits }) {
                self.frags.drain(i..i + ps as usize);
                self.ptrs.push((o, p));
            } else { i += 1; }
        }
    }
    fn put(&mut self, at: u64, v: &Val, ps: u64) {
        let n = v.size();
        self.clear_prov(at, at + n, ps);
        let (l, h) = (at as usize, (at + n) as usize);
        self.bytes[l..h].copy_from_slice(&v.bytes);
        self.init[l..h].copy_from_slice(&v.init);
        for &(o, p) in &v.ptrs { self.ptrs.push((o + at, p)); }
        for &(o, p, i, bits) in &v.frags { self.frags.push((o + at, p, i, bits)); }
        if !self.frags.is_empty() { self.merge_frags(ps); }
    }
}

fn val_bytes(bits: u128, size: u64) -> Val {
    let mut v = Alloc::new(size, 1, Kind::Stack);
    for i in 0..size as usize { v.bytes[i] = (bits >> (8 * i)) as u8; v.init[i] = true; }
    v
}

/// A place in memory (rustc MPlaceTy): the pointer, the metadata of an unsized place, its type.
#[derive(Clone, Copy, Debug)]
struct MPlace { ptr: Ptr, meta: Option<u64>, ty: TyId, variant: Option<u32>, /** a field of a packed struct: the alignment the place has (rustc MPlaceTy::align) */ packed: Option<u64> }

struct Frame {
    body: Rc<Body>,
    /// the allocation of each live local
    locals: Vec<Option<u32>>,
    block: BasicBlock,
    stmt: usize,
    /// where the caller wants the return value, and where it continues
    ret: Option<(MPlace, Option<BasicBlock>)>,
    /// this body's global allocations imported into the interpreter's memory
    imported: HashMap<u32, u32>,
    /// the body of a constant (or promoted) itself: its locals outlive it (rustc StackPopCleanup::Root
    /// { cleanup: false }) -- what the value points to is interned; a local ended by StorageDead is not
    root: bool,
}

struct Interp<'a> {
    prog: &'a Program,
    tcx: &'a Tcx,
    lay: crate::layout::Layouts,
    allocs: Vec<Alloc>,
    frames: Vec<Frame>,
    steps: u64,
    /// constants already evaluated by this run: their final allocation (or failure)
    consts: HashMap<(DefId, Vec<TyId>), Result<u32, ()>>,
    ps: u64,
    /// the type of the checked item
    frames_root_ty: Option<TyId>,
    nesting: u32,
    /// the lazy allocation of each static the evaluation points to (one AllocId per static)
    statics: HashMap<DefId, u32>,
    /// machine.rs static_root_ids: the static whose initializer runs (none for a constant)
    static_root: Option<DefId>,
    /// machine.rs can_access_mut_global: a static's initializer may read mutable global memory
    mut_global_ok: bool,
    /// statics whose initializers are running (a cycle is the query system's E0391)
    in_progress: Vec<DefId>,
    /// validity.rs runs with validation_in_progress: no before_alloc_access hook
    validating: bool,
}

/// rustc machine.rs LINT_TERMINATOR_LIMIT: past it the JIT path reports long_running_const_eval.
const STEP_LIMIT: u64 = 2_000_000;

/// Checks the initializer of const/static `def` (non-generic) of a validated crate.
pub fn check_item(prog: &Program, tcx: &Tcx, def: DefId) -> Outcome {
    let is_static = prog.def(def).kind == DefKind::Static;
    let mut it = Interp { prog, tcx, lay: crate::layout::Layouts::new(), allocs: Vec::new(), frames: Vec::new(), steps: 0, consts: HashMap::new(), ps: tcx.tys.ptr_bytes as u64, frames_root_ty: None, nesting: 0,
        statics: HashMap::new(), static_root: is_static.then_some(def), mut_global_ok: is_static, in_progress: if is_static { vec![def] } else { Vec::new() }, validating: false };
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| it.eval_const(def, &[], true, is_static)));
    let _ = &r;
    let span_of_item = || { let d = prog.def(def); let lo = prog.files[d.file as usize].ast.item(d.item).lo; mir::Span { file: d.file, lo, hi: lo } };
    match r {
        Ok(Ok(_)) => Outcome::Ok,
        Ok(Err(Stop::Ub(msg))) => Outcome::Error(it.root_span().unwrap_or_else(span_of_item), format!("E0080: {msg}")),
        Ok(Err(Stop::Item(msg))) => Outcome::Error(span_of_item(), msg),
        Ok(Err(Stop::Unsupported(w))) => Outcome::Unsupported(w),
        Ok(Err(Stop::Referenced)) => Outcome::Ok,
        Err(p) => {
            if crate::preempt::is_cancelled(&*p) { std::panic::resume_unwind(p); }
            Outcome::Unsupported("interpreter panic".into())
        }
    }
}

/// valtrees.rs const_to_valtree_inner over the value of non-generic constant `def`: the type in it
/// that has no value tree -- a union, a raw pointer that is not an integer or is wide, a function
/// pointer, a trait object, a closure. None: the value has one, or the interpreter cannot tell
/// (const_to_pat then judges the type alone).
pub fn valtree_failure(prog: &Program, tcx: &Tcx, def: DefId) -> Option<TyId> {
    let mut it = Interp { prog, tcx, lay: crate::layout::Layouts::new(), allocs: Vec::new(), frames: Vec::new(), steps: 0, consts: HashMap::new(), ps: tcx.tys.ptr_bytes as u64, frames_root_ty: None, nesting: 0,
        statics: HashMap::new(), static_root: None, mut_global_ok: false, in_progress: Vec::new(), validating: false };
    let ty = tcx.const_ty(prog, def)?;
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> R<Option<TyId>> {
        let a = it.eval_const(def, &[], false, false)?;
        it.validating = true;
        it.valtree_walk(MPlace { ptr: Ptr { prov: Some(Prov { alloc: a, shared: true }), off: 0 }, meta: None, ty, variant: None, packed: None }, 0)
    }));
    match r {
        Ok(Ok(bad)) => bad,
        Ok(Err(_)) => None,
        Err(p) => { if crate::preempt::is_cancelled(&*p) { std::panic::resume_unwind(p); } None }
    }
}

impl<'a> Interp<'a> {
    /// valtrees.rs const_to_valtree_inner: the first type without a value tree in the value at `p`.
    fn valtree_walk(&mut self, p: MPlace, depth: u32) -> R<Option<TyId>> {
        if depth > 64 { return unsupported("deep value tree"); }
        let tys = &self.tcx.tys;
        match tys.kind(p.ty).clone() {
            TyKind::FnDef(..) | TyKind::Bool | TyKind::Int(_) | TyKind::Float(_) | TyKind::Char => Ok(None),
            // (only raw pointers that are integers in disguise; never wide ones)
            TyKind::Ptr(_, pointee) => {
                if self.is_fat(pointee)? { return Ok(Some(p.ty)); }
                let v = self.read_place(p)?;
                Ok((!v.ptrs.is_empty() || !v.frags.is_empty()).then_some(p.ty))
            }
            TyKind::FnPtr(..) | TyKind::Dyn(..) | TyKind::Closure(..) | TyKind::Alias(..) => Ok(Some(p.ty)),
            TyKind::Ref(_, pointee, _) => {
                let v = self.read_place(p)?;
                let q = self.ptr_of(&v, 0)?;
                let meta = if self.is_fat(pointee)? { Some(self.int_of(&v.slice(self.ps, self.ps, self.ps))? as u64) } else { None };
                self.valtree_walk(MPlace { ptr: q, meta, ty: pointee, variant: None, packed: None }, depth + 1)
            }
            TyKind::Str => Ok(None),
            TyKind::Slice(_) | TyKind::Array(..) => {
                let n = self.len_of(p)?;
                for i in 0..n { let e = self.index(p, i)?; if let Some(b) = self.valtree_walk(e, depth + 1)? { return Ok(Some(b)); } }
                Ok(None)
            }
            TyKind::Tuple(l) => {
                let l2: Vec<TyId> = tys.list(l).to_vec();
                let lay = self.layout(p.ty)?;
                for (i, t) in l2.iter().enumerate() {
                    let f = MPlace { ptr: Ptr { off: p.ptr.off + lay.fields[i] as u64, ..p.ptr }, meta: None, ty: *t, variant: None, packed: None };
                    if let Some(b) = self.valtree_walk(f, depth + 1)? { return Ok(Some(b)); }
                }
                Ok(None)
            }
            TyKind::Adt(d, args, _) => {
                let Some(adt) = self.tcx.adts.get(&d) else { return unsupported("adt") };
                if adt.is_union { return Ok(Some(p.ty)); }
                let lay = self.layout(p.ty)?;
                let args: Vec<TyId> = tys.list(args).to_vec();
                let (vi, offs) = if adt.is_enum { let v = self.read_variant(p)? as usize; (v, lay.variant_fields.get(v).cloned().unwrap_or_default()) } else { (0, lay.fields.clone()) };
                let fields: Vec<TyId> = adt.variants[vi].fields.iter().map(|f| self.tcx.tys.subst_erasing_regions(f.ty, &args)).collect();
                for (i, t) in fields.into_iter().enumerate() {
                    let Some(&o) = offs.get(i) else { return unsupported("field offset") };
                    if o == u32::MAX { return unsupported("extension field"); }
                    let f = MPlace { ptr: Ptr { off: p.ptr.off + o as u64, ..p.ptr }, meta: None, ty: t, variant: None, packed: None };
                    if let Some(b) = self.valtree_walk(f, depth + 1)? { return Ok(Some(b)); }
                }
                Ok(None)
            }
            _ => Ok(Some(p.ty)),
        }
    }

    // ---- bodies and frames

    /// The MIR of `def` at `args` (instantiated typeck, built, promoted).
    fn body_of(&self, def: DefId, args: &[TyId]) -> R<Rc<Body>> {
        use crate::mir::build::{build_mir, BuildCx, BuildRoot};
        let (prog, tcx) = (self.prog, self.tcx);
        let state = crate::tcx::EvalState::new();
        let cx = crate::tcx::EvalCx::new(prog, tcx, crate::tcx::TypingEnv::fully_monomorphized(prog), &state);
        let body = if args.is_empty() { (*tcx.typeck(prog, def)).clone() } else {
            match crate::typeck::try_instantiate_body(&cx, prog, tcx, def, args) { Ok(b) => b, Err(_) => return unsupported("instantiate") }
        };
        if !body.errors.is_empty() { return unsupported(format!("no typed body for {}", prog.def_path(def))); }
        let env = crate::tcx::TypingEnv { limit: crate::tcx::RecursionLimit::for_item(prog, body.def), param_env: body.param_env.unwrap_or_default(), mode: crate::tcx::TypingMode::borrowck(tcx, prog, body.def) };
        let eval = crate::tcx::EvalCx::new(prog, tcx, env, &state);
        let root = match BuildRoot::new(BuildCx { prog, tcx, env, eval: &eval }, &body) { Ok(r) => r, Err(e) => return unsupported(format!("build root: {e}")) };
        let owner = match prog.def_id_to_node_id.get(&def) {
            Some((_, relax_front::def_collector::NodeId::AnonConst(expr))) if prog.def(def).kind == DefKind::AnonConst => crate::borrowck::body::CheckedBody { owner: def, kind: crate::borrowck::body::BodyKind::AnonConst(*expr) },
            _ => match crate::borrowck::body::CheckedBody::item(prog, def) { Some(o) => o, None => return unsupported("not a body owner") },
        };
        let mut mir = match build_mir(&root, owner) { Ok(m) => m, Err(e) => return unsupported(format!("build: {:?}", e.reason)) };
        let provider = crate::mir::build::driver::BuildProvider { prog, tcx, memo: Default::default() };
        if let Err(e) = crate::mir::transform::mir_promoted(prog, tcx, env, &provider, &mut mir) { return unsupported(format!("promote: {}", e.0)); }
        Ok(Rc::new(mir))
    }

    fn root_span(&self) -> Option<mir::Span> {
        let f = self.frames.first()?;
        let bb = &f.body.basic_blocks[f.block.0 as usize];
        Some(if f.stmt < bb.statements.len() { bb.statements[f.stmt].source_info.span } else { bb.terminator.as_ref()?.source_info.span })
    }

    /// Evaluates a constant (or static) item to its interned allocation. `root`: the item checked
    /// (its final value is validated; a nested constant is validated by its own check).
    fn eval_const(&mut self, def: DefId, args: &[TyId], root: bool, is_static: bool) -> R<u32> {
        let key = (def, args.to_vec());
        if let Some(r) = self.consts.get(&key) { return r.map_err(|_| Stop::Referenced); }
        if self.nesting > 16 { return unsupported("nested constants"); }
        let body = self.body_of(def, args)?;
        if root { self.frames_root_ty = Some(body.local_decls[0].ty); }
        // (a nested constant runs on a stack of its own; the root's failing frames stay for its span)
        let saved = if root { Vec::new() } else { std::mem::take(&mut self.frames) };
        self.nesting += 1;
        let r = self.run_body(body).and_then(|a| if root { self.intern_and_validate(a, is_static).map(|_| a) } else { Ok(a) });
        self.nesting -= 1;
        if !root { self.frames = saved; }
        match r {
            Ok(a) => { if !root { self.intern_nested(a, def, is_static); } self.consts.insert(key, Ok(a)); Ok(a) }
            Err(Stop::Ub(_)) if !root => { self.consts.insert(key, Err(())); Err(Stop::Referenced) }
            Err(e) => Err(e),
        }
    }

    /// Runs a constant-like body (no arguments) to completion; returns the allocation of its value.
    fn run_body(&mut self, body: Rc<Body>) -> R<u32> {
        let ty = body.local_decls[0].ty;
        let (size, align) = self.size_align(ty)?;
        let ret = self.alloc(Alloc::new(size, align, Kind::Global));
        let place = MPlace { ptr: Ptr { prov: Some(Prov { alloc: ret, shared: false }), off: 0 }, meta: None, ty, variant: None, packed: None };
        let depth = self.frames.len();
        self.push_frame(body, Vec::new(), Some((place, None)))?;
        self.frames.last_mut().unwrap().root = true;
        while self.frames.len() > depth { self.step()?; }
        Ok(ret)
    }

    fn alloc(&mut self, a: Alloc) -> u32 { self.allocs.push(a); (self.allocs.len() - 1) as u32 }

    /// intern.rs intern_const_alloc_recursive for a constant or static another one reads: its memory
    /// becomes global with the interned mutability (a constant's is immutable; a static's root is
    /// mutable when it is `static mut` or not `Freeze`, its inner memory only for `static mut`).
    fn intern_nested(&mut self, root: u32, def: DefId, is_static: bool) {
        let (base, inner) = if is_static { let (m, f) = self.static_mutability(def); (m || !f, m) } else { (false, false) };
        let mut todo = vec![(root, base)];
        while let Some((a, m)) = todo.pop() {
            let al = &mut self.allocs[a as usize];
            if al.global { continue; }
            al.global = true;
            al.mutable = m;
            for &(_, p) in &al.ptrs.clone() { todo.push((p.alloc, inner)); }
        }
    }

    /// (`static mut`, the type is `Freeze`)
    fn static_mutability(&self, def: DefId) -> (bool, bool) {
        let d = self.prog.def(def);
        let m = matches!(self.prog.files[d.file as usize].ast.item(d.item).kind, relax_front::ast::ItemKind::Static(true, _, _));
        let freeze = self.tcx.const_ty(self.prog, def).is_some_and(|t| self.is_freeze(t));
        (m, freeze)
    }

    /// The lazy allocation of static `def` (GlobalAlloc::Static): its size and alignment are the
    /// type's; it is evaluated when accessed.
    fn static_alloc(&mut self, def: DefId) -> R<u32> {
        if let Some(&a) = self.statics.get(&def) { return Ok(a); }
        if self.prog.def(def).kind != DefKind::Static { return unsupported("extern static"); }
        if crate::mir::transform::promote_consts::is_thread_local_static(self.prog, def) { return unsupported("thread-local static"); }
        let Some(ty) = self.tcx.const_ty(self.prog, def) else { return unsupported("static type") };
        let (size, align) = self.size_align(ty)?;
        let (m, freeze) = self.static_mutability(def);
        let mut a = Alloc::new(size, align, Kind::Static(def));
        a.global = true;
        a.lazy = true;
        a.mutable = m || !freeze;
        let a = self.alloc(a);
        self.statics.insert(def, a);
        Ok(a)
    }

    /// memory.rs get_global_alloc of a static: eval_static_initializer (its own evaluation, with
    /// its own static_root_ids and mutable-global access), the result under the lazy id.
    fn resolve_static(&mut self, a: u32) -> R<()> {
        let Kind::Static(def) = self.allocs[a as usize].kind else { return Ok(()) };
        if self.in_progress.contains(&def) { return unsupported("static cycle"); }
        let saved = (self.static_root, self.mut_global_ok, self.validating);
        self.static_root = Some(def);
        self.mut_global_ok = true;
        self.validating = false;
        self.in_progress.push(def);
        let r = self.eval_const(def, &[], false, true);
        self.in_progress.pop();
        (self.static_root, self.mut_global_ok, self.validating) = saved;
        let v = r?;
        let src = self.allocs[v as usize].clone();
        if src.size() != self.allocs[a as usize].size() { return unsupported("static size"); }
        let dst = &mut self.allocs[a as usize];
        dst.bytes = src.bytes;
        dst.init = src.init;
        dst.ptrs = src.ptrs;
        dst.frags = src.frags;
        dst.lazy = false;
        Ok(())
    }

    fn push_frame(&mut self, body: Rc<Body>, args: Vec<Val>, ret: Option<(MPlace, Option<BasicBlock>)>) -> R<()> {
        if self.frames.len() > 64 { return unsupported("deep recursion"); }
        let n = body.local_decls.len();
        let mut locals = vec![None; n];
        // the return place and the arguments live from the start; the rest from StorageLive (or
        // the start, when the body has no storage statements for them)
        let has_storage: Vec<bool> = {
            let mut v = vec![false; n];
            for bb in &body.basic_blocks { for s in &bb.statements { if let StatementKind::StorageLive(l) | StatementKind::StorageDead(l) = s.kind { v[l.0 as usize] = true; } } }
            v
        };
        for i in 0..n {
            if i == 0 || i <= body.arg_count as usize || !has_storage[i] {
                let (size, align) = self.size_align(body.local_decls[i].ty)?;
                locals[i] = Some(self.alloc(Alloc::new(size, align, Kind::Stack)));
            }
        }
        for (i, v) in args.into_iter().enumerate() {
            let a = locals[i + 1].unwrap();
            if v.size() != self.allocs[a as usize].size() { return unsupported("argument size"); }
            let ps = self.ps; self.allocs[a as usize].put(0, &v, ps);
        }
        self.frames.push(Frame { body, locals, block: BasicBlock(0), stmt: 0, ret, imported: HashMap::new(), root: false });
        Ok(())
    }

    fn pop_frame(&mut self) -> R<()> {
        let f = self.frames.pop().unwrap();
        let rv = { let a = f.locals[0].unwrap(); let s = self.allocs[a as usize].size(); self.allocs[a as usize].slice(0, s, self.ps) };
        if !f.root { for l in f.locals.iter().flatten() { self.allocs[*l as usize].live = false; } }
        if let Some((place, next)) = f.ret {
            self.write_val(place, &rv)?;
            if let Some(next) = next { let top = self.frames.last_mut().unwrap(); top.block = next; top.stmt = 0; }
        }
        Ok(())
    }

    // ---- layouts

    fn layout(&self, ty: TyId) -> R<&'static crate::layout::Layout> {
        let tys = &self.tcx.tys;
        if tys.flags(ty).intersects(crate::types::TypeFlags::HAS_TY_PARAM) || tys.has_infer(ty) || tys.has_error(ty) { return unsupported(format!("generic layout of {} in {}", crate::typeck::ty_to_string(false, self.prog, self.tcx, ty), self.frames.last().map_or(String::new(), |f| self.prog.def_path(f.body.source.owner)))); }
        if matches!(tys.kind(ty), TyKind::Slice(_) | TyKind::Str | TyKind::Dyn(..)) { return unsupported("unsized layout"); }
        Ok(self.lay.of(&crate::layout::LayoutCx::new(self.prog), self.tcx, ty))
    }
    fn size_align(&self, ty: TyId) -> R<(u64, u64)> {
        let l = self.layout(ty)?;
        // (a value this large is the JIT path's to judge: E0080 too big / out of memory)
        if l.size > 1 << 28 { return unsupported("huge value"); }
        Ok((l.size as u64, l.align as u64))
    }

    fn is_fat(&self, pointee: TyId) -> R<bool> {
        if !self.tcx.tys.flags(pointee).intersects(crate::types::TypeFlags::HAS_TY_PARAM) && matches!(self.tcx.tys.kind(pointee), TyKind::Adt(..)) && self.lay.is_unsized(&crate::layout::LayoutCx::new(self.prog), self.tcx, pointee) { return unsupported("unsized struct pointee"); }
        Ok(match self.tcx.tys.kind(pointee) {
            TyKind::Slice(_) | TyKind::Str => true,
            TyKind::Dyn(..) => return unsupported("dyn pointer"),
            _ => false,
        })
    }

    // ---- memory (memory.rs)

    fn alloc_name(&self, a: u32) -> String { format!("ALLOC{a}") }

    /// memory.rs check_and_deref_ptr: the allocation `ptr` may access `size` bytes of; then
    /// get_global_alloc / before_access_global / before_alloc_access for global memory.
    fn check_access(&mut self, ptr: Ptr, size: u64, align: u64, write: bool) -> R<Option<u32>> {
        let a = self.check_bounds(ptr, size, align)?;
        let Some(ai) = a else { return Ok(None) };
        if let Kind::Static(d) = self.allocs[ai as usize].kind {
            // machine.rs before_alloc_access: the static being initialized (a zero-sized write manifests nothing)
            if !self.validating && Some(d) == self.static_root && (size > 0 || !write) { return ub("encountered static that tried to access itself during initialization"); }
            // (a static's initializer evaluates every static it touches: the query detects cycles)
            if self.allocs[ai as usize].lazy && (size > 0 || self.static_root.is_some()) { self.resolve_static(ai)?; }
        }
        let al = &self.allocs[ai as usize];
        if size > 0 && al.global {
            if write {
                if al.mutable { return ub("modifying a static's initial value from another static's initializer"); }
                return ub(format!("writing to {} which is read-only", self.alloc_name(ai)));
            }
            if al.mutable && !self.mut_global_ok { return ub("constant accesses mutable global memory"); }
        }
        Ok(a)
    }

    fn check_bounds(&self, ptr: Ptr, size: u64, align: u64) -> R<Option<u32>> {
        let Some(p) = ptr.prov else {
            if size == 0 && ptr.off != 0 { return Ok(None); }
            return ub(format!("memory access failed: attempting to access {size} byte{}, but got {:#x}[noalloc] which is a dangling pointer (it has no provenance)", if size == 1 { "" } else { "s" }, ptr.off));
        };
        let a = &self.allocs[p.alloc as usize];
        if !a.live { return ub(format!("memory access failed: {} has been freed, so this pointer is dangling", self.alloc_name(p.alloc))); }
        if let Kind::Function(_) = a.kind { if size > 0 { return ub(format!("accessing {} which contains a function", self.alloc_name(p.alloc))); } }
        if ptr.off.checked_add(size).is_none_or(|e| e > a.size()) {
            let left = a.size().saturating_sub(ptr.off);
            return ub(format!("memory access failed: attempting to access {size} byte{}, but got {} which is only {left} byte{} from the end of the allocation", if size == 1 { "" } else { "s" }, self.ptr_str(ptr), if left == 1 { "" } else { "s" }));
        }
        // memory.rs check_misalign: the allocation's alignment and the offset into it
        if align > 1 {
            if a.align < align { return ub(format!("accessing memory based on pointer with alignment {}, but alignment {align} is required", a.align)); }
            if ptr.off % align != 0 { let has = 1u64 << ptr.off.trailing_zeros().min(63); return ub(format!("accessing memory with alignment {has}, but alignment {align} is required")); }
        }
        Ok(Some(p.alloc))
    }

    fn ptr_str(&self, p: Ptr) -> String {
        match p.prov { Some(pr) if p.off == 0 => self.alloc_name(pr.alloc), Some(pr) => format!("{}+{:#x}", self.alloc_name(pr.alloc), p.off), None => format!("{:#x}[noalloc]", p.off) }
    }

    fn read(&mut self, ptr: Ptr, size: u64, align: u64) -> R<Val> {
        match self.check_access(ptr, size, align, false)? {
            None => Ok(Alloc::new(size, 1, Kind::Stack)),
            Some(a) => Ok(self.allocs[a as usize].slice(ptr.off, size, self.ps)),
        }
    }

    /// machine.rs before_memory_write + memory.rs get_mut: the target is writable.
    fn write(&mut self, ptr: Ptr, v: &Val, align: u64) -> R<()> {
        let size = v.size();
        let Some(a) = self.check_access(ptr, size, align, true)? else { return Ok(()) };
        if size == 0 { return Ok(()); }
        // (memory.rs get_alloc_raw_mut: read-only memory; then machine.rs before_memory_write)
        if !self.allocs[a as usize].mutable { return ub(format!("writing to {} which is read-only", self.alloc_name(a))); }
        if ptr.prov.unwrap().shared { return ub("writing through a pointer that was derived from a shared (immutable) reference"); }
        let ps = self.ps;
        self.allocs[a as usize].put(ptr.off, v, ps);
        Ok(())
    }

    fn read_place(&mut self, p: MPlace) -> R<Val> {
        let (size, align) = self.size_align(p.ty)?;
        let align = p.packed.map_or(align, |a| a.min(align));
        self.read(p.ptr, size, align)
    }
    fn write_val(&mut self, p: MPlace, v: &Val) -> R<()> {
        let (size, align) = self.size_align(p.ty)?;
        let align = p.packed.map_or(align, |a| a.min(align));
        if size != v.size() { return unsupported("write of a value of another size"); }
        self.write(p.ptr, v, align)
    }

    // ---- scalars of values (allocation.rs read_scalar)

    fn uninit_err(&self, v: &Val, what: &str) -> Stop {
        let first = v.init.iter().position(|b| !b).unwrap_or(0);
        let last = v.init.iter().rposition(|b| !b).map_or(first, |x| x + 1);
        Stop::Ub(format!("reading memory {what}[{:#x}..{:#x}], but memory is uninitialized at [{first:#x}..{last:#x}], and this operation requires initialized memory", 0, v.size()))
    }

    /// A pointer-to-integer error (ReadPointerAsInt, ReadPartialPointer, exposing a pointer). Relax's std
    /// turns pointers into integers where rustc's std uses provenance-preserving intrinsics (its
    /// `wrapping_byte_add`, `addr`, `is_null`), so inside Relax's std such an error is not the
    /// program's: the run ends as Unsupported there.
    pub(super) fn ptr_int_ub<T>(&self, msg: &str) -> R<T> {
        if let Some(f) = self.frames.last() {
            let krate = self.prog.def(f.body.source.owner).krate;
            if matches!(self.prog.syms.str(self.prog.crates[krate as usize].name), "core" | "alloc" | "std") { return unsupported(format!("std: {msg}")); }
        }
        ub(msg.to_string())
    }

    /// allocation.rs read_scalar without read_provenance: a typed read of an integer, float, bool or
    /// char finds no provenance in its bytes (ReadPointerAsInt). `program`: the read is the program's
    /// own (a `ptr_read` of its memory), not one of Relax's std pointer casts.
    pub(super) fn check_int_read(&self, v: &Val, ty: TyId, program: bool) -> R<()> {
        if !matches!(self.tcx.tys.kind(ty), TyKind::Int(_) | TyKind::Float(_) | TyKind::Bool | TyKind::Char) || (v.ptrs.is_empty() && v.frags.is_empty()) { return Ok(()); }
        if program { return ub("unable to turn pointer into integer"); }
        self.ptr_int_ub("unable to turn pointer into integer")
    }

    /// An integer: initialised, no provenance.
    fn int_of(&self, v: &Val) -> R<u128> {
        if v.init.iter().any(|b| !b) { return Err(self.uninit_err(v, "")); }
        if !v.ptrs.is_empty() || !v.frags.is_empty() {
            if v.ptrs.is_empty() || v.size() != self.ps { return self.ptr_int_ub("unable to read parts of a pointer from memory"); }
            return self.ptr_int_ub("unable to turn pointer into integer");
        }
        let mut bits = 0u128;
        for (i, b) in v.bytes.iter().enumerate() { bits |= (*b as u128) << (8 * i); }
        Ok(bits)
    }

    /// A thin pointer at the start of `v` (provenance_map.rs read_ptr: one pointer-sized entry, or
    /// fragments of one pointer in order).
    fn ptr_of(&self, v: &Val, at: u64) -> R<Ptr> {
        let ps = self.ps;
        if v.init[at as usize..(at + ps) as usize].iter().any(|b| !b) { return Err(self.uninit_err(v, "")); }
        let mut off = 0u64;
        for i in 0..ps as usize { off |= (v.bytes[at as usize + i] as u64) << (8 * i); }
        if let Some(&(_, p)) = v.ptrs.iter().find(|x| x.0 == at) { return Ok(Ptr { prov: Some(p), off }); }
        let overlapping = v.ptrs.iter().any(|&(o, _)| o < at + ps && o + ps > at);
        let frags: Vec<_> = v.frags.iter().filter(|f| f.0 >= at && f.0 < at + ps).collect();
        if !overlapping && frags.is_empty() { return Ok(Ptr { prov: None, off }); }
        // (provenance_map.rs read_ptr: every byte a fragment at its own index, all of one pointer:
        // the same provenance and the same pointer bits)
        if !overlapping && frags.len() == ps as usize && frags.iter().all(|f| f.1 == frags[0].1 && f.3 == frags[0].3 && (f.0 - at) == f.2 as u64) { return Ok(Ptr { prov: Some(frags[0].1), off }); }
        self.ptr_int_ub("unable to read parts of a pointer from memory")
    }

    fn ptr_val(&self, p: Ptr, meta: Option<u64>) -> Val {
        let ps = self.ps;
        let mut v = val_bytes(p.off as u128, ps);
        if let Some(pr) = p.prov { v.ptrs.push((0, pr)); }
        if let Some(m) = meta { let mv = val_bytes(m as u128, ps); let mut w = Alloc::new(2 * ps, 1, Kind::Stack); w.put(0, &v, ps); w.put(ps, &mv, ps); return w; }
        v
    }

    fn bool_of(&self, v: &Val) -> R<bool> {
        let b = self.int_of(v)?;
        match b { 0 => Ok(false), 1 => Ok(true), _ => ub(format!("interpreting an invalid 8-bit value as a bool: {b:#04x}")) }
    }

    fn int_ty(&self, ty: TyId) -> Option<(u64, bool)> {
        match self.tcx.tys.kind(ty) {
            TyKind::Int(p) => { let (bytes, signed) = self.tcx.tys.int_info(*p); Some((bytes as u64, signed)) }
            TyKind::Char => Some((4, false)),
            TyKind::Bool => Some((1, false)),
            _ => None,
        }
    }

    // ---- places (place.rs, projection.rs)

    fn local_place(&self, l: Local) -> R<MPlace> {
        let f = self.frames.last().unwrap();
        let Some(a) = f.locals[l.0 as usize] else { return ub("accessing a dead local variable") };
        if !self.allocs[a as usize].live { return ub("accessing a dead local variable"); }
        Ok(MPlace { ptr: Ptr { prov: Some(Prov { alloc: a, shared: false }), off: 0 }, meta: None, ty: f.body.local_decls[l.0 as usize].ty, variant: None, packed: None })
    }

    fn eval_place(&mut self, place: &Place) -> R<MPlace> {
        let mut p = self.local_place(place.local)?;
        for elem in &place.projection {
            p = match elem {
                PlaceElem::Deref => {
                    let pointee = match self.tcx.tys.kind(p.ty) {
                        TyKind::Ref(_, t, _) | TyKind::Ptr(_, t) => *t,
                        _ => return unsupported("deref of a smart pointer"),
                    };
                    let v = self.read_place(p)?;
                    let ptr = self.ptr_of(&v, 0)?;
                    let meta = if self.is_fat(pointee)? { Some(self.int_of(&v.slice(self.ps, self.ps, self.ps))? as u64) } else { None };
                    MPlace { ptr, meta, ty: pointee, variant: None, packed: None }
                }
                PlaceElem::Field(f, fty) => {
                    let l = self.layout(p.ty)?;
                    let offs = match p.variant { Some(v) if !l.variant_fields.is_empty() => &l.variant_fields[v as usize], _ => &l.fields };
                    let Some(&o) = offs.get(f.0 as usize) else { return unsupported("field offset") };
                    if o == u32::MAX { return unsupported("extension field"); }
                    let packed = match self.tcx.tys.kind(p.ty) { TyKind::Adt(d, ..) => self.tcx.adts.get(d).map_or(0, |a| a.repr_packed as u64), _ => 0 };
                    let packed = if packed > 0 { Some(p.packed.map_or(packed, |q| q.min(packed))) } else { p.packed };
                    MPlace { ptr: Ptr { off: p.ptr.off + o as u64, ..p.ptr }, meta: None, ty: *fty, variant: None, packed }
                }
                PlaceElem::Downcast(_, v) => MPlace { variant: Some(v.0), ..p },
                PlaceElem::Index(l) => {
                    let iv = { let lp = self.local_place(*l)?; self.read_place(lp)? };
                    let i = self.int_of(&iv)? as u64;
                    self.index(p, i)?
                }
                PlaceElem::ConstantIndex { offset, min_length: _, from_end } => {
                    let len = self.len_of(p)?;
                    let i = if *from_end { len - offset } else { *offset };
                    self.index(p, i)?
                }
                PlaceElem::OpaqueCast(t) | PlaceElem::Subtype(t) => MPlace { ty: *t, ..p },
                PlaceElem::Subslice { .. } => return unsupported("subslice"),
            };
        }
        Ok(p)
    }

    fn len_of(&self, p: MPlace) -> R<u64> {
        match self.tcx.tys.kind(p.ty) {
            TyKind::Array(_, n) => n.try_to_target_usize(&self.tcx.tys).map_or_else(|| unsupported("array length"), Ok),
            TyKind::Slice(_) | TyKind::Str => p.meta.map_or_else(|| unsupported("slice length"), Ok),
            _ => unsupported("length of a non-array"),
        }
    }

    fn index(&self, p: MPlace, i: u64) -> R<MPlace> {
        let elem = match self.tcx.tys.kind(p.ty) { TyKind::Array(e, _) | TyKind::Slice(e) => *e, TyKind::Str => self.tcx.tys.u8_, _ => return unsupported("index") };
        let len = self.len_of(p)?;
        if i >= len { return ub(format!("indexing out of bounds: the len is {len} but the index is {i}")); }
        let (size, _) = self.size_align(elem)?;
        Ok(MPlace { ptr: Ptr { off: p.ptr.off + i * size, ..p.ptr }, meta: None, ty: elem, variant: None, packed: p.packed })
    }

    // ---- operands and constants

    fn eval_operand(&mut self, op: &Operand) -> R<(Val, TyId)> {
        match op {
            Operand::Copy(pl) | Operand::Move(pl) => {
                let p = self.eval_place(pl)?;
                let v = self.read_place(p)?;
                // a typed copy of a scalar (rustc read_immediate of BackendRepr::Scalar/ScalarPair) must be
                // initialised; aggregates are copied as memory
                if self.is_scalar_ty(p.ty) && v.init.iter().any(|b| !b) {
                    let name = match p.ptr.prov { Some(pr) => format!("at {}", self.alloc_name(pr.alloc)), None => String::new() };
                    return Err(self.uninit_err(&v, &name));
                }
                // ... and a pointer is read with its provenance (allocation.rs read_scalar with
                // read_provenance: provenance_map.rs read_ptr rejects bytes of different pointers)
                if matches!(self.tcx.tys.kind(p.ty), TyKind::Ref(..) | TyKind::Ptr(..) | TyKind::FnPtr(..)) && v.size() >= self.ps { self.ptr_of(&v, 0)?; }
                self.check_int_read(&v, p.ty, false)?;
                Ok((v, p.ty))
            }
            Operand::Constant(c) => { let t = c.const_.ty(&self.tcx.tys); Ok((self.eval_const_operand(&c.const_)?, t)) }
            Operand::RuntimeChecks(_) => Ok((val_bytes(0, 1), self.tcx.tys.bool_)),
        }
    }

    fn is_scalar_ty(&self, ty: TyId) -> bool {
        matches!(self.tcx.tys.kind(ty), TyKind::Int(_) | TyKind::Float(_) | TyKind::Bool | TyKind::Char | TyKind::Ref(..) | TyKind::Ptr(..) | TyKind::FnPtr(..))
    }

    fn import(&mut self, id: mir::AllocId) -> R<u32> {
        if let Some(&a) = self.frames.last().unwrap().imported.get(&id.0) { return Ok(a); }
        let body = self.frames.last().unwrap().body.clone();
        let a = match body.allocs.global(id) {
            Some(mir::GlobalAlloc::Memory(m)) => {
                let mut a = Alloc::new(m.bytes.len() as u64, m.align, Kind::Global);
                a.global = true;
                a.bytes = m.bytes.clone();
                a.init = if m.init_mask.len() == m.bytes.len() { m.init_mask.clone() } else { vec![true; m.bytes.len()] };
                a.mutable = m.mutability == mir::Mutability::Mut;
                let mut imported_ptrs = Vec::new();
                for &(o, p) in &m.provenance { imported_ptrs.push((o, p.alloc)); }
                let ai = self.alloc(a);
                for (o, inner) in imported_ptrs { let t = self.import(inner)?; self.allocs[ai as usize].ptrs.push((o, Prov { alloc: t, shared: false })); }
                if !m.provenance_fragments.is_empty() { return unsupported("imported pointer fragments"); }
                ai
            }
            Some(mir::GlobalAlloc::Function { def, .. }) => { let mut a = Alloc::new(0, 1, Kind::Function(*def)); a.mutable = false; self.alloc(a) }
            Some(mir::GlobalAlloc::Static(d)) => self.static_alloc(*d)?,
            None => return unsupported("unknown allocation"),
        };
        self.frames.last_mut().unwrap().imported.insert(id.0, a);
        Ok(a)
    }

    fn scalar_val(&mut self, s: &mir::Scalar, ty: TyId) -> R<Val> {
        match *s {
            mir::Scalar::Int { bits, size } => Ok(val_bytes(bits, size as u64)),
            mir::Scalar::Ptr { provenance, offset } => {
                let a = self.import(provenance.alloc)?;
                let _ = ty;
                Ok(self.ptr_val(Ptr { prov: Some(Prov { alloc: a, shared: provenance.shared_ref() }), off: offset }, None))
            }
        }
    }

    fn eval_const_operand(&mut self, c: &Const) -> R<Val> {
        match c {
            Const::Val(v, ty) => match v {
                ConstValue::Scalar(s) => self.scalar_val(s, *ty),
                ConstValue::ZeroSized => Ok(Alloc::new(0, 1, Kind::Stack)),
                ConstValue::Slice { alloc, meta } => { let a = self.import(*alloc)?; Ok(self.ptr_val(Ptr { prov: Some(Prov { alloc: a, shared: true }), off: 0 }, Some(*meta))) }
                ConstValue::Indirect { alloc, offset } => {
                    let a = self.import(*alloc)?;
                    let (size, _) = self.size_align(*ty)?;
                    Ok(self.allocs[a as usize].slice(*offset, size, self.ps))
                }
            },
            Const::Ty(ty, ct) => {
                let (size, _) = self.size_align(*ty)?;
                let Some(leaf) = ct.try_to_value(&self.tcx.tys).and_then(|v| v.valtree.try_to_leaf()) else { return unsupported("type-level constant") };
                Ok(val_bytes(leaf.to_bits(size as u8), size))
            }
            Const::Unevaluated { def, args, promoted, ty } => {
                let targs: Vec<TyId> = args.iter().filter_map(|a| match a { crate::consts::GenericArg::Type(t) => Some(*t), _ => None }).collect();
                let a = match promoted {
                    Some(pi) => {
                        let body = self.frames.last().unwrap().body.clone();
                        let Some(pb) = body.promoted.get(pi.0 as usize) else { return unsupported("promoted") };
                        let pb = Rc::new(pb.clone());
                        let a = self.run_body(pb)?;
                        self.allocs[a as usize].mutable = false;
                        a
                    }
                    None => {
                        let k = self.prog.def(*def).kind;
                        if !matches!(k, DefKind::Const | DefKind::AssocConst | DefKind::AnonConst) { return unsupported("unevaluated non-const"); }
                        let saved = std::mem::take(&mut self.frames);
                        let r = self.eval_const(*def, &targs, false, false);
                        self.frames = saved;
                        r?
                    }
                };
                let (size, _) = self.size_align(*ty)?;
                Ok(self.allocs[a as usize].slice(0, size, self.ps))
            }
        }
    }
}

mod exec;
mod finish;
