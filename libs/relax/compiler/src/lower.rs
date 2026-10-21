// ported from N:rustc_mir_build/src/builder/expr/as_rvalue.rs @8d1a764 (arithmetic checks; round-30 RIR design)
// ported from N:rustc_mir_transform/src/shim.rs @8d1a764
// ported from N:rustc_ty_utils/src/abi.rs @8d1a764
//! Lowering of one type-checked function instance (typeck::Body) to RIR.
//! Small aggregates live in registers as their scalar leaves; everything else
//! lives in stack slots. Locals whose address is taken live in slots.

include!("rt_abi.rs");
use crate::types::AliasTyKind;
use crate::types::{AliasTy};
use relax_types::next_solver::TraitRef;
use relax_types::consts::GenericArg;
use crate::lang_items::LangItem;
use crate::lit::{unescape_str, unescape_bytes, strip_paren, is_assignee_pattern, parse_fmt, FmtPiece, FmtSpec};
use crate::ast::*;
use crate::jit::{FnKey, Unit, ShimInstance};
use relax_types::instance::{Instance, InstanceKind, ShimKind};
use crate::layout::{Layout, Leaf};
use crate::program::{DefId, DefKind, Prim};
use relax_backend::rir::*;
use crate::typeck::{Body, Coerce, Res};
use crate::types::{TyId, TyKind};
use crate::fxhash::HashMap;

mod asm;
mod match_tree;
mod simd;

/// vtable layout: [drop_in_place (0 when no drop), size, align, methods...]
pub const VT_DROP: u32 = 0;
pub const VT_SIZE: u32 = 1;
pub const VT_ALIGN: u32 = 2;
pub const VT_METHODS: u32 = 3;

#[derive(Clone)]
pub enum Val {
    /// scalar leaves in registers
    L(Vec<VReg>),
    /// aggregate in memory at base + offset
    M(VReg, i32),
}

#[derive(Clone)]
pub enum Place {
    Regs(Vec<VReg>),
    Mem(VReg, i32),
}

/// makepad inspect's probes (inspect/probes.rs): a call of probe fn `probe_slot` lowered into slot
/// `slot`'s body before the first statement that starts on `line`, with the address of each
/// named local or parameter in scope there (`params`: (name, T) for a probe parameter `&T`).
/// Set (thread-local) around a re-lowering of the slot on the owner thread; what each point
/// found goes to PROBE_RESULTS.
#[derive(Clone)]
pub struct ProbePoint {
    pub slot: u32,
    pub line: u32,
    pub probe_slot: u32,
    pub params: Vec<(String, TyId)>,
}

crate::gen_local! {
    pub static PROBE_POINTS: std::cell::RefCell<Vec<ProbePoint>>;
    pub static PROBE_RESULTS: std::cell::RefCell<Vec<(u32, Result<(), String>)>>;
}

/// A place matched by a slice pattern (see Lcx::slice_pat).
/// A recorded method resolution with this instance's generic args.
struct MonoMethod {
    def: DefId,
    derefs: usize,
    autoref: u8,
    args: Vec<TyId>,
}

struct SlicePat {
    base: VReg,
    /// dynamic length (slices); None for arrays (length `n`)
    len: Option<VReg>,
    n: u64,
    elem: TyId,
    stride: u32,
    rest: Option<usize>,
    fixed: usize,
}

#[derive(Clone)]
enum LocalSt {
    Regs(Vec<VReg>),
    Slot(VReg),
}

/// drop_tree's leaf-by-leaf state (drop_tree_top)
#[derive(Clone)]
struct TreeCx {
    entries: Vec<(Vec<(u32, u32)>, VReg)>,
    addr: VReg,
    t: TyId,
    /// leaves before this index are not emitted (a pad: they were dropped already)
    skip: usize,
    counter: usize,
    /// where unwinding goes after the remaining leaves
    outer: Option<u32>,
    building: bool,
}

/// Lowering state of a coroutine body (an async block / async fn poll function).
struct Coro {
    /// the coroutine type
    ct: TyId,
    /// the coroutine state (`self` of poll: upvars, flags, state word, state area)
    env: VReg,
    /// the poll's `&mut Context` (null: cancel -- the coroutine is being dropped)
    cx: VReg,
    /// offset of the state word in the state, and its width
    frame_off: i32,
    tag: Mem,
    /// size of the upvars + state word (the state area follows)
    up_size: u32,
    /// the slots of awaited futures (address vreg, suspension point): live only across their
    /// own suspension point
    await_slots: Vec<(VReg, usize)>,
    /// slots made by an await's poll (its result): dead at its suspension point (rustc: the
    /// poll's result is dead before the yield)
    poll_scratch: Vec<(u32, u32, usize)>,
    /// call arguments that are a call's own memory: a result slot (sret) or the private copy of
    /// a by-value aggregate -- the callee cannot keep their address
    call_private: crate::fxhash::HashSet<VReg>,
    call_sret: crate::fxhash::HashSet<VReg>,
    /// Poll<Output> (the poll function's return type)
    poll_ret: TyId,
    /// suspension points: (block ending in the Pending return, block to resume at, block
    /// running the drops when the coroutine is dropped there)
    suspends: Vec<(u32, u32, u32)>,
    /// the shared end of the cancel paths (marks the coroutine returned, then pending_ret)
    cancel_end: Option<u32>,
    /// returns Poll::Pending (nothing else)
    pending_ret: u32,
    /// panics: polled after completion / after a panic
    after_done: u32,
    after_panic: u32,
}

/// One drop of a scope exit (exit_actions): a local by its drop flag, or a temporary (by its
/// flag when it has one).
#[derive(Clone, PartialEq)]
enum DropAct {
    Local(u32),
    Temp(VReg, TyId, Option<VReg>),
}

/// A move out of a matched place (MatchRoot), recorded until a guard holds.
enum PendingMove {
    Field(u32, Vec<(u32, u32)>),
    Local(u32),
    TempPart(VReg, Vec<(u32, u32)>),
    Temp(VReg),
}

#[derive(Clone, Copy)]
enum MatchRoot {
    Local(u32),
    /// an rvalue scrutinee, or a field of one: the temporary's drop flag
    Temp(VReg),
}

/// The temporary owning a scrutinee (MatchRoot::Temp): its address and type, and the
/// scrutinee's field path in it (empty: the scrutinee is the temporary).
struct ScrutTemp {
    addr: VReg,
    ty: TyId,
    path: Vec<(u32, u32)>,
}

struct LoopCx {
    /// (name, position of the label: hygiene)
    label: Option<(u32, u32)>,
    brk: u32,
    cont: u32,
    result: Option<(Place, TyId)>,
    /// drop-scope depth when the loop was entered (break/continue drop deeper scopes)
    scope_depth: usize,
    /// statement temporaries alive when the loop was entered (break/continue drop newer ones)
    temps_depth: usize,
}

pub struct Lcx<'a> {
    /// SPEC-V: this body is unverified code: calls that pass `&mut` beside another pointer-holding
    /// argument go to the callee's unverified-mode twin
    plain_calls: bool,
    /// Relaxed Rust D3: the run-time disjoint-index checks of this body's clean verdict ((earlier,
    /// later) index expressions), and the slot each earlier index is kept in (by its span).
    d3: Option<std::sync::Arc<Vec<(relax_types::mir::Span, relax_types::mir::Span)>>>,
    d3_slots: HashMap<(u32, u32, u32), u32>,
    /// lowering a const/static initializer: borrowed temporaries live in static memory
    /// (rustc's constant promotion), not on the initializer's stack
    const_init: bool,
    // N:rustc_mir_build/src/builder/mod.rs:777-785: const owners always check overflow.
    overflow_checks: bool,
    /// a #[track_caller] fn: its implicit `&'static Location` parameter
    caller_loc: Option<VReg>,
    pub u: &'a Unit,
    /// the slot being built (closures created here are its children)
    cur_fn: u32,
    pub b: &'a Body,
    closure_storage: HashMap<u32, crate::layout::ClosureStorage>,
    pub f: Func,
    cur: u32,
    pos: u32,
    locals: Vec<Option<LocalSt>>,
    addr_taken: Vec<bool>,
    loops: Vec<LoopCx>,
    /// Some(ptr) when the function returns through a hidden pointer
    sret: Option<VReg>,
    ret_ty: TyId,
    /// closure being lowered (its expr id), for parameter binding
    pub closure_of: Option<u32>,
    pub new_closures: Vec<(u32, u32)>, // (closure expr, fn id)
    pub errors: Vec<String>,
    /// Body-local answers, discarded if nested constant evaluation re-collects types.
    query_epoch: u64,
    layouts: HashMap<TyId, &'static Layout>,
    /// Repeated uses of a type need the same cleanup decision, including live-session
    /// extension allocations.
    drop_types: HashMap<TyId, bool>,
    /// Trait dispatch is fully monomorphized here; repeated calls in this body share it.
    trait_methods: HashMap<DefId, HashMap<Vec<TyId>, crate::instance::Instance>>,
    /// drop scopes: locals (index) that own a value needing drop, innermost last
    scopes: Vec<Vec<u32>>,
    /// per local: its drop flag (1 = holds a live value)
    drop_flags: Vec<Option<VReg>>,
    /// scope depth a `let` declared each local at (its drop flag registers there)
    decl_depth: Vec<Option<usize>>,
    /// instructions run at function entry (drop flag initialisation)
    entry_inits: Vec<Inst>,
    /// fold branches on known constants (a check only); the registers a fold relied on, which must
    /// be written exactly once by the finished function (checked in lower_checked_body)
    pub(crate) fold: bool,
    pub(crate) fold_regs: std::cell::RefCell<Vec<VReg>>,
    /// rvalue temporaries to drop at the end of the current statement (with a drop flag
    /// when parts may have been moved out)
    stmt_temps: Vec<(VReg, TyId, Option<VReg>)>,
    /// owner of the place a pattern currently matches (by-value bindings move out of it)
    match_root: Option<MatchRoot>,
    /// in an FnOnce closure body: the drop flag (env, offset) of each by-value capture
    capture_flag: HashMap<u32, (VReg, i32)>,
    /// drop flags of evaluated operands not yet consumed (guard_operand): on an exit they drop
    /// before the statement's temporaries (rustc: an operand's drop is scheduled in the scope of
    /// the expression using it, nested inside the statement's temporary scope)
    guard_flags: HashMap<VReg, ()>,
    /// while lowering an edition-2024 `if let` condition: the scope its scrutinee temporaries
    /// belong to (they drop with its bindings, in creation order)
    cond_rescope: Option<usize>,
    /// the next indirect call is a virtual call of a #[track_caller] trait method: it passes
    /// the caller location (the vtable holds the tracked fn itself)
    indirect_tracked: bool,
    /// integer SIMD values evaluated at compile time: (slot base, offset) -> their bytes
    pub(crate) simd_consts: HashMap<(u32, i32), Vec<u8>>,
    /// temporaries whose lifetime a `let` extends to the end of its block (`let r = &make();`),
    /// one list per drop scope (parallel to `scopes`)
    /// temporaries living to the end of a scope (let-extended; `while let` scrutinees), with
    /// the drop flag of a scrutinee parts of which may be moved out
    scope_temps: Vec<Vec<(VReg, TyId, Option<VReg>)>>,
    /// per scope temporary: how many of the scope's locals were declared before it (it drops
    /// after those, before later ones: rustc schedules an extended temporary's drop when the
    /// `let` initializer makes it, before the `let`'s own bindings)
    scope_temp_pos: Vec<Vec<usize>>,
    /// per scope: how many statement temporaries existed when it was entered (an early exit
    /// drops each scope's locals after the temporaries of the statements inside it)
    scope_marks: Vec<usize>,
    /// or-pattern -> vreg holding the index of the alternative that matched (set by test_pat,
    /// read by bind_pat_place: bindings come from the matching alternative)
    or_sel: HashMap<u32, VReg>,
    /// makepad inspect: probe points of this body (and whether each was placed), and the `let`
    /// locals in scope while lowering (innermost last)
    probe_pts: Vec<(ProbePoint, bool)>,
    probe_scope: Vec<u32>,
    /// or-pattern alternatives fixed at compile time (a guarded arm tried one alternative
    /// combination at a time, rustc's per-candidate guard evaluation)
    or_force: HashMap<u32, usize>,
    /// moves out of the scrutinee recorded while binding a guarded arm: they happen only once
    /// the guard holds (rustc binds by reference for the guard, by value after it)
    defer_moves: Option<Vec<PendingMove>>,
    /// drop_tree is below a packed struct: dropped parts go through an aligned copy
    drop_misaligned: bool,
    /// temporary drop flag -> parts moved out of that temporary (path, flag)
    temp_moved: HashMap<u32, Vec<(Vec<(u32, u32)>, VReg)>>,
    /// operands of `&` in extending position of the `let` initializer being lowered
    extend_ops: crate::fxhash::HashSet<u32>,
    /// field path (variant, field)* from `match_root`'s local to the place the pattern being
    /// bound matches; None below a dereference (nothing can be moved out there)
    pat_path: Option<Vec<(u32, u32)>>,
    /// per local: field paths moved out of it, each with a flag (1 = moved out); the local's
    /// drop skips moved subtrees (partial moves)
    field_moved: HashMap<u32, Vec<(Vec<(u32, u32)>, VReg)>>,
    /// the place being lowered is written or mutably borrowed (DerefMut / IndexMut)
    want_mut: bool,
    /// locals owned by the enclosing function (captured by the closure being lowered):
    /// the closure neither drops them nor tracks their drop flags
    foreign: Vec<bool>,
    /// the unreachable block opened after the last diverging expression
    dead_blk: Option<u32>,
    /// > 0 while emitting a cleanup pad or a drop: calls there are plain calls (a panic in a
    /// destructor is not unwound into the same cleanup again)
    nounwind: u32,
    /// cleanup pad blocks by (drop action key, next block); the Resume block under the empty key
    pad_cache: HashMap<Vec<u64>, u32>,
    /// the unwind target of calls emitted now: None = from the drop state (cleanup_pad);
    /// Some(p) = this pad (Some(None): no cleanup) -- a drop in a sequence of drops unwinds
    /// into the drops after it, never into itself
    pad_fixed: Option<Option<u32>>,
    building_pad: bool,
    /// the last cleanup pad and the drop-state hash it was made for (consecutive calls of a
    /// statement mostly share their drop state)
    last_pad: Option<(u64, u32)>,
    /// lowering a coroutine's poll function
    coro: Option<Coro>,
    /// the coroutine's (expr, captures, state pointer, state field offsets)
    coro_caps: Option<(u32, Vec<u32>, VReg, Vec<u32>)>,
    tree_cx: Option<TreeCx>,

    /// per scope: where the statement temporaries of its block's tail expression start
    /// (edition < 2024: they belong to the enclosing temporary scope, so an exit drops them
    /// after this scope's locals)
    tail_marks: Vec<Option<usize>>,
}

/// A definition's path as source names it: an impl's item is named through its self type
/// (`ttf_parser::Face::parse`; a trait impl's `ttf_parser::<Face as Trait>::method`), not by the
/// impl's empty name (`def_path`'s `ttf_parser::::parse`). From the impl's syntax only: no
/// resolution (every new slot is named).
pub fn display_path(prog: &crate::program::Program, d: crate::program::DefId) -> String {
    use crate::program::DefKind;
    let def = prog.def(d);
    if def.parent == crate::program::NO_DEF || prog.def(def.parent).kind != DefKind::Impl {
        return prog.def_path(d);
    }
    let imp = prog.def(def.parent);
    let file = &prog.files[imp.file as usize];
    let Some(ii) = file.ast.as_impl(&file.ast.item(imp.item).kind) else { return prog.def_path(d) };
    let mut s = if imp.parent == crate::program::NO_DEF { String::new() } else { display_path(prog, imp.parent) };
    if !s.is_empty() {
        s.push_str("::");
    }
    let self_ty = ty_label(file, ii.self_ty, 0);
    match &ii.trait_ {
        Some(t) => {
            s.push('<');
            s.push_str(&self_ty);
            s.push_str(" as ");
            s.push_str(&path_label(file, t));
            s.push('>');
        }
        None => s.push_str(&self_ty),
    }
    s.push_str("::");
    s.push_str(prog.syms.str(def.name));
    s
}

/// A path as written, without its generic arguments.
fn path_label(file: &crate::parser::ParsedFile, p: &crate::ast::Path) -> String {
    let mut s = String::new();
    for (k, seg) in file.ast.segs(p.segs).iter().enumerate() {
        if k > 0 || p.segs.global() {
            s.push_str("::");
        }
        s.push_str(file.text(seg.name));
    }
    s
}

/// A type as written (generic arguments and lengths left out), for names.
fn ty_label(file: &crate::parser::ParsedFile, t: crate::ast::TyId, depth: u32) -> String {
    use crate::ast::Ty;
    if depth > 6 {
        return "_".into();
    }
    match file.ast.ty(t) {
        Ty::Path(p) => path_label(file, p),
        Ty::Ref(_, m, inner) => format!("&{}{}", if *m { "mut " } else { "" }, ty_label(file, *inner, depth + 1)),
        Ty::Ptr(m, inner) => format!("*{} {}", if *m { "mut" } else { "const" }, ty_label(file, *inner, depth + 1)),
        Ty::Slice(inner) => format!("[{}]", ty_label(file, *inner, depth + 1)),
        Ty::Array(inner, _) => format!("[{}; _]", ty_label(file, *inner, depth + 1)),
        Ty::Paren(inner) => ty_label(file, *inner, depth + 1),
        Ty::Tuple(l) => format!("({})", file.ast.list(*l).iter().map(|x| ty_label(file, *x, depth + 1)).collect::<Vec<_>>().join(", ")),
        Ty::Never => "!".into(),
        Ty::DynTrait(..) => "dyn _".into(),
        _ => "_".into(),
    }
}

pub fn fn_name(u: &Unit, key: &FnKey) -> String {
    match key {
        FnKey::Inst(d, _) => display_path(&u.prog, *d),
        FnKey::Closure(_, e, p) => format!("{{closure#{}}}@{}", e, p),
        FnKey::Shim(shim) => shim_name(&u.prog, &u.tcx, shim),
        FnKey::Glue(k, t) => format!("glue{}<{}>", k, crate::typeck::ty_to_string(false, &u.prog, &u.tcx, *t)),
        FnKey::Sandbox(b) if *b == u32::MAX => "sandbox:invalid-fn-pointer".to_string(),
        FnKey::Sandbox(b) => format!("sandbox:{}", u.fns[*b as usize].name),
        FnKey::Plain(b) => format!("{} [unverified]", u.fns[*b as usize].name),
    }
}

/// A move-path step through `*box` (field paths are (variant, field) pairs otherwise).
const BOX_STEP: u32 = u32::MAX;
/// a move-path step into element i of a fixed-length array: (ARRAY_STEP, i)
const ARRAY_STEP: u32 = u32::MAX - 1;

impl<'a> Lcx<'a> {
    /// `cur_fn`: the slot this body is built for (its closures' parent).
    pub fn new(u: &'a Unit, b: &'a Body, name: String, cur_fn: u32) -> Lcx<'a> {
        // SPEC-V: unverified code (no clean verdict yet, or an unverified-mode twin) sends its
        // aliasing-prone calls to unverified-mode twins
        let plain_calls = crate::jit::PLAIN_LOWER.with(|c| c.get()) == Some(cur_fn) || ((cur_fn as usize) < u.fns.len() && !u.verdict_clean(cur_fn));
        let mut f = Func::new(name, b.file);
        let entry = f.block();
        let n = b.locals.len();
        let ret = u.tcx.normalize_erasing_regions(&u.prog, crate::tcx::TypingEnv::fully_monomorphized(&u.prog), b.ret);
        let mut closure_storage = HashMap::default();
        for &ce in b.min_captures.keys() { closure_storage.insert(ce, crate::layout::ClosureStorage::for_codegen(&u.prog, &u.tcx, b, ce)); }
        // the verdict's disjoint-index checks belong in verified code only (unverified code and its
        // twins run as written, SPEC-V)
        let d3 = if plain_calls || (cur_fn as usize) >= u.fns.len() { None } else { u.disjoint_checks(cur_fn) };
        Lcx {
            closure_storage,
            u,
            cur_fn,
            plain_calls,
            d3,
            d3_slots: HashMap::default(),
            b,
            f,
            cur: entry,
            pos: 0,
            locals: vec![None; n],
            addr_taken: vec![false; n],
            loops: Vec::new(),
            sret: None,
            ret_ty: ret,
            closure_of: None,
            new_closures: Vec::new(),
            errors: Vec::new(),
            query_epoch: u.tcx.collect_epoch.load(std::sync::atomic::Ordering::Relaxed),
            layouts: HashMap::default(),
            drop_types: HashMap::default(),
            trait_methods: HashMap::default(),
            scopes: vec![Vec::new()],
            drop_flags: vec![None; n],
            decl_depth: vec![None; n],
            entry_inits: Vec::new(),
            fold: u.prog.session.options.const_fuel,
            fold_regs: std::cell::RefCell::new(Vec::new()),
            caller_loc: None,
            stmt_temps: Vec::new(),
            match_root: None,
            scope_temps: vec![Vec::new()],
            scope_temp_pos: vec![Vec::new()],
            scope_marks: vec![0],
            or_sel: HashMap::default(),
            probe_pts: PROBE_POINTS.try_with(|p| p.borrow().iter().filter(|x| x.slot == cur_fn).map(|x| (x.clone(), false)).collect()).unwrap_or_default(),
            probe_scope: Vec::new(),
            or_force: HashMap::default(),
            defer_moves: None,
            drop_misaligned: false,
            temp_moved: HashMap::default(),
            capture_flag: HashMap::default(),
            guard_flags: HashMap::default(),
            cond_rescope: None,
            indirect_tracked: false,
            simd_consts: HashMap::default(),
            extend_ops: Default::default(),
            pat_path: None,
            field_moved: HashMap::default(),
            want_mut: false,
            const_init: false,
            // Relax currently has only the release session policy (checks off).
            overflow_checks: false,
            foreign: vec![false; n],
            dead_blk: None,
            nounwind: 0,
            pad_cache: HashMap::default(),
            pad_fixed: None,
            building_pad: false,
            last_pad: None,
            coro: None,
            coro_caps: None,
            tree_cx: None,

            tail_marks: vec![None],
        }
    }

    /// A field of a provided crate's type is read only when its layout is the same on both
    /// sides (provided.rs `trusted`); otherwise rustc's fields are the crate's business.
    fn check_provided_field(&mut self, t: TyId) {
        if let TyKind::Adt(d, _, _) = self.u.tcx.tys.kind(t) {
            let krate = self.u.prog.def(*d).krate;
            if self.u.provided.iter().any(|(k, _)| *k == krate) && !crate::provided::trusted(&self.u.prog, &self.u.tcx, krate, t, 0) && !self.relax_only_instance(t) {
                let ts = crate::typeck::ty_to_string(false, &self.u.prog, &self.u.tcx, t);
                self.provided_err(krate, format!("a field of `{}` is read from JIT-compiled code, but `{}` lives in a provided crate (built by rustc into the loader) and its layout is not C-shaped: call a method of the crate instead", ts, ts));
            }
        }
    }

    /// A provided crate's static lives in the loader; Relax's copy of it would be a second one.
    /// Plain immutable data (trusted, no interior mutability) may be read from the copy.
    fn check_provided_static(&mut self, d: DefId, t: TyId) {
        let krate = self.u.prog.def(d).krate;
        if self.u.provided.iter().any(|(k, _)| *k == krate) && !crate::provided::trusted(&self.u.prog, &self.u.tcx, krate, t, 0) {
            let name = self.u.prog.def_path(d);
            self.provided_err(krate, format!("static `{}` is read from JIT-compiled code, but it lives in a provided crate (built by rustc into the loader), where its one copy is: call a fn of the crate instead", name));
        }
    }

    /// A type only JIT-compiled code makes: no bound fn's signature reaches it
    /// (provided::crossing_types), e.g. an inline shell's result or `LiveIdMap<LiveId, X>`.
    /// A provided crate's type whose values reach its rustc code (a bound fn takes or returns
    /// it) cannot be built here: rustc would read Relax's layout of its non-C-shaped fields.
    fn check_provided_built(&mut self, t: TyId) {
        if let TyKind::Adt(d, _, _) = self.u.tcx.tys.kind(t) {
            let krate = self.u.prog.def(*d).krate;
            if self.u.provided.iter().any(|(k, _)| *k == krate) && !crate::provided::trusted(&self.u.prog, &self.u.tcx, krate, t, 0) && !self.relax_only_instance(t) {
                let ts = crate::typeck::ty_to_string(false, &self.u.prog, &self.u.tcx, t);
                self.provided_err(krate, format!("`{}` is built in JIT-compiled code, but values of it reach the rustc-built code of its provided crate (a bound fn takes or returns it) and its layout is not C-shaped: build it with a fn of the crate, or keep it out of bound signatures", ts));
            }
        }
    }

    /// A provided crate's own fn compiled here (it has no shim) that calls std's raw ownership
    /// operations (Arc/Rc reference counts through raw pointers, Box/Arc/Rc::from_raw) works
    /// objects rustc allocated with Relax's std, whose layouts differ (KernelRef::clone was
    /// Arc::increment_strong_count on rustc's ArcInner: Stage aborted). Such a fn must be marked
    /// `#[relax_api]` (an op), so it runs on the compiled side.
    fn check_provided_ownership(&mut self, callee: u32) {
        let crate::jit::FnKey::Inst(d, args) = self.u.fns[self.cur_fn as usize].key.clone() else { return };
        let krate = self.u.prog.def(d).krate;
        if !args.is_empty() || !self.u.provided.iter().any(|(k, _)| *k == krate) {
            return;
        }
        let crate::jit::FnKey::Inst(cd, cargs) = self.u.fns[callee as usize].key.clone() else { return };
        let path = self.u.prog.def_path(cd);
        // a Box of a provided crate's opaque type is one owner: JIT code allocates it, rustc's
        // drop shim runs on the inner value, JIT code frees the box; wrapping and unwrapping it
        // moves no object across (the host's VM slot: take_vm / restore_vm)
        if (path.starts_with("core::boxed::") || path.starts_with("alloc::boxed::")) && cargs.first().is_some_and(|t| matches!(self.u.tcx.tys.kind(*t), TyKind::Adt(ad, a, _) if a.is_empty() && self.u.provided_types.contains_key(ad))) {
            return;
        }
        const RAW: [&str; 9] = [
            "::increment_strong_count",
            "::decrement_strong_count",
            "::from_raw",
            "::into_raw",
            "::from_raw_in",
            "::into_raw_with_allocator",
            "::increment_weak_count",
            "::decrement_weak_count",
            "::assume_init_drop",
        ];
        let std_owner = path.starts_with("alloc::") || path.starts_with("std::") || path.starts_with("core::");
        if std_owner && RAW.iter().any(|r| path.ends_with(r)) {
            let key = crate::provided::key(&self.u.prog, &self.u.tcx, d);
            self.provided_err(krate, format!("`{}` calls `{}` in JIT-compiled code, but `{}` lives in a provided crate: it works an object rustc allocated with makepad run's std, whose layout differs: mark it #[relax_api] (an op), or make it generic over the type JIT code allocates (an instance of a generic fn is compiled for its caller's type)", key, path, key));
        }
    }

    fn relax_only_instance(&self, t: TyId) -> bool {
        !self.u.provided_shared_contains(t)
    }

    /// An opaque provided enum (provided.rs; rustc's layout, no tag known to Relax) cannot be
    /// matched on or built by variant in JIT-compiled code.
    fn check_provided_opaque_enum(&mut self, t: TyId, what: &str) {
        if let TyKind::Adt(d, a, _) = self.kind(t) {
            if a.is_empty() && self.u.provided_types.contains_key(&d) {
                let ts = crate::typeck::ty_to_string(false, &self.u.prog, &self.u.tcx, t);
                let krate = self.u.prog.def(d).krate;
                self.provided_err(krate, format!("`{}` is {} by variant in JIT-compiled code, but it lives in a provided crate (built by rustc into the loader) and its layout is not C-shaped: use a method of the crate (e.g. a fieldless kind)", ts, what));
            }
        }
    }

    /// An error of JIT-compiled code reaching into provided crate `krate` the way it cannot
    /// (check_provided_*): also a violation row of the boundary audit when one runs.
    fn provided_err(&mut self, krate: u32, msg: String) {
        let at = self.errors.len();
        self.err(self.pos, msg);
        if crate::boundary::on() {
            if let Some(e) = self.errors.get(at) {
                crate::boundary::note_violation(self.u, krate, self.cur_fn, e);
            }
        }
    }

    fn err(&mut self, pos: u32, mut msg: String) {
        // A provided crate's own fn compiled here (it has no shim): say which manifest decides.
        if msg.contains("lives in a provided crate") {
            if let crate::jit::FnKey::Inst(d, args) = self.u.fns[self.cur_fn as usize].key.clone() {
                let krate = self.u.prog.def(d).krate;
                if args.is_empty() && self.u.provided.iter().any(|(k, _)| *k == krate) {
                    let name = self.u.prog.syms.str(self.u.prog.crates[krate as usize].name).replace('_', "-");
                    msg.push_str(&format!(
                        " (`{}` of {} runs here because it is not marked `#[relax_api]` in that crate: mark it, or call one of the crate's ops)",
                        crate::provided::key(&self.u.prog, &self.u.tcx, d),
                        name
                    ));
                }
            }
        }
        let prog = &self.u.prog;
        self.errors.push(prog.record_error(self.b.file, pos, "lower", msg));
    }

    fn ast(&self) -> &'a Ast {
        // the program outlives the lowering context; the AST is never mutated during lowering
        let p: *const Ast = &self.u.prog.files[self.b.file as usize].ast;
        unsafe { &*p }
    }

    fn src_text(&self, lo: u32, hi: u32) -> std::borrow::Cow<'a, str> {
        String::from_utf8_lossy(self.u.prog.files[self.b.file as usize].bytes(lo, hi))
    }

    // ------------------------------------------------------------ emission

    fn emit(&mut self, i: Inst) {
        // a call that may unwind while something needs dropping: an Invoke whose unwind edge
        // runs the drops (rustc MIR: the call terminator's `unwind: Cleanup(bb)`)
        let pad = match &i {
            Inst::Call(c, _, _) if self.nounwind == 0 && self.dead_blk != Some(self.cur) && self.may_unwind(c) => match self.pad_fixed {
                Some(p) => p,
                None if self.cleanup_needed() => Some(self.cleanup_pad()),
                None => None,
            },
            _ => None,
        };
        let b = &mut self.f.blocks[self.cur as usize];
        b.insts.push(i);
        b.pos.push(((self.b.file as u64) << 32) | self.pos as u64);
        if let Some(pad) = pad {
            let next = self.f.block();
            self.term(Term::Invoke(next, pad));
            self.switch(next);
        }
    }

    /// Calls that can start or pass on unwinding: Relax code and the panic entries.
    fn may_unwind(&self, c: &Callee) -> bool {
        match c {
            // every Relax callee may unwind here: whether one was compiled (and found nounwind)
            // before this body is lowered depends on build order; the optimiser demotes the
            // invokes of the build's inline candidates found nounwind (opt::demote_invokes)
            Callee::Fn(_) => true,
            Callee::Indirect(_) => true,
            Callee::CFn(_, s) | Callee::CIndirect(_, s) => s.unwinds,
            Callee::Host(a) => *a == self.u.rs.panic_site || *a == self.u.rs.panic_bounds || *a == self.u.rs.panic_impl,
            _ => false,
        }
    }

    /// Whether unwinding from here has anything to drop.
    fn cleanup_needed(&self) -> bool {
        if !self.unwind_on() {
            return false;
        }
        !self.stmt_temps.is_empty() || self.scopes.iter().any(|s| !s.is_empty()) || self.scope_temps.iter().any(|s| !s.is_empty())
    }

    /// The cleanup pad for the current drop state: the drops an early return would run
    /// (exit_actions(0, 0), by the runtime drop flags), then Resume. Pads are chains of one
    /// block per drop that end in the shared Resume block, shared by suffix as rustc's drop
    /// trees are: the call sites of one scope differ only in their innermost drops.
    fn cleanup_pad(&mut self) -> u32 {
        let h = self.drop_state_hash();
        if let Some((lh, p)) = self.last_pad {
            if lh == h {
                return p;
            }
        }
        let acts = self.exit_actions(0, 0);
        let p = self.pad_for(&acts);
        self.last_pad = Some((h, p));
        p
    }

    /// A hash of everything exit_actions(0, 0) and the pad's code depend on.
    fn drop_state_hash(&self) -> u64 {
        use std::hash::Hasher;
        let mut h = crate::fxhash::FxHasher::default();
        let temp = |h: &mut crate::fxhash::FxHasher, x: &(VReg, TyId, Option<VReg>)| {
            h.write_u32(x.0 .0);
            h.write_u32(x.1 .0);
            h.write_u32(x.2.map_or(u32::MAX, |f| f.0));
            if let Some(es) = x.2.and_then(|f| self.temp_moved.get(&f.0)) {
                h.write_usize(es.len());
            }
        };
        for i in 0..self.scopes.len() {
            h.write_usize(0x5c09e);
            h.write_usize(self.scope_marks.get(i).copied().unwrap_or(0));
            h.write_usize(self.tail_marks.get(i).copied().flatten().map_or(usize::MAX, |t| t));
            for &li in &self.scopes[i] {
                h.write_u32(li);
                h.write_u32(self.drop_flags[li as usize].map_or(u32::MAX, |f| f.0));
                match &self.locals[li as usize] {
                    Some(LocalSt::Slot(a)) => h.write_u32(a.0),
                    Some(LocalSt::Regs(r)) => {
                        h.write_usize(r.len() | 0x8000_0000);
                        for v in r {
                            h.write_u32(v.0);
                        }
                    }
                    None => h.write_u32(u32::MAX - 1),
                }
                if let Some(es) = self.field_moved.get(&li) {
                    h.write_usize(es.len());
                }
            }
            if let Some(ts) = self.scope_temps.get(i) {
                for x in ts {
                    temp(&mut h, x);
                }
            }
            if let Some(ps) = self.scope_temp_pos.get(i) {
                for p in ps {
                    h.write_usize(*p);
                }
            }
        }
        h.write_usize(0x7e3b);
        for x in &self.stmt_temps {
            temp(&mut h, x);
        }
        h.finish()
    }

    /// The pad that runs the drops `acts` (in order), then Resume.
    fn pad_for(&mut self, acts: &[DropAct]) -> u32 {
        let (cur, dead, pos) = (self.cur, self.dead_blk, self.pos);
        let fixed = self.pad_fixed.take();
        let was_building = std::mem::replace(&mut self.building_pad, true);
        self.dead_blk = None;
        self.nounwind += 1;
        let mut next = match self.pad_cache.get(&Vec::new()) {
            Some(&b) => b,
            None => {
                let b = self.f.block();
                self.switch(b);
                self.term(Term::Resume);
                self.pad_cache.insert(Vec::new(), b);
                b
            }
        };
        for a in acts.iter().rev() {
            let mut key = self.drop_act_key(a);
            key.push(0x7000_0000_0000_0000 | next as u64);
            next = match self.pad_cache.get(&key) {
                Some(&b) => b,
                None => {
                    let b = self.f.block();
                    self.switch(b);
                    self.emit_drop_act(a);
                    self.goto(next);
                    self.pad_cache.insert(key, b);
                    b
                }
            };
        }
        self.nounwind -= 1;
        self.cur = cur;
        self.dead_blk = dead;
        self.pos = pos;
        self.pad_fixed = fixed;
        self.building_pad = was_building;
        next
    }

    /// Runs the drops `seq` on the current path. A drop that panics unwinds into the drops
    /// after it and then the cleanup of everything still live (rustc drop elaboration: a
    /// drop's unwind edge goes to the next drop), never into itself.
    fn emit_drop_seq(&mut self, seq: Vec<DropAct>) {
        if self.dead_blk == Some(self.cur) || seq.is_empty() {
            return;
        }
        if !self.unwind_on() || self.nounwind > 0 {
            for a in &seq {
                self.emit_drop_act(a);
            }
            return;
        }
        let outer: Vec<DropAct> = self.exit_actions(0, 0).into_iter().filter(|a| !seq.contains(a)).collect();
        let saved = self.pad_fixed.take();
        for k in 0..seq.len() {
            let mut rest: Vec<DropAct> = seq[k + 1..].to_vec();
            rest.extend(outer.iter().cloned());
            let pad = if rest.is_empty() { None } else { Some(self.pad_for(&rest)) };
            self.pad_fixed = Some(pad);
            self.emit_drop_act(&seq[k]);
        }
        self.pad_fixed = saved;
    }
    fn term(&mut self, t: Term) {
        let b = &mut self.f.blocks[self.cur as usize];
        b.term = t;
        b.term_pos = ((self.b.file as u64) << 32) | self.pos as u64;
    }
    fn goto(&mut self, target: u32) {
        self.term(Term::Jump(target));
    }
    fn switch(&mut self, b: u32) {
        self.cur = b;
    }
    /// After a diverging expression: continue emitting into an unreachable block.
    fn dead(&mut self) {
        let b = self.f.block();
        self.switch(b);
        self.dead_blk = Some(b);
    }
    fn iconst(&mut self, v: i64) -> VReg {
        let d = self.f.vreg(Cls::I);
        self.emit(Inst::Iconst(d, v));
        d
    }
    /// The pointer stored at `cs + off` in a constant: its symbol, a run-independent small
    /// value (dangling / null), or a load from the constant at run time; None without `cs`.
    fn const_ptr_val(&mut self, cs: Option<relax_backend::rir::Sym>, off: u32) -> Option<VReg> {
        let at = cs?.plus(off as i64);
        Some(match crate::syms::read_leaf(self.u, at, Mem::Int(8, false)) {
            relax_backend::passes::RoRead::Ptr(p) => self.sym_val(p),
            relax_backend::passes::RoRead::Bytes(v) if v < 4096 => self.iconst(v as i64),
            _ => {
                let a = self.sym_val(at);
                let d = self.f.vreg(Cls::I);
                self.emit(Inst::Load(Mem::Int(8, false), d, a, 0));
                d
            }
        })
    }
    /// A symbol's value (an address, a site index) in a vreg (syms.rs).
    fn sym_val(&mut self, s: relax_backend::rir::Sym) -> VReg {
        let d = self.f.vreg(Cls::I);
        self.emit(Inst::Addr(d, s));
        d
    }
    /// A leaf of a constant's data as a vreg: an integer / float folds to an immediate, a
    /// pointer to its symbol; None when the bytes cannot be read as either (syms.rs).
    fn const_leaf_at(&mut self, at: relax_backend::rir::Sym, lf: crate::layout::Leaf) -> Option<VReg> {
        match crate::syms::read_leaf(self.u, at, lf.mem) {
            relax_backend::passes::RoRead::Bytes(raw) => Some(self.const_leaf(raw, lf)),
            relax_backend::passes::RoRead::Ptr(t) => Some(self.sym_val(t)),
            relax_backend::passes::RoRead::No | relax_backend::passes::RoRead::Range(_) => None,
        }
    }
    fn addr_add(&mut self, base: VReg, off: i32) -> VReg {
        if off == 0 {
            return base;
        }
        let k = self.iconst(off as i64);
        let d = self.f.vreg(Cls::I);
        self.emit(Inst::IBin(IOp::Add, U64, d, base, k));
        d
    }

    // ------------------------------------------------------------ types

    fn refresh_queries(&mut self) {
        let epoch = self.u.tcx.collect_epoch.load(std::sync::atomic::Ordering::Relaxed);
        if epoch != self.query_epoch {
            self.query_epoch = epoch;
            self.layouts.clear();
            self.drop_types.clear();
            self.trait_methods.clear();
        }
    }
    fn layout(&mut self, t: TyId) -> &'static Layout {
        self.refresh_queries();
        let u = self.u;
        let (l, too_big) = match self.layouts.get(&t) {
            Some(&l) => (l, false),
            None => {
                let erased = u.tcx.tys.erase_regions(t);
                let l = u.layout_of(erased);
                let too_big = u.lay.is_too_big(erased);
                if !too_big { self.layouts.insert(t, l); }
                (l, too_big)
            }
        };
        if too_big {
            let t = u.tcx.tys.erase_regions(t);
            let ts = crate::typeck::ty_to_string(false, &self.u.prog, &self.u.tcx, t);
            self.err(self.pos, format!("type `{}` is 4 GiB or larger, which makepad run does not lay out yet", ts));
        }
        l
    }
    // rustc_codegen_ssa/src/mir/mod.rs: FunctionCx::monomorphize. Identity
    // bodies stay shared with typeck/borrowck; codegen normalizes only reads.
    fn monomorphize(&self, t: TyId) -> TyId {
        self.u.tcx.normalize_erasing_regions(&self.u.prog, crate::tcx::TypingEnv::fully_monomorphized(&self.u.prog), t)
    }
    /// typeck's recorded generic args, monomorphized.
    fn mono_list(&self, l: crate::types::TyList) -> Vec<TyId> {
        self.u.tcx.tys.list(l).iter().map(|&t| self.monomorphize(t)).collect()
    }
    fn mono_method(&self, m: crate::typeck::MethodRes) -> MonoMethod {
        MonoMethod { def: m.def, derefs: m.derefs as usize, autoref: m.autoref, args: self.mono_list(m.args) }
    }
    fn body_ty(&self, e: ExprId) -> TyId { self.monomorphize(self.b.ty(e)) }
    fn body_pty(&self, p: PatId) -> TyId { self.monomorphize(self.b.pty(p)) }
    fn kind(&self, t: TyId) -> TyKind {
        self.kind_ref(t).clone()
    }
    fn kind_ref(&self, t: TyId) -> &'a TyKind {
        self.u.tcx.tys.kind(self.monomorphize(t))
    }
    fn needs_drop_glue(&mut self, t: TyId) -> bool {
        // These kinds own no values or live-session extension blocks. Avoid even the
        // shared needs_drop query (and the local map) for scalar and pointer operands.
        if matches!(self.u.tcx.tys.kind(t), TyKind::Int(_) | TyKind::Float(_) | TyKind::Bool | TyKind::Char | TyKind::Never | TyKind::Str | TyKind::Ref(..) | TyKind::Ptr(..) | TyKind::FnPtr(..) | TyKind::FnDef(..)) {
            return false;
        }
        self.refresh_queries();
        if let Some(&drop) = self.drop_types.get(&t) {
            return drop;
        }
        let drop = self.u.needs_drop_glue(t);
        self.drop_types.insert(t, drop);
        drop
    }
    fn resolve_instance(&mut self, d: DefId, args: &[TyId]) -> crate::instance::Instance {
        self.refresh_queries();
        if let Some(found) = self.trait_methods.get(&d).and_then(|m| m.get(args)) { return found.clone(); }
        let found = crate::instance::Instance::expect_resolve(&self.u.tcx, &self.u.prog,
            crate::tcx::TypingEnv::fully_monomorphized(&self.u.prog), d, args);
        self.trait_methods.entry(d).or_default().insert(args.to_vec(), found.clone());
        found
    }
    fn int_ty(&self, t: TyId) -> IntTy {
        match self.u.tcx.tys.kind(t) {
            TyKind::Int(p) => {
                let (b, s) = crate::types::Types::int_info(*p);
                IntTy { bits: (b * 8) as u8, signed: s }
            }
            TyKind::Bool => IntTy { bits: 8, signed: false },
            TyKind::Char => IntTy { bits: 32, signed: false },
            _ => U64,
        }
    }
    /// str, slices and trait objects: places of them are fat pointers (Place::Regs)
    /// Is `t` a trait object or a struct whose unsized tail is one?
    fn dyn_tail(&mut self, t: TyId) -> bool {
        let u = self.u;
        matches!(u.lay.unsized_tail(&crate::layout::LayoutCx::new(&u.prog), &u.tcx, t), Some((_, tl)) if matches!(u.tcx.tys.kind(tl), TyKind::Dyn(..)))
    }

    /// Alignment of the sized fields of an unsized type (1 for a bare `dyn`).
    fn sized_prefix_align(&mut self, t: TyId) -> u32 {
        match self.kind(t) {
            TyKind::Dyn(..) | TyKind::Str | TyKind::Slice(_) => 1,
            TyKind::Adt(d, args, _) => {
                let adt = match self.u.tcx.adts.get(&d) {
                    Some(a) => a,
                    None => return 1,
                };
                let nf = match adt.variants.first() {
                    Some(v) => v.fields.len(),
                    None => return 1,
                };
                let mut a = 1;
                for i in 0..nf {
                    let ft = self.u.tcx.field_ty(&self.u.prog, crate::tcx::TypingEnv::fully_monomorphized(&self.u.prog), d, 0, i as u32, self.u.tcx.tys.list(args));
                    let fa = if i + 1 == nf && self.unsized_ty(ft) { self.sized_prefix_align(ft) } else { self.layout(ft).align };
                    a = a.max(fa);
                }
                a
            }
            _ => self.layout(t).align,
        }
    }

    fn unsized_ty(&mut self, t: TyId) -> bool {
        match self.u.tcx.tys.kind(t) {
            TyKind::Str | TyKind::Slice(_) | TyKind::Dyn(..) => true,
            TyKind::Adt(..) | TyKind::Tuple(_) => {
                let u = self.u;
                u.lay.is_unsized(&crate::layout::LayoutCx::new(&u.prog), &u.tcx, t)
            }
            _ => false,
        }
    }
    fn is_wide(&self, t: TyId) -> bool {
        matches!(self.u.tcx.tys.kind(t), TyKind::Int(Prim::I128) | TyKind::Int(Prim::U128))
    }
    /// A placeholder value of type `t` after an error (keeps lowering going without panics).
    fn dummy_val(&mut self, t: TyId) -> Val {
        let l = self.layout(t);
        match &l.leaves {
            Some(leaves) => {
                let mut v = Vec::with_capacity(leaves.len());
                for lf in leaves {
                    let d = self.f.vreg(lf.cls);
                    match lf.cls {
                        Cls::I => self.emit(Inst::Iconst(d, 0)),
                        c => self.emit(Inst::Fconst(d, 0, c == Cls::F64)),
                    }
                    v.push(d);
                }
                Val::L(v)
            }
            None => {
                let a = self.new_slot_for(t);
                Val::M(a, 0)
            }
        }
    }
    fn is_f64(&self, t: TyId) -> bool {
        matches!(self.u.tcx.tys.kind(t), TyKind::Float(Prim::F64))
    }

    fn fresh_leaves(&mut self, leaves: &[Leaf]) -> Vec<VReg> {
        let mut v = Vec::with_capacity(leaves.len());
        for l in leaves {
            v.push(self.f.vreg(l.cls));
        }
        v
    }

    /// A slot for a value of type `t` that begins here: its SlotAddr is emitted at this point
    /// and nowhere else (`Slot::born`; a coroutine body's slots are rearranged around its
    /// suspension points afterwards, so they are not).
    fn new_slot_for(&mut self, t: TyId) -> VReg {
        self.new_slot_flagged(t, false, false)
    }

    /// A call's by-value argument copy (`Slot::arg`).
    fn new_arg_slot_for(&mut self, t: TyId) -> VReg {
        self.new_slot_flagged(t, true, false)
    }

    /// `arg`: a call's by-value argument copy (`Slot::arg`); `ret`: a call's result buffer
    /// (`Slot::ret`).
    fn new_slot_flagged(&mut self, t: TyId, arg: bool, ret: bool) -> VReg {
        let l = self.layout(t);
        let s = self.f.slot(l.size, l.align);
        let sl = &mut self.f.slots[s as usize];
        sl.born = self.coro.is_none();
        sl.arg = arg;
        sl.ret = ret;
        let d = self.f.vreg(Cls::I);
        self.emit(Inst::SlotAddr(d, s));
        d
    }

    // ------------------------------------------------------------ values and places

    /// Loads a value of type `t` from memory into leaves if it fits in registers.
    fn load_val(&mut self, base: VReg, off: i32, t: TyId) -> Val {
        let l = self.layout(t);
        match &l.leaves {
            Some(leaves) => {
                // a pointer-sized leaf the type's niche keeps from 0 (references, Box, NonNull,
                // a Vec's buffer pointer): codegen folds its null tests (Func::nonnull)
                let nn = l.niche.filter(|n| n.bytes == 8 && n.start >= 1 && n.start <= n.end).map(|n| n.off);
                // an unsigned scalar leaf the niche limits to start..=end (char, bool, enum
                // tags): value ranges decide its niche tests (Func::ranged)
                let rg = l.niche.filter(|n| n.bytes < 8 && n.start <= n.end && (n.start > 0 || n.end < (1u128 << (8 * n.bytes)) - 1));
                let mut v = Vec::with_capacity(leaves.len());
                for lf in leaves {
                    let d = self.f.vreg(lf.cls);
                    self.emit(Inst::Load(lf.mem, d, base, off + lf.off as i32));
                    if nn == Some(lf.off) && matches!(lf.mem, relax_backend::rir::Mem::Int(8, _)) {
                        self.f.nonnull.push(d);
                    }
                    if let (Some(n), relax_backend::rir::Mem::Int(b, false)) = (rg, lf.mem) {
                        if n.off == lf.off && n.bytes == b as u32 {
                            self.f.ranged.push((d, b, n.start as u64, n.end as u64));
                        }
                    }
                    v.push(d);
                }
                Val::L(v)
            }
            None => Val::M(base, off),
        }
    }

    fn store_val(&mut self, base: VReg, off: i32, t: TyId, v: Val) {
        let l = self.layout(t);
        self.store_val_with_layout(base, off, l, v);
    }

    fn store_val_with_layout(&mut self, base: VReg, off: i32, l: &crate::layout::Layout, v: Val) {
        match v {
            Val::L(regs) => {
                if let Some(leaves) = &l.leaves {
                    for (i, lf) in leaves.iter().enumerate() {
                        if i < regs.len() {
                            self.emit(Inst::Store(lf.mem, base, off + lf.off as i32, regs[i]));
                        }
                    }
                }
            }
            Val::M(src, soff) => {
                let s = self.addr_add(src, soff);
                let d = self.addr_add(base, off);
                if l.size > 0 {
                    self.emit(Inst::Copy(d, s, l.size));
                }
            }
        }
    }

    /// Value as register leaves (loads from memory when needed). A value of the wrong shape
    /// (only after an error, or a lowering bug) is padded so lowering never indexes past it;
    /// a shape bug without an earlier error is reported as one.
    fn to_leaves(&mut self, v: Val, t: TyId) -> Vec<VReg> {
        let mut r = match v {
            Val::L(r) => r,
            Val::M(b, o) => match self.load_val(b, o, t) {
                Val::L(r) => r,
                Val::M(..) => Vec::new(),
            },
        };
        let l = self.layout(t);
        if let Some(leaves) = &l.leaves {
            if r.len() < leaves.len() {
                // unreachable code after a diverging expression legitimately has no value
                if self.errors.is_empty() && !self.cur_unreachable() {
                    let ts = crate::typeck::ty_to_string(false, &self.u.prog, &self.u.tcx, t);
                    self.err(self.pos, format!("internal: value of `{}` has {} parts, expected {} -- makepad run bug, report it", ts, r.len(), leaves.len()));
                }
                for lf in &leaves[r.len()..] {
                    let d = self.f.vreg(lf.cls);
                    match lf.cls {
                        Cls::I => self.emit(Inst::Iconst(d, 0)),
                        c => self.emit(Inst::Fconst(d, 0, c == Cls::F64)),
                    }
                    r.push(d);
                }
            }
        }
        r
    }

    fn read_place(&mut self, p: &Place, t: TyId) -> Val {
        match p {
            Place::Regs(r) => {
                let mut v = Vec::new();
                for x in r {
                    let c = self.f.vregs[x.0 as usize];
                    let d = self.f.vreg(c);
                    self.emit(Inst::Mov(d, *x));
                    v.push(d);
                }
                Val::L(v)
            }
            Place::Mem(b, o) => {
                let v = self.load_val(*b, *o, t);
                if let Val::M(..) = v {
                    // aggregate: copy so later writes to the place do not alias the value
                    let l = self.layout(t);
                    let tmp = self.new_slot_for(t);
                    let src = self.addr_add(*b, *o);
                    if l.size > 0 {
                        self.emit(Inst::Copy(tmp, src, l.size));
                    }
                    return Val::M(tmp, 0);
                }
                v
            }
        }
    }

    fn write_place(&mut self, p: &Place, t: TyId, v: Val) {
        match p {
            Place::Regs(r) => {
                let src = self.to_leaves(v, t);
                for i in 0..r.len().min(src.len()) {
                    self.emit(Inst::Mov(r[i], src[i]));
                }
            }
            Place::Mem(b, o) => self.store_val(*b, *o, t, v),
        }
    }

    /// `&place` as leaves: the address, or for an unsized place its fat pointer.
    fn ref_to_place(&mut self, p: &Place, t: TyId) -> Vec<VReg> {
        if self.unsized_ty(t) {
            if let Place::Regs(r) = p {
                return r.clone();
            }
        }
        vec![self.place_addr(p, t)]
    }

    /// Address of a place (spilling register places to a fresh slot).
    fn place_addr(&mut self, p: &Place, t: TyId) -> VReg {
        match p {
            Place::Mem(b, o) => self.addr_add(*b, *o),
            Place::Regs(r) => {
                let a = self.new_slot_for(t);
                self.store_val(a, 0, t, Val::L(r.clone()));
                a
            }
        }
    }

    fn val_to_place(&mut self, v: Val) -> Place {
        match v {
            Val::L(r) => Place::Regs(r),
            Val::M(b, o) => Place::Mem(b, o),
        }
    }

    /// MAKEPAD_RUN_FIELD_PROF: one field-access site of the body being lowered (jit.rs field_note).
    #[allow(clippy::too_many_arguments)]
    fn field_note(&mut self, t: TyId, adt: &crate::tcx::AdtT, fi: u32, off: u32, fsz: u32, ssz: u32, salign: u32) {
        let prog = &self.u.prog;
        let file = self.b.file as usize;
        let src = &prog.files[file].src;
        let p = (self.pos as usize).min(src.len());
        // the site's line as a byte range of the file (samples of any instruction on it count)
        let lo = src[..p].iter().rposition(|c| *c == b'\n').map_or(0, |x| x + 1);
        let hi = src[p..].iter().position(|c| *c == b'\n').map_or(src.len(), |x| p + x);
        let (line, _) = prog.files[file].line_col(self.pos);
        let fname = adt.variants[0].fields.get(fi as usize).map_or(String::new(), |f| prog.syms.str(f.name).to_string());
        let site = crate::jit::FieldSite {
            func: if (self.cur_fn as usize) < self.u.fns.len() { self.u.fns[self.cur_fn as usize].name.to_string() } else { String::from("?") },
            file: self.b.file,
            line_lo: lo as u32,
            line_hi: hi as u32,
            at: format!("{}:{}", prog.file_paths[file], line),
            ty: crate::typeck::ty_to_string(false, prog, &self.u.tcx, t),
            field: fi,
            fname,
            off,
            size: fsz,
            ssize: ssz,
            salign,
            fixed: adt.repr_c || adt.repr_packed > 0 || adt.repr_int.is_some(),
        };
        crate::jit::field_note(site);
    }

    /// A new value of a live-editable struct (M10) has no extension block yet: its ext word is
    /// zeroed where the value is built.
    fn zero_ext(&mut self, a: VReg, off: i32, t: TyId) {
        if let Some(e) = self.layout(t).ext {
            // bit 0: built by code that knows dead slots (removed or retyped fields) and does
            // not set them -- the drop glue then leaves them alone (lower.rs ext_drop_fn)
            let z = self.iconst(has_dead_slots(self.u, t) as i64);
            self.emit(Inst::Store(Mem::Int(8, false), a, off + e as i32, z));
        }
    }

    /// Field `fi` of a live-editable struct that an edit added (M10, hotreload.md section 13):
    /// the value's extension block (allocated, or grown, on first use by a newer version), its
    /// fields not yet set taken from their `Default`, then the field's place in the block.
    fn ext_project(&mut self, p: &Place, t: TyId, fi: u32) -> (Place, TyId) {
        let Some((ext_off, fields, size, align)) = ext_fields(self.u, t) else {
            self.err(self.pos, "internal: extension field of a struct without an extension word -- makepad run bug, report it".to_string());
            return (Place::Regs(Vec::new()), self.u.tcx.tys.error);
        };
        let (base, boff) = match p {
            Place::Mem(r, o) => (*r, *o),
            Place::Regs(_) => {
                self.err(self.pos, "internal: a live-editable struct in registers -- makepad run bug, report it".to_string());
                return (Place::Regs(Vec::new()), self.u.tcx.tys.error);
            }
        };
        let slot = self.f.vreg(Cls::I);
        self.emit(Inst::IBinI(IOp::Add, U64, slot, base, boff as i64 + ext_off as i64));
        let f: rt_fn_ty!(fn(u64, u64, u64) -> u64) = rt_ext_get;
        let fs = self.u.syms.intern(crate::syms::SymKind::Rt("ext_get"), f as usize as u64);
        let (sz, al) = (self.iconst(size as i64), self.iconst(align as i64));
        let blk = self.f.vreg(Cls::I);
        self.emit(Inst::Call(Callee::Host(fs), vec![slot, sz, al], vec![blk]));
        // the fields this value's block does not hold yet (count in the header) get their default
        let cnt = self.f.vreg(Cls::I);
        self.emit(Inst::Load(Mem::Int(4, false), cnt, blk, 0));
        let n = fields.len() as i64;
        let all = self.iconst(n);
        let short = self.icmp(Cond::Lt, false, cnt, all);
        let fill = self.f.block();
        let done = self.f.block();
        self.term(Term::Branch(short, fill, done));
        self.switch(fill);
        let td = self.u.core_path(&["default", "Default"]);
        let m = td.and_then(|td| self.u.prog.traits[self.u.prog.def(td).sub as usize].iter().copied().find(|it| self.u.prog.name(*it) == "default"));
        for (k, &(_, ft, off)) in fields.iter().enumerate() {
            // a dead slot (a field an edit removed again) that owns nothing needs no value
            let dead_plain = match self.kind(t) {
                TyKind::Adt(d, _, _) => self.u.tcx.adts.get(&d).and_then(|a| a.variants[0].fields.get(fields[k].0 as usize).map(|f| f.name)).map_or(false, |n| is_dead_field(self.u, n)),
                _ => false,
            } && !self.needs_drop_glue(ft);
            if dead_plain {
                continue;
            }
            let kk = self.iconst(k as i64);
            let need = self.icmp(Cond::Le, false, cnt, kk);
            let init = self.f.block();
            let next = self.f.block();
            self.term(Term::Branch(need, init, next));
            self.switch(init);
            // a retyped scalar: the old value from its dead base slot, widened
            let conv = match self.kind(t) {
                TyKind::Adt(d, args, _) => self.u.tcx.adts.get(&d).and_then(|a| a.ext_conv.iter().find(|c| c.0 == fields[k].0).map(|c| (c.1, self.u.tcx.tys.subst_erasing_regions(a.variants[0].fields[c.1 as usize].ty, self.u.tcx.tys.list(args))))),
                _ => None,
            };
            let lt = self.layout(t);
            // the field's declared default (`field: T = expr`, `#[rust(expr)]`)
            let dconst = match self.kind(t) {
                TyKind::Adt(d, _, _) => self.u.tcx.adts.get(&d).and_then(|a| a.variants[0].fields.get(fields[k].0 as usize).map(|f| f.name)).and_then(|n| self.u.field_default_const(d, n)),
                _ => None,
            };
            match (td, m) {
                _ if conv.is_some() => {
                    let (src, st) = conv.unwrap();
                    let soff = lt.fields.get(src as usize).copied().unwrap_or(0);
                    let v = self.read_place(&Place::Mem(base, boff + soff as i32), st);
                    let v = self.cast(v, st, ft);
                    self.write_place(&Place::Mem(blk, off as i32), ft, v);
                }
                _ if dconst.is_some() => {
                    let v = self.const_val(dconst.unwrap(), Vec::new(), ft);
                    self.write_place(&Place::Mem(blk, off as i32), ft, v);
                }
                (Some(td), Some(m)) if self.u.tcx.type_implements_trait(&self.u.prog, crate::tcx::TypingEnv::fully_monomorphized(&self.u.prog), TraitRef { def_id: td, args: vec![GenericArg::Type(ft)] }).must_apply_modulo_regions() => {
                    let instance = crate::instance::Instance::expect_resolve(&self.u.tcx, &self.u.prog, crate::tcx::TypingEnv::fully_monomorphized(&self.u.prog), m, &[ft]);

                    let v = self.emit_instance(instance, Vec::new(), ft);
                    self.write_place(&Place::Mem(blk, off as i32), ft, v);
                }
                _ => {
                    let ts = crate::typeck::ty_to_string(false, &self.u.prog, &self.u.tcx, ft);
                    self.err(self.pos, format!("a field added live to a running struct needs a default, and `{}` has no Default impl -- restart the app", ts));
                }
            }
            self.term(Term::Jump(next));
            self.switch(next);
        }
        self.emit(Inst::Store(Mem::Int(4, false), blk, 0, all));
        self.term(Term::Jump(done));
        self.switch(done);
        let (_, ft, off) = fields.iter().copied().find(|x| x.0 == fi).unwrap_or((fi, self.u.tcx.tys.error, 16));
        (Place::Mem(blk, off as i32), ft)
    }

    /// Projection to field `fi` of variant `vi` of a place of type `t`.
    fn project(&mut self, p: &Place, t: TyId, vi: u32, fi: u32) -> (Place, TyId) {
        let k = self.kind_ref(t);
        let l = self.layout(t);
        // A memory projection needs only the selected field. In particular, projecting
        // each field of a large struct must not instantiate all its fields each time.
        let u = self.u;
        let field_ty = |i: usize| match k {
            TyKind::Tuple(v) => u.tcx.tys.list(*v)[i],
            TyKind::Closure(..) => u.closure_storage(t).fields[i],
            TyKind::Adt(d, args, _) => u.tcx.field_ty(&u.prog, crate::tcx::TypingEnv::fully_monomorphized(&u.prog), *d, vi, i as u32, self.u.tcx.tys.list(*args)),
            _ => u.tcx.tys.error,
        };
        let (fty, off, nfields): (TyId, u32, usize) = match k {
            TyKind::Closure(..) => {
                let storage = u.closure_storage(t);
                (storage.fields[fi as usize], l.fields[fi as usize], storage.fields.len())
            }
            TyKind::Tuple(v) if (fi as usize) < u.tcx.tys.list(*v).len() => (u.tcx.tys.list(*v)[fi as usize], l.fields.get(fi as usize).copied().unwrap_or(0), u.tcx.tys.list(*v).len()),
            TyKind::Adt(d, args, _) => {
                // an opaque type of a provided crate (rustc's layout, size only): every way of
                // reaching a field -- `x.f`, a struct pattern, a derived or written impl body --
                // ends here, so this is where it is refused (provided.rs)
                if self.u.provided_types.contains_key(d) {
                    let ts = crate::typeck::ty_to_string(false, &self.u.prog, &self.u.tcx, t);
                    let krate = self.u.prog.def(*d).krate;
                    self.provided_err(krate, format!("a field of `{}` is reached from JIT-compiled code (a field access, a pattern or a trait impl's body), but `{}` lives in a provided crate (built by rustc into the loader) and its layout is rustc's alone: call a fn of the crate, or route the impl to rustc's (jit.rs provided_fmt_hash)", ts, ts));
                    // the fn fails with the error above; lowering goes on over a scratch place of
                    // the field's type so nothing after it trips over a shapeless one
                    let ft = self.u.tcx.adts.get(d).and_then(|a| a.variants.get(vi as usize)).and_then(|v| v.fields.get(fi as usize)).map(|_| self.u.tcx.field_ty(&self.u.prog, crate::tcx::TypingEnv::fully_monomorphized(&self.u.prog), *d, vi, fi, self.u.tcx.tys.list(*args))).unwrap_or(self.u.tcx.tys.unit);
                    let a = self.new_slot_for(ft);
                    return (Place::Mem(a, 0), ft);
                }
                let adt = self.u.tcx.adts.get(d).unwrap();
                let var = &adt.variants[vi as usize];
                let ft = var.fields.get(fi as usize).map(|_| field_ty(fi as usize));
                let off = if adt.is_enum {
                    l.variant_fields.get(vi as usize).and_then(|v| v.get(fi as usize)).copied()
                } else if adt.is_union {
                    Some(0)
                } else {
                    l.fields.get(fi as usize).copied()
                };
                // MAKEPAD_RUN_FIELD_PROF: a struct field reached through memory, for the hot-field
                // layout measurement (jit.rs field_note)
                if !adt.is_enum && !adt.is_union && matches!(p, Place::Mem(..)) && crate::jit::field_prof_on() {
                    if let (Some(o), Some(ft)) = (off, ft) {
                        let fsz = self.layout(ft).size;
                        self.field_note(t, &adt, fi, o, fsz, l.size, l.align);
                    }
                }
                // a field an edit added (M10): in the value's extension block
                if off == Some(u32::MAX) && l.ext.is_some() {
                    return self.ext_project(p, t, fi);
                }
                match (off, ft) {
                    (Some(off), Some(ft)) => (ft, off, var.fields.len()),
                    _ => {
                        let ts = crate::typeck::ty_to_string(false, &self.u.prog, &self.u.tcx, t);
                        self.err(self.pos, format!("internal: no field {} in the layout of `{}` -- makepad run bug, report it", fi, ts));
                        (self.u.tcx.tys.error, 0, var.fields.len())
                    }
                }
            }
            _ => {
                let ts = crate::typeck::ty_to_string(false, &self.u.prog, &self.u.tcx, t);
                self.err(self.pos, format!("internal: field {} of non-aggregate `{}` -- makepad run bug, report it", fi, ts));
                return (Place::Regs(Vec::new()), self.u.tcx.tys.error);
            }
        };
        // a place of a dynamically sized struct is its fat pointer: fields are at data + offset,
        // the unsized tail keeps the pointer metadata
        if let Place::Regs(r) = p {
            if !r.is_empty() && self.unsized_ty(t) {
                // a `dyn` tail sits at the end of the sized fields rounded up to the dynamic
                // type's alignment, read from the vtable (rustc: field offset of an unsized
                // tail = align_to(unaligned offset, align_of_val), codegen place.rs)
                let dyn_tail = r.len() == 2 && fi as usize + 1 == nfields && self.dyn_tail(fty);
                let a = if dyn_tail {
                    // the end of the sized fields in memory (the tail stays last; the others
                    // may be reordered)
                    let mut unaligned = 0u32;
                    for j in 0..fi as usize {
                        let po = l.fields.get(j).copied().unwrap_or(0);
                        let sz = self.layout(field_ty(j)).size;
                        unaligned = unaligned.max(po + sz);
                    }
                    // `repr(packed(n))` caps the tail's alignment at n
                    let packed = match self.kind(t) {
                        TyKind::Adt(d, _, _) => self.u.tcx.adts.get(&d).map_or(0, |a| a.repr_packed),
                        _ => 0,
                    };
                    let pa = self.sized_prefix_align(fty);
                    let pa = if packed > 0 { pa.min(packed) } else { pa };
                    let x = crate::layout::round_up(unaligned, pa);
                    let va0 = self.f.vreg(Cls::I);
                    self.emit(Inst::Load(Mem::Int(8, false), va0, r[1], 16));
                    let va = if packed > 0 {
                        let pn = self.iconst(packed as i64);
                        let c0 = self.f.vreg(Cls::I);
                        self.emit(Inst::ICmp(Cond::Lt, false, c0, va0, pn));
                        let v = self.f.vreg(Cls::I);
                        self.emit(Inst::Sel(v, c0, va0, pn));
                        v
                    } else {
                        va0
                    };
                    let one = self.iconst(1);
                    let am1 = self.f.vreg(Cls::I);
                    self.emit(Inst::IBin(IOp::Sub, U64, am1, va, one));
                    let xc = self.iconst(x as i64);
                    let sum = self.f.vreg(Cls::I);
                    self.emit(Inst::IBin(IOp::Add, U64, sum, xc, am1));
                    let zero = self.iconst(0);
                    let neg = self.f.vreg(Cls::I);
                    self.emit(Inst::IBin(IOp::Sub, U64, neg, zero, va));
                    let o = self.f.vreg(Cls::I);
                    self.emit(Inst::IBin(IOp::And, U64, o, sum, neg));
                    let d = self.f.vreg(Cls::I);
                    self.emit(Inst::IBin(IOp::Add, U64, d, r[0], o));
                    d
                } else {
                    self.addr_add(r[0], off as i32)
                };
                if self.unsized_ty(fty) {
                    let mut v = vec![a];
                    v.extend_from_slice(&r[1..]);
                    return (Place::Regs(v), fty);
                }
                return (Place::Mem(a, 0), fty);
            }
        }
        match p {
            Place::Mem(b, o) => (Place::Mem(*b, *o + off as i32), fty),
            Place::Regs(r) => {
                // leaves of preceding fields
                let mut start = 0usize;
                for i in 0..fi as usize {
                    let fl = self.layout(field_ty(i));
                    start += fl.leaves.as_ref().map_or(0, |x| x.len());
                }
                let fl = self.layout(fty);
                let n = fl.leaves.as_ref().map_or(0, |x| x.len());
                let end = (start + n).min(r.len());
                (Place::Regs(r[start.min(end)..end].to_vec()), fty)
            }
        }
    }

    // ------------------------------------------------------------ locals

    fn alloc_local(&mut self, li: u32) -> LocalSt {
        let t = self.monomorphize(self.b.locals[li as usize].ty);
        let l = self.layout(t);
        let st = match (&l.leaves, self.addr_taken[li as usize]) {
            (Some(leaves), false) => LocalSt::Regs(self.fresh_leaves(leaves)),
            _ => {
                let a = self.new_slot_for(t);
                LocalSt::Slot(a)
            }
        };
        self.locals[li as usize] = Some(st.clone());
        st
    }

    fn local_place(&mut self, li: u32) -> Place {
        let st = match &self.locals[li as usize] {
            Some(s) => s.clone(),
            None => self.alloc_local(li),
        };
        match st {
            LocalSt::Regs(r) => Place::Regs(r),
            LocalSt::Slot(a) => Place::Mem(a, 0),
        }
    }

    /// Marks locals whose address is taken (they live in memory).
    fn scan_addr_taken(&mut self) {
        let ast = self.ast();
        let b = self.b;
        for (k, m) in &b.methods {
            if m.autoref != 0 {
                if let ExprKind::MethodCall(recv, _) = &ast.expr(ExprId(*k)).kind {
                    self.mark_root(*recv);
                }
            }
        }
        for i in 0..b.expr_ty.len() {
            let e = b.expr_at(i);
            match &ast.expr(e).kind {
                ExprKind::AddrOf(_, _, x) => self.mark_root(*x),
                // `a op= b` through an op-assign trait takes `&mut a`
                ExprKind::AssignOp(_, x, _) if b.binops.contains_key(&e.0) => self.mark_root(*x),
                // calling a closure passes its state by reference (`c()` on an FnMut closure
                // mutates the state in place: a register copy would lose the change and drop
                // the old captures twice)
                ExprKind::Call(f, _) if matches!(self.kind_ref(b.ty(*f)), TyKind::Closure(..)) => self.mark_root(*f),
                // overloaded index / deref take `&self` / `&mut self` of the base
                ExprKind::Index(x, _) if b.ov_index.contains_key(&e.0) => self.mark_root(*x),
                ExprKind::Unary(UnOp::Deref, x) if b.ov_deref(e.0, 0).is_some() => self.mark_root(*x),
                _ => {}
            }
        }
        // locals captured by reference live in memory
        for ce in &b.closures {
            let caps = self.storage_for_closure(*ce).roots.clone();
            for (i, li) in caps.iter().enumerate() {
                if !self.capture_by_value(*ce, i, *li) {
                    self.addr_taken[*li as usize] = true;
                }
            }
        }
    }

    fn mark_root(&mut self, e: ExprId) {
        let ast = self.ast();
        match &ast.expr(e).kind {
            ExprKind::Path(_) => {
                if let Some(Res::Local(li)) = self.b.res.get(&e.0) {
                    self.addr_taken[*li as usize] = true;
                }
            }
            ExprKind::Field(x, _) | ExprKind::TupleField(x, _) | ExprKind::Paren(x) => self.mark_root(*x),
            ExprKind::Index(x, _) => self.mark_root(*x),
            _ => {}
        }
    }

    // ------------------------------------------------------------ function entry

    /// Lowers a fn item body. `params`: ABI parameter vregs are created here.
    /// Puts the drop-flag initialisations at the start of the entry block.
    pub fn finish(&mut self) {
        let inits = std::mem::take(&mut self.entry_inits);
        let flags: Vec<VReg> = if self.coro.is_none() { inits.iter().filter_map(|i| if let Inst::Iconst(f, 0) = i { Some(*f) } else { None }).collect() } else { Vec::new() };
        if !inits.is_empty() {
            let b = &mut self.f.blocks[0];
            let n = inits.len();
            let mut insts = inits;
            insts.extend(b.insts.drain(..));
            b.insts = insts;
            let mut pos = vec![0u64; n];
            pos.extend(b.pos.drain(..));
            b.pos = pos;
        }
        elaborate_drop_flags(&mut self.f, &flags);
        if let Some(c) = self.coro.take() {
            // the transform moves the body's slots around the suspension points
            for s in self.f.slots.iter_mut() {
                s.born = false;
            }
            match coroutine_transform(&mut self.f, &c) {
                Ok((sz, al, shape)) => {
                    let area = (sz, al);
                    if self.errors.is_empty() {
                        if self.u.tcx.coro_shapes.get(&c.ct).is_none() {
                            self.u.tcx.coro_shapes.insert(c.ct, shape);
                        }
                        if self.u.tcx.coro_slot_ty.get(&self.cur_fn).is_none() {
                            self.u.tcx.coro_slot_ty.insert(self.cur_fn, c.ct);
                        }
                        match self.u.tcx.coro_frames.get(&c.ct) {
                            Some(prev) if prev != area => {
                                // the second lowering of a future that asked for its own layout
                                // (jit.rs coro_frame) saved a local of its own type across a suspension
                                if self.u.coro_selfref.lock().unwrap_or_else(|e| e.into_inner()).contains(&c.ct) {
                                    self.err(self.pos, "recursion in an async fn or block requires boxing: the future would contain itself (E0733)".to_string());
                                } else {
                                    self.err(self.pos, format!("internal: coroutine saved locals {:?} lowered again as {:?}", prev, area));
                                }
                            }
                            Some(_) => {}
                            None => self.u.tcx.coro_frames.insert(c.ct, area),
                        }
                    }
                }
                Err(e) => self.err(self.pos, e),
            }
        }
    }

    pub fn lower_fn_body(&mut self, def: DefId) {
        self.overflow_checks |= matches!(self.u.prog.def(def).kind, DefKind::Const | DefKind::AssocConst | DefKind::Static | DefKind::AnonConst);
        self.scan_addr_taken();
        let ast = self.ast();
        let prog_def_item = self.u.prog.def(def).item;
        if self.u.prog.def(def).kind == DefKind::AnonConst {
            let e = relax_types::const_eval::const_body(&self.u.prog, def).expect("anonymous const body");
            let out = self.f.vreg(Cls::I);
            self.f.params.push(out);
            self.pos = ast.expr(e).lo;
            self.const_init = true;
            let t = self.monomorphize(self.b.ret);
            let v = self.lower_expr_coerced(e, t);
            self.store_val(out, 0, t, v);
            self.term(Term::Ret(Vec::new()));
            return;
        }
        let (fsig, body) = match &ast.item(prog_def_item).kind {
            ItemKind::Fn(s, b, _) => (*ast.fn_sig(*s), b.or_else(|| self.u.prog.fn_body(self.u.prog.def(def).file, prog_def_item).ok().flatten())),
            ItemKind::Const(_, e) | ItemKind::Static(_, _, e) if e.is_some() => {
                // const/static initializer thunk: fn(out: *mut T)
                let e = self.u.prog.const_body(self.u.prog.def(def).file, prog_def_item).expect("checked initializer").unwrap_or(e.get().unwrap());
                let out = self.f.vreg(Cls::I);
                self.f.params.push(out);
                self.pos = ast.expr(e).lo;
                self.const_init = true;
                // stored at the item's declared type (`const P: &[u32] = &[1, 2, 3];` unsizes)
                let t = match self.u.tcx.const_ty(&self.u.prog, def) {
                    Some(ct) => self.u.tcx.tys.subst_erasing_regions(ct, &self.b.args.clone()),
                    None => self.body_ty(e),
                };
                let v = self.lower_expr_coerced(e, t);
                self.store_val(out, 0, t, v);
                self.term(Term::Ret(Vec::new()));
                return;
            }
            _ => return,
        };
        let ret = self.monomorphize(self.b.ret);
        let rl = self.layout(ret);
        // self param: local 0 when present
        let mut li_self = None;
        if fsig.self_param.is_some() {
            li_self = Some(0u32);
        }
        // C-family bodies follow their declared convention (callbacks, IMPs).
        let abi = self.u.tcx.fn_sig(&self.u.prog, def).map(|s| s.header.abi).unwrap_or(crate::types::Abi::Rust);
        let c_abi = abi.is_c_like() && self.u.prog.def(def).kind != DefKind::ForeignFn;
        if c_abi {
            let mut ptys = Vec::new();
            if let Some(li) = li_self {
                ptys.push(self.monomorphize(self.b.locals[li as usize].ty));
            }
            for p in &self.b.param_pats {
                ptys.push(self.body_pty(*p));
            }
            let cs = crate::cabi::c_sig(self.u, &ptys, ret, None, abi);
            self.f.cabi = Some(Box::new(cs));
        }
        // `#[unsafe(naked)]`: the body is its naked_asm! and nothing else (lower/asm.rs)
        if self.is_naked(prog_def_item) {
            if !c_abi {
                self.err(ast.item(prog_def_item).lo, "naked functions with the Rust ABI are not supported by Relax (give it an explicit ABI such as `extern \"C\"`)".to_string());
            }
            self.lower_naked(body);
            return;
        }
        let c_agg_ret = match &self.f.cabi {
            Some(cs) => Some(cs.ret.is_some()),
            None => None,
        };
        let ret_regs = match &rl.leaves {
            _ if c_agg_ret == Some(true) => false,
            Some(l) if c_agg_ret == Some(false) || ret_fits(l) => {
                for lf in l {
                    self.f.rets.push(lf.cls);
                }
                true
            }
            _ => false,
        };
        if !ret_regs {
            let p = self.f.vreg(Cls::I);
            self.f.params.push(p);
            self.sret = Some(p);
            self.f.sret = Some(p);
        }
        let mut param_vals: Vec<(Option<u32>, Option<PatId>, TyId, Vec<VReg>, bool)> = Vec::new();
        if let Some(li) = li_self {
            let t = self.monomorphize(self.b.locals[li as usize].ty);
            let (regs, by_ptr) = self.abi_param_c(t, c_abi);
            param_vals.push((Some(li), None, t, regs, by_ptr));
        }
        let rust_call = self.u.tcx.fn_sig(&self.u.prog, def).is_some_and(|s| s.header.abi == crate::types::Abi::RustCall);
        let patterns = self.b.param_pats.clone();
        for (index, p) in patterns.iter().enumerate() {
            let t = self.body_pty(*p);
            let (regs, by_ptr) = if rust_call && index + 1 == patterns.len() { self.abi_tuple_param(t) } else { self.abi_param_c(t, c_abi) };
            param_vals.push((None, Some(*p), t, regs, by_ptr));
        }
        // #[track_caller]: the caller's location arrives as an implicit last argument
        if self.u.tcx.fn_sig(&self.u.prog, def).map_or(false, |s| s.track_caller) {
            let p = self.f.vreg(Cls::I);
            self.f.params.push(p);
            self.caller_loc = Some(p);
        }
        for (li, pat, t, regs, by_ptr) in param_vals {
            let v = if by_ptr { Val::M(regs[0], 0) } else { Val::L(regs) };
            match (li, pat) {
                (Some(li), _) => {
                    let pl = self.local_place(li);
                    self.write_place(&pl, t, v);
                    self.own_local(li);
                }
                (None, Some(p)) if !matches!(ast.pat(p), Pat::Ident { by_ref: false, sub: None, .. }) && self.needs_drop_glue(t) => {
                    // a parameter pattern (`ref s: S`, `(a, _): (D, D)`, `_: D`): the argument is a
                    // variable of the function like `let _tmp = arg; let pat = _tmp;` (rustc):
                    // what the pattern does not move out drops when the function ends
                    self.bind_pat_owned(p, v, t, 0);
                }
                (None, Some(p)) => self.bind_pat(p, v, t),
                _ => {}
            }
        }
        if let (Some(bid), true) = (body, fsig.asyncness) {
            // `async fn`: returns its body's coroutine, which owns every parameter
            self.pos = ast.block(bid).lo;
            let ce = crate::tcx::ASYNC_FN_BODY | bid.0;
            let ct = self.coroutine_ty(ce);
            let t = match self.kind(ct) {
                TyKind::Closure(file, e2, crate::types::ClosureArgs { parent_args: owner, .. }) => {
                    let parent = match self.kind(owner) {
                        TyKind::FnDef(d, args, _) => self.u.fn_id(FnKey::Inst(d, self.u.tcx.tys.list(args).to_vec())),
                        _ => self.cur_fn,
                    };
                    let id = self.u.fn_id(FnKey::Closure(file, e2, parent));
                    self.new_closures.push((e2, id));
                    ct
                }
                _ => ct,
            };
            let v = self.build_coroutine(ce, t);
            self.ret_val(v, ret);
            return;
        }
        if let Some(bid) = body {
            self.pos = ast.block(bid).lo;
            let mark = self.stmt_temps.len();
            let v = self.lower_block(bid);
            // the entire function is a temporary scope: edition-2021 tail temporaries are
            // dropped here, after the body's locals (Reference destructors.scope.temporary)
            let v = self.settle_before_temp_drop(v, ret, mark);
            self.drop_stmt_temps_from(mark);
            self.pos = ast.block(bid).hi;
            // a body without a tail value whose type is not unit diverged on every path
            // (type checking accepted it): its end is unreachable
            let blk = ast.block(bid);
            let tail = match ast.list(blk.stmts).last() {
                Some(Stmt::Expr(x, false)) => Some(*x),
                _ => None,
            };
            // a tail of type `!` (`fn f(x: !) -> T { x }`) never produces a value either
            let never_tail = tail.map_or(false, |x| self.is_never(self.body_ty(x)) && !self.is_never(ret));
            if (tail.is_none() && !self.is_unit(ret)) || never_tail {
                self.term(Term::Unreachable);
            } else {
                self.ret_val(v, ret);
            }
        } else if self.u.prog.def(self.u.prog.def(def).parent).kind == DefKind::Trait {
            // a trait method without a default body reached as an instance: no impl was
            // found for these args (never a silent trap)
            self.pos = ast.item(prog_def_item).lo;
            let tr = self.u.prog.def(def).parent;
            let st = match self.b.args.first() {
                Some(t) => crate::typeck::ty_to_string(false, &self.u.prog, &self.u.tcx, *t),
                None => "?".to_string(),
            };
            let mut tn = self.u.prog.def_path(tr);
            // the trait's own args (`Mul<f64>`): which impl was asked for
            if self.b.args.len() > 1 {
                let n = self.u.tcx.fn_sig(&self.u.prog, def).map_or(self.b.args.len(), |s| s.n_parent_generics as usize).min(self.b.args.len());
                if n > 1 {
                    let a: Vec<String> = self.b.args[1..n].iter().map(|t| crate::typeck::ty_to_string(false, &self.u.prog, &self.u.tcx, *t)).collect();
                    tn = format!("{}<{}>", tn, a.join(", "));
                }
            }
            let m = self.u.prog.name(def).to_string();
            self.err(self.pos, format!("no impl of `{}` for `{}` provides `{}` -- implement the trait for that type", tn, st, m));
        }
    }

    fn is_unit(&self, t: TyId) -> bool {
        matches!(self.u.tcx.tys.kind(t), TyKind::Tuple(v) if v.is_empty())
    }

    /// rustc_ty_utils/src/abi.rs:61-73: the closure receiver precedes its inputs.
    pub fn lower_closure_body(&mut self, ce: u32) {
        let ast = self.ast();
        if ce & crate::tcx::ASYNC_FN_BODY != 0 || matches!(ast.expr(ExprId(ce)).kind, ExprKind::Async(..)) {
            self.scan_addr_taken();
            return self.lower_coroutine_body(ce);
        }
        self.scan_addr_taken();
        let (params, body) = match &ast.expr(ExprId(ce)).kind {
            ExprKind::Closure(row) => { let r = ast.row(*row); (r.params.clone(), r.body) }
            // a closure slot whose expression id names something else in this parse (a code
            // cache entry made from another numbering of the file): an error, never empty code
            _ => {
                self.errors.push(format!("closure slot {} names expression {} of its file, which is not a closure in this program", self.f.name, ce));
                return;
            }
        };
        // the signature's return type (the body may have a type of its own that coerces to it)
        let ret = match self.kind(self.body_ty(ExprId(ce))) {
            TyKind::Closure(_, _, crate::types::ClosureArgs { closure_sig_as_fn_ptr_ty: sig, .. }) => match self.kind(sig) {
                TyKind::FnPtr(_, r, _, _) => r,
                _ => self.body_ty(body),
            },
            _ => self.body_ty(body),
        };
        self.ret_ty = ret;
        let rl = self.layout(ret);
        let ret_regs = match &rl.leaves {
            Some(l) if ret_fits(l) => {
                for lf in l {
                    self.f.rets.push(lf.cls);
                }
                true
            }
            _ => false,
        };
        if !ret_regs {
            let p = self.f.vreg(Cls::I);
            self.f.params.push(p);
            self.sret = Some(p);
            self.f.sret = Some(p);
        }
        // captured state: by-value captures live in the state, by-reference ones point at
        // the owner's locals
        let caps = self.storage_for_closure(ce).roots.clone();
        {
            let ct = self.body_ty(ExprId(ce));
            let TyKind::Closure(_, _, ca) = self.kind(ct) else { panic!("closure type") };

            let env = if self.u.tcx.tys.to_opt_closure_kind(ca.kind_ty()) == Some(relax_types::upvar::ClosureKind::FnOnce) {
                let (regs, by_ptr) = self.abi_param(ct);
                if by_ptr { regs[0] } else { self.place_addr(&Place::Regs(regs), ct) }
            } else {
                let env = self.f.vreg(Cls::I);
                self.f.params.push(env);
                env
            };
            let up = match self.kind(ct) {
                TyKind::Closure(..) => ct,
                _ => self.u.tcx.tys.unit,
            };
            let ul = self.u.closure_storage_layout(up);
            let once = self.u.tcx.tys.to_opt_closure_kind(ca.kind_ty()) == Some(relax_types::upvar::ClosureKind::FnOnce);
            for (i, li) in caps.iter().enumerate() {
                let off = ul.fields.get(i).copied().unwrap_or(0) as i32;
                if self.storage_for_closure(ce).drop_flags && self.capture_by_value(ce, i, *li) {
                    if let Some(&foff) = ul.fields.get(caps.len() + i) {
                        self.capture_flag.insert(*li, (env, foff as i32));
                    }
                }
                let a = if self.capture_by_value(ce, i, *li) {
                    self.addr_add(env, off)
                } else {
                    let d = self.f.vreg(Cls::I);
                    self.emit(Inst::Load(Mem::Int(8, false), d, env, off));
                    d
                };
                self.locals[*li as usize] = Some(LocalSt::Slot(a));
                self.foreign[*li as usize] = true;
                // an FnOnce body owns its by-value captures (rustc: the upvars are moved into
                // the body, which drops what it does not move out at its end): the state's flag
                // is cleared and the capture becomes a local of this body, so partial moves out
                // of it (`item.uri.unwrap_or_default()`) are tracked like any local's and only
                // the rest drops (unwinding included)
                if once && self.capture_by_value(ce, i, *li) {
                    if let Some(&(fenv, foff)) = self.capture_flag.get(li) {
                        let z = self.iconst(0);
                        self.emit(Inst::Store(Mem::Int(1, false), fenv, foff, z));
                        self.capture_flag.remove(li);
                    }
                    self.foreign[*li as usize] = false;
                    self.own_local(*li);
                    // a precise capture owns only its fields: the others are not this
                    // body's to drop
                    if let Some(fs) = self.capture_fields(ce, i) {
                        let lt = self.monomorphize(self.b.locals[*li as usize].ty);
                        let n = match self.kind(lt) {
                            TyKind::Adt(d, _, _) => self.u.tcx.adts.get(&d).map_or(0, |a| a.variants.first().map_or(0, |v| v.fields.len())),
                            _ => 0,
                        };
                        for j in 0..n as u32 {
                            if !fs.contains(&j) {
                                self.moved_field(*li, &[(0, j)]);
                            }
                        }
                    }
                }
            }
        }
        let mut pv = Vec::new();
        for p in ast.list(params) {
            let t = self.body_pty(p.pat);
            let (regs, by_ptr) = self.abi_param(t);
            pv.push((p.pat, t, regs, by_ptr));
        }
        for (p, t, regs, by_ptr) in pv {
            let v = if by_ptr { Val::M(regs[0], 0) } else { Val::L(regs) };
            if !matches!(ast.pat(p), Pat::Ident { by_ref: false, sub: None, .. }) && self.needs_drop_glue(t) {
                // a parameter pattern (`|(a, _): (D, D)|`): as a fn's, what the pattern does not
                // move out drops when the closure body ends (it leaked)
                self.bind_pat_owned(p, v, t, 0);
            } else {
                self.bind_pat(p, v, t);
            }
        }
        self.pos = ast.expr(body).lo;
        // the body is a coercion site like a fn's return (typeck records e.g. Box<closure> ->
        // Box<dyn Fn()> on it; Reference type-coercions.md [coerce.site.return]) and a
        // terminating scope (`|c| *c.borrow_mut() = x` drops the guard; the entire function)
        let mark = self.stmt_temps.len();
        let v = self.lower_expr_coerced(body, ret);
        let v = self.settle_before_temp_drop(v, ret, mark);
        self.drop_stmt_temps_from(mark);
        self.ret_val(v, ret);
    }

    /// The coroutine (closure) type of async block / async fn body `ce`.
    fn coroutine_ty(&mut self, ce: u32) -> TyId {
        if ce & crate::tcx::ASYNC_FN_BODY != 0 {
            let r = self.monomorphize(self.b.ret);
            self.u.tcx.try_normalize_after_erasing_regions(&self.u.prog, crate::tcx::TypingEnv::fully_monomorphized(&self.u.prog), r).expect("codegen normalization")
        } else {
            self.body_ty(ExprId(ce))
        }
    }

    /// `Poll<t>`.
    fn poll_of(&mut self, t: TyId) -> TyId {
        match self.u.tcx.lang_items.get(LangItem::Poll) {
            Some(d) => self.u.tcx.tys.intern(TyKind::Adt(d, self.u.tcx.tys.mk_list(&[t]), crate::types::RegionArgsId::EMPTY)),
            None => self.u.tcx.tys.error,
        }
    }

    /// The value of coroutine `ce` of type `t`: its captures (moved in, or pointers), their
    /// drop flags set, the frame word 0 (not started); in memory.
    fn build_coroutine(&mut self, ce: u32, t: TyId) -> Val {
        let up = match self.kind(t) {
            TyKind::Closure(..) => t,
            _ => return self.unit(),
        };
        let caps = self.storage_for_closure(ce).roots.clone();
        let mut vals = Vec::new();
        for (i, li) in caps.iter().enumerate() {
            let lt = self.monomorphize(self.b.locals[*li as usize].ty);
            let pl = self.local_place(*li);
            if self.capture_by_value(ce, i, *li) {
                let v = self.read_place(&pl, lt);
                self.moved_local(*li);
                vals.push((v, lt));
            } else {
                let a = self.place_addr(&pl, lt);
                let pt = self.u.tcx.tys.intern(TyKind::Ptr(true, lt));
                vals.push((Val::L(vec![a]), pt));
            }
        }
        if self.storage_for_closure(ce).drop_flags {
            let u8_ = self.u.tcx.tys.prim(Prim::U8);
            for _ in 0..caps.len() {
                let one = self.iconst(1);
                vals.push((Val::L(vec![one]), u8_));
            }
        }
        let z = self.iconst(0);
        let tag_t = *self.u.closure_storage(up).fields.last().unwrap();
        vals.push((Val::L(vec![z]), tag_t));
        let v = self.build_record(up, 0, vals);
        let slot = self.new_slot_for(t);
        self.store_val_with_layout(slot, 0, &self.u.closure_storage_layout(up), v);
        Val::M(slot, 0)
    }

    /// A coroutine's poll function `fn([sret,] state, cx) -> Poll<Output>`: the body lowered as
    /// straight-line code with suspension points (`.await`), then made a state machine by
    /// coroutine_transform (Lcx::finish).
    fn lower_coroutine_body(&mut self, ce: u32) {
        let ast = self.ast();
        let body = if ce & crate::tcx::ASYNC_FN_BODY != 0 {
            BlockId(ce & !crate::tcx::ASYNC_FN_BODY)
        } else {
            match &ast.expr(ExprId(ce)).kind {
                ExprKind::Async(_, b) => *b,
                _ => return,
            }
        };
        let ct = self.coroutine_ty(ce);
        let (sig, up) = match self.kind(ct) {
            TyKind::Closure(_, _, crate::types::ClosureArgs { closure_sig_as_fn_ptr_ty: sig, .. }) => (sig, ct),
            _ => {
                self.err(self.pos, "coroutine type not found".to_string());
                self.term(Term::Unreachable);
                return;
            }
        };
        let out = match self.kind(sig) {
            TyKind::FnPtr(_, r, _, _) => r,
            _ => self.u.tcx.tys.unit,
        };
        let poll_ret = self.poll_of(out);
        // `return x` / `?` inside the body return from the coroutine: Ready(x) (ret_val_with)
        self.ret_ty = out;
        let rl = self.layout(poll_ret);
        let ret_regs = match &rl.leaves {
            Some(l) if ret_fits(l) => {
                for lf in l {
                    self.f.rets.push(lf.cls);
                }
                true
            }
            _ => false,
        };
        if !ret_regs {
            let p = self.f.vreg(Cls::I);
            self.f.params.push(p);
            self.sret = Some(p);
            self.f.sret = Some(p);
        }
        let env = self.f.vreg(Cls::I);
        self.f.params.push(env);
        let cx = self.f.vreg(Cls::I);
        self.f.params.push(cx);
        let ul = self.u.closure_storage_layout(up);
        let caps = self.storage_for_closure(ce).roots.clone();
        let storage = self.u.closure_storage(up);
        let nfields = storage.fields.len();
        let frame_off = ul.fields[nfields - 1] as i32;
        let tag = Mem::Int(self.layout(storage.fields[nfields - 1]).size as u8, false);
        let once = self.storage_for_closure(ce).drop_flags;
        for (i, li) in caps.iter().enumerate() {
            let off = ul.fields.get(i).copied().unwrap_or(0) as i32;
            if once && self.capture_by_value(ce, i, *li) {
                if let Some(&foff) = ul.fields.get(caps.len() + i) {
                    self.capture_flag.insert(*li, (env, foff as i32));
                }
            }
            let a = if self.capture_by_value(ce, i, *li) {
                self.addr_add(env, off)
            } else {
                let d = self.f.vreg(Cls::I);
                self.emit(Inst::Load(Mem::Int(8, false), d, env, off));
                d
            };
            self.locals[*li as usize] = Some(LocalSt::Slot(a));
            self.foreign[*li as usize] = true;
        }
        // fixed blocks the state machine needs (coroutine_transform)
        let saved = self.cur;
        let pending_ret = self.f.block();
        self.switch(pending_ret);
        let vi_p = self.variant_index(poll_ret, LangItem::PollPending);
        let pv = self.build_enum(poll_ret, vi_p, Vec::new());
        self.ret_raw(pv, poll_ret);
        let cancel_end = self.f.block();
        self.switch(cancel_end);
        self.goto(pending_ret);
        let after_done = self.f.block();
        self.switch(after_done);
        self.panic_here("`async fn` resumed after completion");
        let after_panic = self.f.block();
        self.switch(after_panic);
        self.panic_here("`async fn` resumed after panicking");
        self.switch(saved);
        self.coro = Some(Coro { ct, env, cx, frame_off, tag, up_size: ul.size, await_slots: Vec::new(), poll_scratch: Vec::new(), call_private: Default::default(), call_sret: Default::default(), poll_ret, suspends: Vec::new(), cancel_end: Some(cancel_end), pending_ret, after_done, after_panic });
        self.pos = ast.block(body).lo;
        let mark = self.stmt_temps.len();
        let v = self.lower_block(body);
        let v = self.settle_before_temp_drop(v, out, mark);
        self.drop_stmt_temps_from(mark);
        self.coro_caps = Some((ce, caps, env, ul.fields.clone()));
        self.ret_val(v, out);
    }

    /// The end of a coroutine body (its tail or a `return`): the by-value captures it did not
    /// move out drop (rustc moves the upvars into the coroutine and drops them at its end),
    /// then Ready(v) is returned.
    fn coro_complete(&mut self, v: Val, out: TyId) {
        if let Some((ce, caps, env, fields)) = self.coro_caps.clone() {
            for (i, li) in caps.iter().enumerate() {
                if self.capture_by_value(ce, i, *li) {
                    if let Some(&(fenv, foff)) = self.capture_flag.get(li) {
                        let lt = self.monomorphize(self.b.locals[*li as usize].ty);
                        let fl = self.f.vreg(Cls::I);
                        self.emit(Inst::Load(Mem::Int(1, false), fl, fenv, foff));
                        let yes = self.f.block();
                        let join = self.f.block();
                        self.term(Term::Branch(fl, yes, join));
                        self.switch(yes);
                        let a = self.addr_add(env, fields.get(i).copied().unwrap_or(0) as i32);
                        self.drop_at(a, lt);
                        let z = self.iconst(0);
                        self.emit(Inst::Store(Mem::Int(1, false), fenv, foff, z));
                        self.goto(join);
                        self.switch(join);
                    }
                }
            }
        }
        let poll_ret = match &self.coro {
            Some(c) => c.poll_ret,
            None => return,
        };
        let vi = self.variant_index(poll_ret, LangItem::PollReady);
        let pv = self.build_enum(poll_ret, vi, vec![(v, out)]);
        self.ret_raw(pv, poll_ret);
    }

    /// Returns `v: t` without running any drop (a suspension: everything stays live).
    fn ret_raw(&mut self, v: Val, t: TyId) {
        match self.sret {
            Some(p) => {
                self.store_val(p, 0, t, v);
                self.term(Term::Ret(Vec::new()));
            }
            None => {
                let r = self.to_leaves(v, t);
                self.term(Term::Ret(r));
            }
        }
    }

    /// `x.await` (rustc's lowering): `match IntoFuture::into_future(x) { mut f => loop { match
    /// Pin::new_unchecked(&mut f).poll(cx) { Ready(v) => break v, Pending => yield } } }` --
    /// the future lives in a slot (in the frame) as a temporary with a drop flag, so a panic
    /// or dropping the coroutine at this suspension drops it.
    fn lower_await(&mut self, e: ExprId, x: ExprId, t: TyId) -> Val {
        let (cx, poll_ret_self) = match &self.coro {
            Some(c) => (c.cx, c.poll_ret),
            None => {
                self.err(self.pos, "`await` outside of an async body".to_string());
                return self.unit();
            }
        };
        let xt = self.body_ty(x);
        let v = self.lower_expr(x);
        let (itf, fut_t) = {
            let u = self.u;
            let td = match u.core_path(&["future", "IntoFuture"]) {
                Some(d) => d,
                None => return self.unit(),
            };
            let sym_f = u.prog.syms.get("into_future").unwrap_or(u32::MAX);
            let sym_t = u.prog.syms.get("IntoFuture").unwrap_or(u32::MAX);
            let m = u.prog.traits[u.prog.def(td).sub as usize].iter().copied().find(|it| u.prog.def(*it).name == sym_f);
            let at = u.tcx.trait_assoc(&u.prog, td, sym_t);
            let ft = match at {
                Some(ad) => {
                    let p = u.tcx.tys.intern(TyKind::Alias(crate::consts::IsRigid::No, AliasTy { kind: AliasTyKind::Projection { def_id: ad }, args: u.tcx.tys.mk_list(&[xt]), regions: crate::types::RegionArgsId::EMPTY }));
                    let n = u.tcx.normalize_erasing_regions(&u.prog, crate::tcx::TypingEnv::fully_monomorphized(&u.prog), p);
                    u.tcx.try_normalize_after_erasing_regions(&u.prog, crate::tcx::TypingEnv::fully_monomorphized(&u.prog), n).expect("codegen normalization")
                }
                None => xt,
            };
            (m, ft)
        };
        let fut = match itf {
            Some(m) => {
                let instance = crate::instance::Instance::expect_resolve(&self.u.tcx, &self.u.prog, crate::tcx::TypingEnv::fully_monomorphized(&self.u.prog), m, &[xt]);

                self.emit_instance(instance, vec![(v, xt)], fut_t)
            }
            None => v,
        };
        let slot = self.new_slot_for(fut_t);
        if let Some(c) = self.coro.as_mut() {
            let k = c.suspends.len();
            c.await_slots.push((slot, k));
        }
        self.store_val(slot, 0, fut_t, fut);
        // no drop flag: the future is a temporary only between here and its drop after Ready
        // (rustc's `__awaitee`, initialized in its suspend state)
        let mark = self.stmt_temps.len();
        self.stmt_temps.push((slot, fut_t, None));
        // the poll of the awaited future
        let poll = {
            let u = self.u;
            match u.tcx.future_defs(&u.prog) {
                Some((fd, _)) => {
                    let sym = u.prog.syms.get("poll").unwrap_or(u32::MAX);
                    u.prog.traits[u.prog.def(fd).sub as usize].iter().copied().find(|it| u.prog.def(*it).name == sym)
                }
                None => None,
            }
        };
        let poll = match poll {
            Some(p) => p,
            None => return self.unit(),
        };
        let instance = crate::instance::Instance::expect_resolve(&self.u.tcx, &self.u.prog, crate::tcx::TypingEnv::fully_monomorphized(&self.u.prog), poll, &[fut_t]);

        let out_poll = self.poll_of(t);
        let head = self.f.block();
        self.goto(head);
        self.switch(head);
        let usize_ = self.u.tcx.tys.usize_;
        let scratch0 = self.f.slots.len() as u32;
        let r = self.emit_instance(instance, vec![(Val::L(vec![slot]), usize_), (Val::L(vec![cx]), usize_)], out_poll);
        let rp = self.val_to_place(r);
        let pending = self.f.block();
        let vi_ready = self.variant_index(out_poll, LangItem::PollReady);
        self.test_tag(&rp, out_poll, vi_ready, pending);
        let scratch1 = self.f.slots.len() as u32;
        if let Some(c) = self.coro.as_mut() {
            let k = c.suspends.len();
            c.poll_scratch.push((scratch0, scratch1, k));
        }
        let (vp, vt) = self.project(&rp, out_poll, vi_ready, 0);
        let val = self.read_place(&vp, vt);
        let val = match val {
            Val::M(..) => {
                let s2 = self.new_slot_for(t);
                self.store_val(s2, 0, t, val);
                Val::M(s2, 0)
            }
            v => v,
        };
        // completed: the future drops here (the end of the `match` the await lowers to)
        let ready_cur = self.cur;
        let _ = ready_cur;
        let ts = self.stmt_temps.split_off(mark);
        let seq = ts.iter().rev().map(|(a, t2, f2)| DropAct::Temp(*a, *t2, *f2)).collect();
        self.emit_drop_seq(seq);
        let done = self.cur;
        // pending: suspend; resuming polls again; dropping the coroutine here runs the drops
        // of everything live at this point (the future included)
        self.switch(pending);
        self.stmt_temps.push((slot, fut_t, None));
        let cancel_end = self.coro.as_ref().and_then(|c| c.cancel_end).unwrap_or(0);
        let acts = self.exit_actions(0, 0);
        let cancel = self.cancel_chain(&acts, cancel_end);
        self.stmt_temps.truncate(mark);
        let vi_p = self.variant_index(poll_ret_self, LangItem::PollPending);
        let pv = self.build_enum(poll_ret_self, vi_p, Vec::new());
        let pend_blk = self.cur;
        self.ret_raw(pv, poll_ret_self);
        if let Some(c) = self.coro.as_mut() {
            c.suspends.push((pend_blk, head, cancel));
        }
        let _ = e;
        self.switch(done);
        val
    }

    /// Blocks running the drops `acts` (by their flags), then `end` (a coroutine dropped at a
    /// suspension point).
    fn cancel_chain(&mut self, acts: &[DropAct], end: u32) -> u32 {
        let (cur, dead, pos) = (self.cur, self.dead_blk, self.pos);
        let fixed = self.pad_fixed.take();
        self.dead_blk = None;
        // a drop that panics unwinds into the drops after it (rustc's coroutine drop: drop
        // elaboration gives each drop an unwind edge to the rest)
        let saved_nounwind = std::mem::replace(&mut self.nounwind, 0);
        let mut next = end;
        for (k, a) in acts.iter().enumerate().rev() {
            let rest = &acts[k + 1..];
            let pad = if rest.is_empty() { None } else { Some(self.pad_for(rest)) };
            let b = self.f.block();
            self.switch(b);
            self.pad_fixed = Some(pad);
            self.emit_drop_act(a);
            self.pad_fixed = None;
            self.goto(next);
            next = b;
        }
        self.nounwind = saved_nounwind;
        self.cur = cur;
        self.dead_blk = dead;
        self.pos = pos;
        self.pad_fixed = fixed;
        next
    }

    /// C-family pointers use the existing C ABI classifier, including the fixed
    /// parameter count of a variadic call (rustc_ty_utils/src/abi.rs:597).
    fn fn_ptr_callee(&mut self, f: VReg, ps: &[TyId], ret: TyId, header: crate::types::FnHeader, args: &mut Vec<(Val, TyId)>) -> Callee {
        if header.abi == crate::types::Abi::RustCall { self.spread_tuple_argument(args); }
        if header.abi.is_c_like() {
            let fixed = if header.c_variadic { Some(ps.len()) } else { None };
            let mut params = Vec::new();
            for (_, ty) in args { params.push(*ty); }
            let cs = crate::cabi::c_sig(self.u, &params, ret, fixed, header.abi);
            Callee::CIndirect(f, Box::new(cs))
        } else {
            Callee::Indirect(f)
        }
    }

    /// rustc_ty_utils/src/abi.rs: RustCall spreads the final tuple argument.
    /// Reconstitute its place for the source body's parameter pattern.
    fn abi_tuple_param(&mut self, tuple: TyId) -> (Vec<VReg>, bool) {
        let TyKind::Tuple(inputs) = self.kind(tuple) else { panic!("RustCall final tuple") };
        let address = self.new_slot_for(tuple);
        let layout = self.layout(tuple);
        for (index, ty) in self.u.tcx.tys.list(inputs).iter().copied().enumerate() {
            let (regs, indirect) = self.abi_param(ty);
            let value = if indirect { Val::M(regs[0], 0) } else { Val::L(regs) };
            self.store_val(address, layout.fields[index] as i32, ty, value);
        }
        (vec![address], true)
    }

    fn spread_tuple_argument(&mut self, values: &mut Vec<(Val, TyId)>) {
        let (value, tuple) = values.pop().expect("RustCall argument");
        let TyKind::Tuple(inputs) = self.kind(tuple) else { panic!("RustCall final tuple") };
        let place = self.val_to_place(value);
        for i in 0..self.u.tcx.tys.list(inputs).len() {
            let (field, ty) = self.project(&place, tuple, 0, i as u32);
            values.push((self.read_place(&field, ty), ty));
        }
    }

    fn abi_param_c(&mut self, t: TyId, c: bool) -> (Vec<VReg>, bool) {
        if c && self.layout(t).size > 0 {
            if let relax_backend::rir::CArg::Agg(_) = crate::cabi::c_arg(self.u, t) {
                let p = self.f.vreg(Cls::I);
                self.f.params.push(p);
                self.f.noalias.push(p);
                return (vec![p], true);
            }
        }
        self.abi_param(t)
    }

    fn is_packed(&self, t: TyId) -> bool {
        match self.kind(t) {
            TyKind::Adt(d, _, _) => self.u.tcx.adts.get(&d).map_or(false, |a| a.repr_packed != 0),
            _ => false,
        }
    }

    /// rustc_mir_transform/src/shim.rs:539: builtin Clone for tuples and closures.
    pub fn lower_clone_shim(&mut self, t: TyId) {
        let rl = self.layout(t);
        let ret_regs = match &rl.leaves {
            Some(l) if ret_fits(l) => {
                for lf in l {
                    self.f.rets.push(lf.cls);
                }
                true
            }
            _ => false,
        };
        if !ret_regs {
            let p = self.f.vreg(Cls::I);
            self.f.params.push(p);
            self.sret = Some(p);
            self.f.sret = Some(p);
        }
        let a = self.f.vreg(Cls::I);
        self.f.params.push(a);
        self.ret_ty = t;
        let pa = Place::Mem(a, 0);
        // rustc_mir_transform/src/shim.rs:539-553: tuple and closure Clone
        // shims clone each field, including fields which are not Copy.
        let tuple = match self.kind(t) {
            TyKind::Closure(..) => Some(t),
            TyKind::Tuple(_) => Some(t),
            _ => None,
        };
        if let Some(up) = tuple {
            let n = match self.kind(up) {
                TyKind::Tuple(v) => self.u.tcx.tys.list(v).len(),
                TyKind::Closure(..) => self.u.closure_storage(up).fields.len(),
                _ => 0,
            };
            let vals = self.clone_shim_fields(&pa, up, 0, n);
            let v = if self.layout(t).leaves.is_none() {
                let s = self.new_slot_for(t);
                let place = Place::Mem(s, 0);
                for (i, (v, _)) in vals.into_iter().enumerate() {
                    let (fp, ft) = self.project(&place, up, 0, i as u32);
                    self.write_place(&fp, ft, v);
                }
                Val::M(s, 0)
            } else {
                self.build_record(up, 0, vals)
            };
            self.ret_val(v, t);
            return;
        }
        let v = self.read_place(&pa, t);
        self.ret_val(v, t);
    }

    fn clone_shim_fields(&mut self, pa: &Place, t: TyId, vi: u32, n: usize) -> Vec<(Val, TyId)> {
        // rustc_mir_transform/src/shim.rs:662-702: only initialized clones
        // participate in the unwind chain, in reverse initialization order.
        let mut vals = Vec::new();
        let mut initialized = Vec::new();
        let saved_pad = self.pad_fixed;
        for fi in 0..n {
            let pad = self.pad_for(&initialized);
            self.pad_fixed = Some(Some(pad));
            let (fa, ft) = self.project(pa, t, vi, fi as u32);
            let v = match self.kind(ft) {
                TyKind::Int(_) | TyKind::Float(_) | TyKind::Bool | TyKind::Char | TyKind::Ptr(..) | TyKind::FnPtr(..) | TyKind::Ref(false, _, _) => self.read_place(&fa, ft),
                _ => {
                    let td = self.u.tcx.lang_items.get(LangItem::Clone);
                    let m = self.u.tcx.lang_items.get(LangItem::CloneFn);
                    match (td, m) {
                        (Some(td), Some(m)) if self.u.tcx.type_implements_trait(&self.u.prog, crate::tcx::TypingEnv::fully_monomorphized(&self.u.prog), TraitRef { def_id: td, args: vec![GenericArg::Type(ft)] }).must_apply_modulo_regions() => {
                            let instance = crate::instance::Instance::expect_resolve(&self.u.tcx, &self.u.prog, crate::tcx::TypingEnv::fully_monomorphized(&self.u.prog), m, &[ft]);

                            let xa = self.place_addr(&fa, ft);
                            let rt = self.u.tcx.tys.intern(TyKind::Ref(false, ft, crate::types::RegionId::ERASED));
                            self.emit_instance(instance, vec![(Val::L(vec![xa]), rt)], ft)
                        }
                        _ => {
                            let ts = crate::typeck::ty_to_string(false, &self.u.prog, &self.u.tcx, ft);
                            self.err(self.pos, format!("Clone shim: field type `{}` has no Clone impl", ts));
                            self.dummy_val(ft)
                        }
                    }
                }
            };
            let v = if self.u.needs_drop_glue(ft) {
                let addr = self.new_slot_for(ft);
                self.write_place(&Place::Mem(addr, 0), ft, v);
                initialized.insert(0, DropAct::Temp(addr, ft, None));
                self.read_place(&Place::Mem(addr, 0), ft)
            } else { v };
            vals.push((v, ft));
        }
        self.pad_fixed = saved_pad;
        vals
    }

    fn abi_param(&mut self, t: TyId) -> (Vec<VReg>, bool) {
        let l = self.layout(t);
        match &l.leaves {
            Some(leaves) => {
                let regs = self.fresh_leaves(&leaves);
                for r in &regs {
                    self.f.params.push(*r);
                }
                // rustc marks `&mut T` noalias only for T: Unpin (a !Unpin value, e.g. a
                // self-referential future, may be aliased through itself)
                if let TyKind::Ref(true, inner, _) = self.kind(t) {
                    if !self.unsized_ty(inner) && regs.len() == 1 && self.u.is_unpin(inner) {
                        self.f.noalias.push(regs[0]);
                    }
                }
                // a reference's (or fat reference's data) pointer is never null (rustc `nonnull`
                // on reference parameters): the niche tests of iterators over a slice parameter
                // fold
                if let TyKind::Ref(_, inner, _) = self.kind(t) {
                    if !regs.is_empty() && self.f.vregs[regs[0].0 as usize] == Cls::I && (regs.len() == 1 || self.unsized_ty(inner)) {
                        self.f.nonnull.push(regs[0]);
                    }
                }
                // `&T` / `&mut T` of a sized T: dereferenceable for the call (rustc
                // dereferenceable(size_of::<T>()))
                if let TyKind::Ref(_, inner, _) = self.kind(t) {
                    if regs.len() == 1 && !self.unsized_ty(inner) {
                        let sz = self.layout(inner).size;
                        if sz > 0 {
                            self.f.deref.push((regs[0], sz));
                        }
                    }
                }
                (regs, false)
            }
            None => {
                let p = self.f.vreg(Cls::I);
                self.f.params.push(p);
                // by-pointer aggregate params point to the caller's private copy, which this
                // function owns (Func::owned)
                self.f.noalias.push(p);
                self.f.owned.push(p);
                if l.size > 0 {
                    self.f.deref.push((p, l.size));
                }
                (vec![p], true)
            }
        }
    }

    /// No block jumps to the current one (code after a diverging expression or an
    /// `if` whose arms all diverged).
    fn cur_unreachable(&self) -> bool {
        if self.cur == 0 {
            return false;
        }
        if self.dead_blk == Some(self.cur) {
            return true;
        }
        let c = self.cur;
        !self.f.blocks.iter().any(|b| b.term.succs().any(|x| x == c))
    }

    fn ret_val(&mut self, v: Val, t: TyId) {
        self.ret_val_with(v, t, false)
    }

    /// `return` / `?`: an early exit also leaves the statements being evaluated, whose
    /// temporaries drop in scope order (exit_scopes); the normal end keeps the tail's
    /// temporaries for after the locals (pre-2024 tail rule, handled by the block).
    fn ret_val_early(&mut self, v: Val, t: TyId) {
        let v = if self.stmt_temps.is_empty() { v } else { self.settle_before_temp_drop(v, t, 0) };
        self.ret_val_with(v, t, true)
    }

    fn ret_val_with(&mut self, v: Val, t: TyId, early: bool) {
        if self.dead_blk == Some(self.cur) || (!matches!(&v, Val::M(..)) && self.cur_unreachable()) {
            // after a diverging expression: nothing to return
            self.term(Term::Unreachable);
            return;
        }
        // the return value is computed; every live local is dropped before returning
        let v = match v {
            Val::M(b, o) if self.scopes.iter().any(|s| !s.is_empty()) => {
                // keep it out of reach of the drops (it may live in a local's memory)
                let tmp = self.new_slot_for(t);
                self.store_val(tmp, 0, t, Val::M(b, o));
                Val::M(tmp, 0)
            }
            v => v,
        };
        if early {
            self.exit_scopes(0, 0);
        } else {
            self.drop_scopes_from(0);
        }
        if self.coro.is_some() {
            return self.coro_complete(v, t);
        }
        match self.sret {
            Some(p) => {
                self.store_val(p, 0, t, v);
                self.term(Term::Ret(Vec::new()));
            }
            None => {
                let r = self.to_leaves(v, t);
                if r.len() != self.f.rets.len() {
                    self.err(self.pos, "returned value does not match the return type -- check the tail expression's type".to_string());
                    self.term(Term::Unreachable);
                    return;
                }
                self.term(Term::Ret(r));
            }
        }
    }

    // ------------------------------------------------------------ blocks / statements

    fn lower_block(&mut self, b: BlockId) -> Val {
        self.push_scope();
        let v = self.lower_block_stmts(b);
        self.pop_scope();
        v
    }

    fn lower_block_stmts(&mut self, b: BlockId) -> Val {
        let scope_mark = self.probe_scope.len();
        let v = self.lower_block_stmts_inner(b);
        self.probe_scope.truncate(scope_mark);
        v
    }

    /// makepad inspect: the first statement that starts on a probe point's line gets the probe call
    /// before it.
    fn probe_before(&mut self, s: &Stmt) {
        let ast = self.ast();
        let pos = match s {
            Stmt::Let(r) => match ast.row(*r).init { Some(e) => ast.expr(e).lo, None => return },
            Stmt::Expr(e, _) => ast.expr(*e).lo,
            Stmt::Attrs(r) => ast.expr(ast.row(*r).expr).lo,
            _ => return,
        };
        let line = self.u.prog.files[self.b.file as usize].line_col(pos).0 as u32;
        for k in 0..self.probe_pts.len() {
            if self.probe_pts[k].1 || self.probe_pts[k].0.line != line {
                continue;
            }
            self.probe_pts[k].1 = true;
            let pt = self.probe_pts[k].0.clone();
            let r = self.emit_probe(&pt);
            let _ = PROBE_RESULTS.try_with(|x| x.borrow_mut().push((pt.probe_slot, r)));
        }
    }

    /// The probe call: each probe parameter `name: &T` gets the address of the local or parameter
    /// `name` in scope (a register-held one is copied to a slot first: a snapshot).
    fn emit_probe(&mut self, pt: &ProbePoint) -> Result<(), String> {
        let mut params: Vec<u32> = Vec::new();
        for p in self.b.param_pats.clone() {
            self.pat_binding_locals(p, &mut params);
        }
        if !self.b.locals.is_empty() && self.u.prog.syms.str(self.b.locals[0].name) == "self" {
            params.push(0);
        }
        let mut args = Vec::new();
        for (name, want) in &pt.params {
            let li = self.probe_scope.iter().rev().chain(params.iter()).copied().find(|&li| self.u.prog.syms.str(self.b.locals[li as usize].name) == name.as_str());
            let Some(li) = li else {
                let mut names: Vec<String> = self.probe_scope.iter().chain(params.iter()).map(|&li| self.u.prog.syms.str(self.b.locals[li as usize].name).to_string()).collect();
                names.sort();
                names.dedup();
                return Err(format!("no local or parameter `{}` in scope at line {} (in scope: {})", name, pt.line, names.join(", ")));
            };
            let t = self.u.tcx.tys.erase_regions(self.monomorphize(self.b.locals[li as usize].ty));
            // the same type modulo regions (`want` has none: probes.rs by_name_params)
            if self.u.tcx.tys.erase_all_regions(t) != *want {
                return Err(format!("`{}` is `{}` at line {}, the probe takes `&{}`", name, crate::typeck::ty_to_string(false, &self.u.prog, &self.u.tcx, t), pt.line, crate::typeck::ty_to_string(false, &self.u.prog, &self.u.tcx, *want)));
            }
            let addr = match self.local_place(li) {
                Place::Mem(a, 0) => a,
                Place::Mem(a, off) => {
                    let d = self.f.vreg(Cls::I);
                    self.emit(Inst::IBinI(IOp::Add, U64, d, a, off as i64));
                    d
                }
                Place::Regs(r) => {
                    let s = self.new_slot_for(t);
                    self.store_val(s, 0, t, Val::L(r));
                    s
                }
            };
            args.push(addr);
        }
        // a plain call (no unwind edge here): probes.rs makes it a guarded invoke whose unwind
        // edge disables the probe
        let b = &mut self.f.blocks[self.cur as usize];
        b.insts.push(Inst::Call(Callee::Fn(pt.probe_slot), args, Vec::new()));
        b.pos.push(((self.b.file as u64) << 32) | self.pos as u64);
        Ok(())
    }

    fn lower_block_stmts_inner(&mut self, b: BlockId) -> Val {
        let ast = self.ast();
        let blk = ast.block(b);
        let n = blk.stmts.len();
        let mut result = Val::L(Vec::new());
        for (i, s) in ast.list(blk.stmts).iter().enumerate() {
            if !self.probe_pts.is_empty() {
                self.probe_before(s);
            }
            // this statement's temporaries (an enclosing statement's stay pending)
            let mark = self.stmt_temps.len();
            if i + 1 == n && matches!(s, Stmt::Expr(_, false)) && self.u.prog.files[self.b.file as usize].edition < 2024 {
                if let Some(t) = self.tail_marks.last_mut() {
                    *t = Some(mark);
                }
            }
            match s {
                Stmt::Let(r) => {
                    let crate::ast::LetStmt { attrs, pat, init, else_, .. } = ast.row(*r);
                    if !self.active(*attrs) {
                        continue;
                    }
                    let t = self.body_pty(*pat);
                    // the bound variables belong to this block's scope even when their drop flag is
                    // made later in a nested one (`let y; if c { y = v; }`: y lives to the block end)
                    let mut lis = Vec::new();
                    self.pat_binding_locals(*pat, &mut lis);
                    let depth = self.scopes.len() - 1;
                    if !self.probe_pts.is_empty() {
                        self.probe_scope.extend(lis.iter().copied());
                    }
                    for li in lis {
                        self.decl_depth[li as usize] = Some(depth);
                    }
                    match init {
                        Some(e) if matches!(ast.pat(*pat), Pat::Wild) && else_.is_none() && self.is_place_expr(*e) => {
                            // `let _ = place;` evaluates the place expression (an index is bounds
                            // checked and panics, an overloaded Index runs) but neither reads,
                            // moves nor drops it (Reference expressions.md place expression
                            // contexts; patterns.md wildcard pattern)
                            self.pos = ast.expr(*e).lo;
                            let _ = self.lower_place(*e);
                        }
                        Some(e) if else_.is_none() && self.is_place_expr(*e) && (matches!(ast.pat(*pat), Pat::Ident { by_ref: true, sub: None, .. }) || self.unsized_ty(t)) => {
                            // the initializer of `let` is a place expression context: `let ref a =
                            // *"abc"` borrows the place itself (an unsized one cannot be read as a
                            // value; Reference expressions.md place expression contexts)
                            self.pos = ast.expr(*e).lo;
                            self.want_mut = self.pat_binds_mut_ref(*pat);
                            let (place, pt) = self.lower_place(*e);
                            self.want_mut = false;
                            self.bind_pat_place(*pat, &place, pt);
                        }
                        Some(e)
                            if else_.is_none()
                                && !matches!(ast.pat(*pat), Pat::Wild | Pat::Ident { sub: None, .. })
                                && !self.b.coerce(e.0).is_some()
                                && self.field_move_path(*e).is_some() =>
                        {
                            // `let S { a, .. } = local(.f)*;` binds in place: what is moved out
                            // is tracked per field, the rest stays owned by the local
                            self.pos = ast.expr(*e).lo;
                            let (place, pt) = self.lower_place(*e);
                            let (li, path) = self.field_move_path(*e).unwrap_or((0, Vec::new()));
                            let saved = (self.match_root, self.pat_path.take());
                            self.match_root = Some(MatchRoot::Local(li));
                            self.pat_path = Some(path);
                            self.bind_pat_place(*pat, &place, pt);
                            self.match_root = saved.0;
                            self.pat_path = saved.1;
                        }
                        Some(e)
                            if else_.is_none()
                                && !matches!(ast.pat(*pat), Pat::Wild | Pat::Ident { sub: None, by_ref: false, .. })
                                && !self.b.coerce(e.0).is_some()
                                && self.is_place_expr(*e)
                                && (self.pat_binds_by_ref(*pat) || !self.pat_moves_droppable(*pat)) =>
                        {
                            // `let E(ref mut r, _) = *p;` / a captured variable: by-reference
                            // bindings borrow the place itself, never a copy of it (rustc binds
                            // patterns against the place expression)
                            self.pos = ast.expr(*e).lo;
                            self.want_mut = self.pat_binds_mut_ref(*pat);
                            let (place, pt) = self.lower_place(*e);
                            self.want_mut = false;
                            self.bind_pat_place(*pat, &place, pt);
                        }
                        Some(e) => {
                            self.pos = ast.expr(*e).lo;
                            let mut ext = Vec::new();
                            self.collect_extending(*e, &mut ext);
                            for x in &ext {
                                self.extend_ops.insert(*x);
                            }
                            let sc_mark = self.scope_temps.last().map_or(0, |v| v.len());
                            let v = if self.unsized_ty(t) && self.is_place_expr(*e) {
                                // A let-else slice initializer is a place, just like a match
                                // scrutinee. Keep its fat pointer (including the length), rather
                                // than loading an unsized value through only its data pointer.
                                self.want_mut = self.pat_binds_mut_ref(*pat);
                                let (place, pt) = self.lower_place(*e);
                                self.want_mut = false;
                                self.read_place(&place, pt)
                            } else {
                                self.lower_expr_coerced(*e, t)
                            };
                            for x in &ext {
                                self.extend_ops.remove(x);
                            }
                            match else_ {
                                None if matches!(ast.pat(*pat), Pat::Wild) => self.drop_val(v, t),
                                None if !matches!(ast.pat(*pat), Pat::Ident { .. }) && self.needs_drop_glue(t) && self.rvalue_rooted(*e) => {
                                    // `let (x, _) = (a, b);`: the value is a temporary of the
                                    // statement; what the pattern does not move out of it drops at
                                    // the statement's end (with `ref` bindings: the block's)
                                    let place = self.val_to_place(v);
                                    let a = self.place_addr(&place, t);
                                    let f = self.f.vreg(Cls::I);
                                    self.entry_inits.push(Inst::Iconst(f, 0));
                                    self.emit(Inst::Iconst(f, 1));
                                    let saved = (self.match_root, self.pat_path.take());
                                    self.match_root = Some(MatchRoot::Temp(f));
                                    self.pat_path = Some(Vec::new());
                                    let pos = self.scopes.last().map_or(0, |s| s.len());
                                    self.bind_pat_place(*pat, &Place::Mem(a, 0), t);
                                    self.match_root = saved.0;
                                    self.pat_path = saved.1;
                                    if self.pat_binds_by_ref(*pat) {
                                        if !self.scope_temps.is_empty() {
                                            let d = self.scope_temps.len() - 1;
                                            self.push_scope_temp_at(d, (a, t, Some(f)), pos);
                                        }
                                    } else {
                                        self.stmt_temps.push((a, t, Some(f)));
                                    }
                                }
                                // in a const / static initializer a `ref` binding borrows a
                                // temporary of the initializer's 'static scope: it lives in
                                // static memory (`const C: &[N; 2] = { let [_, ref x @ .., _] =
                                // [..]; x };`)
                                // the same for a promotable initializer anywhere (rustc
                                // promote_consts: the `ref` binding's borrow of the temporary is
                                // promoted -- `let [_, ref x @ .., _] = [N(1), ..]` in a const fn
                                // returning `&'static`)
                                None if self.pat_binds_by_ref(*pat) && !self.is_place_expr(*e) && (self.const_init || (!self.pat_binds_mut_ref(*pat) && self.promotable(*e))) => {
                                    let l = self.layout(t);
                                    let addr = self.u.rw_sym(l.size.max(1) as u64, l.align.max(8) as u64, Some(t));
                                    let p = self.f.vreg(Cls::I);
                                    self.emit(Inst::Addr(p, addr));
                                    self.store_val(p, 0, t, v);
                                    self.bind_pat_place(*pat, &Place::Mem(p, 0), t);
                                }
                                None => self.bind_pat(*pat, v, t),
                                Some(eb) => {
                                    let fail = self.f.block();
                                    let place = self.val_to_place(v);
                                    // the initializer's temporaries get drop flags: the failing
                                    // path drops them before the else block (whose own exits must
                                    // not drop them again)
                                    let lo_t = mark.min(self.stmt_temps.len());
                                    for k in lo_t..self.stmt_temps.len() {
                                        if self.stmt_temps[k].2.is_none() {
                                            let fl = self.f.vreg(Cls::I);
                                            self.entry_inits.push(Inst::Iconst(fl, 0));
                                            self.emit(Inst::Iconst(fl, 1));
                                            self.stmt_temps[k].2 = Some(fl);
                                        }
                                    }
                                    // so do lifetime-extended ones (`let 0 = &make().1 else`):
                                    // the failing path drops them before the else block too
                                    let sc_len = self.scope_temps.last().map_or(0, |v| v.len());
                                    for k in sc_mark.min(sc_len)..sc_len {
                                        if self.scope_temps.last().unwrap()[k].2.is_none() {
                                            let fl = self.f.vreg(Cls::I);
                                            self.entry_inits.push(Inst::Iconst(fl, 0));
                                            self.emit(Inst::Iconst(fl, 1));
                                            self.scope_temps.last_mut().unwrap()[k].2 = Some(fl);
                                        }
                                    }
                                    self.test_pat(*pat, &place, t, fail);
                                    let ok = self.cur;
                                    self.switch(fail);
                                    // a value that did not match is dropped before the else block runs
                                    // (rustc: the scrutinee temporary ends with the failed match)
                                    let owned = self.needs_drop_glue(t) && self.rvalue_rooted(*e);
                                    if owned {
                                        let a = self.place_addr(&place, t);
                                        self.drop_at(a, t);
                                    }
                                    // and so are the initializer's temporaries (`make().field`):
                                    // the else block runs after them (they stay registered for
                                    // the matching path, which drops them at the statement's end)
                                    let mut ts: Vec<(VReg, TyId, Option<VReg>)> = self.stmt_temps[mark.min(self.stmt_temps.len())..].to_vec();
                                    if let Some(top) = self.scope_temps.last() {
                                        ts.extend(top[sc_mark.min(top.len())..].iter().copied());
                                    }
                                    for (a, tt, fl) in ts.into_iter().rev() {
                                        if let Some(fl) = fl {
                                            self.drop_flagged_temp(a, tt, fl);
                                            self.emit(Inst::Iconst(fl, 0));
                                        }
                                    }
                                    self.lower_block_tscope(*eb);
                                    self.term(Term::Unreachable);
                                    self.switch(ok);
                                    if owned {
                                        // matched: what the pattern does not move drops at the
                                        // statement's end
                                        let a = self.place_addr(&place, t);
                                        let f = self.f.vreg(Cls::I);
                                        self.entry_inits.push(Inst::Iconst(f, 0));
                                        self.emit(Inst::Iconst(f, 1));
                                        let saved = (self.match_root, self.pat_path.take());
                                        self.match_root = Some(MatchRoot::Temp(f));
                                        self.pat_path = Some(Vec::new());
                                        self.bind_pat_place(*pat, &Place::Mem(a, 0), t);
                                        self.match_root = saved.0;
                                        self.pat_path = saved.1;
                                        self.stmt_temps.push((a, t, Some(f)));
                                    } else {
                                        self.bind_pat_place(*pat, &place, t);
                                    }
                                }
                            }
                        }
                        None => {
                            // declared, assigned later
                            if let Pat::Ident { .. } = ast.pat(*pat) {
                                if let Some(&li) = self.b.pat_local.get(&pat.0) {
                                    self.alloc_local(li);
                                }
                            }
                        }
                    }
                }
                // stripped at load (program::strip_cfg_stmts)
                Stmt::Item(_) | Stmt::Empty | Stmt::Attrs(..) => {}
                Stmt::Expr(e, semi) => {
                    self.pos = ast.expr(*e).lo;
                    let v = self.lower_expr(*e);
                    // a block's tail is a coercion site (`&self.v` as `&[T]`, `self` as `&dyn Tr`)
                    let et = self.body_ty(*e);
                    let v = if i == n - 1 && !*semi { self.apply_coercion(*e, v, et) } else { v };
                    if i == n - 1 && !*semi {
                        result = v;
                    } else {
                        // a discarded rvalue that owns resources is dropped here
                        let et = self.body_ty(*e);
                        if !self.is_place_expr(*e) && self.needs_drop_glue(et) {
                            self.drop_val(v, et);
                        } else if self.needs_drop_glue(et) {
                            // `x;` / `s.f;`: a place in value context is moved out and the
                            // temporary dropped at the statement's end (rustc: use of moved
                            // value afterwards)
                            let se = strip_paren(ast, *e);
                            let moved = match (self.field_move_path(se), self.b.res.get(&se.0).copied()) {
                                (Some((li, path)), _) => {
                                    self.moved_field(li, &path);
                                    true
                                }
                                (None, Some(Res::Local(li))) if self.foreign[li as usize] => {
                                    self.moved_local(li);
                                    true
                                }
                                _ => false,
                            };
                            if moved {
                                self.drop_val(v, et);
                            }
                        }
                    }
                }
            }
            // a block's tail expression is a temporary scope only from edition 2024
            // (Reference destructors.scope.temporary.edition2024); before, its temporaries
            // live to the end of the enclosing temporary scope
            let is_tail = i + 1 == n && matches!(s, Stmt::Expr(_, false));
            if !is_tail || self.u.prog.files[self.b.file as usize].edition >= 2024 {
                if is_tail {
                    result = self.settle_before_temp_drop(result, self.body_ty(match s { Stmt::Expr(e, _) => *e, _ => ExprId(0) }), mark);
                }
                self.drop_stmt_temps_from(mark);
            }
        }
        result
    }

    /// A value that may live in a temporary about to be dropped is read into registers / its
    /// own slot first.
    fn settle_before_temp_drop(&mut self, v: Val, t: TyId, mark: usize) -> Val {
        match v {
            Val::M(..) if mark < self.stmt_temps.len() => {
                let pl = self.val_to_place(v);
                let x = self.read_place(&pl, t);
                match x {
                    Val::M(..) => {
                        let slot = self.new_slot_for(t);
                        self.store_val(slot, 0, t, x);
                        Val::M(slot, 0)
                    }
                    x => x,
                }
            }
            v => v,
        }
    }

    /// A block that is a temporary scope of its own and whose value is discarded (loop
    /// bodies, the diverging else block of let-else): Reference destructors.scope.temporary
    /// "The body of an if, while or loop expression".
    fn lower_block_tscope(&mut self, b: BlockId) -> Val {
        let mark = self.stmt_temps.len();
        self.lower_block(b);
        self.drop_stmt_temps_from(mark);
        self.unit()
    }

    fn active(&self, attrs: List<Attr>) -> bool {
        if attrs.is_empty() {
            return true;
        }
        let prog = &self.u.prog;
        let krate = prog.file_crate[self.b.file as usize];
        crate::cfg::active(prog.src(self.b.file), prog.files[self.b.file as usize].ast.list(attrs), &prog.crates[krate as usize].cfg)
    }

    // ------------------------------------------------------------ autoderef

    /// One deref step of the place `pl: pt` for expression `e`, step `k`: built-in for
    /// references/pointers, a `Deref::deref` (or `deref_mut` in a mutable context) call
    /// for overloaded steps.
    fn deref_place(&mut self, e: u32, k: usize, pl: Place, pt: TyId) -> (Place, TyId) {
        if let Some((m, args)) = self.b.ov_deref(e, k) {
            let args = self.mono_list(args);
            return self.overloaded_deref(m, args, pl, pt);
        }
        let inner = match self.kind(pt) {
            TyKind::Ref(_, i, _) | TyKind::Ptr(_, i) => i,
            _ => pt,
        };
        let p = self.read_place(&pl, pt);
        let p = self.to_leaves(p, pt);
        if self.unsized_ty(inner) {
            (Place::Regs(p), inner)
        } else {
            (Place::Mem(p[0], 0), inner)
        }
    }

    fn overloaded_deref(&mut self, m: DefId, args: Vec<TyId>, pl: Place, pt: TyId) -> (Place, TyId) {
        let mut def = m;
        let mut mutbl = false;
        if self.want_mut {
            if let Some(td) = self.u.tcx.lang_items.get(LangItem::DerefMut) {
                if self.u.tcx.type_implements_trait(&self.u.prog, crate::tcx::TypingEnv::fully_monomorphized(&self.u.prog), TraitRef { def_id: td, args: vec![GenericArg::Type(pt)] }).must_apply_modulo_regions() {
                    for &it in &self.u.prog.traits[self.u.prog.def(td).sub as usize] {
                        if self.u.prog.name(it) == "deref_mut" {
                            def = it;
                            mutbl = true;
                        }
                    }
                }
            }
        }
        let target = {
            let u = self.u;
            let td = u.prog.def(m).parent;
            let tsym = u.prog.syms.get("Target").unwrap_or(u32::MAX);
            match u.tcx.trait_assoc(&u.prog, td, tsym) {
                Some(ad) => {
                    let proj = u.tcx.tys.intern(TyKind::Alias(crate::consts::IsRigid::No, AliasTy { kind: AliasTyKind::Projection { def_id: ad }, args: u.tcx.tys.mk_list(&[pt]), regions: crate::types::RegionArgsId::EMPTY }));
                    u.tcx.normalize_erasing_regions(&u.prog, crate::tcx::TypingEnv::fully_monomorphized(&u.prog), proj)
                }
                None => u.tcx.tys.error,
            }
        };
        let instance = crate::instance::Instance::expect_resolve(&self.u.tcx, &self.u.prog, crate::tcx::TypingEnv::fully_monomorphized(&self.u.prog), def, &args);

        let addr = self.place_addr(&pl, pt);
        let self_t = self.u.tcx.tys.intern(TyKind::Ref(mutbl, pt, crate::types::RegionId::ERASED));
        let ret_t = self.u.tcx.tys.intern(TyKind::Ref(mutbl, target, crate::types::RegionId::ERASED));
        let r = self.emit_instance(instance, vec![(Val::L(vec![addr]), self_t)], ret_t);
        let r = self.to_leaves(r, ret_t);
        if self.unsized_ty(target) {
            (Place::Regs(r), target)
        } else {
            (Place::Mem(r[0], 0), target)
        }
    }

    /// `for p in it` over an Iterator: `loop { match it.next() { Some(p) => body, None => break } }`.
    fn lower_for_iterator(&mut self, p: PatId, it: ExprId, b: BlockId, l: Option<(u32, u32)>) {
        let (next, nargs) = self.b.for_next.get(&it.0).cloned().unwrap();
        let nargs = self.mono_list(nargs);
        let (into, iargs, iter_t) = self.b.for_into[&it.0].clone();
        let iargs = self.mono_list(iargs);
        let iter_t = self.monomorphize(iter_t);
        let head_t = self.body_ty(it);
        let head_v = self.lower_expr(it);
        let instance = crate::instance::Instance::expect_resolve(&self.u.tcx, &self.u.prog, crate::tcx::TypingEnv::fully_monomorphized(&self.u.prog), into, &iargs);

        let iter_v = self.emit_instance(instance, vec![(head_v, head_t)], iter_t);
        // the iterator lives in memory for the loop (next takes &mut self)
        let pl = self.val_to_place(iter_v);
        let iter_addr = self.place_addr(&pl, iter_t);
        let instance = crate::instance::Instance::expect_resolve(&self.u.tcx, &self.u.prog, crate::tcx::TypingEnv::fully_monomorphized(&self.u.prog), next, &nargs);

        let opt_t = {
            let sig = self.u.tcx.fn_sig(&self.u.prog, next).cloned();
            match sig {
                Some(sig) => {
                    let x = self.u.tcx.tys.subst_erasing_regions(sig.ret, &nargs);
                    let u = self.u;
                    u.tcx.normalize_erasing_regions(&u.prog, crate::tcx::TypingEnv::fully_monomorphized(&u.prog), x)
                }
                None => self.u.tcx.tys.error,
            }
        };
        let some_vi = match self.kind(opt_t) {
            TyKind::Adt(od, _, _) => {
                let mut vi = 0;
                if let Some(adt) = self.u.tcx.adts.get(&od) {
                    for (k, v) in adt.variants.iter().enumerate() {
                        if self.u.tcx.lang_items.get(LangItem::OptionSome) == Some(v.def) {
                            vi = k as u32;
                        }
                    }
                }
                vi
            }
            _ => 1,
        };
        let head = self.f.block();
        let body = self.f.block();
        let exit = self.f.block();
        self.goto(head);
        self.switch(head);
        self.const_fuel_tick();
        let self_t = self.u.tcx.tys.intern(TyKind::Ref(true, iter_t, crate::types::RegionId::ERASED));
        let r = self.emit_instance(instance, vec![(Val::L(vec![iter_addr]), self_t)], opt_t);
        let rpl = self.val_to_place(r);
        let ra = self.place_addr(&rpl, opt_t);
        let opl = Place::Mem(ra, 0);
        self.test_tag(&opl, opt_t, some_vi, exit);
        self.goto(body);
        self.switch(body);
        // the iterator is a temporary of the loop: an early exit out of the loop (return, `?`,
        // a break to an outer loop) drops it too (Reference destructors.scope: the `for`
        // iterator lives for the whole loop)
        let iter_temp = self.stmt_temps.len();
        self.stmt_temps.push((iter_addr, iter_t, None));
        self.push_scope();
        let (fp, ft) = self.project(&opl, opt_t, some_vi, 0);
        let v = self.read_place(&fp, ft);
        if !matches!(self.ast().pat(p), Pat::Ident { by_ref: false, sub: None, .. }) && self.needs_drop_glue(ft) {
            // `for _ in ..` / `for (a, _) in ..`: the item lives to the end of the iteration
            let top = self.scopes.len() - 1;
            self.bind_pat_owned(p, v, ft, top);
        } else {
            self.bind_pat(p, v, ft);
        }
        self.loops.push(LoopCx { label: l, brk: exit, cont: head, result: None, scope_depth: self.scopes.len() - 1, temps_depth: self.stmt_temps.len() });
        self.lower_block_tscope(b);
        self.loops.pop();
        self.pop_scope();
        self.stmt_temps.truncate(iter_temp);
        self.goto(head);
        self.switch(exit);
        // the iterator itself is dropped after the loop
        self.drop_at(iter_addr, iter_t);
    }

    /// `base[i]` through `Index::index` / `IndexMut::index_mut`.
    fn overloaded_index(&mut self, m: DefId, args: Vec<TyId>, pl: Place, pt: TyId, i: ExprId, t: TyId, span: (u32, u32)) -> (Place, TyId) {
        let mut def = m;
        let mut mutbl = false;
        if self.want_mut {
            if let Some(td) = self.u.tcx.lang_items.get(LangItem::IndexMut) {
                if self.u.tcx.type_implements_trait(&self.u.prog, crate::tcx::TypingEnv::fully_monomorphized(&self.u.prog), TraitRef { def_id: td, args: self.u.tcx.complete_args(&self.u.prog, td, &args, relax_types::types::RegionArgsId::EMPTY) }).must_apply_modulo_regions() {
                    for &it in &self.u.prog.traits[self.u.prog.def(td).sub as usize] {
                        if self.u.prog.name(it) == "index_mut" {
                            def = it;
                            mutbl = true;
                        }
                    }
                }
            }
        }
        let instance = crate::instance::Instance::expect_resolve(&self.u.tcx, &self.u.prog, crate::tcx::TypingEnv::fully_monomorphized(&self.u.prog), def, &args);

        let saved = self.want_mut;
        self.want_mut = false;
        // (an index coerced to the impl's index type -- `S[bar]` with `impl Index<fn()>`,
        // `v[&string]` with `Index<&str>` -- passes the coerced value)
        let it = match args.get(1) {
            Some(&ia) if self.b.coerce(i.0).is_some() => ia,
            _ => self.body_ty(i),
        };
        let iv = self.lower_expr_coerced(i, it);
        self.want_mut = saved;
        if self.d3.is_some() && it == self.u.tcx.tys.usize_ {
            let leaves = self.to_leaves(iv.clone(), it);
            self.disjoint_index(span.0, span.1, leaves[0]);
        }
        let addr = self.place_addr(&pl, pt);
        // an array indexed through its slice's impl: `&[T; N]` unsizes to `&[T]`
        let (self_v, self_t) = match (self.kind(pt), self.kind(args[0])) {
            (TyKind::Array(_, n), TyKind::Slice(_)) => {
                let n = n.try_to_target_usize(&self.u.tcx.tys).expect("normalized array length");
                let l = self.iconst(n as i64);
                (Val::L(vec![addr, l]), self.u.tcx.tys.intern(TyKind::Ref(mutbl, args[0], crate::types::RegionId::ERASED)))
            }
            // rustc_codegen_ssa/src/mir/rvalue.rs: a reference to an unsized
            // place carries the place's metadata as well as its data address.
            _ if self.unsized_ty(pt) => (match &pl { Place::Regs(regs) => Val::L(regs.clone()), _ => panic!("unsized index receiver without metadata") }, self.u.tcx.tys.intern(TyKind::Ref(mutbl, pt, crate::types::RegionId::ERASED))),
            _ => (Val::L(vec![addr]), self.u.tcx.tys.intern(TyKind::Ref(mutbl, pt, crate::types::RegionId::ERASED))),
        };
        let ret_t = self.u.tcx.tys.intern(TyKind::Ref(mutbl, t, crate::types::RegionId::ERASED));
        let r = self.emit_instance(instance, vec![(self_v, self_t), (iv, it)], ret_t);
        let r = self.to_leaves(r, ret_t);
        if self.unsized_ty(t) {
            (Place::Regs(r), t)
        } else {
            (Place::Mem(r[0], 0), t)
        }
    }

    // ------------------------------------------------------------ drops


    /// Drops the value of type `t` at `addr` (calls the type's drop glue).
    fn drop_at(&mut self, addr: VReg, t: TyId) {
        if self.needs_drop_glue(t) {
            let id = self.u.fn_id(FnKey::Glue(crate::jit::GLUE_DROP, t));
            // a destructor that panics is not unwound into a pad that would drop the same
            // value again (rustc drop elaboration: a drop's unwind edge skips its own place)
            // outside a drop sequence (emit_drop_seq) the drop state may still list this value;
            // a drop that cannot unwind (std types all the way down) needs no pad
            let plain = self.pad_fixed.is_none() || !self.u.drop_may_unwind(t);
            self.nounwind += plain as u32;
            self.emit(Inst::Call(Callee::Fn(id), vec![addr], Vec::new()));
            self.nounwind -= plain as u32;
        }
    }

    /// An operand of type `&..&prim` used where the operator works on `prim` (`!&b`, `-&x`, as
    /// std's operator impls for references): load through the references.
    fn deref_prim_operand(&mut self, v: Val, mut from: TyId, to: TyId) -> (Val, TyId) {
        let mut v = v;
        while from != to {
            let inner = match self.kind(from) {
                TyKind::Ref(_, i, _) => i,
                _ => break,
            };
            let p = self.to_leaves(v.clone(), from);
            if p.is_empty() {
                break;
            }
            v = self.read_place(&Place::Mem(p[0], 0), inner);
            from = inner;
        }
        (v, from)
    }

    fn drop_val(&mut self, v: Val, t: TyId) {
        if !self.needs_drop_glue(t) {
            return;
        }
        let pl = self.val_to_place(v);
        let a = self.place_addr(&pl, t);
        // a value no local or temporary owns (an expression statement's result): its drop is a
        // call like any other, unwinding into this frame's cleanup
        let id = self.u.fn_id(FnKey::Glue(crate::jit::GLUE_DROP, t));
        if self.u.drop_may_unwind(t) {
            self.emit(Inst::Call(Callee::Fn(id), vec![a], Vec::new()));
        } else {
            self.nounwind += 1;
            self.emit(Inst::Call(Callee::Fn(id), vec![a], Vec::new()));
            self.nounwind -= 1;
        }
    }

    /// The drop flag of a local (created and registered in the current scope on first use).
    fn flag_of(&mut self, li: u32) -> VReg {
        if let Some(f) = self.drop_flags[li as usize] {
            return f;
        }
        let f = self.f.vreg(Cls::I);
        self.entry_inits.push(Inst::Iconst(f, 0));
        self.drop_flags[li as usize] = Some(f);
        let top = self.scopes.len() - 1;
        let at = self.decl_depth[li as usize].filter(|d| *d <= top).unwrap_or(top);
        // a scope drops its locals in reverse declaration order, whatever order they were
        // first assigned in (`let (x, y); y = ..; x = ..;` drops y, then x); locals are
        // numbered in declaration order
        let pos = self.scopes[at].iter().position(|&x| x > li).unwrap_or(self.scopes[at].len());
        self.scopes[at].insert(pos, li);
        f
    }

    /// A local now owns a value: drop a previous one still owned (rebinding in loops),
    /// then set its flag.
    fn own_local(&mut self, li: u32) {
        if self.foreign[li as usize] {
            return;
        }
        let lt = self.monomorphize(self.b.locals[li as usize].ty);
        if !self.needs_drop_glue(lt) {
            return;
        }
        let f = self.flag_of(li);
        self.emit(Inst::Iconst(f, 1));
    }

    /// Drop a previous value of a local about to be overwritten (before the write).
    fn drop_local_before_rebind(&mut self, li: u32) {
        let lt = self.monomorphize(self.b.locals[li as usize].ty);
        if self.needs_drop_glue(lt) && self.drop_flags[li as usize].is_some() {
            self.drop_local_if_live(li);
        }
    }

    fn moved_local(&mut self, li: u32) {
        if self.foreign[li as usize] {
            // a by-value capture moved out of an FnOnce closure's state: clear its drop flag
            if let Some(&(env, off)) = self.capture_flag.get(&li) {
                let z = self.iconst(0);
                self.emit(Inst::Store(Mem::Int(1, false), env, off, z));
            }
            return;
        }
        let lt = self.monomorphize(self.b.locals[li as usize].ty);
        if self.needs_drop_glue(lt) {
            let f = self.flag_of(li);
            self.emit(Inst::Iconst(f, 0));
        }
    }

    fn drop_local_if_live(&mut self, li: u32) {
        // nothing runs after a diverging expression
        if self.dead_blk == Some(self.cur) {
            return;
        }
        let f = match self.drop_flags[li as usize] {
            Some(f) => f,
            None => return,
        };
        let lt = self.monomorphize(self.b.locals[li as usize].ty);
        let yes = self.f.block();
        let join = self.f.block();
        self.term(Term::Branch(f, yes, join));
        self.switch(yes);
        let pl = self.local_place(li);
        let a = self.place_addr(&pl, lt);
        match self.field_moved.get(&li).cloned() {
            Some(entries) => {
                self.drop_tree_top(&entries, a, lt);
                for (_, mf) in &entries {
                    self.emit(Inst::Iconst(*mf, 0));
                }
            }
            None => self.drop_at(a, lt),
        }
        // (a pad resumes unwinding: the flag is never read again)
        if !self.building_pad {
            self.emit(Inst::Iconst(f, 0));
        }
        self.goto(join);
        self.switch(join);
    }

    /// `local.f.g` (struct / tuple fields, no dereference) as (local, [(0, field)..]).
    fn field_move_path(&self, e: ExprId) -> Option<(u32, Vec<(u32, u32)>)> {
        let ast = self.ast();
        match &ast.expr(e).kind {
            ExprKind::Paren(x) => self.field_move_path(*x),
            ExprKind::Path(_) => match self.b.res.get(&e.0) {
                Some(Res::Local(li)) if !self.foreign[*li as usize] => Some((*li, Vec::new())),
                _ => None,
            },
            ExprKind::Field(x, _) | ExprKind::TupleField(x, _) => {
                let (derefs, fi) = self.b.field_idx.get(&e.0).map(|&(d, f)| (d as usize, f))?;
                let (li, mut p) = self.field_move_path(*x)?;
                // through a Box only (rustc: `*b` of a Box is a place that can be moved out of;
                // other derefs are borrows)
                let mut t = self.body_ty(*x);
                for _ in 0..derefs {
                    t = self.box_inner(t)?;
                    p.push((BOX_STEP, 0));
                }
                p.push((0, fi));
                Some((li, p))
            }
            // `*b` of a Box local: its contents (moving them out leaves the allocation to free)
            ExprKind::Unary(UnOp::Deref, x) => {
                let (li, mut p) = self.field_move_path(*x)?;
                self.box_inner(self.body_ty(*x))?;
                p.push((BOX_STEP, 0));
                Some((li, p))
            }
            _ => None,
        }
    }

    /// T of `Box<T>`; rustc_middle/src/ty/adt.rs:374 identifies the owning ADT by flag.
    fn box_inner(&self, t: TyId) -> Option<TyId> {
        match self.kind(t) {
            TyKind::Adt(d, a, _) if self.u.tcx.adts.get(&d).is_some_and(|a| a.is_box()) && !a.is_empty() => Some(self.u.tcx.tys.list(a)[0]),
            _ => None,
        }
    }

    /// The value at `path` inside local `li` was moved out (path empty: the whole local).
    fn moved_field(&mut self, li: u32, path: &[(u32, u32)]) {
        if path.is_empty() {
            self.moved_local(li);
            return;
        }
        if self.foreign[li as usize] {
            // part of a capture moved out: the capture is not dropped again (its other
            // fields leak; never a double drop)
            self.moved_local(li);
            return;
        }
        let existing = self.field_moved.get(&li).and_then(|v| v.iter().find(|x| x.0 == path).map(|x| x.1));
        let f = match existing {
            Some(f) => f,
            None => {
                let f = self.f.vreg(Cls::I);
                self.entry_inits.push(Inst::Iconst(f, 0));
                self.field_moved.entry(li).or_default().push((path.to_vec(), f));
                f
            }
        };
        self.emit(Inst::Iconst(f, 1));
    }

    /// Drops the value of type `t` at `addr` (path `prefix` inside its local) except the
    /// parts `entries` say were moved out.
    /// A partly moved-out value dropped leaf by leaf (drop_tree): each leaf's drop that may
    /// unwind gets a pad dropping the leaves after it (drop_tree again, skipping the dropped
    /// ones), then the outer cleanup -- rustc's drop ladder over the move paths.
    fn drop_tree_top(&mut self, entries: &[(Vec<(u32, u32)>, VReg)], addr: VReg, t: TyId) {
        let outer = self.pad_fixed.flatten();
        let saved = self.tree_cx.replace(TreeCx { entries: entries.to_vec(), addr, t, skip: 0, counter: 0, outer, building: false });
        self.drop_tree(entries, addr, t, &[]);
        self.tree_cx = saved;
    }

    fn tree_leaf(&mut self, a: VReg, t: TyId) {
        if !self.needs_drop_glue(t) {
            return;
        }
        let id = self.u.fn_id(FnKey::Glue(crate::jit::GLUE_DROP, t));
        let may = self.u.drop_may_unwind(t);
        self.tree_leaf_call_may(Callee::Fn(id), a, may);
    }

    fn tree_leaf_call(&mut self, c: Callee, a: VReg) {
        self.tree_leaf_call_may(c, a, false);
    }

    fn tree_leaf_call_may(&mut self, c: Callee, a: VReg, may: bool) {
        let (k, skip, building) = match self.tree_cx.as_mut() {
            Some(tc) => {
                let k = tc.counter;
                tc.counter += 1;
                (k, tc.skip, tc.building)
            }
            None => {
                self.nounwind += 1;
                self.emit(Inst::Call(c, vec![a], Vec::new()));
                self.nounwind -= 1;
                return;
            }
        };
        if k < skip {
            return;
        }
        let pad = if may && !building && self.unwind_on() && self.nounwind == 0 && self.dead_blk != Some(self.cur) {
            // the leaves after this one, then the outer cleanup
            let tc = self.tree_cx.clone().unwrap();
            let (cur, pos) = (self.cur, self.pos);
            let pb = self.f.block();
            self.switch(pb);
            self.tree_cx = Some(TreeCx { skip: k + 1, counter: 0, building: true, ..tc.clone() });
            self.nounwind += 1;
            self.drop_tree(&tc.entries, tc.addr, tc.t, &[]);
            self.nounwind -= 1;
            match tc.outer {
                Some(o) => self.goto(o),
                None => self.term(Term::Resume),
            }
            self.tree_cx = Some(tc);
            self.cur = cur;
            self.pos = pos;
            Some(pb)
        } else {
            None
        };
        match pad {
            Some(pb) => {
                let saved = self.pad_fixed.replace(Some(pb));
                self.emit(Inst::Call(c, vec![a], Vec::new()));
                self.pad_fixed = saved;
            }
            None => {
                self.nounwind += 1;
                self.emit(Inst::Call(c, vec![a], Vec::new()));
                self.nounwind -= 1;
            }
        }
    }

    fn drop_tree(&mut self, entries: &[(Vec<(u32, u32)>, VReg)], addr: VReg, t: TyId, prefix: &[(u32, u32)]) {
        if !self.needs_drop_glue(t) {
            return;
        }
        let here = entries.iter().find(|x| x.0 == prefix).map(|x| x.1);
        let under = |p: &[(u32, u32)]| entries.iter().any(|x| x.0.len() > p.len() && x.0.starts_with(p));
        let deeper = under(prefix);
        let skip = match here {
            Some(f) => {
                let keep = self.f.block();
                let join = self.f.block();
                self.term(Term::Branch(f, join, keep));
                self.switch(keep);
                Some(join)
            }
            None => None,
        };
        if deeper {
            if let Some(inner) = self.box_inner(t) {
                // a Box something was moved out of: drop what its contents still own, then free
                // the allocation without dropping them again
                let mut sub = prefix.to_vec();
                sub.push((BOX_STEP, 0));
                let ptr = self.f.vreg(Cls::I);
                self.emit(Inst::Load(Mem::Int(8, false), ptr, addr, 0));
                self.drop_tree(entries, ptr, inner, &sub);
                if let Some(free) = self.u.core_path(&["boxed", "box_free_after_move"]) {
                    let id = self.u.fn_id(FnKey::Inst(free, vec![inner]));
                    self.tree_leaf_call(Callee::Fn(id), ptr);
                }
                if let Some(j) = skip {
                    self.goto(j);
                    self.switch(j);
                }
                return;
            }
        }
        if deeper {
            if let TyKind::Array(el, n) = self.kind(t) {
                let n = n.try_to_target_usize(&self.u.tcx.tys).expect("normalized array length");
                // an array some elements were moved out of (`let [a, _] = arr;`): element by
                // element
                // rustc elaborate_drops open_drop_for_array: the elements with move paths
                // ("kept") and the ranges between them are dropped last range first (the drop
                // ladder is built over the reversed list), a range in index order
                let l = self.layout(el);
                let stride = crate::layout::round_up(l.size, l.align);
                let mut groups: Vec<(u64, u64, bool)> = Vec::new();
                let mut start = 0u64;
                for i in 0..n {
                    let mut sub = prefix.to_vec();
                    sub.push((ARRAY_STEP, i as u32));
                    let has = entries.iter().any(|x| x.0.starts_with(&sub));
                    if has {
                        if start < i {
                            groups.push((start, i, false));
                        }
                        groups.push((i, i + 1, true));
                        start = i + 1;
                    }
                }
                if start < n {
                    groups.push((start, n, false));
                }
                // nothing actually moved out at run time: the whole array in index order (rustc
                // elaborates a full drop where the array is initialized on every path; the
                // ladder only where it may be partly moved -- approximated by the flags)
                let sub_flags: Vec<VReg> = entries.iter().filter(|x| x.0.len() > prefix.len() && x.0.starts_with(prefix)).map(|x| x.1).collect();
                let mut any = sub_flags[0];
                for f in &sub_flags[1..] {
                    any = self.ibin(IOp::Or, false, any, *f);
                }
                let ladder = self.f.block();
                let whole = self.f.block();
                let done = self.f.block();
                self.term(Term::Branch(any, ladder, whole));
                self.switch(whole);
                self.tree_leaf(addr, t);
                self.goto(done);
                self.switch(ladder);
                for (a, b, kept) in groups.into_iter().rev() {
                    for i in a..b {
                        let fa = self.addr_add(addr, (i as u32 * stride) as i32);
                        if kept {
                            let mut sub = prefix.to_vec();
                            sub.push((ARRAY_STEP, i as u32));
                            self.drop_tree(entries, fa, el, &sub);
                        } else {
                            self.tree_leaf(fa, el);
                        }
                    }
                }
                self.goto(done);
                self.switch(done);
                if let Some(j) = skip {
                    self.goto(j);
                    self.switch(j);
                }
                return;
            }
        }
        if !deeper {
            if self.drop_misaligned {
                // a place inside a packed struct may be misaligned: moved to an aligned
                // temporary first (rustc add_moves_for_packed_drops)
                let l = self.layout(t);
                let tmp = self.new_slot_for(t);
                self.emit(Inst::Copy(tmp, addr, l.size));
                self.tree_leaf(tmp, t);
            } else {
                self.tree_leaf(addr, t);
            }
        } else {
            let adt = match self.kind(t) {
                TyKind::Adt(d, _, _) => self.u.tcx.adts.get(&d),
                _ => None,
            };
            let base = Place::Mem(addr, 0);
            match adt {
                Some(a) if a.is_enum => {
                    // a move out of variant vi: when any of its flags is set the value is that
                    // variant; drop its remaining fields, else the whole value
                    let join = self.f.block();
                    for vi in 0..a.variants.len() as u32 {
                        let flags: Vec<VReg> = entries.iter().filter(|x| x.0.len() > prefix.len() && x.0.starts_with(prefix) && x.0[prefix.len()].0 == vi).map(|x| x.1).collect();
                        if flags.is_empty() {
                            continue;
                        }
                        let mut any = flags[0];
                        for f in &flags[1..] {
                            any = self.ibin(IOp::Or, false, any, *f);
                        }
                        let yes = self.f.block();
                        let next = self.f.block();
                        self.term(Term::Branch(any, yes, next));
                        self.switch(yes);
                        for fi in 0..a.variants[vi as usize].fields.len() as u32 {
                            let (fp, ft) = self.project(&base, t, vi, fi);
                            let fa = self.place_addr(&fp, ft);
                            let mut sub = prefix.to_vec();
                            sub.push((vi, fi));
                            self.drop_tree(entries, fa, ft, &sub);
                        }
                        self.goto(join);
                        self.switch(next);
                    }
                    self.tree_leaf(addr, t);
                    self.goto(join);
                    self.switch(join);
                }
                _ => {
                    // struct / tuple: field by field (a type with a Drop impl cannot be moved
                    // out of, so its own drop never applies here)
                    let n = match (&adt, self.kind(t)) {
                        (Some(a), _) => a.variants.first().map_or(0, |v| v.fields.len()),
                        (None, TyKind::Tuple(v)) => self.u.tcx.tys.list(v).len(),
                        _ => 0,
                    };
                    let saved = self.drop_misaligned;
                    self.drop_misaligned |= self.is_packed(t);
                    for fi in 0..n as u32 {
                        let (fp, ft) = self.project(&base, t, 0, fi);
                        let fa = self.place_addr(&fp, ft);
                        let mut sub = prefix.to_vec();
                        sub.push((0, fi));
                        self.drop_tree(entries, fa, ft, &sub);
                    }
                    self.drop_misaligned = saved;
                }
            }
        }
        if let Some(j) = skip {
            self.goto(j);
            self.switch(j);
        }
    }

    /// Emits drops for scopes `from..` (innermost first) without popping them.
    fn drop_scopes_from(&mut self, from: usize) {
        if self.dead_blk == Some(self.cur) {
            return;
        }
        let mut seq = Vec::new();
        let mut i = self.scopes.len();
        while i > from {
            i -= 1;
            // locals and `let`-extended temporaries of this scope, in reverse declaration order
            self.scope_drop_acts(i, &mut seq);
        }
        self.emit_drop_seq(seq);
    }

    /// An early exit to scope depth `scope_from` / statement-temporary depth `temps_from`:
    /// innermost first, each scope's statement temporaries, then its locals (rustc scope.rs:
    /// the scopes are exited innermost first, a statement's temporaries being nested inside
    /// the enclosing block's locals). Emitted on the current path only.
    fn exit_scopes(&mut self, scope_from: usize, temps_from: usize) {
        if self.dead_blk == Some(self.cur) {
            return;
        }
        let acts = self.exit_actions(scope_from, temps_from);
        self.emit_drop_seq(acts);
    }

    /// The drops an exit to scope depth `scope_from` / statement-temporary depth `temps_from`
    /// runs, in order: innermost scope first, each scope's statement temporaries, then its
    /// locals and `let`-extended temporaries; a pre-2024 block tail's temporaries drop with the
    /// enclosing scope, before its own statement temporaries (they were made after them).
    fn exit_actions(&self, scope_from: usize, temps_from: usize) -> Vec<DropAct> {
        let mut acts = Vec::new();
        // a scope's statement temporaries, last first; the operands still held by the
        // expressions being evaluated (guards) before the rest
        let temps = |acts: &mut Vec<DropAct>, a: usize, b: usize| {
            let is_guard = |x: &(VReg, TyId, Option<VReg>)| x.2.map_or(false, |f| self.guard_flags.contains_key(&f));
            for x in self.stmt_temps[a..b].iter().rev() {
                if is_guard(x) {
                    acts.push(DropAct::Temp(x.0, x.1, x.2));
                }
            }
            for x in self.stmt_temps[a..b].iter().rev() {
                if !is_guard(x) {
                    acts.push(DropAct::Temp(x.0, x.1, x.2));
                }
            }
        };
        let mut upper = self.stmt_temps.len();
        let mut carried: Vec<(usize, usize)> = Vec::new();
        let mut i = self.scopes.len();
        while i > scope_from {
            i -= 1;
            let mark = self.scope_marks.get(i).copied().unwrap_or(0).max(temps_from);
            for (a, b) in std::mem::take(&mut carried) {
                temps(&mut acts, a, b);
            }
            if mark < upper {
                let tail = self.tail_marks.get(i).copied().flatten().filter(|t| *t >= mark && *t < upper);
                match tail {
                    Some(t) => {
                        temps(&mut acts, mark, t);
                        carried.push((t, upper));
                    }
                    None => temps(&mut acts, mark, upper),
                }
                upper = mark;
            }
            self.scope_drop_acts(i, &mut acts);
        }
        for (a, b) in carried {
            temps(&mut acts, a, b);
        }
        if temps_from < upper {
            temps(&mut acts, temps_from, upper);
        }
        acts
    }

    fn emit_drop_act(&mut self, a: &DropAct) {
        match *a {
            DropAct::Local(li) => self.drop_local_if_live(li),
            DropAct::Temp(p, t, None) => self.drop_at(p, t),
            DropAct::Temp(p, t, Some(f)) => self.drop_flagged_temp(p, t, f),
        }
    }

    /// What a drop action's code depends on (cleanup pads are shared by it).
    fn drop_act_key(&self, a: &DropAct) -> Vec<u64> {
        let mut k: Vec<u64> = Vec::new();
        match *a {
            DropAct::Local(li) => {
                k.push(li as u64);
                k.push(self.drop_flags[li as usize].map_or(u64::MAX, |f| f.0 as u64));
                match &self.locals[li as usize] {
                    Some(LocalSt::Slot(v)) => k.push(0xD000_0000_0000_0000 | v.0 as u64),
                    Some(LocalSt::Regs(r)) => {
                        k.push(0xC000_0000_0000_0000 | r.len() as u64);
                        k.extend(r.iter().map(|v| v.0 as u64));
                    }
                    None => k.push(0xB000_0000_0000_0000),
                }
                if let Some(es) = self.field_moved.get(&li) {
                    for (p, m) in es {
                        k.push(0xA000_0000_0000_0000 | m.0 as u64);
                        k.extend(p.iter().map(|(x, y)| ((*x as u64) << 32) | *y as u64));
                    }
                }
            }
            DropAct::Temp(p, t, f) => {
                k.push(0x9000_0000_0000_0000 | p.0 as u64);
                k.push(t.0 as u64);
                k.push(f.map_or(u64::MAX, |f| f.0 as u64));
                if let Some(es) = f.and_then(|f| self.temp_moved.get(&f.0)) {
                    for (p, m) in es {
                        k.push(0x8000_0000_0000_0000 | m.0 as u64);
                        k.extend(p.iter().map(|(x, y)| ((*x as u64) << 32) | *y as u64));
                    }
                }
            }
        }
        k
    }

    /// Adds a temporary living to the end of scope `d`, after the locals declared so far.
    fn push_scope_temp(&mut self, d: usize, x: (VReg, TyId, Option<VReg>)) {
        let pos = self.scopes.get(d).map_or(0, |s| s.len());
        self.push_scope_temp_at(d, x, pos);
    }

    /// Adds a temporary living to the end of scope `d` that drops after its first `pos`
    /// locals (a value its own `let` / parameter pattern binds from: before those bindings).
    fn push_scope_temp_at(&mut self, d: usize, x: (VReg, TyId, Option<VReg>), pos: usize) {
        self.scope_temps[d].push(x);
        if let Some(p) = self.scope_temp_pos.get_mut(d) {
            p.push(pos);
        }
    }

    /// The drops of scope `i`'s locals and scope temporaries, last declared first.
    fn scope_drop_acts(&self, i: usize, acts: &mut Vec<DropAct>) {
        let locals: &[u32] = self.scopes.get(i).map_or(&[], |v| &v[..]);
        let ts: &[(VReg, TyId, Option<VReg>)] = self.scope_temps.get(i).map_or(&[], |v| &v[..]);
        let pos: &[usize] = self.scope_temp_pos.get(i).map_or(&[], |v| &v[..]);
        let mut t = ts.len();
        let mut k = locals.len();
        loop {
            while t > 0 && pos.get(t - 1).copied().unwrap_or(0) >= k {
                t -= 1;
                let x = ts[t];
                acts.push(DropAct::Temp(x.0, x.1, x.2));
            }
            if k == 0 {
                break;
            }
            k -= 1;
            acts.push(DropAct::Local(locals[k]));
        }
        while t > 0 {
            t -= 1;
            let x = ts[t];
            acts.push(DropAct::Temp(x.0, x.1, x.2));
        }
    }

    fn push_scope(&mut self) {
        self.scopes.push(Vec::new());
        self.scope_temps.push(Vec::new());
        self.scope_temp_pos.push(Vec::new());
        self.scope_marks.push(self.stmt_temps.len());
        self.tail_marks.push(None);
    }

    fn pop_scope(&mut self) {
        let n = self.scopes.len();
        self.drop_scopes_from(n - 1);
        self.scopes.pop();
        self.scope_temps.pop();
        self.scope_temp_pos.pop();
        self.scope_marks.pop();
        self.tail_marks.pop();
    }

    /// `&` operands in extending position of a `let` initializer (rustc's temporary lifetime
    /// extension): `&e`, and through tuples, arrays, struct literals, casts, parens, block tails.
    fn collect_extending(&self, e: ExprId, out: &mut Vec<u32>) {
        let ast = self.ast();
        match &ast.expr(e).kind {
            ExprKind::AddrOf(_, _, x) => {
                let x = strip_paren(ast, *x);
                out.push(x.0);
                self.extend_place_operands(x, out);
                self.collect_extending(x, out);
            }
            ExprKind::Paren(x) | ExprKind::Cast(x, _) => self.collect_extending(*x, out),
            ExprKind::Tuple(v) | ExprKind::Array(v) => {
                for x in ast.list(*v) {
                    self.collect_extending(*x, out);
                }
            }
            ExprKind::Struct(row) => {
                for f in ast.list(ast.row(*row).fields) {
                    self.collect_extending(f.expr, out);
                }
            }
            ExprKind::Block(b, _) | ExprKind::Unsafe(b) => {
                if let Some(Stmt::Expr(x, false)) = ast.stmts(*b).last() {
                    self.collect_extending(*x, out);
                }
            }
            _ => {}
        }
    }

    /// The operands of an extended place expression are extended too (Reference
    /// destructors.scope.lifetime-extension.sub-expressions: "If a borrow, dereference,
    /// field, or tuple indexing expression has an extended temporary scope then so does its
    /// operand. If an indexing expression has an extended temporary scope then the indexed
    /// expression also has an extended temporary scope."): `let s = &vec![1, 2][..];`
    fn extend_place_operands(&self, e: ExprId, out: &mut Vec<u32>) {
        let ast = self.ast();
        let base = match &ast.expr(e).kind {
            ExprKind::Index(b, _) | ExprKind::Field(b, _) | ExprKind::TupleField(b, _) | ExprKind::Unary(UnOp::Deref, b) | ExprKind::AddrOf(_, _, b) => *b,
            _ => return,
        };
        let b = strip_paren(ast, base);
        out.push(b.0);
        self.extend_place_operands(b, out);
    }

    /// Drops the statement temporaries registered after `mark` (innermost first).
    /// Drops the (flagged) statement temporaries from `mark` on this path only: they stay
    /// registered for the other paths, flags cleared here.
    /// After a failed guard dropped them: the flags of scope `d`'s temporaries are clear, so a
    /// later candidate's failure (or-patterns run the guard again) does not drop them again.
    fn clear_scope_temp_flags(&mut self, d: usize) {
        let fs: Vec<VReg> = self.scope_temps.get(d).map(|v| v.iter().filter_map(|x| x.2).collect()).unwrap_or_default();
        for f in fs {
            self.emit(Inst::Iconst(f, 0));
        }
    }

    fn drop_stmt_temps_here(&mut self, mark: usize) {
        let ts: Vec<(VReg, TyId, Option<VReg>)> = self.stmt_temps.get(mark..).map(|x| x.to_vec()).unwrap_or_default();
        for (a, t, f) in ts.into_iter().rev() {
            if let Some(f) = f {
                self.drop_flagged_temp(a, t, f);
                self.emit(Inst::Iconst(f, 0));
            }
        }
    }

    fn drop_stmt_temps_from(&mut self, mark: usize) {
        if mark >= self.stmt_temps.len() {
            return;
        }
        let ts = self.stmt_temps.split_off(mark);
        let seq = ts.iter().rev().map(|(a, t, f)| DropAct::Temp(*a, *t, *f)).collect();
        self.emit_drop_seq(seq);
    }

    /// Root local of a place expression (for partial moves).
    fn place_root_local(&self, e: ExprId) -> Option<u32> {
        match &self.ast().expr(e).kind {
            ExprKind::Path(_) => match self.b.res.get(&e.0) {
                Some(Res::Local(li)) => Some(*li),
                _ => None,
            },
            ExprKind::Field(x, _) | ExprKind::TupleField(x, _) | ExprKind::Paren(x) => self.place_root_local(*x),
            // `*b` of a Box: still a place owned by `b` (rustc move_paths/builder.rs: a Deref
            // of a Box is a move path, of a reference / raw pointer none), so the arms' moves
            // out of `match *b` are tracked in `b` and its drop frees the box without them
            ExprKind::Unary(UnOp::Deref, x) if self.box_inner(self.body_ty(*x)).is_some() => self.place_root_local(*x),
            _ => None,
        }
    }

    // ------------------------------------------------------------ patterns

    fn bind_pat(&mut self, p: PatId, v: Val, t: TyId) {
        let ast = self.ast();
        match ast.pat(p) {
            Pat::Ident { sub: None, by_ref: false, .. } if !self.b.pat_res.contains_key(&p.0) && !self.b.pat_bind_ref.contains_key(&p.0) => {
                if let Some(&li) = self.b.pat_local.get(&p.0) {
                    // an aggregate rvalue in memory is a fresh temporary (or a by-pointer
                    // parameter copy): the local takes it over instead of copying it
                    self.drop_local_before_rebind(li);
                    if let (Val::M(base, off), None) = (&v, &self.locals[li as usize]) {
                        let l = self.layout(t);
                        if l.leaves.is_none() {
                            let a = self.addr_add(*base, *off);
                            self.locals[li as usize] = Some(LocalSt::Slot(a));
                            self.own_local(li);
                            return;
                        }
                    }
                    let pl = self.local_place(li);
                    self.write_place(&pl, t, v);
                    self.own_local(li);
                }
            }
            _ => {
                let place = self.val_to_place(v);
                self.bind_pat_place(p, &place, t);
            }
        }
    }

    /// Binds the variables of an (already tested) pattern from a place.
    fn bind_pat_place(&mut self, p: PatId, place: &Place, t: TyId) {
        let saved = self.pat_path.clone();
        if self.b.pat_derefs.get(&p.0).map_or(false, |n| *n > 0) {
            self.pat_path = None;
        }
        self.bind_pat_place_inner(p, place, t);
        self.pat_path = saved;
    }

    /// bind_pat_place one projection deeper: (variant, field) appended to `pat_path`.
    fn bind_pat_field(&mut self, p: PatId, place: &Place, t: TyId, vi: u32, fi: u32) {
        let saved = self.pat_path.clone();
        if let Some(pp) = &mut self.pat_path {
            pp.push((vi, fi));
        }
        self.bind_pat_place(p, place, t);
        self.pat_path = saved;
    }

    fn bind_pat_place_inner(&mut self, p: PatId, place: &Place, t: TyId) {
        let ast = self.ast();
        let (pd, td) = self.pat_deref(p, place, t);
        let (place, t) = (&pd, td);
        match ast.pat(p) {
            Pat::Wild | Pat::Rest | Pat::Lit(_) | Pat::Range(..) | Pat::Path(_) => {}
            Pat::Ident { by_ref, sub, .. } => {
                if self.b.pat_res.contains_key(&p.0) {
                    return;
                }
                if let Some(&li) = self.b.pat_local.get(&p.0) {
                    let lt = self.monomorphize(self.b.locals[li as usize].ty);
                    let by_ref = *by_ref || self.b.pat_bind_ref.contains_key(&p.0);
                    let v = if by_ref {
                        match place {
                            // an unsized place (slice / str): the reference is (ptr, len)
                            Place::Regs(r) if self.unsized_ty(t) => Val::L(r.clone()),
                            _ => {
                                let a = self.place_addr(place, t);
                                Val::L(vec![a])
                            }
                        }
                    } else {
                        if self.needs_drop_glue(t) {
                            // moving out of the matched place: its owner no longer drops it
                            // (only this part of it when the path is known)
                            let mv = match self.match_root {
                                Some(MatchRoot::Local(r)) => match self.pat_path.clone() {
                                    Some(p) => Some(PendingMove::Field(r, p)),
                                    None => Some(PendingMove::Local(r)),
                                },
                                Some(MatchRoot::Temp(f)) => match self.pat_path.clone() {
                                    // a part of the temporary: only that part is not dropped
                                    Some(p) if !p.is_empty() => Some(PendingMove::TempPart(f, p)),
                                    _ => Some(PendingMove::Temp(f)),
                                },
                                None => None,
                            };
                            if let Some(mv) = mv {
                                match &mut self.defer_moves {
                                    Some(v) => v.push(mv),
                                    None => self.do_move(mv),
                                }
                            }
                        }
                        self.read_place(place, t)
                    };
                    self.drop_local_before_rebind(li);
                    let pl = self.local_place(li);
                    self.write_place(&pl, lt, v);
                    if !by_ref {
                        self.own_local(li);
                    }
                }
                if let Some(s) = sub {
                    self.bind_pat_place(*s, place, t);
                }
            }
            Pat::Paren(x) => self.bind_pat_place(*x, place, t),
            Pat::Box(x) => {
                self.pat_path = None;
                self.bind_pat_place(*x, place, t)
            }
            Pat::Ref(_, x) => {
                let inner = match self.kind(t) {
                    TyKind::Ref(_, i, _) => i,
                    _ => t,
                };
                let ptr = self.read_place(place, t);
                let ptr = self.to_leaves(ptr, t);
                if !ptr.is_empty() {
                    self.pat_path = None;
                    // `&[a, ..]` against `&[T]`: the pointee place keeps the length
                    let pl = if self.unsized_ty(inner) { Place::Regs(ptr) } else { Place::Mem(ptr[0], 0) };
                    self.bind_pat_place(*x, &pl, inner);
                }
            }
            Pat::Tuple(v) => {
                let n = match self.kind(t) {
                    TyKind::Tuple(x) => self.u.tcx.tys.list(x).len(),
                    _ => 0,
                };
                let idx = seq_indices(ast, ast.list(*v), n);
                for (pi, fi) in idx {
                    let (fp, ft) = self.project(place, t, 0, fi);
                    self.bind_pat_field(ast.list(*v)[pi], &fp, ft, 0, fi);
                }
            }
            Pat::TupleStruct(_, v) => {
                let vi = self.variant_of_pat(p);
                let n = self.variant_len(t, vi);
                let idx = seq_indices(ast, ast.list(*v), n);
                for (pi, fi) in idx {
                    let (fp, ft) = self.project(place, t, vi, fi);
                    self.bind_pat_field(ast.list(*v)[pi], &fp, ft, vi, fi);
                }
            }
            Pat::Struct(_, fields, _) => {
                let vi = self.variant_of_pat(p);
                for fp in ast.list(*fields) {
                    let fname = { let tx = self.ident_text(fp.name); match crate::ast::tuple_index(tx).ok_or(()) { Ok(n) => n | 0x8000_0000, Err(_) => self.u.prog.syms.get(tx).unwrap_or(u32::MAX) } };
                    if let Some(fi) = self.field_index(t, vi, fname) {
                        let (pl, ft) = self.project(place, t, vi, fi);
                        self.bind_pat_field(fp.pat, &pl, ft, vi, fi);
                    }
                }
            }
            Pat::Slice(v) => {
                let v = ast.list(*v).to_vec();
                let sp = match self.slice_pat(&v, place, t) {
                    Some(x) => x,
                    None => return,
                };
                // moving array elements out by pattern: a fixed-length array tracks each
                // element (rustc move paths of constant indices); a slice / `x @ ..` part, the
                // whole owner
                let fixed = matches!(self.kind_ref(t), TyKind::Array(..)) && sp.len.is_none();
                let base_path = if fixed { self.pat_path.clone() } else { None };
                self.pat_path = None;
                for (i, x) in v.iter().enumerate() {
                    if Some(i) == sp.rest {
                        if let Pat::Ident { by_ref, .. } = ast.pat(*x) {
                            let (pl, st) = self.slice_pat_rest(&sp, v.len());
                            self.pat_path = None;
                            // `x @ ..` of a fixed-length array moves the elements it covers, each
                            // its own move path (rustc subslice move paths): the others still drop
                            let by_value = !*by_ref && !self.b.pat_bind_ref.contains_key(&x.0) && self.needs_drop_glue(sp.elem);
                            match (&base_path, by_value) {
                                (Some(bp), true) => {
                                    let after = v.len() - i - 1;
                                    let mut mvs = Vec::new();
                                    for idx in i..(sp.n as usize - after) {
                                        let mut p2 = bp.clone();
                                        p2.push((ARRAY_STEP, idx as u32));
                                        match self.match_root {
                                            Some(MatchRoot::Local(r)) => mvs.push(PendingMove::Field(r, p2)),
                                            Some(MatchRoot::Temp(f)) => mvs.push(PendingMove::TempPart(f, p2)),
                                            None => {}
                                        }
                                    }
                                    for mv in mvs {
                                        match &mut self.defer_moves {
                                            Some(dv) => dv.push(mv),
                                            None => self.do_move(mv),
                                        }
                                    }
                                    let saved_root = self.match_root.take();
                                    self.bind_pat_place(*x, &pl, st);
                                    self.match_root = saved_root;
                                }
                                _ => self.bind_pat_place(*x, &pl, st),
                            }
                        }
                        continue;
                    }
                    let pl = self.slice_pat_elem(&sp, v.len(), i);
                    self.pat_path = base_path.clone().map(|mut p| {
                        let idx = match sp.rest {
                            Some(r) if i > r => sp.n as usize - (v.len() - i),
                            _ => i,
                        };
                        p.push((ARRAY_STEP, idx as u32));
                        p
                    });
                    self.bind_pat_place(*x, &pl, sp.elem);
                }
            }
            Pat::Or(alts) if self.or_force.contains_key(&p.0) => {
                let k = self.or_force[&p.0];
                let a = ast.list(*alts)[k];
                self.bind_pat_place(a, place, t);
            }
            Pat::Or(alts) => {
                // bind the alternative that matched (rustc: each alternative binds the same names)
                let alts = alts.clone();
                let mut bound = Vec::new();
                self.pat_binding_locals(p, &mut bound);
                if bound.is_empty() {
                    return;
                }
                let sel = match self.or_sel.get(&p.0) {
                    Some(r) => *r,
                    None => {
                        // irrefutable position (`let (Ok(x) | Err(x)) = r;`): find the alternative now
                        let none = self.f.block();
                        let go = self.f.block();
                        self.test_pat(p, place, t, none);
                        self.goto(go);
                        self.switch(none);
                        self.term(Term::Unreachable);
                        self.switch(go);
                        self.or_sel[&p.0]
                    }
                };
                // the bound locals get their storage here, before the branches (a slot address
                // made in one alternative's block would not dominate the others)
                let mut lis = Vec::new();
                self.pat_binding_locals(ast.list(alts)[0], &mut lis);
                for li in lis {
                    self.local_place(li);
                }
                let join = self.f.block();
                for (i, a) in ast.list(alts).iter().enumerate() {
                    if i + 1 < alts.len() {
                        let k = self.iconst(i as i64);
                        let c = self.icmp(Cond::Eq, false, sel, k);
                        let yes = self.f.block();
                        let no = self.f.block();
                        self.term(Term::Branch(c, yes, no));
                        self.switch(yes);
                        self.bind_pat_place(*a, place, t);
                        self.goto(join);
                        self.switch(no);
                    } else {
                        self.bind_pat_place(*a, place, t);
                        self.goto(join);
                    }
                }
                self.switch(join);
            }
            Pat::Mac(_) => {}
        }
    }

    /// Binds pattern `p` to the value `v: t` kept as a temporary of scope `scope`: what the pattern
    /// does not move out of it drops when that scope ends (per part).
    fn bind_pat_owned(&mut self, p: PatId, v: Val, t: TyId, scope: usize) {
        let place = self.val_to_place(v);
        let a = self.place_addr(&place, t);
        let f = self.f.vreg(Cls::I);
        self.entry_inits.push(Inst::Iconst(f, 0));
        self.emit(Inst::Iconst(f, 1));
        let saved = (self.match_root, self.pat_path.take());
        self.match_root = Some(MatchRoot::Temp(f));
        self.pat_path = Some(Vec::new());
        // the argument is a variable declared before the pattern's bindings: it drops after them
        let pos = self.scopes.get(scope).map_or(0, |s| s.len());
        self.bind_pat_place(p, &Place::Mem(a, 0), t);
        self.match_root = saved.0;
        self.pat_path = saved.1;
        if scope < self.scope_temps.len() {
            self.push_scope_temp_at(scope, (a, t, Some(f)), pos);
        }
    }

    /// Does pattern `p` move a value that needs dropping out of what it matches (a by-value
    /// binding of such a type)? Without one, a place is only read / borrowed, never consumed.
    fn pat_moves_droppable(&mut self, p: PatId) -> bool {
        let ast = self.ast();
        match ast.pat(p).clone() {
            Pat::Ident { by_ref, sub, .. } => {
                let mut moves = false;
                if !by_ref && !self.b.pat_bind_ref.contains_key(&p.0) && !self.b.pat_res.contains_key(&p.0) {
                    if let Some(&li) = self.b.pat_local.get(&p.0) {
                        let lt = self.monomorphize(self.b.locals[li as usize].ty);
                        moves = self.needs_drop_glue(lt);
                    }
                }
                moves || sub.map_or(false, |x| self.pat_moves_droppable(x))
            }
            Pat::TupleStruct(_, v) | Pat::Tuple(v) | Pat::Slice(v) | Pat::Or(v) => ast.list(v).iter().any(|x| self.pat_moves_droppable(*x)),
            Pat::Struct(_, fs, _) => ast.list(fs).iter().any(|f| self.pat_moves_droppable(f.pat)),
            Pat::Ref(_, x) | Pat::Paren(x) | Pat::Box(x) => self.pat_moves_droppable(x),
            _ => false,
        }
    }

    /// Does pattern `p` bind anything by mutable reference (`ref mut x` or binding mode)?
    fn pat_binds_mut_ref(&self, p: PatId) -> bool {
        let ast = self.ast();
        match ast.pat(p) {
            Pat::Ident { by_ref, mutbl, sub, .. } => {
                (*by_ref && *mutbl) || self.b.pat_bind_ref.get(&p.0) == Some(&2) || sub.map_or(false, |x| self.pat_binds_mut_ref(x))
            }
            Pat::TupleStruct(_, v) | Pat::Tuple(v) | Pat::Slice(v) | Pat::Or(v) => ast.list(*v).iter().any(|x| self.pat_binds_mut_ref(*x)),
            Pat::Struct(_, fs, _) => ast.list(*fs).iter().any(|f| self.pat_binds_mut_ref(f.pat)),
            Pat::Ref(_, x) | Pat::Paren(x) | Pat::Box(x) => self.pat_binds_mut_ref(*x),
            _ => false,
        }
    }

    /// Does pattern `p` bind anything by reference (`ref x`, or by the default binding mode)?
    fn pat_binds_by_ref(&self, p: PatId) -> bool {
        let mut lis = Vec::new();
        self.pat_binding_locals(p, &mut lis);
        let ast = self.ast();
        let mut found = false;
        fn walk(s: &Lcx, ast: &Ast, p: PatId, found: &mut bool) {
            match ast.pat(p) {
                Pat::Ident { by_ref, sub, .. } => {
                    if *by_ref || s.b.pat_bind_ref.contains_key(&p.0) {
                        *found = true;
                    }
                    if let Some(x) = sub {
                        walk(s, ast, *x, found);
                    }
                }
                Pat::TupleStruct(_, v) | Pat::Tuple(v) | Pat::Slice(v) | Pat::Or(v) => {
                    for x in ast.list(*v) {
                        walk(s, ast, *x, found);
                    }
                }
                Pat::Struct(_, fs, _) => {
                    for f in ast.list(*fs) {
                        walk(s, ast, f.pat, found);
                    }
                }
                Pat::Ref(_, x) | Pat::Paren(x) | Pat::Box(x) => walk(s, ast, *x, found),
                _ => {}
            }
        }
        let _ = lis;
        walk(self, ast, p, &mut found);
        found
    }

    /// Elements of a tuple / array type (types and their places in `pl`), primitives only.
    fn prim_elems(&mut self, pl: &Place, t: TyId) -> Option<Vec<(Place, TyId)>> {
        let is_prim = |k: &TyKind| matches!(k, TyKind::Int(_) | TyKind::Float(_) | TyKind::Bool | TyKind::Char);
        match self.kind(t) {
            TyKind::Tuple(v) => {
                let mut out = Vec::new();
                for i in 0..self.u.tcx.tys.list(v).len() {
                    if !is_prim(&self.kind(self.u.tcx.tys.list(v)[i])) {
                        return None;
                    }
                    out.push(self.project(pl, t, 0, i as u32));
                }
                Some(out)
            }
            TyKind::Array(e, n) if is_prim(&self.kind(e)) && n.try_to_target_usize(&self.u.tcx.tys).is_some_and(|n| n <= 64) => {
                let n = n.try_to_target_usize(&self.u.tcx.tys).expect("normalized array length");
                let l = self.layout(e);
                let stride = crate::layout::round_up(l.size, l.align);
                let base = self.place_addr(pl, t);
                let mut out = Vec::new();
                for i in 0..n as u32 {
                    let a = self.addr_add(base, (i * stride) as i32);
                    out.push((Place::Mem(a, 0), e));
                }
                Some(out)
            }
            _ => None,
        }
    }

    /// `a cond b` (Lt/Le/Gt/Ge) for tuples / arrays of primitives, lexicographically.
    fn lex_cmp(&mut self, pa: &Place, pb: &Place, t: TyId, cond: Cond) -> Option<VReg> {
        let ea = self.prim_elems(pa, t)?;
        let eb = self.prim_elems(pb, t)?;
        let res = self.f.vreg(Cls::I);
        let join = self.f.block();
        let strict = match cond {
            Cond::Lt | Cond::Le => Cond::Lt,
            _ => Cond::Gt,
        };
        for ((xa, et), (xb, _)) in ea.iter().zip(eb.iter()) {
            let x = self.read_place(xa, *et);
            let x = self.to_leaves(x, *et);
            let y = self.read_place(xb, *et);
            let y = self.to_leaves(y, *et);
            let fl = matches!(self.kind_ref(*et), TyKind::Float(_));
            let f64_ = self.is_f64(*et);
            let signed = matches!(self.kind_ref(*et), TyKind::Int(_)) && self.int_ty(*et).signed;
            let eq = self.f.vreg(Cls::I);
            if fl {
                self.emit(Inst::FCmp(Cond::Eq, f64_, eq, x[0], y[0]));
            } else {
                self.emit(Inst::ICmp(Cond::Eq, signed, eq, x[0], y[0]));
            }
            let next = self.f.block();
            let differ = self.f.block();
            self.term(Term::Branch(eq, next, differ));
            self.switch(differ);
            if fl {
                self.emit(Inst::FCmp(strict, f64_, res, x[0], y[0]));
            } else {
                self.emit(Inst::ICmp(strict, signed, res, x[0], y[0]));
            }
            self.goto(join);
            self.switch(next);
        }
        // every element equal: only `<=` / `>=` hold
        let all_eq = matches!(cond, Cond::Le | Cond::Ge) as i64;
        self.emit(Inst::Iconst(res, all_eq));
        self.goto(join);
        self.switch(join);
        Some(res)
    }

    /// Drops the unsized value at `p` with metadata `len`: a slice's elements, or a struct's
    /// fields with a slice tail.
    fn drop_unsized(&mut self, p: VReg, len: VReg, t: TyId) {
        match self.kind(t) {
            TyKind::Slice(el) if self.needs_drop_glue(el) => {
                let l = self.layout(el);
                let stride = crate::layout::round_up(l.size, l.align);
                let i = self.f.vreg(Cls::I);
                self.emit(Inst::Iconst(i, 0));
                let head = self.f.block();
                let body = self.f.block();
                let exit = self.f.block();
                self.goto(head);
                self.switch(head);
                let c = self.f.vreg(Cls::I);
                self.emit(Inst::ICmp(Cond::Lt, false, c, i, len));
                self.term(Term::Branch(c, body, exit));
                self.switch(body);
                let st = self.iconst(stride as i64);
                let off = self.f.vreg(Cls::I);
                self.emit(Inst::IBin(IOp::Mul, U64, off, i, st));
                let a = self.f.vreg(Cls::I);
                self.emit(Inst::IBin(IOp::Add, U64, a, p, off));
                if self.unwind_on() && self.nounwind == 0 && self.u.drop_may_unwind(el) {
                    // an element drop that panics: the elements after it still drop (rustc's
                    // drop_in_place::<[T]> drop ladder), then unwinding goes on
                    let (cur, pos) = (self.cur, self.pos);
                    let pad = self.f.block();
                    let ph = self.f.block();
                    let pb = self.f.block();
                    let pr = self.f.block();
                    self.switch(pad);
                    let j = self.f.vreg(Cls::I);
                    self.emit(Inst::IBinI(IOp::Add, U64, j, i, 1));
                    self.goto(ph);
                    self.switch(ph);
                    let c2 = self.f.vreg(Cls::I);
                    self.emit(Inst::ICmp(Cond::Lt, false, c2, j, len));
                    self.term(Term::Branch(c2, pb, pr));
                    self.switch(pb);
                    let off2 = self.f.vreg(Cls::I);
                    self.emit(Inst::IBinI(IOp::Mul, U64, off2, j, stride as i64));
                    let a2 = self.f.vreg(Cls::I);
                    self.emit(Inst::IBin(IOp::Add, U64, a2, p, off2));
                    self.nounwind += 1;
                    self.drop_at(a2, el);
                    self.nounwind -= 1;
                    self.emit(Inst::IBinI(IOp::Add, U64, j, j, 1));
                    self.goto(ph);
                    self.switch(pr);
                    self.term(Term::Resume);
                    self.cur = cur;
                    self.pos = pos;
                    let saved = self.pad_fixed.replace(Some(pad));
                    self.drop_at(a, el);
                    self.pad_fixed = saved;
                } else {
                    self.drop_at(a, el);
                }
                let one = self.iconst(1);
                self.emit(Inst::IBin(IOp::Add, U64, i, i, one));
                self.goto(head);
                self.switch(exit);
            }
            TyKind::Dyn(..) => {
                // a `dyn` tail: through the vtable's drop entry (0 when nothing to drop)
                let fp = self.f.vreg(Cls::I);
                self.emit(Inst::Load(Mem::Int(8, false), fp, len, (VT_DROP * 8) as i32));
                let yes = self.f.block();
                let join = self.f.block();
                self.term(Term::Branch(fp, yes, join));
                self.switch(yes);
                self.nounwind += 1;
                self.emit(Inst::Call(Callee::Indirect(fp), vec![p], Vec::new()));
                self.nounwind -= 1;
                self.goto(join);
                self.switch(join);
            }
            TyKind::Adt(d, _, _) => {
                let adt = match self.u.tcx.adts.get(&d) {
                    Some(a) if !a.is_enum && !a.is_union => a,
                    _ => return,
                };
                // the type's own Drop first, then the fields (the tail last)
                if let (Some(dtor), Some(td)) = (self.u.tcx.adt_destructor(d), self.u.tcx.lang_items.get(LangItem::Drop)) {
                    {
                        let trait_ref = relax_types::next_solver::TraitRef { def_id: td, args: vec![relax_types::consts::GenericArg::Type(t)] };
                        let selected = self.u.tcx.codegen_select_candidate(&self.u.prog, crate::tcx::TypingEnv::fully_monomorphized(&self.u.prog), trait_ref.clone()).unwrap_or_else(|error| panic!("established destructor dispatch failed for {trait_ref:?}: {error:?}"));
                        let relax_types::next_solver::select::ImplSource::UserDefined { args, .. } = selected else { panic!("destructor without impl") };
                        let mut iargs = Vec::new();
                        for arg in args { match arg {
                            relax_types::consts::GenericArg::Type(t) => iargs.push(t),
                            relax_types::consts::GenericArg::Const(c) => iargs.push(self.u.tcx.tys.intern(TyKind::Const(c))),
                            relax_types::consts::GenericArg::Lifetime(_) => {},
                        } }
                        let id = self.u.fn_id(FnKey::Inst(dtor.did, iargs));
                        // `&mut self` of an unsized type: (data, metadata)
                        self.emit(Inst::Call(Callee::Fn(id), vec![p, len], Vec::new()));
                    }
                }
                let n = adt.variants[0].fields.len();
                let place = Place::Regs(vec![p, len]);
                for fi in 0..n {
                    // Project through the fat pointer: a dyn tail's offset must
                    // be rounded to its vtable alignment, just like a field read.
                    let (field, ft) = self.project(&place, t, 0, fi as u32);
                    let ptr = self.ref_to_place(&field, ft);
                    if fi + 1 == n && self.unsized_ty(ft) {
                        self.drop_unsized(ptr[0], ptr[1], ft);
                    } else {
                        self.drop_at(ptr[0], ft);
                    }
                }
            }
            _ => {}
        }
    }

    /// Could evaluating `e` leave the enclosing expression early (`?`, return, break,
    /// continue)? A conservative scan of its source text.
    fn may_exit(&self, e: ExprId) -> bool {
        let ex = self.ast().expr(e);
        let src = &self.u.prog.files[self.b.file as usize].src;
        let (lo, hi) = (ex.lo as usize, (ex.hi as usize).min(src.len()));
        if lo >= hi {
            return false;
        }
        let t = &src[lo..hi];
        t.contains(&b'?') || t.windows(6).any(|w| w == b"return") || t.windows(5).any(|w| w == b"break") || t.windows(8).any(|w| w == b"continue")
    }

    /// `may_exit`, or the operand may unwind: a call, a macro, an index (rustc's operand temps
    /// drop on the unwind edge of a later operand's call too: `f(s, g())` with g panicking
    /// drops s).
    fn may_exit_or_unwind(&self, e: ExprId) -> bool {
        if self.may_exit(e) {
            return true;
        }
        if !self.unwind_on() {
            return false;
        }
        let ex = self.ast().expr(e);
        let src = &self.u.prog.files[self.b.file as usize].src;
        let (lo, hi) = (ex.lo as usize, (ex.hi as usize).min(src.len()));
        lo < hi && src[lo..hi].iter().any(|c| matches!(c, b'(' | b'[' | b'!'))
    }

    fn unwind_on(&self) -> bool {
        let options = &self.u.prog.session.options;
            !(!options.unwind)
    }

    /// An operand already evaluated while later ones may still exit early (`f(a(), b()?)`):
    /// kept in memory with a drop flag so the exit drops it (rustc drops the evaluated
    /// operands); `unguard` clears the flags once the operands are consumed.
    fn guard_operand(&mut self, rest: &[ExprId], v: Val, t: TyId, guards: &mut Vec<VReg>) -> Val {
        if !self.needs_drop_glue(t) || !rest.iter().any(|x| self.may_exit_or_unwind(*x)) {
            return v;
        }
        let pl = self.val_to_place(v);
        let a = self.place_addr(&pl, t);
        let f = self.f.vreg(Cls::I);
        self.entry_inits.push(Inst::Iconst(f, 0));
        self.emit(Inst::Iconst(f, 1));
        self.stmt_temps.push((a, t, Some(f)));
        self.guard_flags.insert(f, ());
        guards.push(f);
        Val::M(a, 0)
    }

    fn unguard(&mut self, guards: &[VReg]) {
        for f in guards {
            self.emit(Inst::Iconst(*f, 0));
        }
    }

    /// Locals bound by pattern `p` (in pattern order).
    fn pat_binding_locals(&self, p: PatId, out: &mut Vec<u32>) {
        let ast = self.ast();
        match ast.pat(p) {
            Pat::Ident { sub, .. } => {
                if let Some(&li) = self.b.pat_local.get(&p.0) {
                    if !self.b.pat_res.contains_key(&p.0) {
                        out.push(li);
                    }
                }
                if let Some(s) = sub {
                    self.pat_binding_locals(*s, out);
                }
            }
            Pat::TupleStruct(_, v) | Pat::Tuple(v) | Pat::Slice(v) | Pat::Or(v) => {
                for x in ast.list(*v) {
                    self.pat_binding_locals(*x, out);
                }
            }
            Pat::Struct(_, fs, _) => {
                for f in ast.list(*fs) {
                    self.pat_binding_locals(f.pat, out);
                }
            }
            Pat::Ref(_, x) | Pat::Paren(x) | Pat::Box(x) => self.pat_binding_locals(*x, out),
            _ => {}
        }
    }

    /// The parts of a place matched by a slice pattern: element base address, dynamic
    /// length (slices) or constant length (arrays), element type and stride.
    fn slice_pat(&mut self, v: &[PatId], place: &Place, t: TyId) -> Option<SlicePat> {
        let ast = self.ast();
        let rest = v.iter().position(|x| crate::typeck::is_rest_pat(ast, *x));
        let fixed = v.len() - rest.is_some() as usize;
        let (elem, base, len, n) = match self.kind(t) {
            TyKind::Array(e, n) => {
                let n = n.try_to_target_usize(&self.u.tcx.tys).expect("normalized array length");
                let a = self.place_addr(place, t);
                (e, a, None, n)
            }
            TyKind::Slice(e) => match place {
                Place::Regs(r) if r.len() == 2 => (e, r[0], Some(r[1]), 0),
                _ => {
                    self.err(self.pos, "internal error: slice place without a length".to_string());
                    return None;
                }
            },
            _ => return None,
        };
        let el = self.layout(elem);
        let stride = crate::layout::round_up(el.size, el.align);
        Some(SlicePat { base, len, n, elem, stride, rest, fixed })
    }

    /// Place of pattern `i` (of `count`) of a slice pattern.
    fn slice_pat_elem(&mut self, sp: &SlicePat, count: usize, i: usize) -> Place {
        let before = match sp.rest {
            Some(r) => i < r,
            None => true,
        };
        if before {
            return Place::Mem(sp.base, (i as u32 * sp.stride) as i32);
        }
        // after the rest: element (len - (count - i))
        let from_end = (count - i) as u64;
        match sp.len {
            None => Place::Mem(sp.base, ((sp.n.saturating_sub(from_end)) as u32 * sp.stride) as i32),
            Some(len) => {
                let k = self.iconst(from_end as i64);
                let idx = self.ibin(IOp::Sub, false, len, k);
                let s = self.iconst(sp.stride as i64);
                let off = self.ibin(IOp::Mul, false, idx, s);
                let a = self.ibin(IOp::Add, false, sp.base, off);
                Place::Mem(a, 0)
            }
        }
    }

    /// Place and type of the `name @ ..` part of a slice pattern.
    fn slice_pat_rest(&mut self, sp: &SlicePat, count: usize) -> (Place, TyId) {
        let r = sp.rest.unwrap_or(0);
        let after = (count - r - 1) as u64;
        let off = (r as u32 * sp.stride) as i32;
        match sp.len {
            None => {
                let m = sp.n.saturating_sub(sp.fixed as u64);
                let t = self.u.tcx.tys.intern(TyKind::Array(sp.elem, self.u.tcx.tys.const_usize(m)));
                (Place::Mem(sp.base, off), t)
            }
            Some(len) => {
                let a = self.addr_add(sp.base, off);
                let k = self.iconst((r as u64 + after) as i64);
                let l = self.ibin(IOp::Sub, false, len, k);
                let t = self.u.tcx.tys.intern(TyKind::Slice(sp.elem));
                (Place::Regs(vec![a, l]), t)
            }
        }
    }

    fn ident_text(&self, id: Ident) -> &'a str {
        let f = &self.u.prog.files[self.b.file as usize];
        let p: *const crate::parser::ParsedFile = &**f;
        let f = unsafe { &*p };
        f.text(id)
    }

    fn variant_of_pat(&self, p: PatId) -> u32 {
        match self.b.pat_res.get(&p.0) {
            Some(Res::Def(d)) => {
                let def = self.u.prog.def(*d);
                if def.kind == DefKind::Variant {
                    def.sub
                } else {
                    0
                }
            }
            _ => 0,
        }
    }

    fn variant_len(&self, t: TyId, vi: u32) -> usize {
        if let TyKind::Adt(d, _, _) = self.u.tcx.tys.kind(t) {
            if let Some(a) = self.u.tcx.adts.get(d) {
                return a.variants.get(vi as usize).map_or(0, |v| v.fields.len());
            }
        }
        0
    }

    fn field_index(&self, t: TyId, vi: u32, name: u32) -> Option<u32> {
        if let TyKind::Adt(d, _, _) = self.u.tcx.tys.kind(t) {
            if let Some(a) = self.u.tcx.adts.get(d) {
                let v = a.variants.get(vi as usize)?;
                for (i, f) in v.fields.iter().enumerate() {
                    if f.name == name {
                        return Some(i as u32);
                    }
                }
            }
        }
        None
    }

    /// Emits the test of a refutable pattern: falls through on match, jumps to `fail` otherwise.
    /// Match ergonomics: the place a pattern matches after peeling its recorded references.
    fn pat_deref(&mut self, p: PatId, place: &Place, t: TyId) -> (Place, TyId) {
        let n = match self.b.pat_derefs.get(&p.0) {
            Some(n) => *n,
            None => return (place.clone(), t),
        };
        let mut pl = place.clone();
        let mut pt = t;
        for _ in 0..n {
            let inner = match self.kind(pt) {
                TyKind::Ref(_, i, _) => i,
                _ => break,
            };
            let v = self.read_place(&pl, pt);
            let r = self.to_leaves(v, pt);
            pl = if self.unsized_ty(inner) { Place::Regs(r) } else { Place::Mem(r[0], 0) };
            pt = inner;
        }
        (pl, pt)
    }

    fn test_pat(&mut self, p: PatId, place: &Place, t: TyId, fail: u32) {
        let ast = self.ast();
        let (pd, td) = self.pat_deref(p, place, t);
        let (place, t) = (&pd, td);
        match ast.pat(p) {
            Pat::Wild | Pat::Rest => {}
            Pat::Ident { sub, .. } => {
                if let Some(Res::Def(d)) = self.b.pat_res.get(&p.0).copied() {
                    self.test_def_pat(p, d, place, t, fail);
                }
                if let Some(s) = sub {
                    self.test_pat(*s, place, t, fail);
                }
            }
            Pat::Path(_) => {
                if let Some(Res::Def(d)) = self.b.pat_res.get(&p.0).copied() {
                    self.test_def_pat(p, d, place, t, fail);
                }
            }
            Pat::Paren(x) | Pat::Box(x) => self.test_pat(*x, place, t, fail),
            Pat::Lit(e) => {
                let v = self.read_place(place, t);
                let lv = self.lower_expr_coerced(*e, t);
                let c = self.eq_vals(v, lv, t);
                let ok = self.f.block();
                self.term(Term::Branch(c, ok, fail));
                self.switch(ok);
            }
            Pat::Range(a, b, incl) => {
                let v = self.read_place(place, t);
                let v = self.to_leaves(v, t);
                let fl = matches!(self.kind_ref(t), TyKind::Float(_));
                let signed = self.int_ty(t).signed;
                let f64_ = self.is_f64(t);
                if let Some(a) = a {
                    let av = self.lower_expr(*a);
                    let av = self.to_leaves(av, t);
                    let c = self.f.vreg(Cls::I);
                    if fl {
                        self.emit(Inst::FCmp(Cond::Ge, f64_, c, v[0], av[0]));
                    } else {
                        self.emit(Inst::ICmp(Cond::Ge, signed, c, v[0], av[0]));
                    }
                    let ok = self.f.block();
                    self.term(Term::Branch(c, ok, fail));
                    self.switch(ok);
                }
                if let Some(b) = b {
                    let bv = self.lower_expr(*b);
                    let bv = self.to_leaves(bv, t);
                    let c = self.f.vreg(Cls::I);
                    let cond = if *incl { Cond::Le } else { Cond::Lt };
                    if fl {
                        self.emit(Inst::FCmp(cond, f64_, c, v[0], bv[0]));
                    } else {
                        self.emit(Inst::ICmp(cond, signed, c, v[0], bv[0]));
                    }
                    let ok = self.f.block();
                    self.term(Term::Branch(c, ok, fail));
                    self.switch(ok);
                }
            }
            Pat::Tuple(v) => {
                let n = match self.kind(t) {
                    TyKind::Tuple(x) => self.u.tcx.tys.list(x).len(),
                    _ => 0,
                };
                let idx = seq_indices(ast, ast.list(*v), n);
                for (pi, fi) in idx {
                    let (fp, ft) = self.project(place, t, 0, fi);
                    self.test_pat(ast.list(*v)[pi], &fp, ft, fail);
                }
            }
            Pat::TupleStruct(_, v) => {
                let vi = self.variant_of_pat(p);
                self.test_variant(p, place, t, fail);
                let n = self.variant_len(t, vi);
                let idx = seq_indices(ast, ast.list(*v), n);
                for (pi, fi) in idx {
                    let (fp, ft) = self.project(place, t, vi, fi);
                    self.test_pat(ast.list(*v)[pi], &fp, ft, fail);
                }
            }
            Pat::Struct(_, fields, _) => {
                let vi = self.variant_of_pat(p);
                self.test_variant(p, place, t, fail);
                for fp in ast.list(*fields) {
                    let fname = { let tx = self.ident_text(fp.name); match crate::ast::tuple_index(tx).ok_or(()) { Ok(n) => n | 0x8000_0000, Err(_) => self.u.prog.syms.get(tx).unwrap_or(u32::MAX) } };
                    if let Some(fi) = self.field_index(t, vi, fname) {
                        let (pl, ft) = self.project(place, t, vi, fi);
                        self.test_pat(fp.pat, &pl, ft, fail);
                    }
                }
            }
            Pat::Ref(_, x) => {
                let inner = match self.kind(t) {
                    TyKind::Ref(_, i, _) => i,
                    _ => t,
                };
                let ptr = self.read_place(place, t);
                let ptr = self.to_leaves(ptr, t);
                if !ptr.is_empty() {
                    let pl = if self.unsized_ty(inner) { Place::Regs(ptr) } else { Place::Mem(ptr[0], 0) };
                    self.test_pat(*x, &pl, inner, fail);
                }
            }
            Pat::Or(alts) if self.or_force.contains_key(&p.0) => {
                let k = self.or_force[&p.0];
                let a = ast.list(*alts)[k];
                self.test_pat(a, place, t, fail);
            }
            Pat::Or(alts) => {
                let ok = self.f.block();
                // which alternative matched is only needed to bind its names
                let mut bound = Vec::new();
                self.pat_binding_locals(p, &mut bound);
                let sel = match self.or_sel.get(&p.0) {
                    Some(r) => Some(*r),
                    None if bound.is_empty() => None,
                    None => {
                        let r = self.f.vreg(Cls::I);
                        self.or_sel.insert(p.0, r);
                        Some(r)
                    }
                };
                for (i, a) in ast.list(*alts).iter().enumerate() {
                    let next = if i + 1 == alts.len() { fail } else { self.f.block() };
                    self.test_pat(*a, place, t, next);
                    if let Some(sel) = sel {
                        self.emit(Inst::Iconst(sel, i as i64));
                    }
                    self.goto(ok);
                    if i + 1 < alts.len() {
                        self.switch(next);
                    }
                }
                self.switch(ok);
            }
            Pat::Slice(v) => {
                let v = ast.list(*v).to_vec();
                let sp = match self.slice_pat(&v, place, t) {
                    Some(x) => x,
                    None => return,
                };
                if let Some(len) = sp.len {
                    // length test: exactly the patterns, or at least them with a rest
                    let k = self.iconst(sp.fixed as i64);
                    let c = self.icmp(if sp.rest.is_some() { Cond::Ge } else { Cond::Eq }, false, len, k);
                    let ok = self.f.block();
                    self.term(Term::Branch(c, ok, fail));
                    self.switch(ok);
                }
                for (i, x) in v.iter().enumerate() {
                    if Some(i) == sp.rest {
                        continue;
                    }
                    let pl = self.slice_pat_elem(&sp, v.len(), i);
                    self.test_pat(*x, &pl, sp.elem, fail);
                }
            }
            Pat::Mac(_) => {
                self.err(self.pos, "macro in pattern position is not supported".to_string());
            }
        }
    }

    fn test_def_pat(&mut self, p: PatId, d: DefId, place: &Place, t: TyId, fail: u32) {
        let k = self.u.prog.def(d).kind;
        match k {
            DefKind::Variant => {
                let vi = self.u.prog.def(d).sub;
                self.test_tag(place, t, vi, fail);
            }
            DefKind::Const | DefKind::AssocConst => {
                // a constant pattern matches structurally, by its value (rustc const_to_pat):
                // variants, fields and pointees of the evaluated constant, read at compile time
                let args = self.b.res_args.get(&(p.0 | crate::typeck::PAT_KEY)).copied().unwrap_or_default();
                let args = self.mono_list(args);
                match self.u.const_addr_shared(d, args) {
                    Some(addr) => {
                        let cs = self.u.data_sym(addr);
                        self.test_const_value(place, t, addr, Some(cs), fail, 0)
                    }
                    None => {
                        self.err(self.pos, format!("could not evaluate constant {} -- see its own error", self.u.prog.def_path(d)));
                    }
                }
            }
            _ => {}
        }
    }

    /// Jumps to `fail` unless the value at `place: t` equals the constant value at host address
    /// `c` (field by field; references compare their pointees, str/slices their contents).
    /// `cs` is the same address as a symbol (syms.rs) when known: integers, tags and lengths are
    /// read at compile time from `c`; a pointer value the test compares comes from `cs` (its
    /// symbol, or a load from the constant at run time), never as a bare number.
    fn test_const_value(&mut self, place: &Place, t: TyId, c: u64, cs: Option<relax_backend::rir::Sym>, fail: u32, depth: u32) {
        if depth > 32 {
            return;
        }
        let k = self.kind(t);
        let branch = |s: &mut Self, cond: VReg| {
            let ok = s.f.block();
            s.term(Term::Branch(cond, ok, fail));
            s.switch(ok);
        };
        match k {
            TyKind::Int(_) | TyKind::Bool | TyKind::Char | TyKind::Ptr(..) | TyKind::FnPtr(..) | TyKind::Float(_) => {
                let l = self.layout(t);
                let lf = match &l.leaves {
                    Some(v) if v.len() == 1 => v[0],
                    _ => return,
                };
                let cv = if matches!(k, TyKind::Ptr(..) | TyKind::FnPtr(..)) {
                    match self.const_ptr_val(cs, lf.off) {
                        Some(v) => v,
                        None => return self.err(self.pos, "a pointer in a constant pattern whose target makepad run cannot name".to_string()),
                    }
                } else {
                    let raw = unsafe { read_raw(c + lf.off as u64, lf.mem) };
                    self.const_leaf(raw, lf)
                };
                let v = self.read_place(place, t);
                let r = self.eq_vals(v, Val::L(vec![cv]), t);
                branch(self, r);
            }
            TyKind::Ref(_, inner, _) => {
                let unsized_ = self.unsized_ty(inner);
                let cp = unsafe { read_raw(c, Mem::Int(8, false)) };
                let cps = cs.and_then(|s| match crate::syms::read_leaf(self.u, s, Mem::Int(8, false)) {
                    relax_backend::passes::RoRead::Ptr(p) => Some(p),
                    _ => None,
                });
                match self.kind(inner) {
                    TyKind::Str => {
                        let cl = unsafe { read_raw(c + 8, Mem::Int(8, false)) };
                        let v = self.read_place(place, t);
                        let a = match self.const_ptr_val(cs, 0) {
                            Some(a) => a,
                            None => return self.err(self.pos, "a string constant pattern whose bytes makepad run cannot name".to_string()),
                        };
                        let l = self.iconst(cl as i64);
                        let r = self.eq_vals(v, Val::L(vec![a, l]), t);
                        branch(self, r);
                    }
                    TyKind::Slice(el) => {
                        let cl = unsafe { read_raw(c + 8, Mem::Int(8, false)) };
                        let v = self.read_place(place, t);
                        let v = self.to_leaves(v, t);
                        let n = self.iconst(cl as i64);
                        let same = self.f.vreg(Cls::I);
                        self.emit(Inst::ICmp(Cond::Eq, false, same, v[1], n));
                        branch(self, same);
                        let es = {
                            let l = self.layout(el);
                            crate::layout::round_up(l.size, l.align)
                        };
                        for i in 0..cl.min(4096) {
                            let a = self.addr_add(v[0], (i as u32 * es) as i32);
                            self.test_const_value(&Place::Mem(a, 0), el, cp + i * es as u64, cps.map(|p| p.plus((i * es as u64) as i64)), fail, depth + 1);
                        }
                    }
                    _ if unsized_ => {}
                    _ => {
                        let v = self.read_place(place, t);
                        let v = self.to_leaves(v, t);
                        self.test_const_value(&Place::Mem(v[0], 0), inner, cp, cps, fail, depth + 1);
                    }
                }
            }
            TyKind::Tuple(v) => {
                let l = self.layout(t);
                for i in 0..self.u.tcx.tys.list(v).len() {
                    let (fp, ft) = self.project(place, t, 0, i as u32);
                    self.test_const_value(&fp, ft, c + l.fields[i] as u64, cs.map(|s| s.plus(l.fields[i] as i64)), fail, depth + 1);
                }
            }
            TyKind::Array(el, n) => {
                let n = n.try_to_target_usize(&self.u.tcx.tys).expect("normalized array length");
                let es = {
                    let l = self.layout(el);
                    crate::layout::round_up(l.size, l.align)
                };
                let base = self.place_addr(place, t);
                for i in 0..n.min(4096) {
                    let a = self.addr_add(base, (i as u32 * es) as i32);
                    self.test_const_value(&Place::Mem(a, 0), el, c + i * es as u64, cs.map(|s| s.plus((i * es as u64) as i64)), fail, depth + 1);
                }
            }
            TyKind::Adt(d, _, _) => {
                let adt = match self.u.tcx.adts.get(&d) {
                    Some(a) => a,
                    None => return,
                };
                let l = self.layout(t);
                let vi = if adt.is_enum {
                    let raw = match l.tag {
                        Some((toff, tmem)) => {
                            let raw = unsafe { read_raw(c + toff as u64, tmem) };
                            match tmem {
                                Mem::Int(1, true) => raw as u8 as i8 as i128,
                                Mem::Int(2, true) => raw as u16 as i16 as i128,
                                Mem::Int(4, true) => raw as u32 as i32 as i128,
                                Mem::Int(8, true) => raw as i64 as i128,
                                _ => raw as i128,
                            }
                        }
                        None => 0,
                    };
                    let Some(disc) = decode_tag_const(self.u, raw, &l, &adt) else { return; };
                    match adt.discriminants(&self.u.tcx, &self.u.prog).iter().position(|(_, v)| v.signed_value() == disc) {
                        Some(i) => i as u32,
                        None => return,
                    }
                } else {
                    0
                };
                if adt.is_union {
                    return;
                }
                self.test_tag(place, t, vi, fail);
                let nf = adt.variants[vi as usize].fields.len();
                for fi in 0..nf {
                    let off = if adt.is_enum { l.variant_fields[vi as usize][fi] } else { l.fields[fi] };
                    let (fp, ft) = self.project(place, t, vi, fi as u32);
                    self.test_const_value(&fp, ft, c + off as u64, cs.map(|s| s.plus(off as i64)), fail, depth + 1);
                }
            }
            _ => {}
        }
    }

    /// Resolved variant identity, as in rustc_ast_lowering/src/lib.rs:2895.
    fn variant_index(&self, t: TyId, item: LangItem) -> u32 {
        if let TyKind::Adt(d, _, _) = self.kind(t) {
            if let Some(adt) = self.u.tcx.adts.get(&d) {
                for (i, v) in adt.variants.iter().enumerate() {
                    if self.u.tcx.lang_items.get(item) == Some(v.def) {
                        return i as u32;
                    }
                }
            }
        }
        0
    }

    /// rustc_ast_lowering/src/expr.rs:1945-2033 @8d1a764.
    fn lower_try(&mut self, e: ExprId, x: ExprId, t: TyId) -> Val {
        let xt = self.body_ty(x);
        let v = self.lower_expr(x);
        let (branch, args, flow) = self.b.try_branch[&e.0].clone();
        let args = self.mono_list(args);
        let flow = self.monomorphize(flow);
        let env = crate::tcx::TypingEnv::fully_monomorphized(&self.u.prog);
        let instance = crate::instance::Instance::expect_resolve(&self.u.tcx, &self.u.prog, env.clone(), branch, &args);
        let v = self.emit_instance(instance, vec![(v, xt)], flow);
        let place = match v {
            Val::M(b, o) => Place::Mem(b, o),
            Val::L(r) => {
                let a = self.new_slot_for(flow);
                self.store_val(a, 0, flow, Val::L(r));
                Place::Mem(a, 0)
            }
        };
        let fail = self.f.block();
        let vi_ok = self.variant_index(flow, LangItem::ControlFlowContinue);
        self.test_tag(&place, flow, vi_ok, fail);
        let ok_blk = self.cur;
        self.switch(fail);
        let vi_err = self.variant_index(flow, LangItem::ControlFlowBreak);
        let (ep, et) = self.project(&place, flow, vi_err, 0);
        let ev = self.read_place(&ep, et);
        let (fd, args) = self.b.try_from_residual[&e.0].clone();
        let args = self.mono_list(args);
        let instance = crate::instance::Instance::expect_resolve(&self.u.tcx, &self.u.prog, env, fd, &args);
        let r = self.emit_instance(instance, vec![(ev, et)], self.ret_ty);
        self.ret_val_early(r, self.ret_ty);
        self.switch(ok_blk);
        let (pp, _) = self.project(&place, flow, vi_ok, 0);
        self.read_place(&pp, t)
    }

    fn test_variant(&mut self, p: PatId, place: &Place, t: TyId, fail: u32) {
        if let Some(Res::Def(d)) = self.b.pat_res.get(&p.0).copied() {
            if self.u.prog.def(d).kind == DefKind::Variant {
                let vi = self.u.prog.def(d).sub;
                self.test_tag(place, t, vi, fail);
            }
        }
    }

    /// The discriminant of the enum value at `place: t`, decoded (one vreg, two for a 128-bit tag).
    fn read_tag(&mut self, place: &Place, t: TyId) -> Option<Vec<VReg>> {
        let l = self.layout(t);
        match place {
            Place::Mem(b, o) => self.load_disc(*b, *o, t),
            Place::Regs(r) if !r.is_empty() => {
                let raw = match (l.tag, &l.leaves) {
                    (Some((toff, Mem::Int(bytes, _))), Some(lv)) => {
                        let first = lv.iter().position(|x| x.off == toff).unwrap();
                        r[first..first + if bytes == 16 { 2 } else { 1 }].to_vec()
                    }
                    _ => vec![r[0]],
                };
                let adt = match self.kind(t) {
                    TyKind::Adt(d, _, _) => self.u.tcx.adts.get(&d),
                    _ => None,
                };
                match adt {
                    Some(adt) => {
                        let pos = ((self.b.file as u64) << 32) | self.pos as u64;
                        let cur = self.cur;
                        emit_decode_tag(self.u, &mut self.f, cur, pos, raw, &l, &adt)
                    }
                    None => Some(raw),
                }
            }
            _ => None,
        }
    }

    fn test_tag(&mut self, place: &Place, t: TyId, vi: u32, fail: u32) {
        let adt = match self.kind_ref(t) {
            TyKind::Adt(d, _, _) => self.u.tcx.adts.get(d),
            _ => None,
        };
        let adt = match adt {
            Some(a) => a,
            None => return,
        };
        if adt.variants.len() <= 1 {
            return;
        }
        let Some(discr) = adt.discriminant_for_variant(&self.u.tcx, &self.u.prog, vi) else { return; };
        let disc128 = discr.signed_value();
        let disc = disc128 as i64;
        let Some(tag) = self.read_tag(place, t) else { return; };
        let k = self.iconst(disc);
        let c = if tag.len() == 2 {
            let hi = self.iconst((disc128 as u128 >> 64) as i64);
            self.wide_cmp(BinOp::Eq, false, (tag[0], tag[1]), (k, hi))
        } else { self.icmp(Cond::Eq, true, tag[0], k) };
        let ok = self.f.block();
        self.term(Term::Branch(c, ok, fail));
        self.switch(ok);
    }

    fn eq_vals(&mut self, a: Val, b: Val, t: TyId) -> VReg {
        let k = self.kind(t);
        match k {
            TyKind::Float(_) => {
                let f64_ = self.is_f64(t);
                let a = self.to_leaves(a, t);
                let b = self.to_leaves(b, t);
                let c = self.f.vreg(Cls::I);
                self.emit(Inst::FCmp(Cond::Eq, f64_, c, a[0], b[0]));
                c
            }
            TyKind::Ref(_, inner, _) if matches!(self.kind_ref(inner), TyKind::Slice(el) if matches!(self.kind_ref(*el), TyKind::Int(_) | TyKind::Bool | TyKind::Char)) => {
                // `&[u8]` against a byte-string pattern: equal lengths and bytes
                let el = match self.kind(inner) {
                    TyKind::Slice(el) => el,
                    _ => inner,
                };
                let es = self.layout(el).size.max(1) as i64;
                let a = self.to_leaves(a, t);
                let b = self.to_leaves(b, t);
                let k = self.iconst(es);
                let la = self.ibin(IOp::Mul, false, a[1], k);
                let lb = self.ibin(IOp::Mul, false, b[1], k);
                let rt = self.u.rs.str_eq;
                let c = self.f.vreg(Cls::I);
                self.emit(Inst::Call(Callee::Host(rt), vec![a[0], la, b[0], lb], vec![c]));
                c
            }
            // `&[u8; N]` against a byte-string pattern (`match &tag { b"head" => .. }`): the bytes
            // (Reference patterns.md [patterns.literal]: compared by value; it compared pointers)
            TyKind::Ref(_, inner, _) if matches!(self.kind_ref(inner), TyKind::Array(el, _) if matches!(self.kind_ref(*el), TyKind::Int(_) | TyKind::Bool | TyKind::Char)) => {
                let size = self.layout(inner).size as i64;
                let a = self.to_leaves(a, t);
                let b = self.to_leaves(b, t);
                let n = self.iconst(size);
                let rt = self.u.rs.str_eq;
                let c = self.f.vreg(Cls::I);
                self.emit(Inst::Call(Callee::Host(rt), vec![a[0], n, b[0], n], vec![c]));
                c
            }
            TyKind::Ref(_, inner, _) if matches!(self.kind_ref(inner), TyKind::Str) => {
                let a = self.to_leaves(a, t);
                let b = self.to_leaves(b, t);
                let rt = self.u.rs.str_eq;
                let c = self.f.vreg(Cls::I);
                self.emit(Inst::Call(Callee::Host(rt), vec![a[0], a[1], b[0], b[1]], vec![c]));
                c
            }
            _ => {
                let a = self.to_leaves(a, t);
                let b = self.to_leaves(b, t);
                // all leaves equal
                let mut acc: Option<VReg> = None;
                for i in 0..a.len().min(b.len()) {
                    let c = self.f.vreg(Cls::I);
                    if self.f.vregs[a[i].0 as usize] == Cls::I {
                        self.emit(Inst::ICmp(Cond::Eq, false, c, a[i], b[i]));
                    } else {
                        let f64_ = self.f.vregs[a[i].0 as usize] == Cls::F64;
                        self.emit(Inst::FCmp(Cond::Eq, f64_, c, a[i], b[i]));
                    }
                    acc = Some(match acc {
                        None => c,
                        Some(p) => {
                            let d = self.f.vreg(Cls::I);
                            self.emit(Inst::IBin(IOp::And, U64, d, p, c));
                            d
                        }
                    });
                }
                match acc {
                    Some(c) => c,
                    None => self.iconst(1),
                }
            }
        }
    }

    // ------------------------------------------------------------ expressions

    /// Can the rvalue `e` be promoted to a static (rustc promote_consts: built only from
    /// literals, constants, constructors and operations that cannot fail, of a type without
    /// drop glue)?
    fn promotable(&mut self, e: ExprId) -> bool {
        let t = self.body_ty(e);
        if self.u.tcx.needs_drop(&self.u.prog, crate::tcx::TypingEnv::fully_monomorphized(&self.u.prog), t) || self.unsized_ty(t) {
            return false;
        }
        let ast = self.ast();
        match ast.expr(e).kind.clone() {
            ExprKind::Lit(_) => true,
            ExprKind::Paren(x) => self.promotable(x),
            ExprKind::Unary(UnOp::Neg | UnOp::Not, x) => self.promotable(x) && !self.b.binops.contains_key(&e.0),
            ExprKind::Array(v) | ExprKind::Tuple(v) => ast.list(v).iter().all(|x| self.promotable(*x)),
            ExprKind::Path(_) => match self.b.res.get(&e.0) {
                // constants, unit constructors, fn items (`&VTable { f: g::<T> }`)
                Some(Res::Def(d)) => matches!(self.u.prog.def(*d).kind, DefKind::Const | DefKind::AssocConst | DefKind::Variant | DefKind::Struct | DefKind::Fn | DefKind::AssocFn),
                // rustc_mir_transform/src/promote_consts.rs:379-397: a substituted
                // const parameter is a constant operand. Without this,
                // `&VTable { tag: N, .. }` returns a stack temporary unless inlining
                // happens to keep the borrowed value inside the caller's frame.
                Some(Res::ConstParam(i)) => self.b.args.get(*i as usize).is_some_and(|a| matches!(self.kind(*a), TyKind::Const(c) if matches!(self.u.tcx.tys.const_kind(c), relax_types::consts::ConstKind::Value(_)))),
                _ => false,
            },
            // braced struct literals of promotable fields (no `..base`)
            ExprKind::Struct(row) if ast.row(row).base == crate::ast::OptExprId::NONE && !ast.row(row).rest => ast.list(ast.row(row).fields).iter().all(|f| self.promotable(f.expr)),
            ExprKind::Cast(x, _) => self.promotable(x),
            // an inline const (`const { .. }`, `offset_of!`'s expansion) is a constant: rustc
            // promotes it (lowered here as code; promote_rvalue stores its value statically)
            ExprKind::ConstBlock(_) => true,
            ExprKind::AddrOf(false, false, x) => self.promotable(x),
            ExprKind::Call(f, args) => match self.b.res.get(&strip_paren(ast, f).0) {
                Some(Res::Def(d)) if matches!(self.u.prog.def(*d).kind, DefKind::Variant | DefKind::Struct) => ast.list(args).iter().all(|x| self.promotable(*x)),
                _ => false,
            },
            _ => false,
        }
    }


    /// Constant promotion: the rvalue `x` stored in static memory, its address (None for an
    /// unsized type).
    fn promote_rvalue(&mut self, x: ExprId, shared: bool) -> Option<VReg> {
        let xt = self.body_ty(x);
        if self.unsized_ty(xt) {
            return None;
        }
        let (blk0, at0) = (self.cur, self.f.blocks[self.cur as usize].insts.len());
        let v = self.lower_expr(x);
        let l = self.layout(xt);
        if l.size == 0 {
            // a promoted zero-sized value has no storage: its address is its alignment (as
            // rustc places ZST constants; `&()` is 0x1)
            let p = self.f.vreg(Cls::I);
            self.emit(Inst::Iconst(p, l.align.max(1) as i64));
            return Some(p);
        }
        // a value of constants only, borrowed shared and without interior mutability: one
        // read-only allocation per content (rustc interns constant allocations; `&0u8` twice is
        // one address), written at compile time
        if shared && l.size > 0 && self.cur == blk0 && self.u.tcx.is_freeze(&self.u.prog, crate::tcx::TypingEnv::fully_monomorphized(&self.u.prog), xt) {
            if let Some(buf) = self.const_bytes(&v, l, blk0, at0) {
                let addr = self.u.data_dedup(&buf, l.align.max(1) as u64);
                let addr = self.u.data_sym(addr);
                let p = self.f.vreg(Cls::I);
                self.emit(Inst::Addr(p, addr));
                return Some(p);
            }
        }
        let addr = self.u.rw_sym(l.size.max(1) as u64, l.align.max(8) as u64, Some(xt));
        let p = self.f.vreg(Cls::I);
        self.emit(Inst::Addr(p, addr));
        self.store_val(p, 0, xt, v);
        Some(p)
    }

    /// The bytes of value `v` (layout `l`) when the instructions emitted for it (block `blk`
    /// from `at`) only make constants: register leaves defined by constants, or a fresh slot
    /// written only by stores of constants (array / aggregate literals). None otherwise.
    fn const_bytes(&self, v: &Val, l: &Layout, blk: u32, at: usize) -> Option<Vec<u8>> {
        let insts = &self.f.blocks[blk as usize].insts[at..];
        let mut consts: HashMap<VReg, u64> = HashMap::default();
        let width = |m: Mem| match m {
            Mem::Int(n, _) => n as usize,
            Mem::F32 => 4,
            Mem::F64 => 8,
        };
        let mut buf = vec![0u8; l.size as usize];
        match v {
            Val::L(regs) => {
                let leaves = l.leaves.as_ref()?;
                if regs.len() != leaves.len() {
                    return None;
                }
                for i in insts {
                    match i {
                        Inst::Iconst(d, k) => {
                            consts.insert(*d, *k as u64);
                        }
                        Inst::Fconst(d, b, _) => {
                            consts.insert(*d, *b);
                        }
                        _ => {}
                    }
                }
                for (r, lf) in regs.iter().zip(leaves.iter()) {
                    let b = *consts.get(r)?;
                    let (o, n) = (lf.off as usize, width(lf.mem));
                    buf.get_mut(o..o + n)?.copy_from_slice(&b.to_le_bytes()[..n]);
                }
                Some(buf)
            }
            Val::M(base, off0) => {
                // addresses derived from the fresh slot: vreg -> offset in it
                let mut addrs: HashMap<VReg, i64> = HashMap::default();
                for i in insts {
                    match i {
                        Inst::Iconst(d, k) => {
                            consts.insert(*d, *k as u64);
                        }
                        Inst::Fconst(d, b, _) => {
                            consts.insert(*d, *b);
                        }
                        Inst::SlotAddr(d, _) => {
                            addrs.insert(*d, 0);
                        }
                        Inst::IBin(IOp::Add, _, d, a, k) if addrs.contains_key(a) => {
                            let o = addrs[a] + *consts.get(k)? as i64;
                            addrs.insert(*d, o);
                        }
                        Inst::Store(m, b, o, src) if addrs.contains_key(b) => {
                            let val = *consts.get(src)?;
                            let at = addrs[b] + *o as i64 - *off0 as i64;
                            let n = width(*m);
                            if at < 0 {
                                return None;
                            }
                            buf.get_mut(at as usize..at as usize + n)?.copy_from_slice(&val.to_le_bytes()[..n]);
                        }
                        // anything else touching a slot address (calls, copies, moves) or any
                        // other work: not a plain constant
                        _ => return None,
                    }
                }
                if !addrs.contains_key(base) {
                    return None;
                }
                Some(buf)
            }
        }
    }

    /// `a = v` for a place expression `a`: drops its old value (what of it was not moved out),
    /// then writes.
    fn assign_to(&mut self, a: ExprId, v: Val) {
                // a local: drop its old value if it holds one, then it owns the new one
                if let ExprKind::Path(_) = &self.ast().expr(strip_paren(self.ast(), a)).kind {
                    if let Some(Res::Local(li)) = self.b.res.get(&strip_paren(self.ast(), a).0).copied() {
                        let lt = self.monomorphize(self.b.locals[li as usize].ty);
                        let dynamic = self.unwind_on() && self.nounwind == 0 && self.needs_drop_glue(lt) && self.drop_flags[li as usize].is_some() && !self.foreign[li as usize] && self.u.drop_may_unwind(lt);
                        if dynamic && self.dead_blk != Some(self.cur) {
                            // rustc's drop-and-replace: if dropping the old value panics, the new
                            // one is still assigned on the unwind path (and dropped by the
                            // cleanup that follows)
                            let cp = self.cleanup_pad();
                            let (cur, pos) = (self.cur, self.pos);
                            let pb = self.f.block();
                            self.switch(pb);
                            let pl = self.local_place(li);
                            self.write_place(&pl, lt, v.clone());
                            let f = self.flag_of(li);
                            self.emit(Inst::Iconst(f, 1));
                            // the new value is whole: no part of it is moved out
                            if let Some(es) = self.field_moved.get(&li).cloned() {
                                for (_, mf) in es {
                                    self.emit(Inst::Iconst(mf, 0));
                                }
                            }
                            self.goto(cp);
                            self.cur = cur;
                            self.pos = pos;
                            let saved = self.pad_fixed.replace(Some(pb));
                            self.drop_local_before_rebind(li);
                            self.pad_fixed = saved;
                        } else {
                            self.drop_local_before_rebind(li);
                        }
                        let pl = self.local_place(li);
                        self.write_place(&pl, lt, v);
                        self.own_local(li);
                        return;
                    }
                }
                self.want_mut = true;
                let (pl, pt) = self.lower_place(a);
                self.want_mut = false;
                if self.needs_drop_glue(pt) {
                    let addr = self.place_addr(&pl, pt);
                    // a field that was (partly) moved out: drop only what it still owns
                    let moved = self.field_move_path(a).and_then(|(li, path)| {
                        let es: Vec<(Vec<(u32, u32)>, VReg)> = self.field_moved.get(&li)?.iter().filter(|x| x.0.starts_with(&path)).cloned().collect();
                        if es.is_empty() {
                            None
                        } else {
                            Some((path, es))
                        }
                    });
                    match moved {
                        Some((path, es)) => {
                            self.drop_tree(&es, addr, pt, &path);
                            for (_, f) in &es {
                                self.emit(Inst::Iconst(*f, 0));
                            }
                        }
                        None => {
                            if self.unwind_on() && self.nounwind == 0 && self.dead_blk != Some(self.cur) && self.u.drop_may_unwind(pt) {
                                // drop-and-replace: a panicking drop still leaves the new value in
                                // the place, which the cleanup that follows then drops
                                let cp = self.cleanup_pad();
                                let (cur, pos) = (self.cur, self.pos);
                                let pb = self.f.block();
                                self.switch(pb);
                                self.write_place(&pl, pt, v.clone());
                                self.goto(cp);
                                self.cur = cur;
                                self.pos = pos;
                                let saved = self.pad_fixed.replace(Some(pb));
                                self.drop_at(addr, pt);
                                self.pad_fixed = saved;
                            } else {
                                self.drop_at(addr, pt);
                            }
                        }
                    }
                    // into the place itself (a register-held local's address is a spill copy)
                    self.write_place(&pl, pt, v);
                    return;
                }
                self.write_place(&pl, pt, v);
    }

    /// Assigns the parts of the value at `pl: t` to the assignee expression `a` (tuple, array,
    /// struct literal, `_`, `..` in a destructuring assignment; a place expression otherwise).
    fn assign_parts(&mut self, a: ExprId, pl: &Place, t: TyId) {
        let ast = self.ast();
        let is_rest = |x: ExprId| matches!(ast.expr(x).kind, ExprKind::Range(crate::ast::OptExprId::NONE, crate::ast::OptExprId::NONE, false));
        match &ast.expr(a).kind {
            ExprKind::Paren(x) if matches!(ast.expr(*x).kind, ExprKind::Range(crate::ast::OptExprId::NONE, crate::ast::OptExprId::NONE, false)) => {
                if self.needs_drop_glue(t) {
                    let ad = self.place_addr(pl, t);
                    self.drop_at(ad, t);
                }
            }
            ExprKind::Paren(x) => self.assign_parts(*x, pl, t),
            ExprKind::Underscore => {
                if self.needs_drop_glue(t) {
                    let ad = self.place_addr(pl, t);
                    self.drop_at(ad, t);
                }
            }
            ExprKind::Tuple(elems) => {
                let elems = ast.list(*elems).to_vec();
                let m = match self.kind(t) {
                    TyKind::Tuple(v) => self.u.tcx.tys.list(v).len(),
                    _ => return,
                };
                let r = elems.iter().position(|x| is_rest(*x));
                let n = elems.len();
                let mut used = vec![false; m];
                for (i, x) in elems.iter().enumerate() {
                    if Some(i) == r {
                        continue;
                    }
                    let fi = match r {
                        Some(r) if i > r => m - (n - i),
                        _ => i,
                    };
                    if fi >= m {
                        continue;
                    }
                    used[fi] = true;
                    let (fp, ft) = self.project(pl, t, 0, fi as u32);
                    self.assign_parts(*x, &fp, ft);
                }
                for fi in 0..m {
                    if !used[fi] {
                        let (fp, ft) = self.project(pl, t, 0, fi as u32);
                        if self.needs_drop_glue(ft) {
                            let ad = self.place_addr(&fp, ft);
                            self.drop_at(ad, ft);
                        }
                    }
                }
            }
            ExprKind::Array(elems) => {
                let elems = ast.list(*elems).to_vec();
                let (et, m) = match self.kind(t) {
                    TyKind::Array(e, n) => (e, n.try_to_target_usize(&self.u.tcx.tys).expect("normalized array length") as usize),
                    _ => return,
                };
                let stride = {
                    let l = self.layout(et);
                    crate::layout::round_up(l.size, l.align)
                };
                let base = self.place_addr(pl, t);
                let r = elems.iter().position(|x| is_rest(*x));
                let n = elems.len();
                let mut used = vec![false; m];
                for (i, x) in elems.iter().enumerate() {
                    if Some(i) == r {
                        continue;
                    }
                    let fi = match r {
                        Some(r) if i > r => m - (n - i),
                        _ => i,
                    };
                    if fi >= m {
                        continue;
                    }
                    used[fi] = true;
                    let ea = self.addr_add(base, (fi as u32 * stride) as i32);
                    self.assign_parts(*x, &Place::Mem(ea, 0), et);
                }
                for fi in 0..m {
                    if !used[fi] && self.needs_drop_glue(et) {
                        let ea = self.addr_add(base, (fi as u32 * stride) as i32);
                        self.drop_at(ea, et);
                    }
                }
            }
            ExprKind::Call(_, args) => {
                let args = ast.list(*args).to_vec();
                let vi = match self.b.res.get(&a.0) {
                    Some(Res::Def(d)) if self.u.prog.def(*d).kind == DefKind::Variant => self.u.prog.def(*d).sub,
                    _ => 0,
                };
                let nf = match self.kind(t) {
                    TyKind::Adt(d, _, _) => self.u.tcx.adts.get(&d).map_or(0, |x| x.variants.get(vi as usize).map_or(0, |v| v.fields.len())),
                    _ => 0,
                };
                let r = args.iter().position(|x| is_rest(*x));
                let n = args.len();
                let mut used = vec![false; nf];
                for (i, x) in args.iter().enumerate() {
                    if Some(i) == r {
                        continue;
                    }
                    let fi = match r {
                        Some(r) if i > r => nf.wrapping_sub(n - i),
                        _ => i,
                    };
                    if fi >= nf {
                        continue;
                    }
                    used[fi] = true;
                    let (fp, ft) = self.project(pl, t, vi, fi as u32);
                    self.assign_parts(*x, &fp, ft);
                }
                for fi in 0..nf {
                    if !used[fi] {
                        let (fp, ft) = self.project(pl, t, vi, fi as u32);
                        if self.needs_drop_glue(ft) {
                            let ad = self.place_addr(&fp, ft);
                            self.drop_at(ad, ft);
                        }
                    }
                }
            }
            ExprKind::Struct(row) => {
                let fields = ast.list(ast.row(*row).fields).to_vec();
                let vi = match self.b.res.get(&a.0) {
                    Some(Res::Def(d)) if self.u.prog.def(*d).kind == DefKind::Variant => self.u.prog.def(*d).sub,
                    _ => 0,
                };
                let nf = match self.kind(t) {
                    TyKind::Adt(d, _, _) => self.u.tcx.adts.get(&d).map_or(0, |x| x.variants.get(vi as usize).map_or(0, |v| v.fields.len())),
                    TyKind::Tuple(v) => self.u.tcx.tys.list(v).len(),
                    _ => 0,
                };
                let mut used = vec![false; nf];
                for fe in &fields {
                    let txt = self.ident_text(fe.name);
                    let name = if txt.as_bytes()[0].is_ascii_digit() {
                        txt.parse::<u32>().unwrap_or(0) | 0x8000_0000
                    } else {
                        self.u.prog.syms.get(txt).unwrap_or(u32::MAX)
                    };
                    if let Some(fi) = self.field_index(t, vi, name) {
                        if (fi as usize) < nf {
                            used[fi as usize] = true;
                        }
                        let (fp, ft) = self.project(pl, t, vi, fi);
                        self.assign_parts(fe.expr, &fp, ft);
                    }
                }
                for fi in 0..nf {
                    if !used[fi] {
                        let (fp, ft) = self.project(pl, t, vi, fi as u32);
                        if self.needs_drop_glue(ft) {
                            let ad = self.place_addr(&fp, ft);
                            self.drop_at(ad, ft);
                        }
                    }
                }
            }
            _ => {
                let v = self.read_place(pl, t);
                self.assign_to(a, v);
            }
        }
    }

    /// Length of the first array type inside pointer / ADT type `t` (pointee, then args).
    fn array_len_in(&self, t: TyId, depth: u32) -> u64 {
        if depth > 8 {
            return 0;
        }
        match self.kind(t) {
            TyKind::Array(_, n) => n.try_to_target_usize(&self.u.tcx.tys).expect("normalized array length"),
            TyKind::Ref(_, a, _) | TyKind::Ptr(_, a) => self.array_len_in(a, depth + 1),
            TyKind::Adt(_, xs, _) => self.u.tcx.tys.list(xs).iter().map(|x| self.array_len_in(*x, depth + 1)).find(|n| *n != 0).unwrap_or(0),
            _ => 0,
        }
    }

    /// N of the `[T; N]` -> `[T]` unsizing from pointee `a` to `b` (directly or in a struct tail).
    fn unsize_len(&self, a: TyId, b: TyId) -> u64 {
        match (self.kind(a), self.kind(b)) {
            (TyKind::Array(_, n), TyKind::Slice(_)) => n.try_to_target_usize(&self.u.tcx.tys).expect("normalized array length"),
            (TyKind::Adt(_, xs, _), TyKind::Adt(_, ys, _)) => {
                for (x, y) in self.u.tcx.tys.list(xs).iter().zip(self.u.tcx.tys.list(ys).iter()) {
                    if x != y {
                        return self.unsize_len(*x, *y);
                    }
                }
                0
            }
            _ => 0,
        }
    }

    /// The `&'static Location` a call made here passes to a #[track_caller] callee.
    fn caller_location_val(&mut self) -> VReg {
        if let Some(p) = self.caller_loc {
            return p;
        }
        let (l, c) = self.u.prog.files[self.b.file as usize].line_col(self.pos);
        let path = self.u.prog.file_paths[self.b.file as usize].clone();
        let addr = location_const(self.u, &path, l as u32, c as u32);
        let r = self.f.vreg(Cls::I);
        self.emit(Inst::Addr(r, addr));
        r
    }

    /// Discriminant of the enum value of type `t` at `base + off` (tag decoded, a single
    /// variant's constant).
    fn load_disc(&mut self, base: VReg, off: i32, t: TyId) -> Option<Vec<VReg>> {
        self.check_provided_opaque_enum(t, "matched on");
        let l = self.layout(t);
        let adt = match self.kind(t) {
            TyKind::Adt(d, _, _) => self.u.tcx.adts.get(&d)?,
            _ => return None,
        };
        if l.tag.is_none() && !matches!(l.enc, crate::layout::TagEnc::Single(_)) {
            return None;
        }
        let pos = ((self.b.file as u64) << 32) | self.pos as u64;
        let cur = self.cur;
        emit_load_disc(self.u, &mut self.f, cur, pos, base, off, &l, &adt)
    }

    /// Writes variant `vi`'s discriminant of an enum of type `t` at `base + off`.
    fn store_disc(&mut self, base: VReg, off: i32, t: TyId, vi: u32) {
        self.check_provided_opaque_enum(t, "built");
        let l = self.layout(t);
        let adt = match self.kind(t) {
            TyKind::Adt(d, _, _) => match self.u.tcx.adts.get(&d) {
                Some(a) => a,
                None => return,
            },
            _ => return,
        };
        let pos = ((self.b.file as u64) << 32) | self.pos as u64;
        let cur = self.cur;
        emit_store_disc(self.u, &mut self.f, cur, pos, base, off, &l, &adt, vi);
    }

    fn lower_expr_coerced(&mut self, e: ExprId, target: TyId) -> Val {
        let v = self.lower_expr(e);
        self.apply_coercion(e, v, target)
    }

    fn apply_coercion(&mut self, e: ExprId, v: Val, target: TyId) -> Val {
        match self.b.coerce(e.0) {
            None | Some(Coerce::MutToConst | Coerce::RefToRaw(_)) => v,
            Some(Coerce::Never) => {
                // a `!` value used as another type (`fn f(x: !) -> T { x }`): control never gets
                // here; the rest is emitted into a dead block with a placeholder of T's shape
                // (the target may be unknown here -- a block tail passes its own type -- so the
                // placeholder is memory, which reads as any shape)
                if self.is_never(self.body_ty(e)) && self.dead_blk != Some(self.cur) {
                    self.term(Term::Unreachable);
                    self.dead();
                    let a = if self.is_never(target) {
                        let s = self.f.slot(64, 8);
                        let d = self.f.vreg(Cls::I);
                        self.emit(Inst::SlotAddr(d, s));
                        d
                    } else {
                        self.new_slot_for(target)
                    };
                    return Val::M(a, 0);
                }
                v
            }
            Some(Coerce::Unsize) => {
                // [T; N] -> [T] behind a pointer (&, *const/*mut) or a smart pointer (Box, Rc, ..):
                // the thin pointer gets the length N
                let src = self.body_ty(e);
                let len = match (self.kind(src), self.kind(target)) {
                    (TyKind::Ref(_, a, _) | TyKind::Ptr(_, a), TyKind::Ref(_, b, _) | TyKind::Ptr(_, b)) => self.unsize_len(a, b),
                    // Box<[T; N]> / Box<S<[T; N]>> -> Box<[T]> / Box<S<[T]>>
                    (TyKind::Adt(..), TyKind::Adt(..)) => self.unsize_len(src, target),
                    _ => 0,
                };
                // a block tail passes its own type as the target: the length is the array's in
                // the source (the only array an unsizing can come from)
                let len = if len == 0 && target == src { self.array_len_in(src, 0) } else { len };
                let p = self.to_leaves(v, src);
                if p.len() != 1 {
                    self.err(self.pos, "unsizing `[T; N]` to `[T]` needs a single-pointer value".to_string());
                    return self.unit();
                }
                let l = self.iconst(len as i64);
                Val::L(vec![p[0], l])
            }
            Some(Coerce::ToDyn(src, dy)) => {
                let src = self.monomorphize(src);
                let dy = self.monomorphize(dy);
                // thin pointer (alone in its wrapper) -> (data, vtable)
                let st = self.body_ty(e);
                let p = self.to_leaves(v, st);
                if p.len() != 1 {
                    self.err(self.pos, "unsizing to `dyn` needs a single-pointer value".to_string());
                    return self.unit();
                }
                let addr = match self.u.vtable(src, dy) {
                    Ok(a) => a,
                    Err(m) => {
                        self.err(self.pos, m);
                        return self.unit();
                    }
                };
                let vt = self.f.vreg(Cls::I);
                self.emit(Inst::Addr(vt, self.u.data_sym(addr)));
                let _ = target;
                Val::L(vec![p[0], vt])
            }
            Some(Coerce::Upcast(sd, dd)) => {
                let sd = self.monomorphize(sd);
                let dd = self.monomorphize(dd);
                // (data, vtable of Sub) -> (data, Sub's stored vtable for Super)
                let st = self.body_ty(e);
                let p = self.to_leaves(v, st);
                if p.len() != 2 {
                    self.err(self.pos, "internal: trait object upcast of a value without a vtable -- makepad run bug, report it".to_string());
                    return self.unit();
                }
                let Some(slot) = self.u.tcx.supertrait_vtable_slot(&self.u.prog, sd, dd) else { return Val::L(p) };
                let vt = self.f.vreg(Cls::I);
                self.emit(Inst::Load(Mem::Int(8, false), vt, p[1], (slot * 8) as i32));
                let _ = target;
                Val::L(vec![p[0], vt])
            }
            Some(Coerce::DerefRef(n)) => {
                let st = self.body_ty(e);
                let (inner, m) = match self.kind(st) {
                    TyKind::Ref(m, i, _) => (i, m),
                    _ => return v,
                };
                let p = self.to_leaves(v, st);
                let mut pl = if self.unsized_ty(inner) { Place::Regs(p) } else { Place::Mem(p[0], 0) };
                let mut pt = inner;
                let saved = self.want_mut;
                self.want_mut = m;
                for k in 0..n {
                    let (a, b) = self.deref_place(e.0 | crate::typeck::DEREF_COERCE_KEY, k, pl, pt);
                    pl = a;
                    pt = b;
                }
                self.want_mut = saved;
                let _ = target;
                if self.unsized_ty(pt) {
                    match pl {
                        Place::Regs(r) => Val::L(r),
                        _ => self.unit(),
                    }
                } else {
                    let a = self.place_addr(&pl, pt);
                    Val::L(vec![a])
                }
            }
            Some(Coerce::ReifyFn) => {
                let t = self.body_ty(e);
                if let TyKind::FnDef(d, args, _) = self.kind(t) {
                    // `<T as Trait>::method` as a value: the impl's method (or the default body)
                    let r = self.fn_item_addr(d, self.u.tcx.tys.list(args).to_vec());
                    return Val::L(vec![r]);
                }
                v
            }
            Some(Coerce::UnsafeFn) => v,
            Some(Coerce::ClosureFn) => {
                // rustc_codegen_ssa/src/mir/rvalue.rs:536-550.
                let instance = Instance::resolve_closure(&self.u.tcx, &self.u.prog, self.body_ty(e), relax_types::upvar::ClosureKind::FnOnce);
                let id = self.u.instance_fn_id(instance);
                let r = self.f.vreg(Cls::I);
                self.emit(Inst::FnAddr(r, id));
                Val::L(vec![r])
            }
        }
    }

    fn closure_fn(&mut self, e: ExprId) -> u32 {
        // the closure expression may be wrapped in parens
        let mut ce = e;
        loop {
            match &self.ast().expr(ce).kind {
                ExprKind::Paren(x) => ce = *x,
                _ => break,
            }
        }
        let t = self.body_ty(ce);
        self.closure_fn_of(t)
    }

    /// The function of a closure type (lowered from its owner's body).
    fn closure_fn_of(&mut self, t: TyId) -> u32 {
        let (file, ce, owner) = match self.kind(t) {
            TyKind::Closure(file, ce, crate::types::ClosureArgs { parent_args: owner, .. }) => (file, ce, owner),
            _ => return 0,
        };
        let parent = match self.kind(owner) {
            TyKind::FnDef(d, args, _) => self.u.fn_id(FnKey::Inst(d, self.u.tcx.tys.list(args).to_vec())),
            _ => self.cur_fn,
        };
        let id = self.u.fn_id(FnKey::Closure(file, ce, parent));
        if self.b.file == file && parent == self.u.fn_id(FnKey::Inst(self.b.def, self.b.args.to_vec())) {
            self.new_closures.push((ce, id));
        }
        id
    }

    pub fn lower_expr(&mut self, e: ExprId) -> Val {
        let _why = crate::why::enabled().then(|| crate::why::enter(self.u.prog.why_expr(
            self.b.file, self.ast().expr(e).lo, self.b.def, "lower-expression")));
        let ast = self.ast();
        let ex = ast.expr(e);
        let save_pos = self.pos;
        self.pos = ex.lo;
        let t = self.body_ty(e);
        let v = self.lower_expr_inner(e, t);
        self.pos = save_pos;
        v
    }

    fn unit(&self) -> Val {
        Val::L(Vec::new())
    }

    fn lower_expr_inner(&mut self, e: ExprId, t: TyId) -> Val {
        let ast = self.ast();
        let ex = ast.expr(e);
        // Two-word integer operations need both halves of every operand.
        let wide = match &ex.kind {
            ExprKind::Binary(_, a, b) | ExprKind::AssignOp(_, a, b) => self.is_wide(self.body_ty(*a)) || self.is_wide(self.body_ty(*b)),
            ExprKind::Unary(UnOp::Neg | UnOp::Not, x) | ExprKind::Cast(x, _) => self.is_wide(self.body_ty(*x)) || self.is_wide(t),
            _ => false,
        };
        if wide {
            return self.lower_wide(e, t);
        }
        match &ex.kind {
            ExprKind::Lit(k) => self.lower_lit(e, *k, t),
            ExprKind::Paren(x) => self.lower_expr(*x),
            ExprKind::Path(_) => match self.b.res.get(&e.0).copied() {
                Some(Res::Local(li)) => {
                    let pl = self.local_place(li);
                    let lt = self.monomorphize(self.b.locals[li as usize].ty);
                    let v = self.read_place(&pl, lt);
                    self.moved_local(li);
                    v
                }
                Some(Res::Def(d)) => self.lower_def_value(e, d, t),
                // a const generic param's value in this instance
                Some(Res::ConstParam(i)) => match self.b.args.get(i as usize).map(|a| self.kind(*a)) {
                    Some(TyKind::Const(v)) => {
                        // Scalar constants retain their declared width (bool and
                        // char arguments are not target-usize array lengths).
                        let value = v.try_to_value(&self.u.tcx.tys).expect("normalized const argument");
                        let scalar = value.valtree.try_to_leaf().expect("scalar const argument");
                        Val::L(vec![self.iconst(scalar.to_bits(scalar.size()) as i64)])
                    }
                    _ => {
                        self.err(self.pos, "internal: const generic param without a value in this instance".to_string());
                        self.dummy_val(t)
                    }
                },
                _ => self.unit(),
            },
            ExprKind::Unary(UnOp::Not | UnOp::Neg, x) if self.b.binops.contains_key(&e.0) => {
                // operator trait call: Not::not(x) / Neg::neg(x)
                let m = self.mono_method(self.b.binops[&e.0]);
                let xt = self.body_ty(*x);
                let v = self.lower_expr(*x);
                let instance = crate::instance::Instance::expect_resolve(&self.u.tcx, &self.u.prog, crate::tcx::TypingEnv::fully_monomorphized(&self.u.prog), m.def, &m.args);

                self.emit_instance(instance, vec![(v, xt)], t)
            }
            ExprKind::Unary(op, x) => {
                let xt = self.body_ty(*x);
                match op {
                    // `*x` on a Deref type is `*Deref::deref(&x)` (Reference expressions/
                    // operator-expr.md dereference operator): read through the place
                    // moving out of `*tmp` of a temporary Box (`*opt.take()?`, `*make_box()`): rustc's
                    // drop elaboration leaves only box_free for the temporary (its contents were moved),
                    // so the box is taken by value, its contents read and the allocation freed here;
                    // a temporary's full drop would drop the moved contents a second time
                    UnOp::Deref if self.temp_box_deref(*x) =>
                    {
                        let inner = self.box_inner(xt).unwrap();
                        let b = self.lower_expr(*x);
                        let b = self.to_leaves(b, xt);
                        let v = self.read_place(&Place::Mem(b[0], 0), inner);
                        if let Some(free) = self.u.core_path(&["boxed", "box_free_after_move"]) {
                            let id = self.u.fn_id(FnKey::Inst(free, vec![inner]));
                            self.nounwind += 1;
                            self.emit(Inst::Call(Callee::Fn(id), vec![b[0]], Vec::new()));
                            self.nounwind -= 1;
                        }
                        v
                    }
                    UnOp::Deref if self.b.ov_deref(e.0, 0).is_some() || self.box_inner(xt).is_some() => {
                        let (pl, pt) = self.lower_place(e);
                        let v = self.read_place(&pl, pt);
                        // moving out of `*b` (a Box local): the box is freed without dropping
                        // its contents again (rustc drop elaboration of the moved path)
                        if self.needs_drop_glue(pt) {
                            if let Some((li, path)) = self.field_move_path(e) {
                                self.moved_field(li, &path);
                            }
                        }
                        v
                    }
                    UnOp::Deref => {
                        let p = self.lower_expr(*x);
                        let p = self.to_leaves(p, xt);
                        self.read_place(&Place::Mem(p[0], 0), t)
                    }
                    UnOp::Neg => {
                        let v = self.lower_expr(*x);
                        let (v, xt) = self.deref_prim_operand(v, xt, t);
                        let v = self.to_leaves(v, xt);
                        match self.kind(t) {
                            TyKind::Float(_) => {
                                let f64_ = self.is_f64(t);
                                let d = self.f.vreg(if f64_ { Cls::F64 } else { Cls::F32 });
                                self.emit(Inst::FUnary(FUn::Neg, f64_, d, v[0]));
                                Val::L(vec![d])
                            }
                            _ => {
                                let it = self.int_ty(t);
                                let d = self.f.vreg(Cls::I);
                                if self.overflow_checks && it.signed && !self.negated_literal(*x) {
                                    let min = self.iconst(extend_const(1i64 << (it.bits - 1), it));
                                    let overflow = self.icmp(Cond::Eq, false, v[0], min);
                                    self.overflow_assert(overflow, "attempt to negate with overflow");
                                }
                                self.emit(Inst::INeg(it, d, v[0]));
                                Val::L(vec![d])
                            }
                        }
                    }
                    UnOp::Not => {
                        let v = self.lower_expr(*x);
                        let (v, xt) = self.deref_prim_operand(v, xt, t);
                        let v = self.to_leaves(v, xt);
                        let d = self.f.vreg(Cls::I);
                        if matches!(self.kind_ref(t), TyKind::Bool) {
                            // `!b` as `b == 0` (same value for a bool): a branch on it folds into
                            // the compare-and-branch (test + jz), where `b ^ 1` kept a movzx + xor +
                            // test + jnz per test (a filter closure's `!e.flag` in a hot loop)
                            self.emit(Inst::ICmpI(Cond::Eq, false, d, v[0], 0));
                        } else {
                            let it = self.int_ty(t);
                            self.emit(Inst::INot(it, d, v[0]));
                        }
                        Val::L(vec![d])
                    }
                }
            }
            ExprKind::Binary(op, a, b) => {
                if let Some(m) = self.b.binops.get(&e.0).map(|&m| self.mono_method(m)) {
                    // operator trait call: Op::op(a, b); comparisons take both by reference
                    let at = self.body_ty(*a);
                    let bt = self.body_ty(*b);
                    if m.autoref == 1 {
                        return self.lower_cmp_call(*a, *b, m.def, &m.args, t);
                    }
                    let av = self.lower_expr(*a);
                    // the right operand coerced to the impl's Rhs (`s + &string`)
                    let bt = m.args.get(1).copied().unwrap_or(bt);
                    let bv = self.lower_expr_coerced(*b, bt);
                    let instance = crate::instance::Instance::expect_resolve(&self.u.tcx, &self.u.prog, crate::tcx::TypingEnv::fully_monomorphized(&self.u.prog), m.def, &m.args);

                    return self.emit_instance(instance, vec![(av, at), (bv, bt)], t);
                }
                self.lower_binary(*op, *a, *b, t)
            }
            ExprKind::Assign(a, b) => {
                if is_assignee_pattern(self.ast(), *a) {
                    // destructuring assignment (rustc desugars `(a, b) = e` to
                    // `{ let (x, y) = e; a = x; b = y; }`): the value first, then each part is
                    // assigned left to right; parts matched by `_` / `..` drop with the value
                    let bt = self.body_ty(*b);
                    let v = self.lower_expr(*b);
                    let pl = self.val_to_place(v);
                    self.assign_parts(*a, &pl, bt);
                    return self.unit();
                }
                let at = self.body_ty(*a);
                let v = self.lower_expr_coerced(*b, at);
                self.assign_to(*a, v);
                self.unit()
            }
            ExprKind::AssignOp(_, a, b) if self.b.binops.contains_key(&e.0) => {
                // `a op= b` through <A as OpAssign<B>>::op_assign(&mut a, b)
                let m = self.mono_method(self.b.binops[&e.0]);
                self.want_mut = true;
                let (pl, pt) = self.lower_place(*a);
                self.want_mut = false;
                let addr = self.place_addr(&pl, pt);
                let bt = self.body_ty(*b);
                let bt = m.args.get(1).copied().unwrap_or(bt);
                let bv = self.lower_expr_coerced(*b, bt);
                let instance = crate::instance::Instance::expect_resolve(&self.u.tcx, &self.u.prog, crate::tcx::TypingEnv::fully_monomorphized(&self.u.prog), m.def, &m.args);

                let rt = self.u.tcx.tys.intern(TyKind::Ref(true, pt, crate::types::RegionId::ERASED));
                self.emit_instance(instance, vec![(Val::L(vec![addr]), rt), (bv, bt)], t);
                self.unit()
            }
            ExprKind::AssignOp(op, a, b) => {
                // primitive `place op= v`: v first, then the place (rustc: right-to-left for
                // built-in compound assignment; the overloaded form above is left-to-right).
                let bt = self.body_ty(*b);
                let rv = self.lower_expr(*b);
                let (rv, bt) = self.prim_leaves(rv, bt);
                self.want_mut = true;
                let (pl, pt) = self.lower_place(*a);
                self.want_mut = false;
                let cur = self.read_place(&pl, pt);
                let cur = self.to_leaves(cur, pt);
                let r = self.arith(*op, pt, cur[0], rv[0], bt);
                self.write_place(&pl, pt, Val::L(vec![r]));
                self.unit()
            }
            ExprKind::Cast(x, _) => {
                let xt = self.body_ty(*x);
                if let Some(Coerce::ToDyn(..) | Coerce::Upcast(..) | Coerce::Unsize | Coerce::DerefRef(_) | Coerce::ReifyFn | Coerce::ClosureFn | Coerce::UnsafeFn | Coerce::RefToRaw(_)) = self.b.coerce(x.0) {
                    return self.lower_expr_coerced(*x, t);
                }
                if matches!(self.kind_ref(t), TyKind::Adt(..) | TyKind::Tuple(_) | TyKind::Array(..)) {
                    // a coercion cast without a representation change (`Some(4) as Option<u32>`)
                    return self.lower_expr(*x);
                }
                let v = self.lower_expr(*x);
                self.cast(v, xt, t)
            }
            ExprKind::Block(b, label) => {
                if label.is_some() {
                    let brk = self.f.block();
                    let res = self.result_place(t);
                    let l = label.map(|x| self.lifetime_sym(x));
                    self.loops.push(LoopCx { label: l, brk, cont: brk, result: Some((res.clone(), t)), scope_depth: self.scopes.len(), temps_depth: self.stmt_temps.len() });
                    let v = self.lower_block(*b);
                    self.loops.pop();
                    if !self.is_never(t) {
                        self.write_place(&res, t, v);
                    }
                    self.goto(brk);
                    self.switch(brk);
                    return self.read_place(&res, t);
                }
                self.lower_block(*b)
            }
            ExprKind::Unsafe(b) => self.lower_block(*b),
            ExprKind::ConstBlock(b) => {
                // an integer SIMD vector (simd_shuffle's index, `const { .. }` in core::simd's
                // swizzles): evaluated here, its lanes known to the SIMD intrinsics
                if let Some(v) = self.simd_const_block(*b, t) {
                    return v;
                }
                let previous = self.overflow_checks;
                self.overflow_checks = true;
                let value = self.lower_block(*b);
                self.overflow_checks = previous;
                value
            },
            ExprKind::If(c, then, els) => self.lower_if(*c, *then, els.get(), t),
            ExprKind::Let(p, x) => {
                // only reached as a bare condition (`if let` handles its own); evaluate as a test
                let xt = self.body_ty(*x);
                let v = self.lower_expr(*x);
                let place = self.val_to_place(v);
                let res = self.f.vreg(Cls::I);
                let fail = self.f.block();
                let join = self.f.block();
                self.test_pat(*p, &place, xt, fail);
                self.bind_pat_place(*p, &place, xt);
                self.emit(Inst::Iconst(res, 1));
                self.goto(join);
                self.switch(fail);
                self.emit(Inst::Iconst(res, 0));
                self.goto(join);
                self.switch(join);
                Val::L(vec![res])
            }
            ExprKind::Match(x, arms) => self.lower_match(*x, ast.list(*arms), t),
            ExprKind::While(row) => {
                let crate::ast::WhileRow { cond: c, body: b, label } = ast.row(*row);
                // `while let p = e { body }` is `loop { match e { p => body, _ => break } }`
                // (rustc desugaring): the bindings and the scrutinee temporary live for one
                // iteration and are dropped at its end, on `continue`/`break`, and when the
                // pattern fails
                let head = self.f.block();
                let body = self.f.block();
                let fail = self.f.block();
                let exit = self.f.block();
                self.goto(head);
                self.switch(head);
                self.const_fuel_tick();
                let outer = self.scopes.len();
                self.push_scope();
                let mark = self.stmt_temps.len();
                let l = label.map(|x| self.lifetime_sym(x));
                self.loops.push(LoopCx { label: l, brk: exit, cont: head, result: None, scope_depth: outer, temps_depth: mark });
                self.lower_cond(*c, body, fail);
                let ts = self.stmt_temps.split_off(mark);
                if !self.scope_temps.is_empty() {
                    let d = self.scope_temps.len() - 1;
                    for x in ts {
                        self.push_scope_temp(d, x);
                    }
                }
                self.switch(body);
                self.lower_block_tscope(*b);
                self.loops.pop();
                self.drop_scopes_from(outer);
                self.goto(head);
                self.switch(fail);
                self.drop_scopes_from(outer);
                self.goto(exit);
                self.scopes.truncate(outer);
                self.scope_temps.truncate(outer);
                self.scope_temp_pos.truncate(outer);
                self.scope_marks.truncate(outer);
                self.tail_marks.truncate(outer);
                self.switch(exit);
                self.unit()
            }
            ExprKind::Loop(b, label) => {
                let head = self.f.block();
                let exit = self.f.block();
                let res = self.result_place(t);
                self.goto(head);
                self.switch(head);
                self.const_fuel_tick();

                let l = label.map(|x| self.lifetime_sym(x));
                self.loops.push(LoopCx { label: l, brk: exit, cont: head, result: Some((res.clone(), t)), scope_depth: self.scopes.len(), temps_depth: self.stmt_temps.len() });
                self.lower_block_tscope(*b);
                self.loops.pop();
                self.goto(head);
                self.switch(exit);
                if self.is_never(t) {
                    self.term(Term::Unreachable);
                    self.dead();
                    return self.unit();
                }
                self.read_place(&res, t)
            }
            ExprKind::For(row) => {
                let crate::ast::ForRow { pat: p, iter: it, body: b, label } = ast.row(*row);
                self.lower_for(*p, *it, *b, *label);
                self.unit()
            }
            ExprKind::Break(label, x) => {
                let l = label.map(|y| self.lifetime_sym(y));
                let idx = self.loop_index(l);
                if let Some(i) = idx {
                    if let Some(v) = &x.get() {
                        let val = self.lower_expr(*v);
                        if let Some((pl, pt)) = self.loops[i].result.clone() {
                            self.write_place(&pl, pt, val);
                        }
                    }
                    let depth = self.loops[i].scope_depth;
                    let td = self.loops[i].temps_depth;
                    self.exit_scopes(depth, td);
                    let brk = self.loops[i].brk;
                    self.goto(brk);
                }
                self.dead();
                self.unit()
            }
            ExprKind::Continue(label) => {
                let l = label.map(|y| self.lifetime_sym(y));
                if let Some(i) = self.loop_index(l) {
                    let depth = self.loops[i].scope_depth;
                    let td = self.loops[i].temps_depth;
                    self.exit_scopes(depth, td);
                    let c = self.loops[i].cont;
                    self.goto(c);
                }
                self.dead();
                self.unit()
            }
            ExprKind::Try(x) => self.lower_try(e, *x, t),
            ExprKind::Return(x) => {
                let rt = self.ret_ty;
                let v = match &x.get() {
                    Some(v) => self.lower_expr_coerced(*v, rt),
                    None => self.unit(),
                };
                // leaving the function leaves every enclosing statement: their temporaries
                // (an if-let / match scrutinee's `cell.borrow()`) are dropped too (Reference
                // destructors.md: scopes are exited innermost first)
                self.ret_val_early(v, rt);
                self.dead();
                self.unit()
            }
            ExprKind::Tuple(v) => {
                let tys = match self.kind(t) {
                    TyKind::Tuple(x) => self.u.tcx.tys.list(x).to_vec(),
                    _ => Vec::new(),
                };
                let mut vals = Vec::new();
                let mut guards = Vec::new();
                for (i, x) in ast.list(*v).iter().enumerate() {
                    let ft = tys.get(i).copied().unwrap_or(self.u.tcx.tys.error);
                    let val = self.lower_expr_coerced(*x, ft);
                    let val = self.guard_operand(&ast.list(*v)[i + 1..], val, ft, &mut guards);
                    vals.push((val, ft));
                }
                let r = self.build_record(t, 0, vals);
                self.unguard(&guards);
                r
            }
            ExprKind::Array(v) => {
                let et = match self.kind(t) {
                    TyKind::Array(e, _) => e,
                    _ => self.u.tcx.tys.error,
                };
                let el = self.layout(et);
                let stride = crate::layout::round_up(el.size, el.align);
                let a = self.new_slot_for(t);
                let mut guards = Vec::new();
                for (i, x) in ast.list(*v).iter().enumerate() {
                    let val = self.lower_expr_coerced(*x, et);
                    self.store_val(a, (i as u32 * stride) as i32, et, val);
                    // a stored element drops if a later one exits early
                    if self.needs_drop_glue(et) && ast.list(*v)[i + 1..].iter().any(|y| self.may_exit_or_unwind(*y)) {
                        let ea = self.addr_add(a, (i as u32 * stride) as i32);
                        let f = self.f.vreg(Cls::I);
                        self.entry_inits.push(Inst::Iconst(f, 0));
                        self.emit(Inst::Iconst(f, 1));
                        self.stmt_temps.push((ea, et, Some(f)));
                        guards.push(f);
                    }
                }
                self.unguard(&guards);
                Val::M(a, 0)
            }
            ExprKind::Repeat(x, _) => {
                let (et, n) = match self.kind(t) {
                    TyKind::Array(e, n) => (e, n.try_to_target_usize(&self.u.tcx.tys).expect("normalized array length")),
                    _ => (self.u.tcx.tys.error, 0),
                };
                let el = self.layout(et);
                let stride = crate::layout::round_up(el.size, el.align);
                let a = self.new_slot_for(t);
                // the operand coerced to the element type (`[&[]; 4]` as `[&[i16]; 4]`)
                let val = self.lower_expr_coerced(*x, et);
                let is_const = matches!(self.b.res.get(&strip_paren(self.ast(), *x).0), Some(Res::Def(d)) if matches!(self.u.prog.def(*d).kind, DefKind::Const | DefKind::AssocConst));
                if n == 0 && !is_const {
                    // `[v; 0]` evaluates v and drops it right away (rustc: the operand is moved
                    // into zero elements); a const operand creates no value
                    self.drop_val(val.clone(), et);
                }
                if stride == 0 {
                    // zero-sized elements: every copy is the same no-op store (`[(); usize::MAX]`)
                    if n > 0 {
                        self.store_val(a, 0, et, val);
                    }
                } else if n <= 16 {
                    for i in 0..n {
                        self.store_val(a, (i as u32 * stride) as i32, et, val.clone());
                    }
                } else {
                    // loop: i in 0..n
                    let i = self.f.vreg(Cls::I);
                    self.emit(Inst::Iconst(i, 0));
                    let nn = self.iconst(n as i64);
                    let st = self.iconst(stride as i64);
                    let head = self.f.block();
                    let body = self.f.block();
                    let exit = self.f.block();
                    self.goto(head);
                    self.switch(head);
                    let c = self.f.vreg(Cls::I);
                    self.emit(Inst::ICmp(Cond::Lt, false, c, i, nn));
                    self.term(Term::Branch(c, body, exit));
                    self.switch(body);
                    let off = self.f.vreg(Cls::I);
                    self.emit(Inst::IBin(IOp::Mul, U64, off, i, st));
                    let addr = self.f.vreg(Cls::I);
                    self.emit(Inst::IBin(IOp::Add, U64, addr, a, off));
                    self.store_val(addr, 0, et, val);
                    let one = self.iconst(1);
                    self.emit(Inst::IBin(IOp::Add, U64, i, i, one));
                    self.goto(head);
                    self.switch(exit);
                }
                Val::M(a, 0)
            }
            ExprKind::Field(..) | ExprKind::TupleField(..) | ExprKind::Index(..) => {
                if let ExprKind::Index(a, i) = &ex.kind {
                    if self.is_range_index(e, *i) {
                        return self.lower_slice_range(e, *a, *i, t);
                    }
                }
                // moving a field out of a temporary (`make().a`, `(*tmp_box).a`): the temporary
                // is owned by the statement; the rest of it drops at the statement's end
                // (rustc: a move path for the field, the temporary's drop elaborated around it;
                // `three().1.0` is one temporary, `three()`, and one move path in it)
                if self.needs_drop_glue(t) {
                    if let Some((root, path)) = self.rvalue_proj(e) {
                        let (a, rt, f, fp, ft) = self.lower_rvalue_proj(root, &path);
                        let v = self.read_place(&fp, ft);
                        self.moved_temp_part(f, &path);
                        self.stmt_temps.push((a, rt, Some(f)));
                        return v;
                    }
                }
                let mark = self.stmt_temps.len();
                let (pl, pt) = self.lower_place(e);
                let v = self.read_place(&pl, pt);
                // moving a field out of a local: only the rest of the local is dropped
                if self.needs_drop_glue(pt) {
                    match self.field_move_path(e) {
                        Some((li, path)) => self.moved_field(li, &path),
                        // out of a temporary (`make().field`): it is not dropped again
                        None => self.stmt_temps.truncate(mark),
                    }
                }
                v
            }
            ExprKind::AddrOf(_, m, x) => {
                // `&[T; N]` from an array literal, `&local`, `&expr`
                // constant promotion (Reference destructors.md constant promotion, rustc
                // promote_consts): `&<constant rvalue>` borrows a static; places (`&mut local`
                // in a const initializer) and mutable borrows are not promoted
                // (in a const/static initializer every borrowed rvalue lives in static memory:
                // its temporaries have the initializer's 'static scope)
                if !self.is_place_expr(*x) && (self.const_init || (!*m && self.promotable(*x))) {
                    if let Some(p) = self.promote_rvalue(*x, !*m) {
                        return Val::L(vec![p]);
                    }
                }
                let saved = self.want_mut;
                self.want_mut = *m;
                let (pl, pt) = self.lower_place(*x);
                self.want_mut = saved;
                if self.unsized_ty(pt) {
                    // reborrow of an unsized place: the fat pointer itself
                    if let Place::Regs(r) = pl {
                        return Val::L(r);
                    }
                }
                let a = self.place_addr(&pl, pt);
                Val::L(vec![a])
            }
            ExprKind::Call(f, args) => self.lower_call(e, *f, ast.list(*args), t),
            ExprKind::MethodCall(recv, row) => self.lower_method(e, *recv, ast.list(ast.row(*row).args), t),
            ExprKind::Struct(row) => { let crate::ast::StructRow { fields, base, rest, .. } = ast.row(*row); self.lower_struct_lit(e, ast.list(*fields), base.get(), *rest, t) }
            ExprKind::Range(a, b, _) => {
                // Range { start, end } as a plain struct value
                let mut vals = Vec::new();
                let tys = self.range_field_tys(t);
                if let Some(a) = &a.get() {
                    vals.push((self.lower_expr(*a), tys[0]));
                }
                if let Some(b) = &b.get() {
                    let ft = if tys.len() > 1 { tys[1] } else { tys[0] };
                    vals.push((self.lower_expr(*b), ft));
                }
                // RangeInclusive has an extra `exhausted: bool` field
                if let TyKind::Adt(d, _, _) = self.kind(t) {
                    let nf = self.u.tcx.adts.get(&d).map_or(0, |a| a.variants[0].fields.len());
                    while vals.len() < nf {
                        let z = self.iconst(0);
                        vals.push((Val::L(vec![z]), self.u.tcx.tys.bool_));
                    }
                }
                self.build_record(t, 0, vals)
            }
            ExprKind::Closure(row) => {
                // the closure value is its captured state; its code is a function of its own
                let is_move = ast.row(*row).is_move;
                self.closure_fn(e);
                let caps = self.storage_for_closure(e.0).roots.clone();
                if caps.is_empty() {
                    return self.unit();
                }
                let up = match self.kind(t) {
                    TyKind::Closure(..) => t,
                    _ => return self.unit(),
                };
                let mut vals = Vec::new();
                let _ = is_move;
                for (i, li) in caps.into_iter().enumerate() {
                    let lt = self.monomorphize(self.b.locals[li as usize].ty);
                    let pl = self.local_place(li);
                    // by value (a `move` closure, or the body consumes it): moved into the
                    // closure, the enclosing fn no longer drops it (rustc upvar.rs ByValue)
                    if self.capture_by_value(e.0, i, li) {
                        let v = self.read_place(&pl, lt);
                        // a precise capture takes only the fields the body uses: those are
                        // moved out, the rest stays this body's (and drops here)
                        match self.capture_fields(e.0, i) {
                            Some(fs) => {
                                for fi in fs {
                                    self.moved_field(li, &[(0, fi)]);
                                }
                            }
                            None => self.moved_local(li),
                        }
                        vals.push((v, lt));
                    } else {
                        let a = self.place_addr(&pl, lt);
                        let pt = self.u.tcx.tys.intern(TyKind::Ptr(true, lt));
                        vals.push((Val::L(vec![a]), pt));
                    }
                }
                // FnOnce closures: every capture starts owned (drop flags = 1), and the state
                // lives in memory (the closure type has no register leaves)
                if self.storage_for_closure(e.0).drop_flags {
                    let u8_ = self.u.tcx.tys.prim(Prim::U8);
                    let n = vals.len();
                    for _ in 0..n {
                        let one = self.iconst(1);
                        vals.push((Val::L(vec![one]), u8_));
                    }
                    let v = self.build_record(up, 0, vals);
                    let slot = self.new_slot_for(t);
                    self.store_val_with_layout(slot, 0, &self.u.closure_storage_layout(up), v);
                    return Val::M(slot, 0);
                }
                self.build_record(up, 0, vals)
            }
            ExprKind::Async(..) => {
                // the coroutine's code is its poll function (a closure fn of its own)
                self.closure_fn(e);
                self.build_coroutine(e.0, t)
            }
            ExprKind::Await(x) => self.lower_await(e, *x, t),
            ExprKind::Mac(m) => self.lower_mac(e, ast.row(*m), t),
            _ => {
                self.err(ex.lo, "unsupported expression in lowering".to_string());
                self.unit()
            }
        }
    }

    fn range_field_tys(&mut self, t: TyId) -> Vec<TyId> {
        if let TyKind::Adt(d, args, _) = self.kind(t) {
            if let Some(a) = self.u.tcx.adts.get(&d) {
                let mut v = Vec::new();
                for i in 0..a.variants[0].fields.len() {
                    v.push(self.u.tcx.field_ty(&self.u.prog, crate::tcx::TypingEnv::fully_monomorphized(&self.u.prog), d, 0, i as u32, self.u.tcx.tys.list(args)));
                }
                return v;
            }
        }
        vec![self.u.tcx.tys.error]
    }

    fn is_never(&self, t: TyId) -> bool {
        matches!(self.u.tcx.tys.kind(t), TyKind::Never)
    }

    fn lifetime_sym(&self, id: Ident) -> (u32, u32) {
        let t = self.ident_text(id);
        let t = t.trim_start_matches('\'');
        (self.u.prog.syms.get(t).unwrap_or(u32::MAX - 1), id.lo)
    }

    fn loop_index(&self, label: Option<(u32, u32)>) -> Option<usize> {
        let mut i = self.loops.len();
        while i > 0 {
            i -= 1;
            match label {
                None => {
                    // unlabeled break/continue skip labeled blocks (they have cont == brk)
                    if self.loops[i].cont != self.loops[i].brk || self.loops[i].label.is_none() {
                        return Some(i);
                    }
                }
                Some(l) => {
                    if let Some(d) = self.loops[i].label {
                        if d.0 == l.0 && self.u.prog.files[self.b.file as usize].hyg_visible(d.1, l.1) {
                            return Some(i);
                        }
                    }
                }
            }
        }
        None
    }

    fn result_place(&mut self, t: TyId) -> Place {
        let l = self.layout(t);
        match &l.leaves {
            Some(leaves) => Place::Regs(self.fresh_leaves(&leaves)),
            None => {
                let a = self.new_slot_for(t);
                Place::Mem(a, 0)
            }
        }
    }

    /// Builds a struct/tuple value from field values (registers when it fits).
    fn build_record(&mut self, t: TyId, vi: u32, vals: Vec<(Val, TyId)>) -> Val {
        let storage = matches!(self.kind(t), TyKind::Closure(..));
        let l = if storage { self.u.closure_storage_layout(t) } else { self.layout(t).clone() };
        match l.leaves {
            Some(ref leaves) => {
                let mut out = Vec::with_capacity(leaves.len());
                for (v, ft) in vals {
                    let r = self.to_leaves(v, ft);
                    out.extend(r);
                }
                Val::L(out)
            }
            None => {
                let a = if storage {
                    let slot = self.f.slot(l.size, l.align);
                    let address = self.f.vreg(Cls::I);
                    self.emit(Inst::SlotAddr(address, slot));
                    address
                } else {
                    let a = self.new_slot_for(t);
                    self.zero_ext(a, 0, t);
                    a
                };
                let place = Place::Mem(a, 0);
                for (i, (v, _)) in vals.into_iter().enumerate() {
                    let (fp, ft) = self.project(&place, t, vi, i as u32);
                    self.write_place(&fp, ft, v);
                }
                Val::M(a, 0)
            }
        }
    }

    fn lower_lit(&mut self, e: ExprId, k: LitKind, t: TyId) -> Val {
        let ex = self.ast().expr(e);
        let text = self.src_text(ex.lo, ex.hi);
        match k {
            LitKind::Int | LitKind::Float => match self.kind(t) {
                TyKind::Float(p) => {
                    let num = strip_num(&text);
                    if p == Prim::F32 {
                        let v: f32 = num.parse().unwrap_or(0.0);
                        let d = self.f.vreg(Cls::F32);
                        self.emit(Inst::Fconst(d, v.to_bits() as u64, false));
                        Val::L(vec![d])
                    } else {
                        let v: f64 = num.parse().unwrap_or(0.0);
                        let d = self.f.vreg(Cls::F64);
                        self.emit(Inst::Fconst(d, v.to_bits(), true));
                        Val::L(vec![d])
                    }
                }
                _ if self.is_wide(t) => {
                    let v = crate::const_fold::parse_int_lit(&text).unwrap_or(0);
                    let lo = self.iconst(v as u64 as i64);
                    let hi = self.iconst((v >> 64) as u64 as i64);
                    Val::L(vec![lo, hi])
                }
                _ => {
                    let v = crate::const_fold::parse_int_lit(&text).unwrap_or(0) as i64;
                    let it = self.int_ty(t);
                    let v = extend_const(v, it);
                    Val::L(vec![self.iconst(v)])
                }
            },
            LitKind::Bool(b) => Val::L(vec![self.iconst(b as i64)]),
            LitKind::Char => {
                let s = unescape_str(&text);
                let c = s.chars().next().unwrap_or('\0');
                Val::L(vec![self.iconst(c as i64)])
            }
            LitKind::Byte => {
                let b = unescape_bytes(&text);
                Val::L(vec![self.iconst(*b.first().unwrap_or(&0) as i64)])
            }
            LitKind::Str | LitKind::RawStr => {
                let s = unescape_str(&text);
                let addr = self.u.lit_sym(s.as_bytes());
                let p = self.f.vreg(Cls::I);
                self.emit(Inst::Addr(p, addr));
                let l = self.iconst(s.len() as i64);
                Val::L(vec![p, l])
            }
            LitKind::ByteStr => {
                let b = unescape_bytes(&text);
                let addr = self.u.lit_sym(&b);
                let p = self.f.vreg(Cls::I);
                self.emit(Inst::Addr(p, addr));
                Val::L(vec![p])
            }
            LitKind::CStr => {
                // a fat &CStr: the bytes with their nul (CStr { inner: [c_char] })
                let mut b = unescape_bytes(&text);
                b.push(0);
                let addr = self.u.lit_sym(&b);
                let p = self.f.vreg(Cls::I);
                self.emit(Inst::Addr(p, addr));
                let l = self.iconst(b.len() as i64);
                Val::L(vec![p, l])
            }
        }
    }

    fn lower_def_value(&mut self, e: ExprId, d: DefId, t: TyId) -> Val {
        let k = self.u.prog.def(d).kind;
        match k {
            DefKind::Const | DefKind::AssocConst => {
                let args = self.b.res_args.get(&e.0).copied().unwrap_or_default();
                let args = self.mono_list(args);
                self.const_val(d, args, t)
            }
            DefKind::Static => {
                crate::boundary::note_static(self.u, self.b.def, &self.b.args, d);
                if !self.u.provided.is_empty() {
                    self.check_provided_static(d, t);
                }
                let addr = self.static_addr(d);
                let addr = self.u.syms.intern(crate::syms::SymKind::Static(d), addr);
                let p = self.f.vreg(Cls::I);
                self.emit(Inst::Addr(p, addr));
                self.read_place(&Place::Mem(p, 0), t)
            }
            DefKind::ForeignStatic => match self.u.foreign_sym(d) {
                // `extern { static X: T; }`: the C global's current value
                Some(addr) => {
                    let p = self.f.vreg(Cls::I);
                    self.emit(Inst::Addr(p, addr));
                    self.read_place(&Place::Mem(p, 0), t)
                }
                None => {
                    let n = self.u.prog.def_path(d);
                    self.err(self.pos, format!("foreign static `{}` not found in the loaded libraries -- link it (#[link])", n));
                    self.dummy_val(t)
                }
            },
            DefKind::Variant | DefKind::Struct => {
                let vi = if k == DefKind::Variant { self.u.prog.def(d).sub } else { 0 };
                // a tuple constructor as a value: a zero-sized fn item (calls and reification go
                // through FnKey::Inst, which jit fn_id maps to the constructor shim)
                if let TyKind::FnDef(..) = self.kind(t) {
                    return self.unit();
                }
                // unit variant / unit struct value
                self.build_enum(t, vi, Vec::new())
            }
            // fn items are zero-sized values; calls resolve them directly
            _ => self.unit(),
        }
    }

    /// Static `d`'s storage. Its initializer failed (address 0, reported as its own error):
    /// this build fails too, as for a constant, rather than running code through a null address.
    fn static_addr(&mut self, d: DefId) -> u64 {
        let addr = self.u.static_addr_shared(d);
        if addr == 0 {
            self.err(self.pos, format!("could not evaluate static {} -- see its own error", self.u.prog.def_path(d)));
        }
        addr
    }

    fn const_val(&mut self, d: DefId, args: Vec<TyId>, t: TyId) -> Val {
        match self.u.const_addr_shared(d, args) {
            Some(addr) => {
                let addr = self.u.data_sym(addr);
                let l = self.layout(t);
                // scalars become immediates (a pointer: its symbol)
                if let Some(leaves) = &l.leaves {
                    if leaves.len() == 1 {
                        let lf = leaves[0];
                        if let Some(v) = self.const_leaf_at(addr.plus(lf.off as i64), lf) {
                            return Val::L(vec![v]);
                        }
                    }
                }
                // an aggregate is copied out of the constant's allocation: every use of a const
                // is a fresh value (rustc: a `Constant` operand, mir_build as_operand; the
                // evaluated allocation is immutable and only promoted shared borrows point into
                // it). A Val::M of the allocation itself would be adopted as the storage of
                // `let mut h = C;` (bind_pat) or of `&mut C`'s temporary, and writes would land
                // in the constant
                let p = self.f.vreg(Cls::I);
                self.emit(Inst::Addr(p, addr));
                self.read_place(&Place::Mem(p, 0), t)
            }
            None => {
                self.err(self.pos, format!("could not evaluate constant {} -- see its own error", self.u.prog.def_path(d)));
                self.dummy_val(t)
            }
        }
    }

    fn const_leaf(&mut self, raw: u64, lf: Leaf) -> VReg {
        match lf.cls {
            Cls::I => {
                let v = match lf.mem {
                    Mem::Int(1, true) => raw as u8 as i8 as i64,
                    Mem::Int(2, true) => raw as u16 as i16 as i64,
                    Mem::Int(4, true) => raw as u32 as i32 as i64,
                    _ => raw as i64,
                };
                self.iconst(v)
            }
            Cls::F32 => {
                let d = self.f.vreg(Cls::F32);
                self.emit(Inst::Fconst(d, raw & 0xffff_ffff, false));
                d
            }
            // type layouts never have vector leaves
            Cls::F64 | Cls::V128 | Cls::V256 => {
                let d = self.f.vreg(Cls::F64);
                self.emit(Inst::Fconst(d, raw, true));
                d
            }
        }
    }

    /// Enum (or struct) value of variant `vi` with field values.
    fn build_enum(&mut self, t: TyId, vi: u32, vals: Vec<(Val, TyId)>) -> Val {
        let adt = match self.kind_ref(t) {
            TyKind::Adt(d, _, _) => self.u.tcx.adts.get(d),
            _ => None,
        };
        let adt = match adt {
            Some(a) => a,
            None => return self.unit(),
        };
        if !adt.is_enum {
            return self.build_record(t, 0, vals);
        }
        let l = self.layout(t);
        let Some(discr) = adt.discriminant_for_variant(&self.u.tcx, &self.u.prog, vi) else { return self.unit(); };
        let disc128 = discr.signed_value();
        let disc = disc128 as i64;
        if l.tag == Some((0, l.leaves.as_ref().and_then(|v| v.first()).map_or(Mem::Int(0, false), |x| x.mem))) && l.enc == crate::layout::TagEnc::Direct && l.leaves.as_ref().map_or(false, |v| v.len() == 1) {
            // fieldless enum: the tag itself
            return Val::L(vec![self.iconst(disc)]);
        }
        let a = self.new_slot_for(t);
        let _ = (disc, &l);
        self.store_disc(a, 0, t, vi);
        let place = Place::Mem(a, 0);
        for (i, (v, _)) in vals.into_iter().enumerate() {
            let (fp, ft) = self.project(&place, t, vi, i as u32);
            self.write_place(&fp, ft, v);
        }
        Val::M(a, 0)
    }

    fn lower_struct_lit(&mut self, e: ExprId, fields: &[FieldExpr], base: Option<ExprId>, rest: bool, t: TyId) -> Val {
        if !self.u.provided.is_empty() {
            self.check_provided_built(t);
        }
        let d = match self.b.res.get(&e.0) {
            Some(Res::Def(d)) => *d,
            _ => return self.unit(),
        };
        let vi = if self.u.prog.def(d).kind == DefKind::Variant { self.u.prog.def(d).sub } else { 0 };
        let a = self.new_slot_for(t);
        self.zero_ext(a, 0, t);
        let place = Place::Mem(a, 0);
        if let Some(bx) = base {
            return self.lower_struct_update(e, fields, bx, t, vi, a);
        }
        let adt = match self.kind_ref(t) {
            TyKind::Adt(ad, _, _) => self.u.tcx.adts.get(ad),
            _ => None,
        };
        if let Some(adt) = &adt {
            if adt.is_enum {
                self.store_disc(a, 0, t, vi);
            }
        }
        let mut guards = Vec::new();
        let exprs: Vec<ExprId> = fields.iter().map(|f| f.expr).collect();
        for (k, fe) in fields.iter().enumerate() {
            let txt = self.ident_text(fe.name);
            let name = if txt.as_bytes()[0].is_ascii_digit() {
                txt.parse::<u32>().unwrap_or(0) | 0x8000_0000
            } else {
                self.u.prog.syms.get(txt).unwrap_or(u32::MAX)
            };
            if let Some(fi) = self.field_index(t, vi, name) {
                let (fp, ft) = self.project(&place, t, vi, fi);
                let v = self.lower_expr_coerced(fe.expr, ft);
                self.write_place(&fp, ft, v);
                // a written field drops if a later field's expression exits early
                if self.needs_drop_glue(ft) && exprs[k + 1..].iter().any(|y| self.may_exit_or_unwind(*y)) {
                    let fa = self.place_addr(&fp, ft);
                    let f = self.f.vreg(Cls::I);
                    self.entry_inits.push(Inst::Iconst(f, 0));
                    self.emit(Inst::Iconst(f, 1));
                    self.stmt_temps.push((fa, ft, Some(f)));
                    guards.push(f);
                }
            }
        }
        // `S { a: 1, .. }`: the omitted fields from their declared defaults (typeck struct_defaults)
        if rest {
            let defs = self.b.struct_defaults.get(&e.0).cloned().unwrap_or_default();
            for (fi, cd) in defs {
                let (fp, ft) = self.project(&place, t, vi, fi);
                let v = self.const_val(cd, Vec::new(), ft);
                self.write_place(&fp, ft, v);
            }
        }
        self.unguard(&guards);
        self.load_val(a, 0, t)
    }

    /// `S { f: x, ..base }` (Reference expr.struct.update): the given fields are evaluated first,
    /// then the remaining fields are moved or copied out of `base` (rustc builds the base as a
    /// place: a local base keeps what was not moved; its moved fields are tracked per field).
    fn lower_struct_update(&mut self, _e: ExprId, fields: &[FieldExpr], bx: ExprId, t: TyId, vi: u32, a: VReg) -> Val {
        let place = Place::Mem(a, 0);
        let mut given: Vec<(u32, Val, TyId)> = Vec::new();
        for fe in fields {
            let txt = self.ident_text(fe.name);
            let name = if txt.as_bytes()[0].is_ascii_digit() {
                txt.parse::<u32>().unwrap_or(0) | 0x8000_0000
            } else {
                self.u.prog.syms.get(txt).unwrap_or(u32::MAX)
            };
            if let Some(fi) = self.field_index(t, vi, name) {
                let (_, ft) = self.project(&place, t, vi, fi);
                let v = self.lower_expr_coerced(fe.expr, ft);
                given.push((fi, v, ft));
            }
        }
        let nf = match self.kind(t) {
            TyKind::Adt(ad, _, _) => self.u.tcx.adts.get(&ad).map_or(0, |x| x.variants.get(vi as usize).map_or(0, |v| v.fields.len())),
            _ => 0,
        };
        let root = self.field_move_path(bx);
        let mark = self.stmt_temps.len();
        let (bp, bt) = self.lower_place(bx);
        for fi in 0..nf as u32 {
            if given.iter().any(|g| g.0 == fi) {
                continue;
            }
            let (src, ft) = self.project(&bp, bt, vi, fi);
            let v = self.read_place(&src, ft);
            let (dst, _) = self.project(&place, t, vi, fi);
            self.write_place(&dst, ft, v);
            if self.needs_drop_glue(ft) {
                if let Some((li, path)) = &root {
                    let mut p = path.clone();
                    p.push((vi, fi));
                    self.moved_field(*li, &p);
                }
            }
        }
        if root.is_none() && !self.base_is_borrowed_place(bx) {
            // a temporary base: its fields that were not taken are dropped with it
            // (the base temporary itself is not dropped as a whole: its taken fields moved)
            let ba = self.place_addr(&bp, bt);
            let mut k = mark;
            while k < self.stmt_temps.len() {
                if self.stmt_temps[k].0 == ba {
                    self.stmt_temps.remove(k);
                } else {
                    k += 1;
                }
            }
            for (fi, _, ft) in &given {
                if self.needs_drop_glue(*ft) {
                    let (src, _) = self.project(&bp, bt, vi, *fi);
                    let fa = self.place_addr(&src, *ft);
                    self.drop_at(fa, *ft);
                }
            }
            // a live-editable struct's base (M10): its extension block, whose fields moved or
            // were dropped above, is freed
            if let Some(e) = self.layout(bt).ext {
                let ba = self.place_addr(&bp, bt);
                let blk = self.f.vreg(Cls::I);
                self.emit(Inst::Load(Mem::Int(8, false), blk, ba, e as i32));
                let blk = {
                    let m = self.iconst(!1i64);
                    self.ibin(IOp::And, false, blk, m)
                };
                let f: rt_fn_ty!(fn(u64)) = rt_ext_free;
                let fs = self.u.syms.intern(crate::syms::SymKind::Rt("ext_free"), f as usize as u64);
                self.emit(Inst::Call(Callee::Host(fs), vec![blk], Vec::new()));
            }
        }
        for (fi, v, ft) in given {
            let (dst, _) = self.project(&place, t, vi, fi);
            self.write_place(&dst, ft, v);
        }
        self.load_val(a, 0, t)
    }

    // ------------------------------------------------------------ places

    pub fn lower_place(&mut self, e: ExprId) -> (Place, TyId) {
        let ast = self.ast();
        let ex = ast.expr(e);
        let t = self.body_ty(e);
        match &ex.kind {
            ExprKind::Paren(x) => self.lower_place(*x),
            ExprKind::Path(_) => match self.b.res.get(&e.0).copied() {
                Some(Res::Local(li)) => {
                    let lt = self.monomorphize(self.b.locals[li as usize].ty);
                    (self.local_place(li), lt)
                }
                Some(Res::Def(d)) if self.u.prog.def(d).kind == DefKind::Static => {
                    crate::boundary::note_static(self.u, self.b.def, &self.b.args, d);
                    if !self.u.provided.is_empty() {
                        self.check_provided_static(d, t);
                    }
                    let addr = self.static_addr(d);
                    let addr = self.u.syms.intern(crate::syms::SymKind::Static(d), addr);
                    let p = self.f.vreg(Cls::I);
                    self.emit(Inst::Addr(p, addr));
                    (Place::Mem(p, 0), t)
                }
                Some(Res::Def(d)) if self.u.prog.def(d).kind == DefKind::ForeignStatic => match self.u.foreign_sym(d) {
                    Some(addr) => {
                        let p = self.f.vreg(Cls::I);
                        self.emit(Inst::Addr(p, addr));
                        (Place::Mem(p, 0), t)
                    }
                    None => {
                        let v = self.lower_expr(e);
                        (self.val_to_place(v), t)
                    }
                },
                _ => {
                    // an rvalue: its own subexpressions are not part of the place (want_mut ends)
                    let saved = std::mem::replace(&mut self.want_mut, false);
                    let v = self.lower_expr(e);
                    self.want_mut = saved;
                    (self.temp_place(e, v, t), t)
                }
            },
            ExprKind::Unary(UnOp::Deref, x) if self.b.ov_deref(e.0, 0).is_some() => {
                let (pl, pt) = self.lower_place(*x);
                self.deref_place(e.0, 0, pl, pt)
            }
            ExprKind::Unary(UnOp::Deref, x) => {
                let xt = self.body_ty(*x);
                let saved = std::mem::replace(&mut self.want_mut, false);
                let p = self.lower_expr(*x);
                self.want_mut = saved;
                let p = self.to_leaves(p, xt);
                if self.unsized_ty(t) {
                    return (Place::Regs(p), t);
                }
                (Place::Mem(p[0], 0), t)
            }
            ExprKind::Field(x, _) | ExprKind::TupleField(x, _) => {
                let (derefs, fi) = match self.b.field_idx.get(&e.0) {
                    Some(&(d, f)) => (d as usize, f),
                    None => return (Place::Regs(Vec::new()), t),
                };
                let (mut pl, mut pt) = self.lower_place(*x);
                for k in 0..derefs {
                    let (a, b2) = self.deref_place(e.0, k, pl, pt);
                    pl = a;
                    pt = b2;
                }
                crate::boundary::note_field(self.u, self.b.def, &self.b.args, pt, fi as u32);
                if !self.u.provided.is_empty() {
                    self.check_provided_field(pt);
                }
                self.project(&pl, pt, 0, fi)
            }
            ExprKind::Index(a, i) if self.is_range_index(e, *i) => {
                // `v[a..b]` as a place: the sub-slice's fat pointer
                let v = self.lower_slice_range(e, *a, *i, t);
                (self.val_to_place(v), t)
            }
            ExprKind::Index(a, i) => {
                let derefs = self.b.index_derefs(e.0);
                let (mut pl, mut pt) = self.lower_place(*a);
                for k in 0..derefs {
                    let (a2, b2) = self.deref_place(e.0, k, pl, pt);
                    pl = a2;
                    pt = b2;
                }
                if let Some((m, args)) = self.b.ov_index.get(&e.0).cloned() {
                    let args = self.mono_list(args);
                    // the call's location is the brackets (rustc, issue 114388: `x\n[0]` is at
                    // `[0]`): the `[` between the base and the index operand
                    let (alo, ilo) = (self.ast().expr(*a).hi, self.ast().expr(*i).lo);
                    let src = &self.u.prog.files[self.b.file as usize].src;
                    if let Some(k) = (alo..ilo).find(|&k| src.get(k as usize) == Some(&b'[')) {
                        self.pos = k;
                    }
                    return self.overloaded_index(m, args, pl, pt, *i, t, (ex.lo, ex.hi));
                }
                let it = self.body_ty(*i);
                let iv = self.lower_expr(*i);
                let iv = self.to_leaves(iv, it);
                let (elem, base, len) = match self.kind(pt) {
                    TyKind::Array(el, n) => {
                        let n = n.try_to_target_usize(&self.u.tcx.tys).expect("normalized array length");
                        let a = self.place_addr(&pl, pt);
                        let l = self.iconst(n as i64);
                        (el, a, l)
                    }
                    TyKind::Slice(el) => match &pl {
                        Place::Regs(r) if r.len() == 2 => (el, r[0], r[1]),
                        _ => {
                            self.err(ex.lo, "slice place without length".to_string());
                            return (Place::Regs(Vec::new()), t);
                        }
                    },
                    _ => {
                        self.err(ex.lo, "cannot index".to_string());
                        return (Place::Regs(Vec::new()), t);
                    }
                };
                self.bounds_check(iv[0], len);
                self.disjoint_index(ex.lo, ex.hi, iv[0]);
                let el = self.layout(elem);
                let stride = crate::layout::round_up(el.size, el.align);
                let s = self.iconst(stride as i64);
                let off = self.f.vreg(Cls::I);
                self.emit(Inst::IBin(IOp::Mul, U64, off, iv[0], s));
                let addr = self.f.vreg(Cls::I);
                self.emit(Inst::IBin(IOp::Add, U64, addr, base, off));
                (Place::Mem(addr, 0), elem)
            }
            _ => {
                // an rvalue: its own subexpressions are not part of the place (want_mut ends)
                let saved = std::mem::replace(&mut self.want_mut, false);
                let v = self.lower_expr(e);
                self.want_mut = saved;
                (self.temp_place(e, v, t), t)
            }
        }
    }

    /// An rvalue used as a place (borrowed, autoref'd, dereferenced, a field base): a temporary
    /// dropped at the end of the statement (or of the block when a `let` extends it).
    fn temp_place(&mut self, e: ExprId, v: Val, t: TyId) -> Place {
        let pl = self.val_to_place(v);
        if !self.needs_drop_glue(t) || self.unsized_ty(t) {
            return pl;
        }
        let a = self.place_addr(&pl, t);
        if self.extend_ops.contains(&e.0) {
            if !self.scope_temps.is_empty() {
                let d = self.scope_temps.len() - 1;
                self.push_scope_temp(d, (a, t, None));
            }
        } else {
            self.stmt_temps.push((a, t, None));
        }
        Place::Mem(a, 0)
    }

    /// built-in slicing `x[range]` (not an overloaded Index impl)
    /// Range identities follow rustc_ast_lowering/src/expr.rs:1459.
    fn is_range_index(&self, e: ExprId, i: ExprId) -> bool {
        if self.b.ov_index.contains_key(&e.0) {
            return false;
        }
        match self.kind(self.body_ty(i)) {
            TyKind::Adt(d, _, _) => matches!(self.u.tcx.lang_items.by_def.get(&d), Some(LangItem::Range | LangItem::RangeFrom | LangItem::RangeTo | LangItem::RangeFull | LangItem::RangeInclusiveStruct | LangItem::RangeToInclusive)),
            _ => false,
        }
    }

    /// Whether this lowering makes its verdict's run-time disjoint-index checks (D3).
    pub fn lowers_disjoint_checks(&self) -> bool { self.d3.is_some() }

    /// Relaxed Rust D3 (the verdict's run-time disjoint-index checks): the index `idx` of the index
    /// expression at `lo..hi` is kept when a later borrow is checked against it, and compared with
    /// every earlier index it must differ from; an overlap panics as `get_disjoint_mut(..).unwrap()`
    /// does (the checker proved the earlier expression runs first on every path to this one).
    fn disjoint_index(&mut self, lo: u32, hi: u32, idx: VReg) {
        let Some(checks) = self.d3.clone() else { return };
        let file = self.b.file;
        let at = |s: &relax_types::mir::Span| s.file == file && s.lo == lo && s.hi == hi;
        let slot_of = |this: &mut Self, s: &relax_types::mir::Span| -> u32 {
            *this.d3_slots.entry((s.file, s.lo, s.hi)).or_insert_with(|| this.f.slot(8, 8))
        };
        for (first, second) in checks.iter() {
            if at(second) {
                let s = slot_of(self, first);
                let a = self.f.vreg(Cls::I);
                self.emit(Inst::SlotAddr(a, s));
                let v = self.f.vreg(Cls::I);
                self.emit(Inst::Load(Mem::Int(8, false), v, a, 0));
                let c = self.f.vreg(Cls::I);
                self.emit(Inst::ICmp(Cond::Eq, false, c, idx, v));
                let ok = self.f.block();
                let bad = self.f.block();
                self.term(Term::Branch(c, bad, ok));
                self.switch(bad);
                let site = self.u.site_sym(self.b.file, lo, "called `Result::unwrap()` on an `Err` value: OverlappingIndices");
                let sv = self.sym_val(site);
                let rt = self.u.rs.panic_site;
                self.emit(Inst::Call(Callee::Host(rt), vec![sv], Vec::new()));
                self.term(Term::Unreachable);
                self.switch(ok);
            }
        }
        if checks.iter().any(|(first, _)| at(first)) {
            let s = slot_of(self, &relax_types::mir::Span { file, lo, hi });
            let a = self.f.vreg(Cls::I);
            self.emit(Inst::SlotAddr(a, s));
            self.emit(Inst::Store(Mem::Int(8, false), a, 0, idx));
        }
    }

    fn bounds_check(&mut self, idx: VReg, len: VReg) {
        let c = self.f.vreg(Cls::I);
        self.emit(Inst::ICmp(Cond::Lt, false, c, idx, len));
        let ok = self.f.block();
        let bad = self.f.block();
        self.term(Term::Branch(c, ok, bad));
        self.switch(bad);
        let site = self.u.site_sym(self.b.file, self.pos, "index out of bounds");
        let s = self.sym_val(site);
        let rt = self.u.rs.panic_bounds;
        self.emit(Inst::Call(Callee::Host(rt), vec![idx, len, s], Vec::new()));
        self.term(Term::Unreachable);
        self.switch(ok);
    }

    fn lower_slice_range(&mut self, e: ExprId, a: ExprId, i: ExprId, t: TyId) -> Val {
        // base[lo..hi] -> fat pointer
        let at = self.body_ty(a);
        let (mut pl, mut pt) = self.lower_place(a);
        // autoderef recorded by type checking (Vec<T> -> [T], String -> str, &&[T] ..)
        let derefs = self.b.index_derefs(e.0);
        for k in 0..derefs {
            let (a2, b2) = self.deref_place(e.0, k, pl, pt);
            pl = a2;
            pt = b2;
        }
        while let TyKind::Ref(_, inner, _) = self.kind(pt) {
            let p = self.read_place(&pl, pt);
            let p = self.to_leaves(p, pt);
            pl = if self.unsized_ty(inner) { Place::Regs(p) } else { Place::Mem(p[0], 0) };
            pt = inner;
        }
        let _ = at;
        let (elem_size, base, len) = match self.kind(pt) {
            TyKind::Array(el, n) => {
                let n = n.try_to_target_usize(&self.u.tcx.tys).expect("normalized array length");
                let l = self.layout(el);
                let a = self.place_addr(&pl, pt);
                (crate::layout::round_up(l.size, l.align), a, self.iconst(n as i64))
            }
            TyKind::Slice(el) => {
                let l = self.layout(el);
                match &pl {
                    Place::Regs(r) => (crate::layout::round_up(l.size, l.align), r[0], r[1]),
                    _ => return self.unit(),
                }
            }
            TyKind::Str => match &pl {
                Place::Regs(r) => (1, r[0], r[1]),
                _ => return self.unit(),
            },
            _ => return self.unit(),
        };
        let it = self.body_ty(i);
        let rv = self.lower_expr(i);
        let rv = self.to_leaves(rv, it);
        let item = match self.kind(it) {
            TyKind::Adt(d, _, _) => self.u.tcx.lang_items.by_def.get(&d).copied(),
            _ => None,
        };
        let (lo, hi) = match item {
            Some(LangItem::Range) => (rv[0], rv[1]),
            Some(LangItem::RangeFrom) => (rv[0], len),
            Some(LangItem::RangeTo) => (self.iconst(0), rv[0]),
            Some(LangItem::RangeInclusiveStruct) => {
                let one = self.iconst(1);
                let h = self.f.vreg(Cls::I);
                self.emit(Inst::IBin(IOp::Add, U64, h, rv[1], one));
                (rv[0], h)
            }
            Some(LangItem::RangeToInclusive) => {
                let one = self.iconst(1);
                let h = self.f.vreg(Cls::I);
                self.emit(Inst::IBin(IOp::Add, U64, h, rv[0], one));
                (self.iconst(0), h)
            }
            _ => (self.iconst(0), len),
        };
        // lo <= hi <= len
        let c1 = self.f.vreg(Cls::I);
        self.emit(Inst::ICmp(Cond::Le, false, c1, hi, len));
        let c2 = self.f.vreg(Cls::I);
        self.emit(Inst::ICmp(Cond::Le, false, c2, lo, hi));
        let c = self.f.vreg(Cls::I);
        self.emit(Inst::IBin(IOp::And, U64, c, c1, c2));
        let ok = self.f.block();
        let bad = self.f.block();
        self.term(Term::Branch(c, ok, bad));
        self.switch(bad);
        let site = self.u.site_sym(self.b.file, self.pos, "slice index out of range");
        let s = self.sym_val(site);
        let rt = self.u.rs.panic_bounds;
        self.emit(Inst::Call(Callee::Host(rt), vec![hi, len, s], Vec::new()));
        self.term(Term::Unreachable);
        self.switch(ok);
        let es = self.iconst(elem_size as i64);
        let off = self.f.vreg(Cls::I);
        self.emit(Inst::IBin(IOp::Mul, U64, off, lo, es));
        let p = self.f.vreg(Cls::I);
        self.emit(Inst::IBin(IOp::Add, U64, p, base, off));
        let n = self.f.vreg(Cls::I);
        self.emit(Inst::IBin(IOp::Sub, U64, n, hi, lo));
        let _ = t;
        Val::L(vec![p, n])
    }

    // ------------------------------------------------------------ operators

    /// `<`/`<=`/`>`/`>=` with no operator resolution recorded on a type that is not built in:
    /// a PartialOrd method call (rustc: comparison operators on non-builtin types are
    /// `PartialOrd::lt/le/gt/ge(&a, &b)`). Tuples and arrays of primitives compare inline.
    fn trait_ordering(&self, op: BinOp, t: TyId) -> bool {
        let prim = |k: &TyKind| matches!(k, TyKind::Int(_) | TyKind::Float(_) | TyKind::Bool | TyKind::Char);
        matches!(op, BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge)
            && !matches!(self.kind(t), TyKind::Int(_) | TyKind::Float(_) | TyKind::Bool | TyKind::Char | TyKind::Ptr(..) | TyKind::FnPtr(..))
            && !match self.kind(t) {
                TyKind::Tuple(v) => self.u.tcx.tys.list(v).iter().all(|e| prim(&self.kind(*e))),
                TyKind::Array(e, n) => prim(&self.kind(e)) && n.try_to_target_usize(&self.u.tcx.tys).is_some_and(|n| n <= 64),
                _ => false,
            }
    }

    /// Comparison operators borrow both operands, including unsized places.
    fn lower_cmp_call(&mut self, a: ExprId, b: ExprId, method: DefId, args: &[TyId], t: TyId) -> Val {
        let at = self.body_ty(a);
        let bt = self.body_ty(b);
        let (pa, ta) = self.lower_place(a);
        let ra = self.ref_to_place(&pa, ta);
        let (pb, tb) = self.lower_place(b);
        let rb = self.ref_to_place(&pb, tb);
        let instance = self.resolve_instance(method, args);
        let rta = self.u.tcx.tys.intern(TyKind::Ref(false, at, crate::types::RegionId::ERASED));
        let rtb = self.u.tcx.tys.intern(TyKind::Ref(false, bt, crate::types::RegionId::ERASED));
        self.emit_instance(instance, vec![(Val::L(ra), rta), (Val::L(rb), rtb)], t)
    }

    fn lower_binary(&mut self, op: BinOp, a: ExprId, b: ExprId, t: TyId) -> Val {
        match op {
            BinOp::And | BinOp::Or => {
                let res = self.f.vreg(Cls::I);
                let rhs = self.f.block();
                let short = self.f.block();
                let join = self.f.block();
                // both operands of a lazy boolean operator are terminating scopes (rustc
                // region.rs resolve_expr): their temporaries drop right after the operand, so a
                // skipped right-hand side never drops temporaries it did not create
                let at = self.body_ty(a);
                let mark = self.stmt_temps.len();
                let av = self.lower_expr(a);
                let av = self.to_leaves(av, at);
                self.drop_stmt_temps_from(mark);
                // a diverging operand (`return false && ..`) has no value: what follows is dead
                if av.is_empty() {
                    self.term(Term::Unreachable);
                } else if op == BinOp::And {
                    self.term(Term::Branch(av[0], rhs, short));
                } else {
                    self.term(Term::Branch(av[0], short, rhs));
                }
                self.switch(short);
                self.emit(Inst::Iconst(res, if op == BinOp::And { 0 } else { 1 }));
                self.goto(join);
                self.switch(rhs);
                let bt = self.body_ty(b);
                let mark = self.stmt_temps.len();
                let bv = self.lower_expr(b);
                let bv = self.to_leaves(bv, bt);
                self.drop_stmt_temps_from(mark);
                if bv.is_empty() {
                    // `true || return false`: the right operand diverges
                    self.term(Term::Unreachable);
                } else {
                    self.emit(Inst::Mov(res, bv[0]));
                    self.goto(join);
                }
                self.switch(join);
                Val::L(vec![res])
            }
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                let at = self.body_ty(a);
                if self.trait_ordering(op, at) {
                    // A concrete non-primitive operand still needs PartialOrd when no
                    // operator resolution was recorded on the typed body.
                    let name = match op {
                        BinOp::Lt => "lt",
                        BinOp::Le => "le",
                        BinOp::Gt => "gt",
                        _ => "ge",
                    };
                    let method = self.u.tcx.lang_items.get(LangItem::PartialOrd).and_then(|td| {
                        self.u.prog.traits[self.u.prog.def(td).sub as usize].iter().copied().find(|it| self.u.prog.name(*it) == name)
                    });
                    let Some(method) = method else {
                        self.err(self.pos, format!("missing PartialOrd::{}", name));
                        return self.dummy_val(t);
                    };
                    return self.lower_cmp_call(a, b, method, &[at, self.body_ty(b)], t);
                }
                let av = self.lower_expr(a);
                // (`fn_ptr == fn_item`: the rhs reified to the lhs pointer type)
                let bv = self.lower_expr_coerced(b, at);
                // `&a < &b` on primitives compares the values
                let (av, bv, at) = match self.kind(at) {
                    TyKind::Ref(_, inner, _) if matches!(self.kind_ref(inner), TyKind::Int(_) | TyKind::Float(_) | TyKind::Bool | TyKind::Char | TyKind::Ref(..) | TyKind::FnPtr(..)) => {
                        let bt = self.body_ty(b);
                        let (x, xt) = self.prim_leaves(av, at);
                        let (y, _) = self.prim_leaves(bv, bt);
                        if matches!(self.kind_ref(xt), TyKind::Ref(..)) {
                            (Val::L(x), Val::L(y), at)
                        } else {
                            (Val::L(x), Val::L(y), xt)
                        }
                    }
                    _ => (av, bv, at),
                };
                let cond = match op {
                    BinOp::Eq => Cond::Eq,
                    BinOp::Ne => Cond::Ne,
                    BinOp::Lt => Cond::Lt,
                    BinOp::Le => Cond::Le,
                    BinOp::Gt => Cond::Gt,
                    _ => Cond::Ge,
                };
                match self.kind(at) {
                    TyKind::Float(_) => {
                        let f64_ = self.is_f64(at);
                        let av = self.to_leaves(av, at);
                        let bv = self.to_leaves(bv, at);
                        let d = self.f.vreg(Cls::I);
                        self.emit(Inst::FCmp(cond, f64_, d, av[0], bv[0]));
                        Val::L(vec![d])
                    }
                    // a fat raw pointer (`*const [T]`, `*const dyn Tr`) compares (address, metadata)
                    // like rustc (slice length / vtable included; ordering lexicographic)
                    TyKind::Ptr(_, inner) if self.unsized_ty(inner) => {
                        let av = self.to_leaves(av, at);
                        let bv = self.to_leaves(bv, at);
                        let res = self.f.vreg(Cls::I);
                        let join = self.f.block();
                        let eq0 = self.f.vreg(Cls::I);
                        self.emit(Inst::ICmp(Cond::Eq, false, eq0, av[0], bv[0]));
                        let same = self.f.block();
                        let differ = self.f.block();
                        self.term(Term::Branch(eq0, same, differ));
                        self.switch(differ);
                        let c = match cond {
                            Cond::Le => Cond::Lt,
                            Cond::Ge => Cond::Gt,
                            c => c,
                        };
                        self.emit(Inst::ICmp(c, false, res, av[0], bv[0]));
                        self.goto(join);
                        self.switch(same);
                        let m1 = av.get(1).copied().unwrap_or(av[0]);
                        let m2 = bv.get(1).copied().unwrap_or(bv[0]);
                        self.emit(Inst::ICmp(cond, false, res, m1, m2));
                        self.goto(join);
                        self.switch(join);
                        Val::L(vec![res])
                    }
                    TyKind::Int(_) | TyKind::Bool | TyKind::Char | TyKind::Ptr(..) | TyKind::FnPtr(..) => {
                        let signed = self.int_ty(at).signed && matches!(self.kind_ref(at), TyKind::Int(_));
                        let av = self.to_leaves(av, at);
                        let bv = self.to_leaves(bv, at);
                        let d = self.f.vreg(Cls::I);
                        self.emit(Inst::ICmp(cond, signed, d, av[0], bv[0]));
                        Val::L(vec![d])
                    }
                    _ => {
                        if cond == Cond::Eq || cond == Cond::Ne {
                            let c = self.eq_vals(av, bv, at);
                            if cond == Cond::Ne {
                                let d = self.f.vreg(Cls::I);
                                self.emit(Inst::ICmpI(Cond::Eq, false, d, c, 0));
                                return Val::L(vec![d]);
                            }
                            return Val::L(vec![c]);
                        }
                        // tuples / arrays of primitives (no impl selected): lexicographic, as
                        // core's tuple and array PartialOrd impls (`partial_cmp` per element:
                        // the first unequal pair decides, NaN pairs compare false)
                        let pa = self.val_to_place(av);
                        let pb = self.val_to_place(bv);
                        match self.lex_cmp(&pa, &pb, at, cond) {
                            Some(r) => Val::L(vec![r]),
                            None => {
                                self.err(self.pos, "ordering comparison of non-primitive types is not supported yet".to_string());
                                Val::L(vec![self.iconst(0)])
                            }
                        }
                    }
                }
            }
            _ => {
                let at = self.body_ty(a);
                let bt = self.body_ty(b);
                let av = self.lower_expr(a);
                let (av, _) = self.prim_leaves(av, at);
                let bv = self.lower_expr(b);
                let (bv, bt) = self.prim_leaves(bv, bt);
                if av.is_empty() || bv.is_empty() {
                    let (x, y) = (crate::typeck::ty_to_string(false, &self.u.prog, &self.u.tcx, at), crate::typeck::ty_to_string(false, &self.u.prog, &self.u.tcx, bt));
                    self.err(self.pos, format!("internal: arithmetic on `{}` and `{}` without values -- makepad run bug, report it", x, y));
                    return self.dummy_val(t);
                }
                let r = self.arith(op, t, av[0], bv[0], bt);
                Val::L(vec![r])
            }
        }
    }

    /// The address of fn item `d` as a value (fn pointer, `as` cast): a foreign fn is the
    /// symbol the loader resolves -- the address a direct call uses -- never a JIT slot.
    fn fn_item_addr(&mut self, d: DefId, args: Vec<TyId>) -> VReg {
        let r = self.f.vreg(Cls::I);
        if self.u.prog.def(d).kind == DefKind::ForeignFn {
            if let Some(a) = self.u.foreign_sym(d) {
                self.emit(Inst::Addr(r, a));
                return r;
            }
        }
        let instance = Instance::resolve_for_fn_ptr(&self.u.tcx, &self.u.prog, d, &args).expect("fn pointer instance");
        let id = self.u.instance_fn_id(instance);
        self.emit(Inst::FnAddr(r, id));
        r
    }

    // ------------------------------------------------------------ 128-bit integers (lo, hi)

    fn wide_signed(&self, t: TyId) -> bool {
        matches!(self.u.tcx.tys.kind(t), TyKind::Int(Prim::I128))
    }

    fn ibin(&mut self, op: IOp, signed: bool, a: VReg, b: VReg) -> VReg {
        let d = self.f.vreg(Cls::I);
        self.emit(Inst::IBin(op, IntTy { bits: 64, signed }, d, a, b));
        d
    }

    fn and_k(&mut self, a: VReg, k: i64) -> VReg {
        let kk = self.iconst(k);
        self.ibin(IOp::And, false, a, kk)
    }

    /// logical shift right by a constant
    fn shr_k(&mut self, a: VReg, k: i64) -> VReg {
        let kk = self.iconst(k);
        self.ibin(IOp::Shr, false, a, kk)
    }

    fn popcount64(&mut self, x: VReg) -> VReg {
        let d = self.f.vreg(Cls::I);
        self.emit(Inst::IBit(relax_backend::rir::BitOp::Popcnt, d, x));
        d
    }

    /// leading zeros of a 64-bit value (64 for 0)
    fn clz64(&mut self, x: VReg) -> VReg {
        let d = self.f.vreg(Cls::I);
        self.emit(Inst::IBit(relax_backend::rir::BitOp::Clz, d, x));
        d
    }

    /// trailing zeros of a 64-bit value (64 for 0)
    fn ctz64(&mut self, x: VReg) -> VReg {
        let d = self.f.vreg(Cls::I);
        self.emit(Inst::IBit(relax_backend::rir::BitOp::Ctz, d, x));
        d
    }

    fn rev64(&mut self, x: VReg) -> VReg {
        let d = self.f.vreg(Cls::I);
        self.emit(Inst::IBit(relax_backend::rir::BitOp::Rev, d, x));
        d
    }

    fn bswap64(&mut self, x: VReg) -> VReg {
        let d = self.f.vreg(Cls::I);
        self.emit(Inst::IBit(relax_backend::rir::BitOp::Bswap, d, x));
        d
    }

    fn icmp(&mut self, c: Cond, signed: bool, a: VReg, b: VReg) -> VReg {
        let d = self.f.vreg(Cls::I);
        self.emit(Inst::ICmp(c, signed, d, a, b));
        d
    }

    /// (lo, hi) of an integer value of type `t` (narrower ints are extended).
    fn wide_parts(&mut self, v: Val, t: TyId) -> (VReg, VReg) {
        let (r, t) = self.prim_leaves(v, t);
        if self.is_wide(t) {
            return (r[0], r.get(1).copied().unwrap_or(r[0]));
        }
        let it = self.int_ty(t);
        let d = self.f.vreg(Cls::I);
        self.emit(Inst::Conv(Conv::IntToInt(IntTy { bits: 64, signed: it.signed }), d, r[0]));
        let hi = if it.signed {
            let k = self.iconst(63);
            self.ibin(IOp::Shr, true, d, k)
        } else {
            self.iconst(0)
        };
        (d, hi)
    }

    /// 64 x 64 -> high 64 bits of the unsigned product (through 32-bit halves).
    fn mulhi_u64(&mut self, x: VReg, y: VReg) -> VReg {
        let m32 = self.iconst(0xffff_ffff);
        let s32 = self.iconst(32);
        let x0 = self.ibin(IOp::And, false, x, m32);
        let x1 = self.ibin(IOp::Shr, false, x, s32);
        let y0 = self.ibin(IOp::And, false, y, m32);
        let y1 = self.ibin(IOp::Shr, false, y, s32);
        let p00 = self.ibin(IOp::Mul, false, x0, y0);
        let p01 = self.ibin(IOp::Mul, false, x0, y1);
        let p10 = self.ibin(IOp::Mul, false, x1, y0);
        let p11 = self.ibin(IOp::Mul, false, x1, y1);
        let p00h = self.ibin(IOp::Shr, false, p00, s32);
        let p01l = self.ibin(IOp::And, false, p01, m32);
        let p10l = self.ibin(IOp::And, false, p10, m32);
        let mid0 = self.ibin(IOp::Add, false, p00h, p01l);
        let mid = self.ibin(IOp::Add, false, mid0, p10l);
        let p01h = self.ibin(IOp::Shr, false, p01, s32);
        let p10h = self.ibin(IOp::Shr, false, p10, s32);
        let midh = self.ibin(IOp::Shr, false, mid, s32);
        let h0 = self.ibin(IOp::Add, false, p11, p01h);
        let h1 = self.ibin(IOp::Add, false, h0, p10h);
        self.ibin(IOp::Add, false, h1, midh)
    }

    // N:rustc_mir_build/src/builder/expr/as_rvalue.rs:457-541; round-30 RIR design.
    fn checked_wide_arith(&mut self, op: BinOp, ty: TyId, a: (VReg, VReg), b: (VReg, VReg)) -> Option<(VReg, VReg)> {
        let signed = self.wide_signed(ty);
        if self.overflow_checks && matches!(op, BinOp::Shl | BinOp::Shr) {
            self.shift_check(op, 128, (b.0, Some(b.1)));
        }
        let result = self.wide_arith(op, signed, a, b)?;
        if self.overflow_checks && matches!(op, BinOp::Add | BinOp::Sub | BinOp::Mul) {
            let overflow = if op == BinOp::Mul {
                self.wide_mul_overflow(signed, a, b, result)
            } else if signed {
                self.signed_overflow(op, a.1, b.1, result.1)
            } else if op == BinOp::Add {
                self.wide_cmp(BinOp::Lt, false, result, a)
            } else {
                self.wide_cmp(BinOp::Lt, false, a, b)
            };
            self.overflow_assert(overflow, arithmetic_overflow_message(op));
        }
        Some(result)
    }

    fn wide_mul_overflow(&mut self, signed: bool, a: (VReg, VReg), b: (VReg, VReg), result: (VReg, VReg)) -> VReg {
        // Widen the product to four u64 limbs, then compare its upper half with
        // the result's sign extension (zero extension for unsigned multiplication).
        let p00h = self.ibin(IOp::MulHi, false, a.0, b.0);
        let p01l = self.ibin(IOp::Mul, false, a.0, b.1);
        let p01h = self.ibin(IOp::MulHi, false, a.0, b.1);
        let p10l = self.ibin(IOp::Mul, false, a.1, b.0);
        let p10h = self.ibin(IOp::MulHi, false, a.1, b.0);
        let p11l = self.ibin(IOp::Mul, false, a.1, b.1);
        let p11h = self.ibin(IOp::MulHi, false, a.1, b.1);
        let mid = self.ibin(IOp::Add, false, p00h, p01l);
        let c1 = self.icmp(Cond::Lt, false, mid, p00h);
        let mid2 = self.ibin(IOp::Add, false, mid, p10l);
        let c2 = self.icmp(Cond::Lt, false, mid2, mid);
        let zero = self.iconst(0);
        let mut high = (p11l, p11h);
        for limb in [p01h, p10h, c1, c2] {
            high = self.wide_arith(BinOp::Add, false, high, (limb, zero)).unwrap();
        }
        let expected = if signed {
            let shift = self.iconst(63);
            let sa = self.ibin(IOp::Shr, true, a.1, shift);
            let sb = self.ibin(IOp::Shr, true, b.1, shift);
            let ac = (self.ibin(IOp::And, false, a.0, sb), self.ibin(IOp::And, false, a.1, sb));
            let bc = (self.ibin(IOp::And, false, b.0, sa), self.ibin(IOp::And, false, b.1, sa));
            high = self.wide_arith(BinOp::Sub, false, high, ac).unwrap();
            high = self.wide_arith(BinOp::Sub, false, high, bc).unwrap();
            self.ibin(IOp::Shr, true, result.1, shift)
        } else { zero };
        self.wide_cmp(BinOp::Ne, false, high, (expected, expected))
    }

    /// Arithmetic / bit ops on (lo, hi) pairs.
    fn wide_arith(&mut self, op: BinOp, signed: bool, a: (VReg, VReg), b: (VReg, VReg)) -> Option<(VReg, VReg)> {
        if matches!(op, BinOp::Div | BinOp::Rem) {
            return Some(self.wide_divrem(op == BinOp::Rem, signed, a, b));
        }
        Some(match op {
            BinOp::Add => {
                let lo = self.ibin(IOp::Add, false, a.0, b.0);
                let c = self.icmp(Cond::Lt, false, lo, a.0);
                let h = self.ibin(IOp::Add, false, a.1, b.1);
                (lo, self.ibin(IOp::Add, false, h, c))
            }
            BinOp::Sub => {
                let lo = self.ibin(IOp::Sub, false, a.0, b.0);
                let c = self.icmp(Cond::Lt, false, a.0, b.0);
                let h = self.ibin(IOp::Sub, false, a.1, b.1);
                (lo, self.ibin(IOp::Sub, false, h, c))
            }
            BinOp::Mul => {
                let lo = self.ibin(IOp::Mul, false, a.0, b.0);
                let h0 = self.mulhi_u64(a.0, b.0);
                let c1 = self.ibin(IOp::Mul, false, a.0, b.1);
                let c2 = self.ibin(IOp::Mul, false, a.1, b.0);
                let h1 = self.ibin(IOp::Add, false, h0, c1);
                (lo, self.ibin(IOp::Add, false, h1, c2))
            }
            BinOp::BitAnd => (self.ibin(IOp::And, false, a.0, b.0), self.ibin(IOp::And, false, a.1, b.1)),
            BinOp::BitOr => (self.ibin(IOp::Or, false, a.0, b.0), self.ibin(IOp::Or, false, a.1, b.1)),
            BinOp::BitXor => (self.ibin(IOp::Xor, false, a.0, b.0), self.ibin(IOp::Xor, false, a.1, b.1)),
            BinOp::Shl | BinOp::Shr => {
                // n = b.lo & 127: n >= 64, n == 0, else the two words exchange bits
                let m = self.iconst(127);
                let n = self.ibin(IOp::And, false, b.0, m);
                let lo = self.f.vreg(Cls::I);
                let hi = self.f.vreg(Cls::I);
                let k64 = self.iconst(64);
                let big = self.icmp(Cond::Ge, false, n, k64);
                let (bb, small, join) = (self.f.block(), self.f.block(), self.f.block());
                self.term(Term::Branch(big, bb, small));
                self.switch(bb);
                let n2 = self.ibin(IOp::Sub, false, n, k64);
                if op == BinOp::Shl {
                    let h = self.ibin(IOp::Shl, false, a.0, n2);
                    self.emit(Inst::Mov(hi, h));
                    self.emit(Inst::Iconst(lo, 0));
                } else {
                    let l = self.ibin(IOp::Shr, signed, a.1, n2);
                    self.emit(Inst::Mov(lo, l));
                    if signed {
                        let k63 = self.iconst(63);
                        let s = self.ibin(IOp::Shr, true, a.1, k63);
                        self.emit(Inst::Mov(hi, s));
                    } else {
                        self.emit(Inst::Iconst(hi, 0));
                    }
                }
                self.goto(join);
                self.switch(small);
                let zero = self.iconst(0);
                let is0 = self.icmp(Cond::Eq, false, n, zero);
                let (z, nz) = (self.f.block(), self.f.block());
                self.term(Term::Branch(is0, z, nz));
                self.switch(z);
                self.emit(Inst::Mov(lo, a.0));
                self.emit(Inst::Mov(hi, a.1));
                self.goto(join);
                self.switch(nz);
                let rn = self.ibin(IOp::Sub, false, k64, n);
                if op == BinOp::Shl {
                    let l = self.ibin(IOp::Shl, false, a.0, n);
                    let h0 = self.ibin(IOp::Shl, false, a.1, n);
                    let carry = self.ibin(IOp::Shr, false, a.0, rn);
                    let h = self.ibin(IOp::Or, false, h0, carry);
                    self.emit(Inst::Mov(lo, l));
                    self.emit(Inst::Mov(hi, h));
                } else {
                    let l0 = self.ibin(IOp::Shr, false, a.0, n);
                    let carry = self.ibin(IOp::Shl, false, a.1, rn);
                    let l = self.ibin(IOp::Or, false, l0, carry);
                    let h = self.ibin(IOp::Shr, signed, a.1, n);
                    self.emit(Inst::Mov(lo, l));
                    self.emit(Inst::Mov(hi, h));
                }
                self.goto(join);
                self.switch(join);
                (lo, hi)
            }
            _ => return None,
        })
    }

    /// 128-bit `/` and `%`: a zero divisor panics, and for signed MIN / -1 (rustc's
    /// "attempt to divide/calculate the remainder with overflow" checks, emitted whatever the
    /// overflow-checks setting: Reference expressions/operator-expr.md, division table note †
    /// and [expr.operator.int-overflow.div]);
    /// the quotient/remainder itself comes from a runtime helper (two u64 halves in, two out).
    fn wide_divrem(&mut self, rem: bool, signed: bool, a: (VReg, VReg), b: (VReg, VReg)) -> (VReg, VReg) {
        let (zmsg, omsg) = if rem {
            ("attempt to calculate the remainder with a divisor of zero", "attempt to calculate the remainder with overflow")
        } else {
            ("attempt to divide by zero", "attempt to divide with overflow")
        };
        let z = self.iconst(0);
        let any = self.ibin(IOp::Or, false, b.0, b.1);
        let nz = self.f.vreg(Cls::I);
        self.emit(Inst::ICmp(Cond::Ne, false, nz, any, z));
        let ok = self.f.block();
        let bad = self.f.block();
        self.term(Term::Branch(nz, ok, bad));
        self.switch(bad);
        let site = self.u.site_sym(self.b.file, self.pos, zmsg);
        let sv = self.sym_val(site);
        let rt = self.u.rs.panic_site;
        self.emit(Inst::Call(Callee::Host(rt), vec![sv], Vec::new()));
        self.term(Term::Unreachable);
        self.switch(ok);
        if signed {
            // MIN = (lo 0, hi 1 << 63), -1 = (all ones, all ones)
            let m1 = self.iconst(-1);
            let hmin = self.iconst(i64::MIN);
            let c1 = self.icmp(Cond::Eq, false, b.0, m1);
            let c2 = self.icmp(Cond::Eq, false, b.1, m1);
            let c3 = self.icmp(Cond::Eq, false, a.0, z);
            let c4 = self.icmp(Cond::Eq, false, a.1, hmin);
            let x = self.ibin(IOp::And, false, c1, c2);
            let y = self.ibin(IOp::And, false, c3, c4);
            let both = self.ibin(IOp::And, false, x, y);
            let ok2 = self.f.block();
            let bad2 = self.f.block();
            self.term(Term::Branch(both, bad2, ok2));
            self.switch(bad2);
            let site = self.u.site_sym(self.b.file, self.pos, omsg);
            let sv = self.sym_val(site);
            self.emit(Inst::Call(Callee::Host(rt), vec![sv], Vec::new()));
            self.term(Term::Unreachable);
            self.switch(ok2);
        }
        let f: rt_fn_ty!(fn(u64, u64, u64, u64, u64) -> WideRet) = rt_divrem128;
        let mode = self.iconst((rem as i64) | ((signed as i64) << 1));
        let (lo, hi) = (self.f.vreg(Cls::I), self.f.vreg(Cls::I));
        let fs = self.u.syms.intern(crate::syms::SymKind::Rt("divrem128"), f as usize as u64);
        self.emit(Inst::Call(Callee::Host(fs), vec![mode, a.0, a.1, b.0, b.1], vec![lo, hi]));
        (lo, hi)
    }

    fn wide_cmp(&mut self, op: BinOp, signed: bool, a: (VReg, VReg), b: (VReg, VReg)) -> VReg {
        let one = self.iconst(1);
        match op {
            BinOp::Eq | BinOp::Ne => {
                let e0 = self.icmp(Cond::Eq, false, a.0, b.0);
                let e1 = self.icmp(Cond::Eq, false, a.1, b.1);
                let e = self.ibin(IOp::And, false, e0, e1);
                if op == BinOp::Eq {
                    e
                } else {
                    self.ibin(IOp::Xor, false, e, one)
                }
            }
            _ => {
                // a < b <=> hi(a) < hi(b) || (hi equal && lo(a) <u lo(b)); others by swapping/negating
                let (x, y, neg) = match op {
                    BinOp::Lt => (a, b, false),
                    BinOp::Gt => (b, a, false),
                    BinOp::Ge => (a, b, true),
                    _ => (b, a, true), // Le = !(b < a)
                };
                let hlt = self.icmp(Cond::Lt, signed, x.1, y.1);
                let heq = self.icmp(Cond::Eq, false, x.1, y.1);
                let llt = self.icmp(Cond::Lt, false, x.0, y.0);
                let t0 = self.ibin(IOp::And, false, heq, llt);
                let lt = self.ibin(IOp::Or, false, hlt, t0);
                if neg {
                    self.ibin(IOp::Xor, false, lt, one)
                } else {
                    lt
                }
            }
        }
    }

    /// Expressions with a 128-bit operand or result.
    fn lower_wide(&mut self, e: ExprId, t: TyId) -> Val {
        let ast = self.ast();
        let ex = ast.expr(e);
        match &ex.kind {
            ExprKind::Binary(op, a, b) if matches!(op, BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge) => {
                let at = self.body_ty(*a);
                let signed = self.wide_signed(at) || (!self.is_wide(at) && self.int_ty(at).signed);
                let av = self.lower_expr(*a);
                let x = self.wide_parts(av, at);
                let bt = self.body_ty(*b);
                let bv = self.lower_expr(*b);
                let y = self.wide_parts(bv, bt);
                Val::L(vec![self.wide_cmp(*op, signed, x, y)])
            }
            ExprKind::Binary(op, a, b) => {
                let (at, bt) = (self.body_ty(*a), self.body_ty(*b));
                let av = self.lower_expr(*a);
                let x = self.wide_parts(av, at);
                let bv = self.lower_expr(*b);
                let y = self.wide_parts(bv, bt);
                if !self.is_wide(t) && matches!(op, BinOp::Shl | BinOp::Shr) {
                    if self.overflow_checks { self.shift_check(*op, self.int_ty(t).bits as u32, (y.0, Some(y.1))); }
                    return Val::L(vec![self.arith(*op, t, x.0, y.0, bt)]);
                }
                match self.checked_wide_arith(*op, t, x, y) {
                    Some((lo, hi)) => Val::L(vec![lo, hi]),
                    None => {
                        self.err(ex.lo, "128-bit division/remainder is not supported yet -- divide u64 halves".to_string());
                        self.dummy_val(t)
                    }
                }
            }
            ExprKind::AssignOp(op, a, b) => {
                // the value first, then the place (built-in compound assignment)
                let bt = self.body_ty(*b);
                let bv = self.lower_expr(*b);
                let y = self.wide_parts(bv, bt);
                self.want_mut = true;
                let (pl, pt) = self.lower_place(*a);
                self.want_mut = false;
                let cur = self.read_place(&pl, pt);
                let x = self.wide_parts(cur, pt);
                if !self.is_wide(pt) && matches!(op, BinOp::Shl | BinOp::Shr) {
                    if self.overflow_checks { self.shift_check(*op, self.int_ty(pt).bits as u32, (y.0, Some(y.1))); }
                    let value = self.arith(*op, pt, x.0, y.0, bt);
                    self.write_place(&pl, pt, Val::L(vec![value]));
                    return self.unit();
                }
                match self.checked_wide_arith(*op, pt, x, y) {
                    Some((lo, hi)) => self.write_place(&pl, pt, Val::L(vec![lo, hi])),
                    None => self.err(ex.lo, "128-bit division/remainder is not supported yet -- divide u64 halves".to_string()),
                }
                self.unit()
            }
            ExprKind::Unary(UnOp::Neg, x) => {
                let xt = self.body_ty(*x);
                let v = self.lower_expr(*x);
                let p = self.wide_parts(v, xt);
                let z = (self.iconst(0), self.iconst(0));
                if self.overflow_checks && self.wide_signed(t) && !self.negated_literal(*x) {
                    let min = (self.iconst(0), self.iconst(i64::MIN));
                    let overflow = self.wide_cmp(BinOp::Eq, false, p, min);
                    self.overflow_assert(overflow, "attempt to negate with overflow");
                }
                let (lo, hi) = self.wide_arith(BinOp::Sub, true, z, p).unwrap();
                Val::L(vec![lo, hi])
            }
            ExprKind::Unary(UnOp::Not, x) => {
                let xt = self.body_ty(*x);
                let v = self.lower_expr(*x);
                let p = self.wide_parts(v, xt);
                let m = self.iconst(-1);
                Val::L(vec![self.ibin(IOp::Xor, false, p.0, m), self.ibin(IOp::Xor, false, p.1, m)])
            }
            ExprKind::Cast(x, _) => {
                let xt = self.body_ty(*x);
                let v = self.lower_expr(*x);
                if matches!(self.kind(xt), TyKind::Adt(..)) { return self.cast(v, xt, t); }
                match (self.is_wide(xt), self.kind(t)) {
                    (true, TyKind::Int(_)) if !self.is_wide(t) => {
                        let p = self.wide_parts(v, xt);
                        let it = self.int_ty(t);
                        let d = self.f.vreg(Cls::I);
                        self.emit(Inst::Conv(Conv::IntToInt(it), d, p.0));
                        Val::L(vec![d])
                    }
                    (true, TyKind::Float(_)) => {
                        // Convert the complete integer with one rounding step. Converting
                        // signed halves separately cancels small negatives to zero (or a
                        // multiple of 2048); it also double-rounds large values and f32.
                        let p = self.wide_parts(v, xt);
                        let f64_ = self.is_f64(t);
                        let mode = self.iconst(self.wide_signed(xt) as i64);
                        let name = if f64_ { "wide_f64" } else { "wide_f32" };
                        let addr = wide_float_addrs()[f64_ as usize];
                        let fs = self.u.syms.intern(crate::syms::SymKind::Rt(name), addr);
                        let d = self.f.vreg(if f64_ { Cls::F64 } else { Cls::F32 });
                        self.emit(Inst::Call(Callee::Host(fs), vec![mode, p.0, p.1], vec![d]));
                        Val::L(vec![d])
                    }
                    (_, TyKind::Int(_)) if self.is_wide(t) && matches!(self.kind_ref(xt), TyKind::Int(_) | TyKind::Bool | TyKind::Char) => {
                        let (lo, hi) = self.wide_parts(v, xt);
                        Val::L(vec![lo, hi])
                    }
                    (true, TyKind::Int(_)) => v,
                    (false, TyKind::Int(_)) if matches!(self.kind_ref(xt), TyKind::Float(_)) => {
                        // float -> i128/u128, saturating with NaN -> 0 (Reference
                        // expressions/operator-expr.md numeric cast): split at 2^64 as
                        // hi = trunc(x / 2^64), lo = x - hi * 2^64 (borrowing one from hi when
                        // that is negative), each converted with the saturating 64-bit conversion
                        let signed = self.wide_signed(t);
                        let x = match self.kind(xt) {
                            TyKind::Float(Prim::F32) => {
                                let r = self.to_leaves(v, xt);
                                let d = self.f.vreg(Cls::F64);
                                self.emit(Inst::Conv(Conv::F32ToF64, d, r[0]));
                                d
                            }
                            _ => self.to_leaves(v, xt)[0],
                        };
                        let fk = |s: &mut Self, bits: f64| {
                            let k = s.f.vreg(Cls::F64);
                            s.emit(Inst::Fconst(k, bits.to_bits(), true));
                            k
                        };
                        // magnitude |x| (NaN stays NaN -> 0), clamped to what the type holds
                        // (2^128 - 1 / 2^127 - 1 / 2^127 for negatives), split exactly at 2^64:
                        // hi = trunc(m / 2^64) and lo = m - hi * 2^64 are exact in f64 for any m
                        let inv = fk(self, 1.0 / 18446744073709551616.0);
                        let two64 = fk(self, 18446744073709551616.0);
                        let ax = self.f.vreg(Cls::F64);
                        self.emit(Inst::FUnary(FUn::Abs, true, ax, x));
                        let neg = if signed {
                            let z = fk(self, 0.0);
                            let n = self.f.vreg(Cls::I);
                            self.emit(Inst::FCmp(Cond::Lt, true, n, x, z));
                            Some(n)
                        } else {
                            None
                        };
                        let m = match neg {
                            // unsigned: negatives saturate to 0
                            None => {
                                let z = fk(self, 0.0);
                                let c = self.f.vreg(Cls::F64);
                                self.emit(Inst::FBin(FOp::Max, true, c, x, z));
                                c
                            }
                            // signed: |x| up to 2^127 (the conversion below saturates hi at
                            // i64::MAX for positives; 2^127 negated is i128::MIN)
                            Some(_) => {
                                let lim = fk(self, 170141183460469231731687303715884105728.0);
                                let c = self.f.vreg(Cls::F64);
                                self.emit(Inst::FBin(FOp::Min, true, c, ax, lim));
                                c
                            }
                        };
                        let hf = self.f.vreg(Cls::F64);
                        self.emit(Inst::FBin(FOp::Mul, true, hf, m, inv));
                        let hi0 = self.f.vreg(Cls::I);
                        self.emit(Inst::Conv(Conv::FToInt(true, IntTy { bits: 64, signed: false }), hi0, hf));
                        let hif = self.f.vreg(Cls::F64);
                        self.emit(Inst::Conv(Conv::IntToF(IntTy { bits: 64, signed: false }, true), hif, hi0));
                        let mm = self.f.vreg(Cls::F64);
                        self.emit(Inst::FBin(FOp::Mul, true, mm, hif, two64));
                        let r = self.f.vreg(Cls::F64);
                        self.emit(Inst::FBin(FOp::Sub, true, r, m, mm));
                        let lo0 = self.f.vreg(Cls::I);
                        self.emit(Inst::Conv(Conv::FToInt(true, IntTy { bits: 64, signed: false }), lo0, r));
                        let (lo, hi) = match neg {
                            None => (lo0, hi0),
                            Some(n) => {
                                // positives above i128::MAX saturate: hi >= 2^63 -> MAX
                                let big = self.f.vreg(Cls::I);
                                let k63 = self.iconst(i64::MAX);
                                self.emit(Inst::ICmp(Cond::Gt, false, big, hi0, k63));
                                let posbig = {
                                    let one = self.iconst(1);
                                    let notneg = self.ibin(IOp::Xor, false, n, one);
                                    self.ibin(IOp::And, false, big, notneg)
                                };
                                // m = -(posbig) (all ones when saturating): hi = hi ^ (m & (hi ^ MAX)), lo |= m
                                let zero = self.iconst(0);
                                let msk = self.ibin(IOp::Sub, false, zero, posbig);
                                let hx = self.ibin(IOp::Xor, false, hi0, k63);
                                let hm = self.ibin(IOp::And, false, msk, hx);
                                let hi1 = self.ibin(IOp::Xor, false, hi0, hm);
                                let lo1 = self.ibin(IOp::Or, false, lo0, msk);
                                // negate when x < 0: (lo, hi) ^ s - s with s = -neg
                                let sgn = self.ibin(IOp::Sub, false, zero, n);
                                let lx = self.ibin(IOp::Xor, false, lo1, sgn);
                                let hx2 = self.ibin(IOp::Xor, false, hi1, sgn);
                                let (l2, h2) = self.wide_arith(BinOp::Sub, true, (lx, hx2), (sgn, sgn)).unwrap();
                                // NaN -> 0 (Min(NaN, lim) gave lim)
                                let ok = self.f.vreg(Cls::I);
                                self.emit(Inst::FCmp(Cond::Eq, true, ok, x, x));
                                let keep = self.ibin(IOp::Sub, false, zero, ok);
                                (self.ibin(IOp::And, false, l2, keep), self.ibin(IOp::And, false, h2, keep))
                            }
                        };
                        Val::L(vec![lo, hi])
                    }
                    _ => {
                        self.err(ex.lo, "this 128-bit conversion is not supported yet".to_string());
                        self.dummy_val(t)
                    }
                }
            }
            _ => {
                self.err(ex.lo, "this 128-bit operation is not supported yet".to_string());
                self.dummy_val(t)
            }
        }
    }

    /// Leaves of a primitive operand, loading through references to primitives
    /// (`&x + y`, `*a ^ &b`): returns the leaves and the primitive type.
    fn prim_leaves(&mut self, v: Val, t: TyId) -> (Vec<VReg>, TyId) {
        let mut r = self.to_leaves(v, t);
        let mut t = t;
        while let TyKind::Ref(_, inner, _) = self.kind(t) {
            if !matches!(self.kind_ref(inner), TyKind::Int(_) | TyKind::Float(_) | TyKind::Bool | TyKind::Char | TyKind::Ref(..) | TyKind::FnPtr(..)) {
                break;
            }
            let v = self.load_val(r[0], 0, inner);
            r = self.to_leaves(v, inner);
            t = inner;
        }
        (r, t)
    }

    /// Arithmetic on primitive operands of type `t` (rhs type `bt` matters for shifts).
    fn arith(&mut self, op: BinOp, t: TyId, a: VReg, b: VReg, bt: TyId) -> VReg {
        match self.kind(t) {
            TyKind::Float(_) => {
                let f64_ = self.is_f64(t);
                let d = self.f.vreg(if f64_ { Cls::F64 } else { Cls::F32 });
                let fop = match op {
                    BinOp::Add => FOp::Add,
                    BinOp::Sub => FOp::Sub,
                    BinOp::Mul => FOp::Mul,
                    BinOp::Div => FOp::Div,
                    BinOp::Rem => {
                        // a - trunc(a / b) * b is not exact; call the host fmod
                        let rt = if f64_ { self.u.rs.fmod } else { self.u.rs.fmodf };
                        self.emit(Inst::Call(Callee::Host(rt), vec![a, b], vec![d]));
                        return d;
                    }
                    _ => FOp::Add,
                };
                self.emit(Inst::FBin(fop, f64_, d, a, b));
                d
            }
            _ => {
                let it = self.int_ty(t);
                let d = self.f.vreg(Cls::I);
                let iop = match op {
                    BinOp::Add => IOp::Add,
                    BinOp::Sub => IOp::Sub,
                    BinOp::Mul => IOp::Mul,
                    BinOp::Div => IOp::Div,
                    BinOp::Rem => IOp::Rem,
                    BinOp::BitAnd => IOp::And,
                    BinOp::BitOr => IOp::Or,
                    BinOp::BitXor => IOp::Xor,
                    BinOp::Shl => IOp::Shl,
                    BinOp::Shr => IOp::Shr,
                    _ => IOp::Add,
                };
                if iop == IOp::Div || iop == IOp::Rem {
                    self.div_check(b, it, a, op == BinOp::Rem);
                }
                let b = if iop == IOp::Shl || iop == IOp::Shr {
                    if self.overflow_checks { self.shift_check(op, it.bits as u32, (b, None)); }
                    let _ = bt;
                    let m = self.iconst(it.bits as i64 - 1);
                    let mb = self.f.vreg(Cls::I);
                    self.emit(Inst::IBin(IOp::And, U64, mb, b, m));
                    mb
                } else {
                    b
                };
                self.emit(Inst::IBin(iop, it, d, a, b));
                if self.overflow_checks && matches!(op, BinOp::Add | BinOp::Sub | BinOp::Mul) {
                    let overflow = if op == BinOp::Mul {
                        if it.bits < 64 {
                            let full = self.ibin(IOp::Mul, it.signed, a, b);
                            self.icmp(Cond::Ne, false, full, d)
                        } else {
                            let high = self.ibin(IOp::MulHi, it.signed, a, b);
                            let expected = if it.signed {
                                let shift = self.iconst(63);
                                self.ibin(IOp::Shr, true, d, shift)
                            } else { self.iconst(0) };
                            self.icmp(Cond::Ne, false, high, expected)
                        }
                    } else if it.signed {
                        self.signed_overflow(op, a, b, d)
                    } else if op == BinOp::Add {
                        self.icmp(Cond::Lt, false, d, a)
                    } else {
                        self.icmp(Cond::Lt, false, a, b)
                    };
                    self.overflow_assert(overflow, arithmetic_overflow_message(op));
                }
                d
            }
        }
    }

    // N:rustc_mir_build/src/builder/expr/as_rvalue.rs:103-123,457-541.
    // Round-30 design: checked arithmetic's flag is computed with existing RIR ops.
    // N:rustc_mir_build/src/thir/cx/expr.rs:676-683 lowers -literal as a literal.
    fn negated_literal(&self, mut expr: ExprId) -> bool {
        loop {
            match self.ast().expr(expr).kind {
                ExprKind::Paren(inner) => expr = inner,
                ExprKind::Lit(LitKind::Int) => return true,
                _ => return false,
            }
        }
    }

    fn overflow_assert(&mut self, overflow: VReg, message: &str) {
        let bad = self.f.block();
        let ok = self.f.block();
        self.term(Term::Branch(overflow, bad, ok));
        self.switch(bad);
        self.panic_here(message);
        self.switch(ok);
    }

    fn signed_overflow(&mut self, op: BinOp, a: VReg, b: VReg, result: VReg) -> VReg {
        let lhs = self.ibin(IOp::Xor, false, a, result);
        let rhs = self.ibin(IOp::Xor, false, if op == BinOp::Add { b } else { a },
            if op == BinOp::Add { result } else { b });
        let signs = self.ibin(IOp::And, false, lhs, rhs);
        let zero = self.iconst(0);
        self.icmp(Cond::Lt, true, signs, zero)
    }

    fn shift_check(&mut self, op: BinOp, bits: u32, rhs: (VReg, Option<VReg>)) {
        let width = self.iconst(bits as i64);
        let mut overflow = self.icmp(Cond::Ge, false, rhs.0, width);
        if let Some(high) = rhs.1 {
            let zero = self.iconst(0);
            let high = self.icmp(Cond::Ne, false, high, zero);
            overflow = self.ibin(IOp::Or, false, overflow, high);
        }
        self.overflow_assert(overflow, arithmetic_overflow_message(op));
    }

    fn div_check(&mut self, b: VReg, it: IntTy, a: VReg, rem: bool) {
        let z = self.iconst(0);
        let c = self.f.vreg(Cls::I);
        self.emit(Inst::ICmp(Cond::Ne, false, c, b, z));
        let ok = self.f.block();
        let bad = self.f.block();
        self.term(Term::Branch(c, ok, bad));
        self.switch(bad);
        let site = self.u.site_sym(self.b.file, self.pos, if rem { "attempt to calculate the remainder with a divisor of zero" } else { "attempt to divide by zero" });
        let s = self.sym_val(site);
        let rt = self.u.rs.panic_site;
        self.emit(Inst::Call(Callee::Host(rt), vec![s], Vec::new()));
        self.term(Term::Unreachable);
        self.switch(ok);
        if it.signed {
            // MIN / -1 overflows
            let m1 = self.iconst(-1);
            let min = self.iconst(extend_const(1i64 << (it.bits as u32 - 1).min(63), it));
            let c1 = self.f.vreg(Cls::I);
            self.emit(Inst::ICmp(Cond::Eq, true, c1, b, m1));
            let c2 = self.f.vreg(Cls::I);
            self.emit(Inst::ICmp(Cond::Eq, true, c2, a, min));
            let both = self.f.vreg(Cls::I);
            self.emit(Inst::IBin(IOp::And, U64, both, c1, c2));
            let ok2 = self.f.block();
            let bad2 = self.f.block();
            self.term(Term::Branch(both, bad2, ok2));
            self.switch(bad2);
            let site = self.u.site_sym(self.b.file, self.pos, if rem { "attempt to calculate the remainder with overflow" } else { "attempt to divide with overflow" });
            let s = self.sym_val(site);
            let rt = self.u.rs.panic_site;
            self.emit(Inst::Call(Callee::Host(rt), vec![s], Vec::new()));
            self.term(Term::Unreachable);
            self.switch(ok2);
        }
    }

    fn cast(&mut self, v: Val, from: TyId, to: TyId) -> Val {
        let fk = self.kind(from);
        let tk = self.kind(to);
        // `f as fn(..)` / `f as usize` / `f as *mut ()` of a fn item, `|..| .. as fn(..)` of a
        // non-capturing closure (both zero-sized): the function's address
        let reify = match &fk {
            TyKind::FnDef(d, args, _) if matches!(tk, TyKind::FnPtr(..) | TyKind::Int(_) | TyKind::Ptr(..)) => Some(Ok((*d, args.clone()))),
            TyKind::Closure(..) if matches!(tk, TyKind::FnPtr(..)) => {
                let instance = Instance::resolve_closure(&self.u.tcx, &self.u.prog, from, relax_types::upvar::ClosureKind::FnOnce);
                Some(Err(self.u.instance_fn_id(instance)))
            },
            _ => None,
        };
        if let Some(fa) = reify {
            let r = match fa {
                Ok((d, args)) => self.fn_item_addr(d, self.u.tcx.tys.list(args).to_vec()),
                Err(id) => {
                    let r = self.f.vreg(Cls::I);
                    self.emit(Inst::FnAddr(r, id));
                    r
                }
            };
            if let TyKind::Int(_) = tk {
                let it = self.int_ty(to);
                let d = self.f.vreg(Cls::I);
                self.emit(Inst::Conv(Conv::IntToInt(it), d, r));
                return Val::L(vec![d]);
            }
            return Val::L(vec![r]);
        }
        if let TyKind::Adt(d, _, _) = fk {
            if self.u.tcx.adts.get(&d).is_some_and(|a| a.is_enum) {
                let place = self.val_to_place(v);
                let addr = self.place_addr(&place, from);
                let Some(mut words) = self.load_disc(addr, 0, from) else { return self.unit(); };
                let repr = self.u.tcx.adts[&d].repr_int.unwrap_or(Prim::Isize);
                let signed = crate::types::Types::int_info(repr).1;
                if self.is_wide(to) {
                    if words.len() == 1 {
                        let hi = if signed { { let shift = self.iconst(63); self.ibin(IOp::Shr, true, words[0], shift) } } else { self.iconst(0) };
                        words.push(hi);
                    }
                    return Val::L(words);
                }
                let result = self.f.vreg(Cls::I);
                self.emit(Inst::Conv(Conv::IntToInt(self.int_ty(to)), result, words[0]));
                return Val::L(vec![result]);
            }
        }
        let v = self.to_leaves(v, from);
        if v.is_empty() {
            return Val::L(v);
        }
        let x = v[0];
        match (&fk, &tk) {
            (TyKind::Float(_), TyKind::Float(_)) => {
                let ff = self.is_f64(from);
                let tf = self.is_f64(to);
                if ff == tf {
                    return Val::L(vec![x]);
                }
                let d = self.f.vreg(if tf { Cls::F64 } else { Cls::F32 });
                self.emit(Inst::Conv(if tf { Conv::F32ToF64 } else { Conv::F64ToF32 }, d, x));
                Val::L(vec![d])
            }
            (TyKind::Float(_), TyKind::Int(_)) => {
                let ff = self.is_f64(from);
                let it = self.int_ty(to);
                let d = self.f.vreg(Cls::I);
                self.emit(Inst::Conv(Conv::FToInt(ff, it), d, x));
                Val::L(vec![d])
            }
            (TyKind::Int(_), TyKind::Float(_)) | (TyKind::Bool, TyKind::Float(_)) => {
                let it = self.int_ty(from);
                let tf = self.is_f64(to);
                let d = self.f.vreg(if tf { Cls::F64 } else { Cls::F32 });
                self.emit(Inst::Conv(Conv::IntToF(it, tf), d, x));
                Val::L(vec![d])
            }
            (_, TyKind::Int(_)) | (_, TyKind::Char) => {
                // int/bool/char/pointer -> int: re-extend to the target type
                let it = self.int_ty(to);
                let d = self.f.vreg(Cls::I);
                self.emit(Inst::Conv(Conv::IntToInt(it), d, x));
                Val::L(vec![d])
            }
            (TyKind::Ref(..), TyKind::Ptr(..)) | (TyKind::Ptr(..), TyKind::Ptr(..)) => {
                // fat -> thin keeps the data pointer
                let n = self.layout(to).leaves.as_ref().map_or(1, |l| l.len());
                let mut v = v;
                v.truncate(n);
                Val::L(v)
            }
            (TyKind::Int(_), TyKind::Ptr(..)) if crate::web::BASE != 0 && self.int_ty(from).bits == 32 && self.u.prog.session.options.check_target == crate::options::CheckTarget::Web => {
                // the web target: a 32-bit integer cast to a pointer is a wasm address, the
                // offset into the linear memory's 4 GB window (web.rs)
                let x = if self.int_ty(from).signed {
                    let z = self.f.vreg(Cls::I);
                    self.emit(Inst::IBinI(IOp::And, U64, z, x, 0xffff_ffff));
                    z
                } else { x };
                let d = self.f.vreg(Cls::I);
                self.emit(Inst::IBinI(IOp::Or, U64, d, x, crate::web::BASE as i64));
                Val::L(vec![d])
            }
            (TyKind::FnPtr(..), _) | (TyKind::Int(_), TyKind::Ptr(..)) => Val::L(v),
            _ => Val::L(v),
        }
    }

    // ------------------------------------------------------------ control flow

    /// Branches on a boolean condition expression (handles `let` chains with `&&`).
    fn lower_cond(&mut self, c: ExprId, t: u32, f: u32) {
        let ast = self.ast();
        match &ast.expr(c).kind {
            ExprKind::Let(p, x) => {
                self.want_mut = self.pat_binds_mut_ref(*p);
                let m0 = self.stmt_temps.len();
                let rescope = self.cond_rescope.take();
                let (place, pt, root, temp) = self.scrutinee(*x);
                self.cond_rescope = rescope;
                self.want_mut = false;
                // the scrutinee's temporaries exist only on paths through this `let` (a chain
                // `a && let x = f()` may skip it): they get drop flags
                for k in m0..self.stmt_temps.len() {
                    if self.stmt_temps[k].2.is_none() {
                        let fl = self.f.vreg(Cls::I);
                        self.entry_inits.push(Inst::Iconst(fl, 0));
                        self.emit(Inst::Iconst(fl, 1));
                        self.stmt_temps[k].2 = Some(fl);
                    }
                }
                // edition 2024: they belong to the `if let`'s scope, declared before this let's
                // bindings (dropped after them, before the earlier lets' bindings)
                let bind_pos = rescope.map(|d| self.scopes.get(d).map_or(0, |s| s.len()));
                if let Some(d) = rescope {
                    let ts = self.stmt_temps.split_off(m0.min(self.stmt_temps.len()));
                    for x in ts {
                        self.push_scope_temp(d, x);
                    }
                }
                let fail = self.f.block();
                self.test_pat(*p, &place, pt, fail);
                let saved = (self.match_root, self.pat_path.take());
                self.match_root = root;
                self.pat_path = self.root_path(*x, root, &temp);
                self.bind_pat_place(*p, &place, pt);
                self.match_root = saved.0;
                self.pat_path = saved.1;
                // an rvalue scrutinee lives to the end of the statement (bindings may
                // borrow it); parts moved out clear its flag
                if let (Some(MatchRoot::Temp(fl)), Some(tp)) = (root, &temp) {
                    let (a, tt) = (tp.addr, tp.ty);
                    match (rescope, bind_pos) {
                        (Some(d), Some(pos)) => self.push_scope_temp_at(d, (a, tt, Some(fl)), pos),
                        _ => self.stmt_temps.push((a, tt, Some(fl))),
                    }
                }
                self.goto(t);
                self.switch(fail);
                self.goto(f);
            }
            ExprKind::Binary(BinOp::And, a, b) => {
                let mid = self.f.block();
                self.lower_cond(*a, mid, f);
                self.switch(mid);
                self.lower_cond(*b, t, f);
            }
            ExprKind::Binary(BinOp::Or, a, b) => {
                let mid = self.f.block();
                self.lower_cond(*a, t, mid);
                self.switch(mid);
                self.lower_cond(*b, t, f);
            }
            ExprKind::Paren(x) if !matches!(ast.expr(*x).kind, ExprKind::Let(..)) => self.lower_cond(*x, t, f),
            _ => {
                // a condition is a terminating scope: its temporaries (`c.borrow().len() == 0`)
                // are dropped before either branch runs
                let mark = self.stmt_temps.len();
                let ct = self.body_ty(c);
                let rescope = self.cond_rescope.take();
                let v = self.lower_expr(c);
                self.cond_rescope = rescope;
                let v = self.to_leaves(v, ct);
                self.drop_stmt_temps_from(mark);
                if v.is_empty() {
                    self.goto(f);
                } else {
                    self.term(Term::Branch(v[0], t, f));
                }
            }
        }
    }

    /// Does an `if` condition bind (`let` alone or in a `&&` chain)?
    fn cond_has_let(&self, c: ExprId) -> bool {
        match &self.ast().expr(c).kind {
            ExprKind::Let(..) => true,
            ExprKind::Binary(BinOp::And | BinOp::Or, a, b) => self.cond_has_let(*a) || self.cond_has_let(*b),
            ExprKind::Paren(x) => self.cond_has_let(*x),
            _ => false,
        }
    }

    /// The value of `e` is owned by the expression (an rvalue, or a field of one: `make().1`
    /// moves the field out of the temporary), not by a local / static / pointee place.
    fn rvalue_rooted(&mut self, e: ExprId) -> bool {
        match &self.ast().expr(e).kind {
            ExprKind::Field(x, _) | ExprKind::TupleField(x, _) => {
                // a field reached through an autoderef of a reference, raw pointer or overloaded
                // Deref (`get().desc`, `src(0)?.desc`) lies in the pointee: a place the
                // expression only borrows (rustc as_place.rs: the base is a temporary of the
                // pointer type, the field a place behind its Deref projection). Only a Box's
                // contents are owned by the temporary holding it.
                let derefs = self.b.field_idx.get(&e.0).map_or(0, |&(d, _)| d);
                let mut t = self.body_ty(*x);
                for _ in 0..derefs {
                    match self.box_inner(t) {
                        Some(inner) => t = inner,
                        None => return false,
                    }
                }
                self.rvalue_rooted(*x)
            }
            ExprKind::Paren(x) => self.rvalue_rooted(*x),
            // `*tmp` of a temporary Box whose contents need drop is lowered as the owned contents
            // (the allocation freed at once, see lower_expr): a value like any rvalue
            ExprKind::Unary(UnOp::Deref, x) => self.temp_box_deref(*x),
            _ => !self.is_place_expr(e),
        }
    }

    /// A field projection of a value expression (`mk().0`, `three().1.0`, `(*tmp_box).a`; no
    /// autoderef): the value expression and the field path into it. rustc expr_as_place
    /// (rustc_mir_build/src/builder/expr/as_place.rs): the projection's base is not a place, so
    /// it becomes one temporary (as_temp, with its temporary scope) and the projection is a
    /// place inside it -- moves out of it are move paths of that temporary.
    fn rvalue_proj(&mut self, e: ExprId) -> Option<(ExprId, Vec<(u32, u32)>)> {
        match &self.ast().expr(e).kind {
            ExprKind::Paren(x) => self.rvalue_proj(*x),
            ExprKind::Field(x, _) | ExprKind::TupleField(x, _) => {
                let x = *x;
                let (0, fi) = *self.b.field_idx.get(&e.0)? else { return None };
                let (root, mut path) = match self.rvalue_proj(x) {
                    Some(v) => v,
                    // the base is the value expression (a call, a literal, `*tmp_box`; or a
                    // projection through an autoderef, lowered as a value of its own)
                    None if self.rvalue_rooted(x) => (x, Vec::new()),
                    None => return None,
                };
                path.push((0, fi));
                Some((root, path))
            }
            _ => None,
        }
    }

    /// Lowers the value expression `root` of an rvalue_proj into a temporary with a drop flag
    /// (set) and projects `path` in it: (temporary's address, its type, flag, place, place's
    /// type). The caller registers the temporary; moves out of the place clear their part of it
    /// (moved_temp_part), so its drop is the elaborated drop of what was not moved (rustc
    /// elaborate_drop.rs open_drop_for_adt / drop_ladder: per-field move paths, the fields that
    /// remain dropped in declaration order).
    fn lower_rvalue_proj(&mut self, root: ExprId, path: &[(u32, u32)]) -> (VReg, TyId, VReg, Place, TyId) {
        let rt = self.body_ty(root);
        // an rvalue: its own subexpressions are not part of the place (want_mut ends)
        let saved = std::mem::replace(&mut self.want_mut, false);
        let v = self.lower_expr(root);
        self.want_mut = saved;
        let pl = self.val_to_place(v);
        let a = self.place_addr(&pl, rt);
        let f = self.f.vreg(Cls::I);
        self.entry_inits.push(Inst::Iconst(f, 0));
        self.emit(Inst::Iconst(f, 1));
        let (mut pl, mut pt) = (Place::Mem(a, 0), rt);
        for &(_, fi) in path {
            crate::boundary::note_field(self.u, self.b.def, &self.b.args, pt, fi);
            if !self.u.provided.is_empty() {
                self.check_provided_field(pt);
            }
            (pl, pt) = self.project(&pl, pt, 0, fi);
        }
        (a, rt, f, pl, pt)
    }

    /// `*x` moves the contents out of a temporary Box (x a value expression of type Box<T>, T
    /// with drop glue): lower_expr reads them and frees the allocation.
    fn temp_box_deref(&mut self, x: ExprId) -> bool {
        if self.is_place_expr(x) {
            return false;
        }
        match self.box_inner(self.body_ty(x)) {
            Some(inner) => self.needs_drop_glue(inner),
            None => false,
        }
    }

    /// A struct update base that names memory the update does not own: a place reached through a
    /// reference or raw pointer (`..*self`, `..self.layout`, `..v[i]`) or a static. Its remaining
    /// fields are copied out (typeck requires them Copy: moving out of a borrow is E0507) and nothing
    /// of it is dropped; only a value expression's temporary owns what the update leaves behind.
    fn base_is_borrowed_place(&self, e: ExprId) -> bool {
        match &self.ast().expr(e).kind {
            ExprKind::Paren(x) => self.base_is_borrowed_place(*x),
            ExprKind::Path(_) => self.is_place_expr(e),
            ExprKind::Field(x, _) | ExprKind::TupleField(x, _) => {
                // auto-deref through a reference makes the projection a borrowed place
                let derefs = self.b.field_idx.get(&e.0).map_or(0, |&(d, _)| d);
                (derefs > 0 && self.box_inner(self.body_ty(*x)).is_none()) || self.base_is_borrowed_place(*x)
            }
            // `v[i]` through the Index trait is `*v.index(i)`: borrowed; an array's element is part of the array
            ExprKind::Index(x, _) => self.base_is_borrowed_place(*x) || !matches!(self.kind_ref(self.body_ty(*x)), TyKind::Array(..)),
            // `*r` of a reference or pointer: borrowed; `*b` of a Box temporary owns its contents
            ExprKind::Unary(UnOp::Deref, x) => self.box_inner(self.body_ty(*x)).is_none() || self.base_is_borrowed_place(*x),
            _ => false,
        }
    }

    fn is_place_expr(&self, e: ExprId) -> bool {
        match &self.ast().expr(e).kind {
            // statics are places: borrowed in place (`STATIC.method()`), never copied or dropped
            ExprKind::Path(_) => match self.b.res.get(&e.0) {
                Some(Res::Local(_)) => true,
                Some(Res::Def(d)) => matches!(self.u.prog.def(*d).kind, DefKind::Static | DefKind::ForeignStatic),
                _ => false,
            },
            ExprKind::Field(..) | ExprKind::TupleField(..) | ExprKind::Index(..) | ExprKind::Unary(UnOp::Deref, _) => true,
            ExprKind::Paren(x) => self.is_place_expr(*x),
            _ => false,
        }
    }

    /// The way the `Branch` that ends block `b` (to `tb` / `eb`) goes when its condition is a
    /// constant computed in that block from constants (a const parameter's value, comparisons,
    /// negation); None when it is not known.
    fn known_branch(&self, b: u32, tb: u32, eb: u32) -> Option<bool> {
        let blk = self.f.blocks.get(b as usize)?;
        let Term::Branch(c, t, e) = blk.term else { return None };
        if (t, e) != (tb, eb) { return None; }
        // A register is a constant only if the whole function writes it exactly once (RIR
        // registers are not SSA: a loop or a join may assign the same one again); the walk goes
        // only through such registers.
        fn value(f: &Func, insts: &[&Inst], writes: &dyn Fn(VReg) -> u32, used: &mut Vec<VReg>, v: VReg, depth: u32) -> Option<i128> {
            if depth > 8 || writes(v) != 1 { return None; }
            used.push(v);
            let def = insts.iter().rev().copied().find(|i| matches!(i, Inst::Iconst(d, _) | Inst::Mov(d, _) | Inst::ICmpI(_, _, d, _, _) | Inst::ICmp(_, _, d, _, _) if *d == v))?;
            match def {
                Inst::Iconst(_, k) => Some(*k as i128),
                Inst::Mov(_, a) => value(f, insts, writes, used, *a, depth + 1),
                Inst::ICmpI(cond, signed, _, a, k) => {
                    let (x, k) = (value(f, insts, writes, used, *a, depth + 1)?, *k as i128);
                    Some(cmp(*cond, *signed, x, k) as i128)
                }
                Inst::ICmp(cond, signed, _, a, b) => {
                    let (x, y) = (value(f, insts, writes, used, *a, depth + 1)?, value(f, insts, writes, used, *b, depth + 1)?);
                    Some(cmp(*cond, *signed, x, y) as i128)
                }
                _ => None,
            }
        }
        fn cmp(c: Cond, signed: bool, x: i128, y: i128) -> bool {
            let (x, y) = if signed { (x as i64 as i128, y as i64 as i128) } else { (x as u64 as i128, y as u64 as i128) };
            match c { Cond::Eq => x == y, Cond::Ne => x != y, Cond::Lt => x < y, Cond::Le => x <= y, Cond::Gt => x > y, Cond::Ge => x >= y }
        }
        // Only under a check (the post-monomorphization walk): RIR registers are not SSA, so a
        // definition found by search is a proof only within the branch's own block; the code a
        // run executes is lowered as written.
        if !self.fold { return None; }
        let local: Vec<&Inst> = self.entry_inits.iter().chain(blk.insts.iter()).collect();
        let writes = |v: VReg| -> u32 { rir_writes(&self.f, &self.entry_inits, v) };
        let mut used = Vec::new();
        let known = value(&self.f, &local, &writes, &mut used, c, 0).map(|k| k != 0);
        if known.is_some() { self.fold_regs.borrow_mut().extend(used); }
        known
    }

    fn lower_if(&mut self, c: ExprId, then: BlockId, els: Option<ExprId>, t: TyId) -> Val {
        let tb = self.f.block();
        let eb = self.f.block();
        let join = self.f.block();
        let res = self.result_place(t);
        // `if let` bindings live in a scope that ends with the then-block (Reference
        // destructors.md: the bindings of an if-let condition are dropped after the block;
        // `if let Some(g) = cell.try_borrow().ok() { .. }` releases g there)
        let let_scope = self.cond_has_let(c);
        let depth = self.scopes.len();
        if let_scope {
            self.push_scope();
        }
        // edition 2024 (if_let_rescope): the temporaries of an `if let` condition drop at the
        // end of the then block, after its bindings, and before the else block; before 2024
        // they belong to the enclosing statement
        let rescope = let_scope && self.u.prog.files[self.b.file as usize].edition >= 2024;
        let saved_rescope = self.cond_rescope.take();
        if rescope {
            self.cond_rescope = Some(depth);
        }
        self.lower_cond(c, tb, eb);
        self.cond_rescope = saved_rescope;
        // rustc_mir_transform SimplifyConstCondition: a branch on a constant (`if N != 0` once the
        // const parameter has its value) is taken one way, and its dead successor is not
        // lowered -- what it names (a constant that cannot be evaluated, a closure) is never
        // instantiated, as rustc's monomorphization collector never sees it
        let known = if let_scope { None } else { self.known_branch(self.cur, tb, eb) };
        self.switch(tb);
        // the body of an `if` and its `else` block are temporary scopes (Reference
        // destructors.scope.temporary: "The body of an if ... expression", "The else block")
        let mark = self.stmt_temps.len();
        if known == Some(false) {
            self.term(Term::Unreachable);
            self.dead();
        } else {
            let v = self.lower_block(then);
            if !self.is_never(t) {
                self.write_place(&res, t, v);
            }
        }
        self.drop_stmt_temps_from(mark);
        if let_scope {
            self.drop_scopes_from(depth);
        }
        self.goto(join);
        self.switch(eb);
        if let_scope {
            // the else path: a let of the chain that bound before a later condition failed
            // (`if let z = make() && false`) drops its bindings before the else block (rustc:
            // the condition's scope ends there); unbound ones have their flags clear
            self.drop_scopes_from(depth);
            self.scopes.truncate(depth);
            self.scope_temps.truncate(depth);
            self.scope_temp_pos.truncate(depth);
            self.scope_marks.truncate(depth);
            self.tail_marks.truncate(depth);
        }
        if known == Some(true) {
            self.term(Term::Unreachable);
            self.dead();
        } else if let Some(x) = els {
            let mark = self.stmt_temps.len();
            let v = self.lower_expr_coerced(x, t);
            if !self.is_never(t) {
                self.write_place(&res, t, v);
            }
            self.drop_stmt_temps_from(mark);
        }
        self.goto(join);
        self.switch(join);
        if self.is_never(t) {
            self.term(Term::Unreachable);
            self.dead();
            return self.unit();
        }
        self.read_place(&res, t)
    }

    /// Whether capture `i` (local `li`) of closure `ce` is by value (typeck's decision).
    /// The fields a precise by-value capture owns (None: the whole local).
    fn storage_for_closure(&self, ce: u32) -> &crate::layout::ClosureStorage {
        &self.closure_storage[&ce]
    }

    fn capture_fields(&self, ce: u32, i: usize) -> Option<Vec<u32>> {
        self.storage_for_closure(ce).owned_fields.get(i).cloned().flatten()
    }

    fn capture_by_value(&self, ce: u32, i: usize, _li: u32) -> bool {
        self.storage_for_closure(ce).by_value.get(i).copied().unwrap_or(false)
    }

    /// The field path of a scrutinee in its owner (for partial moves by its patterns).
    fn root_path(&self, x: ExprId, root: Option<MatchRoot>, temp: &Option<ScrutTemp>) -> Option<Vec<(u32, u32)>> {
        match root {
            Some(MatchRoot::Local(r)) => match self.field_move_path(x) {
                Some((li, p)) if li == r => Some(p),
                _ => None,
            },
            // a temporary scrutinee: parts moved out are tracked from the temporary's root
            Some(MatchRoot::Temp(_)) => Some(temp.as_ref().map_or(Vec::new(), |t| t.path.clone())),
            None => None,
        }
    }

    /// Part `path` of the temporary with drop flag `f` was moved out.
    fn moved_temp_part(&mut self, f: VReg, path: &[(u32, u32)]) {
        let existing = self.temp_moved.get(&f.0).and_then(|v| v.iter().find(|x| x.0 == path).map(|x| x.1));
        let mf = match existing {
            Some(m) => m,
            None => {
                let m = self.f.vreg(Cls::I);
                self.entry_inits.push(Inst::Iconst(m, 0));
                self.temp_moved.entry(f.0).or_default().push((path.to_vec(), m));
                m
            }
        };
        self.emit(Inst::Iconst(mf, 1));
    }

    /// Drops the temporary at `a: t` with drop flag `f` (what of it was not moved out).
    fn drop_flagged_temp(&mut self, a: VReg, t: TyId, f: VReg) {
        let yes = self.f.block();
        let join = self.f.block();
        self.term(Term::Branch(f, yes, join));
        self.switch(yes);
        match self.temp_moved.get(&f.0).cloned() {
            Some(entries) => {
                self.drop_tree_top(&entries, a, t);
                for (_, m) in &entries {
                    self.emit(Inst::Iconst(*m, 0));
                }
            }
            None => self.drop_at(a, t),
        }
        self.goto(join);
        self.switch(join);
    }

    /// The matched place and its owner for move tracking; with a temporary owner
    /// (MatchRoot::Temp) also the temporary for the caller to register (its address and type)
    /// and the scrutinee's field path in it.
    /// `matches!(x, pat if guard)` / `assert_matches!`: core expands both to a `match` with the
    /// pattern's arm and a fallback, so `x` is a match scrutinee like `lower_match`'s (rustc
    /// region.rs: no terminating scope; an rvalue -- or the base of a field of one, such as the
    /// guard in `matches!(m.lock().unwrap().phase, ..)` -- is a temporary of the enclosing
    /// statement, dropped at its end), and the pattern's bindings and the guard's temporaries
    /// belong to the arm. Returns the scrutinee place and the blocks entered on a match (arm
    /// left) and on no match / a failed guard; the current block is dead afterwards.
    fn lower_macro_match(&mut self, x: ExprId, pat: Option<PatId>, guard: Option<ExprId>) -> (Place, TyId, u32, u32) {
        self.want_mut = pat.map_or(false, |p| self.pat_binds_mut_ref(p));
        let (place, pt, root, temp) = self.scrutinee(x);
        self.want_mut = false;
        if let (Some(MatchRoot::Temp(fl)), Some(tp)) = (root, &temp) {
            self.stmt_temps.push((tp.addr, tp.ty, Some(fl)));
        }
        let ok = self.f.block();
        let fail = self.f.block();
        if let Some(p) = pat {
            let gmark = self.stmt_temps.len();
            let combos = if guard.is_some() { self.or_combos(p, 1024) } else { None };
            self.enter_arm(x, p, guard, combos, true, &place, pt, root, &temp, fail, gmark);
            self.drop_stmt_temps_from(gmark);
            self.pop_scope();
            self.pop_scope();
        }
        self.goto(ok);
        (place, pt, ok, fail)
    }

    fn scrutinee(&mut self, x: ExprId) -> (Place, TyId, Option<MatchRoot>, Option<ScrutTemp>) {
        let xt = self.body_ty(x);
        // `*tmp_box` of a temporary Box is the owned contents (temp_box_deref), not a place:
        // what the arms move out of it is tracked like any rvalue scrutinee
        let owned_deref = matches!(self.ast().expr(x).kind, ExprKind::Unary(UnOp::Deref, b) if self.temp_box_deref(b));
        if self.is_place_expr(x) && !owned_deref {
            // a field of a value expression (`mk().0`, `three().1.0`): the place lies in one
            // temporary of the scrutinee's temporary scope (rustc expr_as_place: as_temp of the
            // base; region.rs: a match scrutinee is no terminating scope, so its temporaries
            // live to the end of the enclosing statement); what the arms move out of it is not
            // dropped with it
            if let Some((r, path)) = self.rvalue_proj(x) {
                if self.needs_drop_glue(self.body_ty(r)) {
                    let (a, rt, f, pl, pt) = self.lower_rvalue_proj(r, &path);
                    return (pl, pt, Some(MatchRoot::Temp(f)), Some(ScrutTemp { addr: a, ty: rt, path }));
                }
            }
            let (pl, pt) = self.lower_place(x);
            let root = self.place_root_local(x).map(MatchRoot::Local);
            (pl, pt, root, None)
        } else {
            let v = self.lower_expr(x);
            let mut place = self.val_to_place(v);
            let mut root = None;
            if self.needs_drop_glue(xt) {
                // keep the rvalue in memory with a flag; whatever is not moved out is
                // dropped after the match
                let a = self.place_addr(&place, xt);
                place = Place::Mem(a, 0);
                let f = self.f.vreg(Cls::I);
                // clear on every path: a guard lowered once per or-candidate registers one
                // such temporary per candidate, and only the paths that ran it set its flag
                self.entry_inits.push(Inst::Iconst(f, 0));
                self.emit(Inst::Iconst(f, 1));
                root = Some(MatchRoot::Temp(f));
                return (place, xt, root, Some(ScrutTemp { addr: a, ty: xt, path: Vec::new() }));
            }
            (place, xt, root, None)
        }
    }

    /// Enters a match arm whose pattern `pat` is tested against the scrutinee `place: pt` (unless
    /// `test` is false: a decision tree already did), failing to `next`: binds the pattern (moves
    /// out of the scrutinee tracked through `root` / `temp`) and runs the guard. Leaves two
    /// scopes pushed -- the pattern's bindings, then the guard's -- with the current block at the
    /// arm body; the caller pops them (see `lower_match`). `gmark`: the arm's first statement
    /// temporary. `combos`: the guarded arm's or-alternative candidates (`or_combos`).
    #[allow(clippy::too_many_arguments)]
    fn enter_arm(&mut self, x: ExprId, pat: PatId, guard: Option<ExprId>, combos: Option<Vec<Vec<(u32, usize)>>>, test: bool, place: &Place, pt: TyId, root: Option<MatchRoot>, temp: &Option<ScrutTemp>, next: u32, gmark: usize) {
        if let (Some(g), Some(combos)) = (guard, combos) {
            // a guarded arm (rustc match lowering): each or-alternative combination is a
            // candidate of its own -- test it, bind (moves deferred), run the guard; a
            // failing guard tries the next alternative, then the next arm, with the
            // scrutinee untouched
            let body = self.f.block();
            let arm_depth = self.scopes.len();
            self.push_scope();
            let mut lis = Vec::new();
            self.pat_binding_locals(pat, &mut lis);
            for li in &lis {
                self.local_place(*li);
                // the pattern's bindings belong to the arm scope (registered before the
                // guard's scope is pushed)
                let lt = self.monomorphize(self.b.locals[*li as usize].ty);
                if self.needs_drop_glue(lt) && !self.foreign[*li as usize] {
                    self.flag_of(*li);
                }
            }
            // the guard's `let` bindings: a scope of their own, dropped before the guard's
            // temporaries and the pattern's bindings (rustc if-let guard scopes)
            self.push_scope();
            let n = combos.len();
            for (ci, combo) in combos.into_iter().enumerate() {
                let alt_fail = if ci + 1 == n { next } else { self.f.block() };
                self.or_force = combo.into_iter().collect();
                self.test_pat(pat, place, pt, alt_fail);
                let saved = (self.match_root, self.pat_path.take());
                self.match_root = root;
                self.pat_path = self.root_path(x, root, temp);
                self.defer_moves = Some(Vec::new());
                self.bind_pat_place(pat, place, pt);
                let pend = self.defer_moves.take().unwrap_or_default();
                self.match_root = saved.0;
                self.pat_path = saved.1;
                self.or_force.clear();
                let gok = self.f.block();
                let gfail = self.f.block();
                // a guard is a temporary scope: its `let` scrutinee temporaries drop with its
                // bindings, in creation order
                let saved_rescope = self.cond_rescope.replace(self.scopes.len() - 1);
                self.lower_cond(g, gok, gfail);
                self.cond_rescope = saved_rescope;
                // (an if-let guard's temporaries: dropped when it fails, else at the
                // arm's end)
                // guard failed: the bindings were copies, nothing was moved; the guard's
                // own `let` bindings drop here
                self.switch(gfail);
                for li in &lis {
                    if let Some(f) = self.drop_flags[*li as usize] {
                        self.emit(Inst::Iconst(f, 0));
                    }
                }
                self.drop_scopes_from(arm_depth);
                self.clear_scope_temp_flags(arm_depth + 1);
                self.drop_stmt_temps_here(gmark);
                self.goto(alt_fail);
                self.switch(gok);
                for mv in pend {
                    self.do_move(mv);
                }
                self.goto(body);
                if ci + 1 < n {
                    self.switch(alt_fail);
                }
            }
            self.switch(body);
        } else {
            if test {
                self.test_pat(pat, place, pt, next);
            }
            let arm_depth = self.scopes.len();
            self.push_scope();
            let saved = (self.match_root, self.pat_path.take());
            self.match_root = root;
            self.pat_path = self.root_path(x, root, temp);
            // (a guarded arm here has too many or-candidates to try one by one: its moves
            // happen before the guard, as a dynamic or-dispatch has no single move set; a
            // failing guard then leaks the moved parts, never drops them twice)
            self.bind_pat_place(pat, place, pt);
            self.match_root = saved.0;
            self.pat_path = saved.1;
            // the guard's scope (see the candidate path above)
            self.push_scope();
            if let Some(g) = guard {
                let body = self.f.block();
                let gfail = self.f.block();
                let saved_rescope = self.cond_rescope.replace(self.scopes.len() - 1);
                self.lower_cond(g, body, gfail);
                self.cond_rescope = saved_rescope;
                self.switch(gfail);
                let mut lis = Vec::new();
                self.pat_binding_locals(pat, &mut lis);
                for li in &lis {
                    if let Some(f) = self.drop_flags[*li as usize] {
                        self.emit(Inst::Iconst(f, 0));
                    }
                }
                self.drop_scopes_from(arm_depth);
                self.clear_scope_temp_flags(arm_depth + 1);
                self.drop_stmt_temps_here(gmark);
                self.goto(next);
                self.switch(body);
            }
        }
    }

    fn do_move(&mut self, mv: PendingMove) {
        match mv {
            PendingMove::Field(r, p) => self.moved_field(r, &p),
            PendingMove::Local(r) => self.moved_local(r),
            PendingMove::TempPart(f, p) => self.moved_temp_part(f, &p),
            PendingMove::Temp(f) => self.emit(Inst::Iconst(f, 0)),
        }
    }

    /// The or-alternative combinations of pattern `p` in rustc's candidate order (the first
    /// or-pattern varies slowest): or-node -> chosen alternative; None past `cap`. Binding-free
    /// or-patterns are candidates too (rustc runs the guard once per matching alternative:
    /// or-patterns/search-via-bindings.rs counts the runs).
    fn or_combos(&self, p: PatId, cap: usize) -> Option<Vec<Vec<(u32, usize)>>> {
        let ast = self.ast();
        let kids: Vec<PatId> = match ast.pat(p) {
            Pat::Ident { sub, .. } => sub.iter().copied().collect(),
            Pat::Paren(x) | Pat::Box(x) | Pat::Ref(_, x) => vec![*x],
            Pat::Tuple(v) | Pat::TupleStruct(_, v) | Pat::Slice(v) => ast.list(*v).to_vec(),
            Pat::Struct(_, fs, _) => ast.list(*fs).iter().map(|f| f.pat).collect(),
            Pat::Or(alts) => {
                let mut out = Vec::new();
                for (i, a) in ast.list(*alts).iter().copied().enumerate() {
                    for mut c in self.or_combos(a, cap)? {
                        c.insert(0, (p.0, i));
                        out.push(c);
                        if out.len() > cap {
                            return None;
                        }
                    }
                }
                return Some(out);
            }
            _ => Vec::new(),
        };
        let mut acc: Vec<Vec<(u32, usize)>> = vec![Vec::new()];
        for k in kids {
            let cs = self.or_combos(k, cap)?;
            let mut nxt = Vec::new();
            for a in &acc {
                for c in &cs {
                    let mut x = a.clone();
                    x.extend(c.iter().copied());
                    nxt.push(x);
                    if nxt.len() > cap {
                        return None;
                    }
                }
            }
            acc = nxt;
        }
        Some(acc)
    }

    fn lower_match(&mut self, x: ExprId, arms: &[Arm], t: TyId) -> Val {
        // an arm binding `ref mut` makes the scrutinee place mutable (DerefMut / IndexMut)
        let mutable = arms.iter().any(|a| self.pat_binds_mut_ref(a.pat));
        self.want_mut = mutable;
        let (place, pt, root, temp) = self.scrutinee(x);
        self.want_mut = false;
        // an rvalue scrutinee is a temporary of the enclosing statement (Reference
        // destructors.scope.temporary): it drops at the statement's end -- after the block's
        // locals for a pre-2024 tail `match` -- and when a panic unwinds out of an arm; parts
        // moved out clear its flag
        if let (Some(MatchRoot::Temp(fl)), Some(tp)) = (root, &temp) {
            self.stmt_temps.push((tp.addr, tp.ty, Some(fl)));
        }
        let join = self.f.block();
        let res = self.result_place(t);
        let live: Vec<Arm> = arms.iter().filter(|a| self.active(a.attrs)).copied().collect();
        // the or-alternative combinations of each guarded arm (see below)
        let combos: Vec<Option<Vec<Vec<(u32, usize)>>>> = live.iter().map(|a| if a.guard.is_some() { self.or_combos(a.pat, 1024) } else { None }).collect();
        let forced: Vec<bool> = live.iter().zip(&combos).map(|(a, c)| a.guard.is_some() && c.is_some()).collect();
        // the arms' tests as a decision tree (match_tree.rs): each arm is then entered where the
        // tree leaves it, and tested in order only after a failed guard or an undecided pattern
        let mut tree: Option<match_tree::Tree> = None;
        if live.len() >= 3 {
            let e: Vec<u32> = (0..=live.len()).map(|_| self.f.block()).collect();
            let post: Vec<u32> = (0..live.len()).map(|_| self.f.block()).collect();
            let mut tr = match_tree::Tree::new(e, post);
            let pats: Vec<PatId> = live.iter().map(|a| a.pat).collect();
            if let Some(root) = self.match_tree(&pats, &forced, &mut tr, &place, pt) {
                self.goto(root);
                tree = Some(tr);
            }
        }
        // which entries the tree (and the arms that fall through to later ones) reach
        let (mut need_e, mut need_post) = (vec![true; live.len()], vec![true; live.len()]);
        if let Some(tr) = &tree {
            need_e = tr.used_e.clone();
            need_post = tr.used_post.clone();
            for k in 0..live.len() {
                if k > 0 && (need_e[k - 1] || (live[k - 1].guard.is_some() && need_post[k - 1])) {
                    need_e[k] = true;
                }
                if need_e[k] && !forced[k] {
                    need_post[k] = true;
                }
            }
        }
        for (k, arm) in live.iter().enumerate() {
            let tree_mode = tree.is_some();
            let combos_path = forced[k];
            let next = match &tree {
                Some(tr) => tr.e[k + 1],
                None => self.f.block(),
            };
            if let Some(tr) = &tree {
                if !need_e[k] && !need_post[k] {
                    continue;
                }
                if need_e[k] {
                    self.switch(tr.e[k]);
                    if !combos_path {
                        self.test_pat(arm.pat, &place, pt, next);
                        self.goto(tr.post[k]);
                    }
                }
                if !combos_path {
                    self.switch(tr.post[k]);
                }
            }
            // the guard's temporaries (if-let guards) belong to the arm
            let gmark = self.stmt_temps.len();
            self.enter_arm(x, arm.pat, arm.guard, combos[k].clone(), !tree_mode, &place, pt, root, &temp, next, gmark);
            let mark = self.stmt_temps.len();
            let v = self.lower_expr_coerced(arm.body, t);
            if !self.is_never(t) {
                self.write_place(&res, t, v);
            }
            // an arm body is a terminating scope: its temporaries exist on this path only;
            // then the guard's bindings, its temporaries, the pattern's bindings
            self.drop_stmt_temps_from(mark);
            self.pop_scope();
            self.drop_stmt_temps_from(gmark);
            self.pop_scope();
            self.goto(join);
            if !tree_mode {
                self.switch(next);
            }
        }
        // no arm matched: unreachable for exhaustive matches
        if let Some(tr) = &tree {
            self.switch(tr.e[live.len()]);
        }
        self.term(Term::Unreachable);
        self.switch(join);
        if self.is_never(t) {
            self.term(Term::Unreachable);
            self.dead();
            return self.unit();
        }
        self.read_place(&res, t)
    }

    /// A loop head in a checked program counts one iteration of the constant being evaluated
    /// (see Options::const_fuel).
    fn const_fuel_tick(&mut self) {
        if !self.u.prog.session.options.const_fuel { return; }
        let rt = self.u.syms.intern(crate::syms::SymKind::Rt("const_fuel"), const_fuel_addr());
        self.emit(Inst::Call(Callee::Host(rt), Vec::new(), Vec::new()));
    }

    fn lower_for(&mut self, p: PatId, it: ExprId, b: BlockId, label: Option<Ident>) {
        // rustc_ast_lowering/src/expr.rs:1774-1784,1826-1838. Inline the resolved
        // iterator calls through the ordinary optimizer, preserving iterator state
        // (in particular RangeInclusive::exhausted) and custom IntoIterator behavior.
        let l = label.map(|x| self.lifetime_sym(x));
        self.lower_for_iterator(p, it, b, l);
    }

    // ------------------------------------------------------------ calls

    /// Aggregate exports use arena storage even when their ABI happens to return registers.
    /// This is deliberately not a native stack slot: an optimiser must never forward a host
    /// destination as a sandboxed callee's sret pointer. The wrapper's __SbFrame reclaims it.
    fn sandbox_result_slot(&mut self, size: u32, align: u32) -> VReg {
        let alloc = self.u.syms.intern(crate::syms::SymKind::Rt("sb_alloca"), crate::modules::__relax_sb_alloca as *const () as u64);
        let size = self.iconst(size as i64);
        let align = self.iconst(align as i64);
        let p = self.f.vreg(Cls::I);
        self.emit(Inst::Call(Callee::Host(alloc), vec![size, align], vec![p]));
        let good = self.f.block();
        let bad = self.f.block();
        self.term(Term::Branch(p, good, bad));
        self.switch(bad);
        self.panic_here("a result does not fit the module's shadow stack");
        self.switch(good);
        p
    }

    /// SPEC-V: whether a call with these arguments could pass aliasing pointers to a `noalias`
    /// parameter: a `&mut` of a sized `Unpin` type (the parameters lowered with `noalias`) and at least
    /// one other argument that can hold a pointer. Two paths to the same bytes need two such values.
    fn alias_prone(&mut self, args: &[(Val, TyId)]) -> bool {
        let mut muts = false;
        let mut holders = 0;
        for (_, t) in args {
            if let TyKind::Ref(true, inner, _) = self.kind_ref(*t).clone() {
                if !self.unsized_ty(inner) && self.u.is_unpin(inner) { muts = true; }
            }
            if self.may_hold_pointer(*t, 0) { holders += 1; }
        }
        muts && holders >= 2
    }

    fn may_hold_pointer(&mut self, t: TyId, depth: u32) -> bool {
        match self.kind_ref(t).clone() {
            TyKind::Int(_) | TyKind::Float(_) | TyKind::Bool | TyKind::Char | TyKind::Never | TyKind::Str => false,
            TyKind::Tuple(v) => {
                let items: Vec<TyId> = self.u.tcx.tys.list(v).to_vec();
                items.into_iter().any(|x| self.may_hold_pointer(x, depth + 1))
            }
            TyKind::Array(e, _) => depth < 4 && self.may_hold_pointer(e, depth + 1),
            TyKind::Adt(d, args, _) if depth < 4 => {
                if self.layout(t).size == 0 { return false; }
                let u = self.u;
                let fields: Vec<TyId> = match u.tcx.adts.get(&d) {
                    Some(a) => a.variants.iter().flat_map(|v| v.fields.iter().map(|f| u.tcx.tys.subst_erasing_regions(f.ty, u.tcx.tys.list(args)))).collect(),
                    None => return true,
                };
                fields.into_iter().any(|f| self.may_hold_pointer(f, depth + 1))
            }
            _ => true,
        }
    }

    /// Emits a call with Relax's ABI. Returns the result value.
    fn emit_call(&mut self, callee: Callee, mut args: Vec<(Val, TyId)>, ret: TyId) -> Val {
        let def = match &callee {
            Callee::Fn(id) | Callee::CFn(id, _) => {
                let ahead = crate::jit::lower_ahead_fn_key(*id);
                match ahead.as_ref().unwrap_or_else(|| &self.u.fns[*id as usize].key) {
                    FnKey::Inst(d, _) => Some(*d),
                    FnKey::Shim(shim) => Some(shim.kind.def_id()),
                    _ => None,
                }
            },
            _ => None,
        };
        if def.is_some_and(|d| self.u.tcx.fn_item_sig(&self.u.prog, d).is_some_and(|s| s.header.abi == crate::types::Abi::RustCall)) {
            self.spread_tuple_argument(&mut args);
        }
        // SPEC-V: unverified code may break the borrow rules; the callee's `noalias` code must not
        // see such arguments
        let to_twin = self.plain_calls && matches!(callee, Callee::Fn(_)) && self.alias_prone(&args);
        // C ABI calls: every aggregate goes by pointer to a private copy, an aggregate
        // result through a result slot; the backend does the C placement (CSig)
        let csig = match &callee {
            Callee::CHost(_, s) | Callee::CIndirect(_, s) | Callee::CFn(_, s) => Some((**s).clone()),
            _ => None,
        };
        let rl = self.layout(ret);
        let sandbox_result = matches!(self.kind_ref(ret), TyKind::Array(..) | TyKind::Tuple(..) | TyKind::Adt(..))
            && !matches!(self.kind_ref(ret), TyKind::Tuple(v) if v.is_empty())
            && crate::sandbox::is_wrapper_crate(self.u.prog.def(self.b.def).krate)
            && matches!(&callee, Callee::Fn(id) if crate::sandbox::is_sandboxed_slot(self.u, *id))
            && crate::modules::boundary_result(self.u, ret);
        let arena_result = if sandbox_result { Some(self.sandbox_result_slot(rl.size, rl.align)) } else { None };
        let mut abi_args = Vec::new();
        let mut rets = Vec::new();
        let mut sret = None;
        let c_agg_ret = match &csig {
            Some(s) => Some(s.ret.is_some()),
            None => None,
        };
        match &rl.leaves {
            _ if c_agg_ret == Some(true) => {
                let a = self.new_slot_for(ret);
                abi_args.push(a);
                sret = Some(a);
            }
            Some(l) if c_agg_ret == Some(false) || ret_fits(l) => {
                for lf in l {
                    rets.push(self.f.vreg(lf.cls));
                }
            }
            _ => {
                let a = match arena_result {
                    Some(a) => a,
                    None => self.new_slot_flagged(ret, false, true),
                };
                abi_args.push(a);
                sret = Some(a);
            }
        }
        for (v, t) in args.into_iter() {
            let l = self.layout(t);
            let c_agg = csig.is_some() && l.size > 0 && matches!(crate::cabi::c_arg(self.u, t), relax_backend::rir::CArg::Agg(_));
            match l.leaves {
                Some(_) if !c_agg => {
                    let r = self.to_leaves(v, t);
                    abi_args.extend(r);
                }
                _ => {
                    // by pointer to a private copy
                    let tmp = self.new_arg_slot_for(t);
                    self.store_val(tmp, 0, t, v);
                    abi_args.push(tmp);
                    if let Some(c) = self.coro.as_mut() {
                        c.call_private.insert(tmp);
                    }
                }
            }
        }
        if let (Some(a), Some(c)) = (sret, self.coro.as_mut()) {
            c.call_private.insert(a);
            c.call_sret.insert(a);
        }
        if let (Some(_), Callee::Fn(id)) = (sret, &callee) {
            if !self.f.sret_callees.contains(id) {
                self.f.sret_callees.push(*id);
            }
        }
        if let Callee::Fn(id) = &callee {
            crate::boundary::note_call(self.u, self.b.def, &self.b.args, *id);
            self.check_provided_ownership(*id);
        }
        // a #[track_caller] callee gets the caller location (this fn's own when it tracks too)
        if let Callee::Fn(id) = &callee {
            if crate::lower::takes_caller_loc(self.u, *id) {
                let l = self.caller_location_val();
                abi_args.push(l);
            }
        }
        if std::mem::take(&mut self.indirect_tracked) && matches!(callee, Callee::Indirect(_)) {
            let l = self.caller_location_val();
            abi_args.push(l);
        }
        // (the twin has the original's ABI, hence its result slot and implicit caller location)
        let callee = match callee {
            Callee::Fn(id) if to_twin => {
                let twin = self.u.plain_twin(id);
                if twin != id { self.f.plain_calls = true; }
                if sret.is_some() && !self.f.sret_callees.contains(&twin) {
                    self.f.sret_callees.push(twin);
                }
                Callee::Fn(twin)
            }
            c => c,
        };
        self.emit(Inst::Call(callee, abi_args, rets.clone()));
        if self.is_never(ret) {
            self.term(Term::Unreachable);
            self.dead();
            return self.unit();
        }
        if let Some(a) = arena_result {
            if sret.is_none() {
                self.store_val(a, 0, ret, Val::L(rets));
            }
            // Copy while the arena frame is still entered. Return a host temporary, never a
            // borrowed arena value that could be used after __SbFrame::drop restores it.
            let host = self.new_slot_for(ret);
            self.store_val(host, 0, ret, Val::M(a, 0));
            return Val::M(host, 0);
        }
        match sret {
            Some(a) => Val::M(a, 0),
            None => Val::L(rets),
        }
    }

    /// rustc_codegen_ssa/src/mir/block.rs: native instance dispatch. Closure
    /// bodies receive their environment and untupled inputs (abi.rs:61-73).
    fn emit_instance(&mut self, instance: Instance, mut vals: Vec<(Val, TyId)>, ret: TyId) -> Val {
        let args = instance.compact_args(&self.u.tcx);
        match instance.def {
            InstanceKind::Virtual(d, index) => {
                // the trait-object type called through (real-time reach: jit.rs realtime)
                let dy = match self.kind(vals[0].1) {
                    TyKind::Ref(_, t, _) | TyKind::Ptr(_, t) => t,
                    _ => vals[0].1,
                };
                let dy = self.u.tcx.tys.erase_regions(dy);
                if matches!(self.kind(dy), TyKind::Dyn(..)) && !self.f.dyn_calls.contains(&dy.0) {
                    self.f.dyn_calls.push(dy.0);
                }
                let receiver = self.to_leaves(vals[0].0.clone(), vals[0].1);
                assert_eq!(receiver.len(), 2, "virtual receiver metadata");
                let ptr = self.f.vreg(Cls::I);
                self.emit(Inst::Load(Mem::Int(8, false), ptr, receiver[1], (index * 8) as i32));
                vals[0] = (Val::L(vec![receiver[0]]), self.u.tcx.tys.usize_);
                self.indirect_tracked = self.u.tcx.fn_sig(&self.u.prog, d).is_some_and(|s| s.track_caller);
                if self.u.tcx.fn_sig(&self.u.prog, d).is_some_and(|s| s.header.abi == crate::types::Abi::RustCall) { self.spread_tuple_argument(&mut vals); }
                self.emit_call(Callee::Indirect(ptr), vals, ret)
            }
            InstanceKind::Intrinsic(d) => self.intrinsic(d, &args, &vals, ret).expect("intrinsic lowering"),
            InstanceKind::Item(d) if self.u.prog.def(d).kind == DefKind::Closure => {
                let (file, node) = self.u.prog.def_id_to_node_id[&d];
                let coroutine = match node {
                    crate::def_collector::NodeId::AsyncBody(_) => true,
                    crate::def_collector::NodeId::Closure(expr) => self.u.tcx.coroutines.contains(&(file, expr.0)),
                    _ => unreachable!(),
                };
                // EXCEPTION(M7): Future::poll uses the existing coroutine slot ABI.
                if coroutine { return self.emit_call(Callee::Fn(self.u.instance_fn_id(instance)), vals, ret); }
                assert_eq!(vals.len(), 2, "closure call receiver and tuple");
                let tuple = vals.pop().unwrap();
                let TyKind::Tuple(inputs) = self.kind(tuple.1) else { panic!("closure input tuple") };
                let place = self.val_to_place(tuple.0);
                for i in 0..self.u.tcx.tys.list(inputs).len() {
                    let (field, ty) = self.project(&place, tuple.1, 0, i as u32);
                    vals.push((self.read_place(&field, ty), ty));
                }
                self.emit_call(Callee::Fn(self.u.instance_fn_id(instance)), vals, ret)
            }
            InstanceKind::Item(d) => {
                let sig = self.u.tcx.fn_item_sig(&self.u.prog, d).expect("item signature");
                let foreign = self.u.prog.def(d).kind == DefKind::ForeignFn;
                let mut types = Vec::new();
                for value in &vals { types.push(value.1); }
                let callee = if foreign {
                    let Some(address) = self.u.foreign_sym(d) else {
                        self.err(self.pos, format!("unknown foreign function `{}` -- link its library (#[link])", self.u.prog.name(d)));
                        return self.dummy_val(ret);
                    };
                    let fixed = if sig.header.c_variadic { Some(sig.params.len()) } else { None };
                    Callee::CHost(address, Box::new(crate::cabi::c_sig(self.u, &types, ret, fixed, sig.header.abi)))
                } else if sig.header.abi.is_c_like() {
                    let cs = crate::cabi::c_sig(self.u, &types, ret, None, sig.header.abi);
                    Callee::CFn(self.u.instance_fn_id(instance), Box::new(cs))
                } else { Callee::Fn(self.u.instance_fn_id(instance)) };
                let result = self.emit_call(callee, vals, ret);
                if foreign && matches!(self.kind(ret), TyKind::Int(_) | TyKind::Bool | TyKind::Char) {
                    let integer = self.int_ty(ret);
                    if integer.bits < 64 {
                        if let Val::L(ref regs) = result {
                            if regs.len() == 1 {
                                let extended = self.f.vreg(Cls::I);
                                self.emit(Inst::Conv(Conv::IntToInt(integer), extended, regs[0]));
                                return Val::L(vec![extended]);
                            }
                        }
                    }
                }
                result
            }
            InstanceKind::Shim(_) => self.emit_call(Callee::Fn(self.u.instance_fn_id(instance)), vals, ret),
        }
    }

    fn lower_call(&mut self, e: ExprId, f: ExprId, args: &[ExprId], t: TyId) -> Val {
        let mut ft = self.body_ty(f);
        // `#[rustc_legacy_const_generics]` arguments are const generic args, not values
        let legacy_kept: Vec<ExprId>;
        let args: &[ExprId] = match self.b.legacy_args.get(&e.0) {
            Some(lc) => {
                legacy_kept = args.iter().enumerate().filter(|(i, _)| !lc.contains(&(*i as u32))).map(|(_, a)| *a).collect();
                &legacy_kept
            }
            None => args,
        };
        // rustc_mir_build/src/thir/cx/expr.rs:349-373: overloaded calls use
        // the generic body's selected trait, resolved with the instance's args.
        if let Some(selected) = self.b.call_traits.get(&e.0).copied().filter(|s|
            matches!(self.u.tcx.lang_items.by_def.get(&s.trait_def), Some(LangItem::Fn | LangItem::FnMut | LangItem::FnOnce | LangItem::AsyncFn | LangItem::AsyncFnMut | LangItem::AsyncFnOnce))) {
            let tuple = self.monomorphize(selected.tupled_args);
            let TyKind::Tuple(params) = self.kind(tuple) else { panic!("call arguments are not a tuple") };
            let params = self.u.tcx.tys.list(params);
            let method = self.u.prog.traits[self.u.prog.def(selected.trait_def).sub as usize].iter().copied()
                .find(|d| self.u.prog.def(*d).kind == DefKind::AssocFn).expect("call trait method");
            let instance = crate::instance::Instance::expect_resolve(&self.u.tcx, &self.u.prog, crate::tcx::TypingEnv::fully_monomorphized(&self.u.prog), method, &[self.monomorphize(selected.self_ty), tuple]);
            {
                let once = matches!(self.u.tcx.lang_items.by_def.get(&selected.trait_def), Some(LangItem::FnOnce | LangItem::AsyncFnOnce));
                let mutable = matches!(self.u.tcx.lang_items.by_def.get(&selected.trait_def), Some(LangItem::FnMut | LangItem::AsyncFnMut));
                let (recv, recv_ty) = if once && selected.derefs == 0 { (self.lower_expr(f), ft) } else {
                    let (mut place, mut pt) = self.lower_place(f);
                    for k in 0..selected.derefs { (place, pt) = self.deref_place(e.0, k, place, pt); }
                    if once { (self.read_place(&place, pt), pt) } else {
                        let receiver = if self.unsized_ty(pt) {
                            match place { Place::Regs(regs) => Val::L(regs), _ => panic!("unsized callable metadata") }
                        } else { let address = self.place_addr(&place, pt); Val::L(vec![address]) };
                        (receiver, self.u.tcx.tys.intern(TyKind::Ref(mutable, pt, crate::types::RegionId::ERASED)))
                    }
                };
                let mut guards = Vec::new();
                let recv = self.guard_operand(args, recv, recv_ty, &mut guards);
                let tuple_addr = self.new_slot_for(tuple);
                let layout = self.layout(tuple);
                for (i, &arg) in args.iter().enumerate() {
                    let ty = params[i];
                    let value = self.lower_expr_coerced(arg, ty);
                    let value = self.guard_operand(&args[i + 1..], value, ty, &mut guards);
                    self.store_val(tuple_addr, layout.fields[i] as i32, ty, value);
                }
                self.unguard(&guards);
                return self.emit_instance(instance, vec![(recv, recv_ty), (Val::M(tuple_addr, 0), tuple)], t);
            }
        }
        // a callable reached as a place (behind references / smart pointers, or a `dyn Fn`
        // place however it was reached: `(*raw_ptr)(..)`, `(*boxed)(..)`) is called through
        // the place (rustc: the overloaded call takes the callee place by reference)
        let call_derefs = self.b.call_derefs(e.0).or_else(|| if matches!(self.kind_ref(ft), TyKind::Dyn(..)) { Some(0) } else { None });
        if let Some(n) = call_derefs {
            let (mut pl, mut pt) = self.lower_place(f);
            for k in 0..n {
                let (a, b) = self.deref_place(e.0, k, pl, pt);
                pl = a;
                pt = b;
            }
            // a fn item behind references (`identity: &F` with F = `Vec::new` / `T::default`):
            // zero-sized, the call is the item's direct call (rustc: `<&F as Fn>::call` forwards
            // to F's)
            if let TyKind::FnDef(..) = self.kind(pt) {
                ft = pt;
            } else {
                return match self.kind(pt) {
                    TyKind::FnPtr(ps, _, c, _) => {
                        let fv = self.read_place(&pl, pt);
                        let fv = self.to_leaves(fv, pt);
                        let mut vals = Vec::new();
                        let mut guards = Vec::new();
                        for (i, a) in args.iter().enumerate() {
                            let p = self.u.tcx.tys.list(ps).get(i).copied().unwrap_or_else(|| self.body_ty(*a));
                            let v = self.lower_expr_coerced(*a, p);
                            let v = self.guard_operand(&args[i + 1..], v, p, &mut guards);
                            vals.push((v, p));
                        }
                        self.unguard(&guards);
                        let callee = self.fn_ptr_callee(fv[0], self.u.tcx.tys.list(ps), t, c, &mut vals);
                        self.emit_call(callee, vals, t)
                    }
                    _ => {
                        self.err(self.pos, "unsupported call through a pointer".to_string());
                        self.unit()
                    }
                };
            }
        }
        match self.kind_ref(ft) {
            // a tuple constructor (a path, or a fn item value of it): the ADT built in place
            TyKind::FnDef(d0, gargs0, _) if matches!(self.u.prog.def(*d0).kind, DefKind::Variant | DefKind::Struct) => {
                let k = self.u.prog.def(*d0).kind;
                // the constructor's signature instantiated and normalized like any fn's (rustc
                // FnSig of a ctor = its field types, normalized at the call): a field declared
                // through a projection (`struct W<P: Pattern>(P::Searcher)`) is its concrete type
                let ps: Vec<TyId> = match self.u.tcx.ctor_sigs.get(&d0) {
                    Some(s) => s.params.iter().map(|p| self.u.tcx.normalize_erasing_late_bound_regions(&self.u.prog, crate::tcx::TypingEnv::fully_monomorphized(&self.u.prog), self.u.tcx.tys.subst(*p, self.u.tcx.tys.list(*gargs0)))).collect(),
                    None => Vec::new(),
                };
                // (the callee value is zero-sized; an expression producing it still runs)
                if !matches!(self.ast().expr(strip_paren(self.ast(), f)).kind, ExprKind::Path(_)) {
                    let _ = self.lower_expr(f);
                }
                let mut vals = Vec::new();
                let mut guards = Vec::new();
                for (i, a) in args.iter().enumerate() {
                    let pt = ps.get(i).copied().unwrap_or_else(|| self.body_ty(*a));
                    let v = self.lower_expr_coerced(*a, pt);
                    let v = self.guard_operand(&args[i + 1..], v, pt, &mut guards);
                    vals.push((v, pt));
                }
                self.unguard(&guards);
                let vi = if k == DefKind::Variant { self.u.prog.def(*d0).sub } else { 0 };
                self.build_enum(t, vi, vals)
            }
            TyKind::FnDef(d0, gargs0, _) => {
                let instance = crate::instance::Instance::expect_resolve(&self.u.tcx, &self.u.prog, crate::tcx::TypingEnv::fully_monomorphized(&self.u.prog), *d0, self.u.tcx.tys.list(*gargs0));
                let gargs = self.u.tcx.tys.list(*gargs0);
                let sig = self.u.tcx.fn_item_sig(&self.u.prog, *d0).cloned();
                let sig = match sig {
                    Some(s) => s,
                    None => return self.unit(),
                };
                let mut vals = Vec::new();
                let mut guards = Vec::new();
                for (i, a) in args.iter().enumerate() {
                    let pt = if i < sig.params.len() {
                        { let x = self.u.tcx.tys.subst_erasing_regions(sig.params[i], gargs); let u = self.u; u.tcx.normalize_erasing_late_bound_regions(&u.prog, crate::tcx::TypingEnv::fully_monomorphized(&u.prog), x) }
                    } else {
                        self.body_ty(*a)
                    };
                    let v = self.lower_expr_coerced(*a, pt);
                    let v = self.guard_operand(&args[i + 1..], v, pt, &mut guards);
                    vals.push((v, pt));
                }
                // (the operands are consumed by the call below: no exit can drop them again)
                self.unguard(&guards);
                self.emit_instance(instance, vals, t)
            }

            TyKind::FnPtr(ps, _, c, _) => {
                let fv = self.lower_expr(f);
                let fv = self.to_leaves(fv, ft);
                let mut vals = Vec::new();
                let mut guards = Vec::new();
                for (i, a) in args.iter().enumerate() {
                    let pt = self.u.tcx.tys.list(*ps).get(i).copied().unwrap_or_else(|| self.body_ty(*a));
                    let v = self.lower_expr_coerced(*a, pt);
                    let v = self.guard_operand(&args[i + 1..], v, pt, &mut guards);
                    vals.push((v, pt));
                }
                self.unguard(&guards);
                let callee = self.fn_ptr_callee(fv[0], self.u.tcx.tys.list(*ps), t, *c, &mut vals);
                self.emit_call(callee, vals, t)
            }
            _ => {
                self.err(self.pos, format!("unsupported call of {} (selected {:?})", crate::typeck::ty_to_string(false, &self.u.prog, &self.u.tcx, ft), self.b.call_traits.get(&e.0).map(|s| s.trait_def)));
                self.unit()
            }
        }
    }

    /// The position of a method call's name (`f` in `a.f(x)`).
    fn method_name_pos(&self, e: ExprId) -> u32 {
        match &self.ast().expr(e).kind {
            ExprKind::MethodCall(_, row) => self.ast().row(*row).name.lo,
            _ => self.pos,
        }
    }

    fn lower_method(&mut self, e: ExprId, recv: ExprId, args: &[ExprId], t: TyId) -> Val {
        let m = match self.b.methods.get(&e.0) {
            Some(&m) => self.mono_method(m),
            None => return self.unit(),
        };
        let instance = crate::instance::Instance::expect_resolve(&self.u.tcx, &self.u.prog, crate::tcx::TypingEnv::fully_monomorphized(&self.u.prog), m.def, &m.args);
        let virt = matches!(instance.def, InstanceKind::Virtual(..));
        let sig = match self.u.tcx.fn_sig(&self.u.prog, m.def).cloned() {
            Some(s) => s,
            None => return self.unit(),
        };
        let self_pt = { let x = self.u.tcx.tys.subst_erasing_regions(sig.params[0], &m.args); let u = self.u; u.tcx.normalize_erasing_late_bound_regions(&u.prog, crate::tcx::TypingEnv::fully_monomorphized(&u.prog), x) };
        // receiver with derefs, then autoref
        let rt = self.body_ty(recv);
        let recv_val = if m.autoref != 0 {
            // `make().method()`: lower_place makes the rvalue a statement temporary
            // a `&mut self` receiver makes the whole place mutable: overloaded derefs/indexes
            // in it are DerefMut/IndexMut (`(*v).push(x)`; rustc convert_place_derefs_to_mutable)
            let saved = self.want_mut;
            self.want_mut = m.autoref == 2;
            // a shared autoref of a promotable rvalue borrows a promoted static (rustc
            // promote_consts: the receiver's `&tmp` is a borrow like `&[A, B]`, so
            // `[A, B].iter()` may be returned)
            let promoted = if m.autoref == 1 && !self.is_place_expr(recv) && self.promotable(recv) { self.promote_rvalue(recv, true) } else { None };
            let (mut pl, mut pt) = match promoted {
                Some(p) => (Place::Mem(p, 0), rt),
                None => self.lower_place(recv),
            };
            for k in 0..m.derefs {
                let (a2, b2) = self.deref_place(e.0, k, pl, pt);
                pl = a2;
                pt = b2;
            }
            self.want_mut = saved;
            if self.unsized_ty(pt) {
                match pl {
                    Place::Regs(r) => Val::L(r),
                    _ => self.unit(),
                }
            } else if let TyKind::Array(_, n) = self.kind(pt) {
                let n = n.try_to_target_usize(&self.u.tcx.tys).expect("normalized array length");
                // array receiver for a slice method: unsize
                let a = self.place_addr(&pl, pt);
                let is_slice_self = matches!(self.kind_ref(self_pt), TyKind::Ref(_, x, _) if matches!(self.kind_ref(*x), TyKind::Slice(_)));
                if is_slice_self {
                    let l = self.iconst(n as i64);
                    Val::L(vec![a, l])
                } else {
                    Val::L(vec![a])
                }
            } else {
                let a = self.place_addr(&pl, pt);
                Val::L(vec![a])
            }
        } else {
            // `self: Box<Self>` / `Rc<Self>` on a trait object: the method was found at `dyn Tr`
            // behind the pointer, but the receiver is the pointer itself, moved into the call
            // (rustc: the receiver type Box<dyn Tr> matches `Box<Self>` at the first step)
            // (the probe also matches it at the pointer step itself, `Box<Box<dyn Tr>>` with
            // derefs = 1 when the receiver is the inner box: then the steps go to the pointer)
            let ptr_self = virt && m.derefs == 1 && matches!(self.kind(self_pt), TyKind::Adt(..)) && rt == self_pt;
            if m.derefs == 0 || ptr_self {
                self.lower_expr(recv)
            } else {
                // `guard.unwrap()` through Deref: the receiver is borrowed, not moved (a
                // temporary guard is dropped at the end of the statement)
                let (mut pl, _) = self.lower_place(recv);
                let mut cur = rt;
                let mut through_boxes = true;
                for k in 0..m.derefs {
                    through_boxes &= self.box_inner(cur).is_some();
                    let (a2, b2) = self.deref_place(e.0, k, pl, cur);
                    pl = a2;
                    cur = b2;
                }
                // by value through Boxes only (`self: Box<Self>` found under more boxes): the
                // value is moved out of the boxes, which are freed without it
                if through_boxes && self.needs_drop_glue(cur) {
                    if let Some((li, mut path)) = self.field_move_path(recv) {
                        for _ in 0..m.derefs {
                            path.push((BOX_STEP, 0));
                        }
                        self.moved_field(li, &path);
                    }
                }
                self.read_place(&pl, cur)
            }
        };
        let mut guards = Vec::new();
        let recv_val = self.guard_operand(args, recv_val, self_pt, &mut guards);
        let mut vals = vec![(recv_val, self_pt)];
        for (i, a) in args.iter().enumerate() {
            let pt = if i + 1 < sig.params.len() {
                { let x = self.u.tcx.tys.subst_erasing_regions(sig.params[i + 1], &m.args); let u = self.u; u.tcx.normalize_erasing_late_bound_regions(&u.prog, crate::tcx::TypingEnv::fully_monomorphized(&u.prog), x) }
            } else {
                self.body_ty(*a)
            };
            let v = self.lower_expr_coerced(*a, pt);
            let v = self.guard_operand(&args[i + 1..], v, pt, &mut guards);
            vals.push((v, pt));
        }
        self.unguard(&guards);
        self.pos = self.method_name_pos(e);
        self.emit_instance(instance, vals, t)
    }

    /// Calls to `extern "relax-intrinsic"` functions in Relax's core become instructions.
    /// Memory intrinsics of core::mem::intrinsics_mem (generic over T = gargs[0]).
    /// `core::intrinsics_atomic::*`. Orderings are `Ordering as u8` (0 Relaxed, 1 Release,
    /// 2 Acquire, 3 AcqRel, 4 SeqCst); one that is not a constant here becomes SeqCst.
    fn atomic_intrinsic(&mut self, name: &str, gargs: &[TyId], vals: &[(Val, TyId)]) -> Val {
        use relax_backend::rir::{AtomOrd, RmwOp};
        let mut a: Vec<VReg> = Vec::new();
        for (v, ty) in vals {
            let r = self.to_leaves(v.clone(), *ty);
            a.extend(r);
        }
        let ord_at = |s: &Self, k: usize| -> AtomOrd {
            let v = match a.get(k) {
                Some(v) => *v,
                None => return AtomOrd::SeqCst,
            };
            let mut c = None;
            for i in &s.f.blocks[s.cur as usize].insts {
                if let Inst::Iconst(d, x) = i {
                    if *d == v {
                        c = Some(*x);
                    }
                }
            }
            match c {
                Some(0) => AtomOrd::Relaxed,
                Some(1) => AtomOrd::Release,
                Some(2) => AtomOrd::Acquire,
                Some(3) => AtomOrd::AcqRel,
                _ => AtomOrd::SeqCst,
            }
        };
        let m = match gargs.first() {
            Some(g) => {
                let g = *g;
                match self.kind(g) {
                    TyKind::Int(_) | TyKind::Bool | TyKind::Char => {
                        let it = self.int_ty(g);
                        Mem::Int(it.bits / 8, it.signed)
                    }
                    _ => Mem::Int(8, false),
                }
            }
            None => Mem::Int(8, false),
        };
        let rmw = |n: &str| -> Option<RmwOp> {
            Some(match n {
                "atomic_swap" => RmwOp::Xchg,
                "atomic_fetch_add" => RmwOp::Add,
                "atomic_fetch_sub" => RmwOp::Sub,
                "atomic_fetch_and" => RmwOp::And,
                "atomic_fetch_nand" => RmwOp::Nand,
                "atomic_fetch_or" => RmwOp::Or,
                "atomic_fetch_xor" => RmwOp::Xor,
                "atomic_fetch_max" => RmwOp::Max,
                "atomic_fetch_min" => RmwOp::Min,
                _ => return None,
            })
        };
        match name {
            "atomic_load" => {
                let o = ord_at(self, 1);
                let d = self.f.vreg(Cls::I);
                self.emit(Inst::AtomicLoad(m, o, d, a[0]));
                Val::L(vec![d])
            }
            "atomic_store" => {
                let o = ord_at(self, 2);
                self.emit(Inst::AtomicStore(m, o, a[0], a[1]));
                self.unit()
            }
            "atomic_cxchg" | "atomic_cxchg_weak" => {
                let ok = ord_at(self, 3);
                let fail = ord_at(self, 4);
                let d = self.f.vreg(Cls::I);
                self.emit(Inst::AtomicCas(m, ok, fail, d, a[0], a[1], a[2]));
                let b = self.f.vreg(Cls::I);
                self.emit(Inst::ICmp(Cond::Eq, false, b, d, a[1]));
                Val::L(vec![d, b])
            }
            "atomic_fence" | "atomic_compiler_fence" => {
                let o = ord_at(self, 0);
                self.emit(Inst::Fence(o, name == "atomic_compiler_fence"));
                self.unit()
            }
            "spin_loop_hint" => self.unit(),
            n => {
                let mut op = match rmw(n) {
                    Some(op) => op,
                    None => {
                        self.err(self.pos, format!("unknown atomic intrinsic `{}`", n));
                        return self.unit();
                    }
                };
                if let Mem::Int(_, false) = m {
                    op = match op {
                        RmwOp::Max => RmwOp::UMax,
                        RmwOp::Min => RmwOp::UMin,
                        o => o,
                    };
                }
                let o = ord_at(self, 2);
                let d = self.f.vreg(Cls::I);
                self.emit(Inst::AtomicRmw(op, m, o, d, a[0], a[1]));
                Val::L(vec![d])
            }
        }
    }

    fn mem_intrinsic(&mut self, name: &str, gargs: &[TyId], vals: &[(Val, TyId)], t: TyId) -> Option<Val> {
        let g0 = gargs.first().copied();
        let elem = |s: &mut Self| -> (u32, u32) {
            match g0 {
                Some(g) => {
                    let l = s.layout(g);
                    (l.size, l.align)
                }
                None => (0, 1),
            }
        };
        let leaf = |s: &mut Self, k: usize| -> Vec<VReg> {
            let (v, ty) = vals[k].clone();
            s.to_leaves(v, ty)
        };
        Some(match name {
            "size_of" => {
                let (sz, al) = elem(self);
                Val::L(vec![self.iconst(crate::layout::round_up(sz, al) as i64)])
            }
            "align_of" => {
                let (_, al) = elem(self);
                Val::L(vec![self.iconst(al as i64)])
            }
            // `T: Copy` of this instance (std's Copy specializations: memcpy paths)
            "is_copy" => {
                let c = match g0 {
                    Some(g) => self.u.is_copy(g),
                    None => false,
                };
                Val::L(vec![self.iconst(c as i64)])
            }
            // `T: Freeze` of this instance (no UnsafeCell inline: std's sort keeps copies)
            "is_freeze" => {
                let c = match g0 {
                    Some(g) => self.u.tcx.is_freeze(&self.u.prog, crate::tcx::TypingEnv::fully_monomorphized(&self.u.prog), g),
                    None => false,
                };
                Val::L(vec![self.iconst(c as i64)])
            }
            "needs_drop" => {
                let nd = match g0 {
                    Some(g) => self.u.tcx.needs_drop(&self.u.prog, crate::tcx::TypingEnv::fully_monomorphized(&self.u.prog), g),
                    None => false,
                };
                Val::L(vec![self.iconst(nd as i64)])
            }
            "drop_in_place" => {
                let p = leaf(self, 0);
                if let Some(g) = g0 {
                    if let TyKind::Dyn(..) = self.kind(g) {
                        // through the vtable's drop entry (0 when the type has no drop)
                        let fp = self.f.vreg(Cls::I);
                        self.emit(Inst::Load(Mem::Int(8, false), fp, p[1], (VT_DROP * 8) as i32));
                        let yes = self.f.block();
                        let join = self.f.block();
                        self.term(Term::Branch(fp, yes, join));
                        self.switch(yes);
                        self.emit(Inst::Call(Callee::Indirect(fp), vec![p[0]], Vec::new()));
                        self.goto(join);
                        self.switch(join);
                    } else if p.len() == 2 && self.unsized_ty(g) {
                        // `*mut [T]` (or a struct with a slice tail): every element (rustc
                        // drop_in_place::<[T]> loops over the length)
                        self.drop_unsized(p[0], p[1], g);
                    } else if self.needs_drop_glue(g) {
                        // a call like any other: its unwind edge runs this frame's cleanup (the
                        // value dropped here is not part of it)
                        let id = self.u.fn_id(FnKey::Glue(crate::jit::GLUE_DROP, g));
                        self.emit(Inst::Call(Callee::Fn(id), vec![p[0]], Vec::new()));
                    }
                }
                self.unit()
            }
            "transmute" => {
                // reinterpret the bits: through memory, so any two same-size types work
                let (v, st) = vals[0].clone();
                let dt = match gargs.get(1) {
                    Some(d) => *d,
                    None => t,
                };
                let (ls, ld) = (self.layout(st), self.layout(dt));
                if ls.size != ld.size {
                    let (a, b) = (crate::typeck::ty_to_string(false, &self.u.prog, &self.u.tcx, st), crate::typeck::ty_to_string(false, &self.u.prog, &self.u.tcx, dt));
                    self.err(self.pos, format!("transmute between types of different sizes (`{}` {} bytes, `{}` {} bytes)", a, ls.size, b, ld.size));
                    return Some(self.dummy_val(dt));
                }
                let slot = {
                    let big = if ls.align >= ld.align { st } else { dt };
                    self.new_slot_for(big)
                };
                self.store_val(slot, 0, st, v);
                self.load_val(slot, 0, dt)
            }
            "size_of_val" | "align_of_val" => {
                // of the value behind a (possibly fat) pointer
                let p = leaf(self, 0);
                let g = match g0 {
                    Some(g) => g,
                    None => return Some(self.unit()),
                };
                let size = name == "size_of_val";
                let r = match self.kind(g) {
                    TyKind::Dyn(..) => {
                        let d = self.f.vreg(Cls::I);
                        let w = if size { VT_SIZE } else { VT_ALIGN };
                        self.emit(Inst::Load(Mem::Int(8, false), d, p[1], (w * 8) as i32));
                        d
                    }
                    TyKind::Slice(e) if size => {
                        let el = self.layout(e);
                        let k = self.iconst(crate::layout::round_up(el.size, el.align) as i64);
                        let d = self.f.vreg(Cls::I);
                        self.emit(Inst::IBin(IOp::Mul, U64, d, p[1], k));
                        d
                    }
                    TyKind::Str if size => p[1],
                    TyKind::Slice(e) => {
                        let el = self.layout(e);
                        self.iconst(el.align as i64)
                    }
                    TyKind::Str => self.iconst(1),
                    // a struct / tuple with an unsized tail (CStr { inner: [c_char] }, Path,
                    // Fat<dyn Tr>): the sized prefix plus the tail, aligned (rustc
                    // size_and_align_of_dst)
                    _ if self.unsized_ty(g) && p.len() == 2 => {
                        let tail = {
                            let u = self.u;
                            u.lay.unsized_tail(&crate::layout::LayoutCx::new(&u.prog), &u.tcx, g)
                        };
                        let l = self.layout(g);
                        match tail.map(|(o, tt)| (o, tt, self.kind(tt))) {
                            Some((off, _, TyKind::Slice(e))) => {
                                let el = self.layout(e);
                                let stride = crate::layout::round_up(el.size, el.align) as i64;
                                if size {
                                    let n = self.f.vreg(Cls::I);
                                    self.emit(Inst::IBinI(IOp::Mul, U64, n, p[1], stride));
                                    let e2 = self.f.vreg(Cls::I);
                                    self.emit(Inst::IBinI(IOp::Add, U64, e2, n, off as i64 + l.align as i64 - 1));
                                    let r = self.f.vreg(Cls::I);
                                    self.emit(Inst::IBinI(IOp::And, U64, r, e2, -(l.align as i64)));
                                    r
                                } else {
                                    self.iconst(l.align as i64)
                                }
                            }
                            Some((off, _, TyKind::Str)) => {
                                if size {
                                    let e2 = self.f.vreg(Cls::I);
                                    self.emit(Inst::IBinI(IOp::Add, U64, e2, p[1], off as i64 + l.align as i64 - 1));
                                    let r = self.f.vreg(Cls::I);
                                    self.emit(Inst::IBinI(IOp::And, U64, r, e2, -(l.align as i64)));
                                    r
                                } else {
                                    self.iconst(l.align as i64)
                                }
                            }
                            Some((_, _, TyKind::Dyn(..))) => {
                                // the tail at the end of the sized fields rounded up to the dynamic
                                // type's alignment; the whole rounded up to the larger alignment
                                let pa = self.sized_prefix_align(g) as i64;
                                let unaligned = {
                                    let mut m = 0u32;
                                    if let TyKind::Adt(d, args, _) = self.kind(g) {
                                        if let Some(adt) = self.u.tcx.adts.get(&d) {
                                            let fs = adt.variants[0].fields.clone();
                                            for j in 0..fs.len().saturating_sub(1) {
                                                let ft = self.u.tcx.field_ty(&self.u.prog, crate::tcx::TypingEnv::fully_monomorphized(&self.u.prog), d, 0, j as u32, self.u.tcx.tys.list(args));
                                                let sz = self.layout(ft).size;
                                                m = m.max(l.fields.get(j).copied().unwrap_or(0) + sz);
                                            }
                                        }
                                    }
                                    m as i64
                                };
                                let vs = self.f.vreg(Cls::I);
                                self.emit(Inst::Load(Mem::Int(8, false), vs, p[1], (VT_SIZE * 8) as i32));
                                let va0 = self.f.vreg(Cls::I);
                                self.emit(Inst::Load(Mem::Int(8, false), va0, p[1], (VT_ALIGN * 8) as i32));
                                // `repr(packed(n))`: the tail's alignment is capped at n (rustc:
                                // packed structs clamp every field's alignment, the dynamic one too)
                                let packed = match self.kind(g) {
                                    TyKind::Adt(d, _, _) => self.u.tcx.adts.get(&d).map_or(0, |a| a.repr_packed),
                                    _ => 0,
                                };
                                let va = if packed > 0 {
                                    let pn = self.iconst(packed as i64);
                                    let c0 = self.f.vreg(Cls::I);
                                    self.emit(Inst::ICmp(Cond::Lt, false, c0, va0, pn));
                                    let v = self.f.vreg(Cls::I);
                                    self.emit(Inst::Sel(v, c0, va0, pn));
                                    v
                                } else {
                                    va0
                                };
                                let pa = if packed > 0 { pa.min(packed as i64) } else { pa };
                                let pk = self.iconst(pa);
                                let c = self.f.vreg(Cls::I);
                                self.emit(Inst::ICmp(Cond::Lt, false, c, va, pk));
                                let al = self.f.vreg(Cls::I);
                                self.emit(Inst::Sel(al, c, pk, va));
                                if size {
                                    // offset = align_to(unaligned, va); size = align_to(offset + vs, al)
                                    let am1 = self.f.vreg(Cls::I);
                                    self.emit(Inst::IBinI(IOp::Sub, U64, am1, va, 1));
                                    let t1 = self.f.vreg(Cls::I);
                                    self.emit(Inst::IBinI(IOp::Add, U64, t1, am1, unaligned));
                                    let nva = self.f.vreg(Cls::I);
                                    let z = self.iconst(0);
                                    self.emit(Inst::IBin(IOp::Sub, U64, nva, z, va));
                                    let off = self.f.vreg(Cls::I);
                                    self.emit(Inst::IBin(IOp::And, U64, off, t1, nva));
                                    let end = self.f.vreg(Cls::I);
                                    self.emit(Inst::IBin(IOp::Add, U64, end, off, vs));
                                    let alm1 = self.f.vreg(Cls::I);
                                    self.emit(Inst::IBinI(IOp::Sub, U64, alm1, al, 1));
                                    let t2 = self.f.vreg(Cls::I);
                                    self.emit(Inst::IBin(IOp::Add, U64, t2, end, alm1));
                                    let nal = self.f.vreg(Cls::I);
                                    self.emit(Inst::IBin(IOp::Sub, U64, nal, z, al));
                                    let r = self.f.vreg(Cls::I);
                                    self.emit(Inst::IBin(IOp::And, U64, r, t2, nal));
                                    r
                                } else {
                                    al
                                }
                            }
                            _ => self.iconst(if size { crate::layout::round_up(l.size, l.align) } else { l.align } as i64),
                        }
                    }
                    _ => {
                        let l = self.layout(g);
                        self.iconst(if size { crate::layout::round_up(l.size, l.align) } else { l.align } as i64)
                    }
                };
                Val::L(vec![r])
            }
            "ptr_read" => {
                let p = leaf(self, 0);
                let g = g0?;
                self.read_place(&Place::Mem(p[0], 0), g)
            }
            "ptr_write" => {
                let p = leaf(self, 0);
                let g = g0?;
                let v = vals[1].0.clone();
                self.store_val(p[0], 0, g, v);
                self.unit()
            }
            // volatile accesses (rustc volatile_load / volatile_store: LLVM `load volatile`):
            // scalars as AtomicLoad / AtomicStore with AtomOrd::Volatile, which no pass removes,
            // merges, forwards or reorders. A larger value: a volatile access of its first byte
            // keeps the access (and its fault) in place, the value moves as an ordinary read/write.
            "volatile_load" | "volatile_store" => {
                use relax_backend::rir::AtomOrd;
                let p = leaf(self, 0);
                let g = g0?;
                let size = self.layout(g).size;
                let kind = self.kind(g);
                let scalar = match kind {
                    TyKind::Int(_) | TyKind::Bool | TyKind::Char => Some((Mem::Int(self.int_ty(g).bits / 8, self.int_ty(g).signed), None)),
                    TyKind::Ptr(..) | TyKind::FnPtr(..) | TyKind::Ref(..) if size == 8 => Some((Mem::Int(8, false), None)),
                    TyKind::Float(_) if size == 4 || size == 8 => Some((Mem::Int(size as u8, false), Some(size == 8))),
                    _ => None,
                };
                if name == "volatile_load" {
                    match scalar {
                        Some((m, float)) => {
                            let d = self.f.vreg(Cls::I);
                            self.emit(Inst::AtomicLoad(m, AtomOrd::Volatile, d, p[0]));
                            match float {
                                Some(f64_) => {
                                    let x = self.f.vreg(if f64_ { Cls::F64 } else { Cls::F32 });
                                    self.emit(Inst::Conv(Conv::BitsToF(f64_), x, d));
                                    Val::L(vec![x])
                                }
                                None => Val::L(vec![d]),
                            }
                        }
                        None => {
                            if size > 0 {
                                let d = self.f.vreg(Cls::I);
                                self.emit(Inst::AtomicLoad(Mem::Int(1, false), AtomOrd::Volatile, d, p[0]));
                            }
                            self.read_place(&Place::Mem(p[0], 0), g)
                        }
                    }
                } else {
                    let v = vals[1].0.clone();
                    match scalar {
                        Some((m, float)) => {
                            let r = self.to_leaves(v, g);
                            let w = match float {
                                Some(f64_) => {
                                    let b = self.f.vreg(Cls::I);
                                    self.emit(Inst::Conv(Conv::FToBits(f64_), b, r[0]));
                                    b
                                }
                                None => r[0],
                            };
                            self.emit(Inst::AtomicStore(m, AtomOrd::Volatile, p[0], w));
                        }
                        None => {
                            if size > 0 {
                                let d = self.f.vreg(Cls::I);
                                self.emit(Inst::AtomicLoad(Mem::Int(1, false), AtomOrd::Volatile, d, p[0]));
                            }
                            self.store_val(p[0], 0, g, v);
                        }
                    }
                    self.unit()
                }
            }
            "ptr_add" | "ptr_add_mut" => {
                let p = leaf(self, 0);
                let n = leaf(self, 1);
                let (sz, al) = elem(self);
                let stride = crate::layout::round_up(sz, al) as i64;
                let off = self.f.vreg(Cls::I);
                self.emit(Inst::IBinI(IOp::Mul, U64, off, n[0], stride));
                let d = self.f.vreg(Cls::I);
                self.emit(Inst::IBin(IOp::Add, U64, d, p[0], off));
                Val::L(vec![d])
            }
            "slice_from_raw" | "slice_from_raw_mut" | "str_from_raw" => {
                let p = leaf(self, 0);
                let n = leaf(self, 1);
                Val::L(vec![p[0], n[0]])
            }
            "slice_ptr" => {
                let s2 = leaf(self, 0);
                Val::L(vec![s2[0]])
            }
            "null_mut" => Val::L(vec![self.iconst(0)]),
            "forget" => self.unit(),
            "copy_nonoverlapping" => {
                // byte copy loop: n * size bytes
                let src = leaf(self, 0);
                let dst = leaf(self, 1);
                let n = leaf(self, 2);
                let (sz, al) = elem(self);
                let stride = crate::layout::round_up(sz, al) as i64;
                let bytes = self.f.vreg(Cls::I);
                self.emit(Inst::IBinI(IOp::Mul, U64, bytes, n[0], stride));
                let rt = self.u.rs.memcpy;
                self.emit(Inst::Call(Callee::Host(rt), vec![dst[0], src[0], bytes], Vec::new()));
                self.unit()
            }
            "copy" | "write_bytes" => {
                // memmove(dst, src, n * size) / memset(dst, b, n * size)
                let (sz, al) = elem(self);
                let stride = crate::layout::round_up(sz, al) as i64;
                let n = leaf(self, 2);
                let bytes = self.f.vreg(Cls::I);
                self.emit(Inst::IBinI(IOp::Mul, U64, bytes, n[0], stride));
                if name == "copy" {
                    let src = leaf(self, 0);
                    let dst = leaf(self, 1);
                    let rt = self.u.rs.memmove;
                    self.emit(Inst::Call(Callee::Host(rt), vec![dst[0], src[0], bytes], Vec::new()));
                } else {
                    let dst = leaf(self, 0);
                    let b = leaf(self, 1);
                    let rt = self.u.rs.memset;
                    self.emit(Inst::Call(Callee::Host(rt), vec![dst[0], b[0], bytes], Vec::new()));
                }
                self.unit()
            }
            "zeroed" => {
                let g = g0?;
                let l = self.layout(g);
                let slot = self.new_slot_for(g);
                let z = self.iconst(0);
                let n = self.iconst(l.size as i64);
                let rt = self.u.rs.memset;
                self.emit(Inst::Call(Callee::Host(rt), vec![slot, z, n], Vec::new()));
                self.load_val(slot, 0, g)
            }
            "black_box" => vals[0].0.clone(),
            "type_name" => {
                let s = match g0 {
                    Some(g) => crate::typeck::type_name(&self.u.prog, &self.u.tcx, g),
                    None => String::new(),
                };
                // one copy per content (rustc interns the const-evaluated string's allocation)
                let addr = self.u.lit_data(s.as_bytes());
                let addr = self.u.data_sym(addr);
                let p = self.f.vreg(Cls::I);
                self.emit(Inst::Addr(p, addr));
                let l = self.iconst(s.len() as i64);
                Val::L(vec![p, l])
            }
            "type_id" | "type_id_hi" => {
                // a stable 128-bit hash of the canonical type (stable.rs), independent of TyId
                // numbering, so cached code and parallel interning agree (relax-cache.md C0)
                let h = g0.map_or(0, |g| crate::stable::type_hash(&self.u.prog, &self.u.tcx, g));
                let v = if name == "type_id" { h as u64 } else { (h >> 64) as u64 };
                Val::L(vec![self.iconst(v as i64)])
            }
            "discriminant_value" => {
                let p = leaf(self, 0);
                let g = g0?;
                let l = self.layout(g);
                // the tag holds the discriminant itself
                let _ = &l;
                match self.load_disc(p[0], 0, g) {
                    Some(d) if matches!(self.kind(g), TyKind::Adt(..)) => Val::L(d),
                    _ => Val::L(vec![self.iconst(0)]),
                }
            }
            _ => {
                let _ = t;
                return None;
            }
        })
    }

    /// Materialize an aggregate only when it is currently held as scalar leaves.
    fn vx_operand(&mut self, v: Val, ty: TyId) -> VReg {
        if self.layout(ty).size != 16 {
            self.err(self.pos, "SIMD operands must be exactly 16 bytes".into());
            let r = self.f.vreg(Cls::V128);
            self.emit(Inst::VConst(r, 0));
            return r;
        }
        let (base, off) = match v {
            Val::M(base, off) => (base, off),
            v => {
                let base = self.new_slot_for(ty);
                self.store_val(base, 0, ty, v);
                (base, 0)
            }
        };
        let r = self.f.vreg(Cls::V128);
        self.emit(Inst::VLoad(r, base, off));
        r
    }

    fn vx_intrinsic(&mut self, name: &str, vals: &[(Val, TyId)], t: TyId) -> Val {
        let arity = match name {
            "vx_binary" | "vx_unary" | "vx_ternary" => 4,
            "vx_splat" | "vx_store" | "vx_narrow" | "vx_table" => 2,
            "vx_load" | "vx_mask" | "vx_sha1h" => 1,
            "vx_blend" | "vx_sha1rnds4" => 3,
            _ => {
                self.err(self.pos, "unknown SIMD intrinsic".into());
                return self.dummy_val(t);
            }
        };
        if vals.len() != arity {
            self.err(self.pos, "invalid SIMD intrinsic argument count".into());
            return self.dummy_val(t);
        }
        if name == "vx_narrow" && self.layout(t).size != 8 {
            self.err(self.pos, "SIMD narrowing result must be exactly 8 bytes".into());
            return self.dummy_val(t);
        }
        let scalar = |s: &mut Self, k: usize| s.to_leaves(vals[k].0.clone(), vals[k].1)[0];
        // the selector's value when the current block defines it from constants: an Iconst,
        // copies/int casts of one, or and/or/xor/shift/add/sub with an immediate or another
        // constant (`MASK & 0x0f` of a const generic)
        fn const_of(insts: &[Inst], upto: usize, r: VReg) -> Option<i64> {
            for (at, i) in insts[..upto].iter().enumerate().rev() {
                match i {
                    Inst::Iconst(d, x) if *d == r => return Some(*x),
                    Inst::Mov(d, a) | Inst::Conv(Conv::IntToInt(_), d, a) if *d == r => return const_of(insts, at, *a),
                    Inst::IBinI(op, _, d, a, imm) if *d == r => {
                        let x = const_of(insts, at, *a)?;
                        return match op {
                            IOp::And => Some(x & imm),
                            IOp::Or => Some(x | imm),
                            IOp::Xor => Some(x ^ imm),
                            IOp::Shl => Some(x.wrapping_shl(*imm as u32)),
                            IOp::Shr => Some(((x as u64) >> (*imm as u32 & 63)) as i64),
                            IOp::Add => Some(x.wrapping_add(*imm)),
                            IOp::Sub => Some(x.wrapping_sub(*imm)),
                            _ => None,
                        };
                    }
                    Inst::IBin(op, _, d, a, b) if *d == r => {
                        let (x, y) = (const_of(insts, at, *a)?, const_of(insts, at, *b)?);
                        return match op {
                            IOp::And => Some(x & y),
                            IOp::Or => Some(x | y),
                            IOp::Xor => Some(x ^ y),
                            IOp::Shl => Some(x.wrapping_shl(y as u32)),
                            IOp::Shr => Some(((x as u64) >> (y as u32 & 63)) as i64),
                            IOp::Add => Some(x.wrapping_add(y)),
                            IOp::Sub => Some(x.wrapping_sub(y)),
                            _ => None,
                        };
                    }
                    _ => {}
                }
            }
            None
        }
        let constant = |s: &mut Self, k: usize| -> i64 {
            let r = scalar(s, k);
            let insts = &s.f.blocks[s.cur as usize].insts;
            if let Some(x) = const_of(insts, insts.len(), r) {
                return x;
            }
            s.err(s.pos, "SIMD selector must be a compile-time constant".into());
            0
        };
        let vector = |s: &mut Self, k: usize| s.vx_operand(vals[k].0.clone(), vals[k].1);
        let d = self.f.vreg(Cls::V128);
        if name == "vx_sha1rnds4" {
            let func = constant(self, 2);
            if !(0..=3).contains(&func) {
                self.err(self.pos, "SHA1RNDS4 selector must be in 0..=3".into());
                return self.dummy_val(t);
            }
            let a = vector(self, 0);
            let b = vector(self, 1);
            self.emit(Inst::VX(VX::X86Sha1Rnds4(func as u8), d, a, b));
        } else if name == "vx_sha1h" {
            let x = scalar(self, 0);
            let a = self.f.vreg(Cls::V128);
            let r = self.f.vreg(Cls::I);
            self.emit(Inst::VX(VX::FromLo64, a, x, x));
            self.emit(Inst::VX(VX::ArmSha1H, d, a, a));
            self.emit(Inst::VX(VX::Extract(VW::S, 0), r, d, d));
            return Val::L(vec![r]);
        } else if name == "vx_ternary" {
            let selector = constant(self, 3);
            if selector == 8 || selector == 9 {
                // NEON FMLA / FMLS .4s: a + b * c, a - b * c, fused (one rounding, the
                // instruction's own NaN rules).
                let a = vector(self, 0);
                let b = vector(self, 1);
                let c = vector(self, 2);
                self.emit(Inst::VFma(selector == 9, false, d, b, c, a));
                return self.vx_result(d, t);
            }
            let op = match selector {
                0 => VX::X86Sha256Rnds2,
                1 => VX::ArmSha1C,
                2 => VX::ArmSha1P,
                3 => VX::ArmSha1M,
                4 => VX::ArmSha1Su0,
                5 => VX::ArmSha256H,
                6 => VX::ArmSha256H2,
                7 => VX::ArmSha256Su1,
                _ => {
                    self.err(self.pos, "invalid SIMD ternary selector".into());
                    return self.dummy_val(t);
                }
            };
            let a = vector(self, 0);
            let b = if (1..=3).contains(&selector) {
                if self.layout(vals[1].1).size != 4 {
                    self.err(self.pos, "SHA1C/P/M hash_e must be a u32 scalar".into());
                    return self.dummy_val(t);
                }
                let x = scalar(self, 1);
                let v = self.f.vreg(Cls::V128);
                self.emit(Inst::VX(VX::FromLo64, v, x, x));
                v
            } else {
                vector(self, 1)
            };
            let c = vector(self, 2);
            self.emit(Inst::VX3(op, d, a, b, c));
        } else if name == "vx_load" {
            let p = scalar(self, 0);
            self.emit(Inst::VLoad(d, p, 0));
        } else if name == "vx_store" {
            let p = scalar(self, 0);
            let a = vector(self, 1);
            self.emit(Inst::VStore(p, 0, a));
            return Val::L(vec![]);
        } else if name == "vx_narrow" {
            let mut a = vector(self, 0);
            let op = constant(self, 1);
            if op == 30 {
                let k = self.iconst(255);
                let lim = self.f.vreg(Cls::V128);
                let clamped = self.f.vreg(Cls::V128);
                self.emit(Inst::VX(VX::Splat(VW::H), lim, k, k));
                self.emit(Inst::VX(VX::MinU(VW::H), clamped, a, lim));
                a = clamped;
            }
            let pack = match op {
                19 | 30 => VX::PackUsH,
                20 => VX::PackSsH,
                21 => VX::PackSsS,
                _ => {
                    self.err(self.pos, "invalid SIMD narrow selector".into());
                    VX::PackUsH
                }
            };
            self.emit(Inst::VX(pack, d, a, a));
            let lo = self.f.vreg(Cls::I);
            self.emit(Inst::VX(VX::Extract(VW::D, 0), lo, d, d));
            let base = self.new_slot_for(t);
            self.emit(Inst::Store(Mem::Int(8, false), base, 0, lo));
            return Val::M(base, 0);
        } else if name == "vx_table" {
            let a = vector(self, 0);
            let b = vector(self, 1);
            // pshufb wraps indices 16..127; NEON tbl must zero them.
            let k = self.iconst(16);
            let lim = self.f.vreg(Cls::V128);
            let clamped = self.f.vreg(Cls::V128);
            let bad = self.f.vreg(Cls::V128);
            let indices = self.f.vreg(Cls::V128);
            self.emit(Inst::VX(VX::Splat(VW::B), lim, k, k));
            self.emit(Inst::VX(VX::MinU(VW::B), clamped, b, lim));
            self.emit(Inst::VX(VX::CmpEq(VW::B), bad, clamped, lim));
            self.emit(Inst::VX(VX::Or, indices, b, bad));
            self.emit(Inst::VX(VX::ByteShuffle, d, a, indices));
        } else if name == "vx_mask" {
            let a = vector(self, 0);
            let r = self.f.vreg(Cls::I);
            self.emit(Inst::VX(VX::MoveMaskB, r, a, a));
            return Val::L(vec![r]);
        } else if name == "vx_blend" {
            let a = vector(self, 0);
            let b = vector(self, 1);
            let m = vector(self, 2);
            let z = self.f.vreg(Cls::V128);
            let mask = self.f.vreg(Cls::V128);
            self.emit(Inst::VConst(z, 0));
            self.emit(Inst::VX(VX::CmpGtS(VW::B), mask, z, m));
            self.emit(Inst::VSel(d, mask, b, a));
        } else {
            let wi = if name == "vx_binary" {
                3
            } else if name == "vx_splat" {
                1
            } else {
                2
            };
            let w = match constant(self, wi) {
                8 => VW::B,
                16 => VW::H,
                32 => VW::S,
                64 => VW::D,
                _ => {
                    self.err(self.pos, "invalid SIMD lane width".into());
                    VW::B
                }
            };
            if name == "vx_splat" {
                let a = scalar(self, 0);
                self.emit(Inst::VX(VX::Splat(w), d, a, a));
            } else {
                let op = constant(self, if name == "vx_binary" { 2 } else { 1 });
                // Reject combinations the backends cannot encode, rather than silently
                // emitting an unrelated instruction (notably mul/min/max .2d).
                let valid = if name == "vx_binary" {
                    match op {
                        0 | 1 | 6..=11 | 17 | 18 | 29 => true,
                        2..=5 | 16 => matches!(w, VW::B | VW::H),
                        12..=15 => w != VW::D,
                        19 | 20 | 23 => w == VW::H,
                        21 | 22 => w == VW::S,
                        24 | 25 | 27 => w == VW::B,
                        26 => matches!(w, VW::H | VW::S),
                        40..=53 | 60..=63 | 70..=74 | 80 | 81 => w == VW::S,
                        _ => false,
                    }
                } else {
                    match op {
                        0..=3 => true,
                        4 | 7 | 8 | 9 => w == VW::S,
                        5 | 6 => w == VW::B,
                        _ => false,
                    }
                };
                if !valid {
                    self.err(self.pos, "unsupported SIMD selector/lane width combination".into());
                    return self.dummy_val(t);
                }
                let a = vector(self, 0);
                if name == "vx_binary" {
                    let b = vector(self, 1);
                    let vx = match op {
                        0 => VX::Add(w),
                        1 => VX::Sub(w),
                        2 => VX::AddSatS(w),
                        3 => VX::AddSatU(w),
                        4 => VX::SubSatS(w),
                        5 => VX::SubSatU(w),
                        6 => VX::And,
                        7 => VX::Or,
                        8 => VX::Xor,
                        9 => VX::AndNot,
                        10 => VX::CmpEq(w),
                        11 => VX::CmpGtS(w),
                        12 => VX::MinS(w),
                        13 => VX::MinU(w),
                        14 => VX::MaxS(w),
                        15 => VX::MaxU(w),
                        16 => VX::AvgU(w),
                        17 => VX::ZipLo(w),
                        18 => VX::ZipHi(w),
                        19 => VX::PackUsH,
                        20 => VX::PackSsH,
                        21 => VX::PackSsS,
                        22 => VX::PackUsS,
                        23 => VX::MaddH,
                        24 => VX::SadU8,
                        25 => VX::ByteShuffle,
                        26 => VX::MulLo(w),
                        27 => VX::MaddUbs,
                        70 => VX::X86Sha1NextE,
                        71 => VX::X86Sha1Msg1,
                        72 => VX::X86Sha1Msg2,
                        73 => VX::X86Sha256Msg1,
                        74 => VX::X86Sha256Msg2,
                        80 => VX::ArmSha1Su1,
                        81 => VX::ArmSha256Su0,
                        29 => {
                            // Unsigned comparison = signed comparison after sign-bit bias.
                            let k = self.iconst((1u64 << (w.bits() - 1)) as i64);
                            let sign = self.f.vreg(Cls::V128);
                            let aa = self.f.vreg(Cls::V128);
                            let bb = self.f.vreg(Cls::V128);
                            self.emit(Inst::VX(VX::Splat(w), sign, k, k));
                            self.emit(Inst::VX(VX::Xor, aa, a, sign));
                            self.emit(Inst::VX(VX::Xor, bb, b, sign));
                            self.emit(Inst::VX(VX::CmpGtS(w), d, aa, bb));
                            return self.vx_result(d, t);
                        }
                        40..=53 | 60..=63 => {
                            let fop = match op {
                                40 => VX::FAdd(VW::S, NanBits::Intel),
                                60 => VX::FAdd(VW::S, NanBits::Neon),
                                41 => VX::FSub(VW::S, NanBits::Intel),
                                61 => VX::FSub(VW::S, NanBits::Neon),
                                42 => VX::FMul(VW::S, NanBits::Intel),
                                62 => VX::FMul(VW::S, NanBits::Neon),
                                43 => VX::FDiv(VW::S, NanBits::Intel),
                                63 => VX::FDiv(VW::S, NanBits::Neon),
                                44 => VX::FMinLt(VW::S),
                                45 => VX::FMaxGt(VW::S),
                                46 => VX::FCmp(Cond::Eq, VW::S),
                                47 => VX::FCmp(Cond::Ne, VW::S),
                                48 => VX::FCmp(Cond::Lt, VW::S),
                                49 => VX::FCmp(Cond::Le, VW::S),
                                50 => VX::FCmp(Cond::Gt, VW::S),
                                52 => VX::FMinNeon(VW::S),
                                53 => VX::FMaxNeon(VW::S),
                                _ => VX::FCmp(Cond::Ge, VW::S),
                            };
                            self.emit(Inst::VX(fop, d, a, b));
                            return self.vx_result(d, t);
                        }
                        _ => {
                            self.err(self.pos, "invalid SIMD binary selector".into());
                            VX::And
                        }
                    };
                    self.emit(Inst::VX(vx, d, a, b));
                } else {
                    let k = constant(self, 3).clamp(0, 255) as u8;
                    let vx = match op {
                        0 if w == VW::D => {
                            let z = self.f.vreg(Cls::V128);
                            let neg = self.f.vreg(Cls::V128);
                            let mask = self.f.vreg(Cls::V128);
                            self.emit(Inst::VConst(z, 0));
                            self.emit(Inst::VX(VX::Sub(w), neg, z, a));
                            self.emit(Inst::VX(VX::CmpGtS(w), mask, z, a));
                            self.emit(Inst::VSel(d, mask, neg, a));
                            return self.vx_result(d, t);
                        }
                        0 => VX::Abs(w),
                        1 => VX::ShlI(w, k),
                        2 => VX::ShrUI(w, k),
                        3 => VX::ShrSI(w, k),
                        4 => VX::ShufS(k),
                        5 => VX::BytesShrI(k),
                        6 => VX::BytesShlI(k),
                        7 | 9 => {
                            self.emit(Inst::VX(VX::FSqrt(VW::S, if op == 7 { NanBits::Intel } else { NanBits::Neon }), d, a, a));
                            return self.vx_result(d, t);
                        }
                        8 => {
                            self.emit(Inst::VX(VX::FAbs(VW::S), d, a, a));
                            return self.vx_result(d, t);
                        }
                        _ => {
                            self.err(self.pos, "invalid SIMD unary selector".into());
                            VX::Abs(w)
                        }
                    };
                    self.emit(Inst::VX(vx, d, a, a));
                }
            }
        }
        self.vx_result(d, t)
    }

    fn vx_result(&mut self, r: VReg, t: TyId) -> Val {
        if self.layout(t).size != 16 {
            self.err(self.pos, "SIMD results must be exactly 16 bytes".into());
            return self.dummy_val(t);
        }
        let base = self.new_slot_for(t);
        self.emit(Inst::VStore(base, 0, r));
        Val::M(base, 0)
    }

    fn intrinsic(&mut self, d: DefId, gargs: &[TyId], vals: &[(Val, TyId)], t: TyId) -> Option<Val> {
        if self.u.prog.def(d).kind != DefKind::ForeignFn || !self.u.is_intrinsic(d) {
            return None;
        }
        let name = self.u.prog.name(d).to_string();
        if name.starts_with("vx_") {
            return Some(self.vx_intrinsic(&name, vals, t));
        }
        if name.starts_with("simd_") {
            if let Some(v) = self.simd_intrinsic(&name, vals, t) {
                return Some(v);
            }
        }
        if name.starts_with("atomic_") || name == "spin_loop_hint" {
            return Some(self.atomic_intrinsic(&name, gargs, vals));
        }
        if let Some(v) = self.mem_intrinsic(&name, gargs, vals, t) {
            return Some(v);
        }
        let mut a = Vec::new();
        for (v, ty) in vals {
            let r = self.to_leaves(v.clone(), *ty);
            a.extend(r);
        }
        let f64_ = name.ends_with("f64");
        let cls = if f64_ { Cls::F64 } else { Cls::F32 };
        let un = |n: &str| -> Option<FUn> {
            Some(match n {
                "fabs" => FUn::Abs,
                "sqrt" => FUn::Sqrt,
                "floor" => FUn::Floor,
                "ceil" => FUn::Ceil,
                "trunc" => FUn::Trunc,
                "rint" => FUn::RoundEven,
                _ => return None,
            })
        };
        let base = name.trim_end_matches("f64").trim_end_matches("f32").trim_end_matches('_');
        if let Some(op) = un(base) {
            let d = self.f.vreg(cls);
            self.emit(Inst::FUnary(op, f64_, d, a[0]));
            return Some(Val::L(vec![d]));
        }
        match base {
            "fma" => {
                // a * b + c, one rounding: vfmadd231 (x86-64 with FMA3) / fmadd (arm64);
                // std calls it only under `fma_native()`
                let d = self.f.vreg(cls);
                self.emit(Inst::FMulAdd(false, false, f64_, d, a[0], a[1], a[2]));
                Some(Val::L(vec![d]))
            }
            "fma_native" => {
                let opts = &self.u.prog.session.options;
                let on = opts.target.starts_with("aarch64") || opts.cpu.fma;
                Some(Val::L(vec![self.iconst(on as i64)]))
            }
            "fmin" | "fmax" => {
                let d = self.f.vreg(cls);
                let op = if base == "fmin" { FOp::Min } else { FOp::Max };
                self.emit(Inst::FBin(op, f64_, d, a[0], a[1]));
                Some(Val::L(vec![d]))
            }
            "to_bits" => {
                let d = self.f.vreg(Cls::I);
                self.emit(Inst::Conv(Conv::FToBits(f64_), d, a[0]));
                Some(Val::L(vec![d]))
            }
            "from_bits" => {
                let d = self.f.vreg(cls);
                self.emit(Inst::Conv(Conv::BitsToF(f64_), d, a[0]));
                Some(Val::L(vec![d]))
            }
            "tls_block" => {
                // intrinsics_rt::tls_block() -> *mut usize: this thread's thread-local block (word 0
                // the domain base, word 1 the runtime's per-thread record): the inline TLS sequence
                let d = self.f.vreg(Cls::I);
                let sym = self.u.rs.tls_base;
                self.emit(Inst::TlsAddr(d, sym));
                Some(Val::L(vec![d]))
            }
            "str_as_bytes" => Some(Val::L(a)),
            "slice_len" => Some(Val::L(vec![a[1]])),
            "unreachable" => {
                self.term(Term::Unreachable);
                self.dead();
                Some(self.unit())
            }
            "panic_impl" | "abort" => {
                // intrinsics_rt::panic_impl(msg, len, file, file_len, line, col) -> !: the
                // runtime's report + unwind (live) / abort (AOT); abort reports "abort"
                let args = if name == "abort" {
                    let (l, c) = self.u.prog.files[self.b.file as usize].line_col(self.pos);
                    let path = self.u.prog.file_paths[self.b.file as usize].clone();
                    let m = self.u.lit_sym(b"abort");
                    let f = self.u.lit_sym(path.as_bytes());
                    let (mp, fp) = (self.f.vreg(Cls::I), self.f.vreg(Cls::I));
                    self.emit(Inst::Addr(mp, m));
                    self.emit(Inst::Addr(fp, f));
                    vec![mp, self.iconst(5), fp, self.iconst(path.len() as i64), self.iconst(l as i64), self.iconst(c as i64)]
                } else {
                    a
                };
                if args.len() != 6 {
                    self.err(self.pos, format!("intrinsic `{}` takes 6 arguments -- (msg, len, file, file_len, line, col)", name));
                    return Some(self.unit());
                }
                let rt = self.u.rs.panic_impl;
                self.emit(Inst::Call(Callee::Host(rt), args, Vec::new()));
                self.term(Term::Unreachable);
                self.dead();
                Some(self.unit())
            }
            "mul_hi" => {
                // intrinsics_int::mul_hi<T>(a, b): high 64 bits of the full product of 64-bit
                // T (umulh/smulh, mul/imul's rdx); checked/overflowing_mul use it
                let g = match gargs.first() {
                    Some(g) => *g,
                    None => return Some(self.unit()),
                };
                let it = self.int_ty(g);
                if a.len() != 2 || it.bits != 64 {
                    self.err(self.pos, "intrinsic `mul_hi` takes two 64-bit integers -- use it for u64/i64/usize/isize".to_string());
                    return Some(self.unit());
                }
                let d = self.f.vreg(Cls::I);
                self.emit(Inst::IBin(IOp::MulHi, it, d, a[0], a[1]));
                Some(Val::L(vec![d]))
            }
            "ctpop" | "ctlz" | "cttz" | "bswap" | "bitreverse" => {
                // RIR's 64-bit bit operations (IBit); 128-bit values are (lo, hi)
                let g = match gargs.first() {
                    Some(g) => *g,
                    None => return Some(self.unit()),
                };
                let it = self.int_ty(g);
                let bits = it.bits as i64;
                if a.is_empty() {
                    return Some(self.unit());
                }
                if a.len() == 2 {
                    let (lo, hi) = (a[0], a[1]);
                    let r = match name.as_str() {
                        "ctpop" => {
                            let x = self.popcount64(lo);
                            let y = self.popcount64(hi);
                            vec![self.ibin(IOp::Add, false, x, y)]
                        }
                        "ctlz" => {
                            // clz(hi) + (hi == 0) * clz(lo)
                            let ch = self.clz64(hi);
                            let cl = self.clz64(lo);
                            let z = self.f.vreg(Cls::I);
                            self.emit(Inst::ICmpI(Cond::Eq, false, z, hi, 0));
                            let m = self.ibin(IOp::Mul, false, z, cl);
                            vec![self.ibin(IOp::Add, false, ch, m)]
                        }
                        "cttz" => {
                            let cl = self.ctz64(lo);
                            let ch = self.ctz64(hi);
                            let z = self.f.vreg(Cls::I);
                            self.emit(Inst::ICmpI(Cond::Eq, false, z, lo, 0));
                            let m = self.ibin(IOp::Mul, false, z, ch);
                            vec![self.ibin(IOp::Add, false, cl, m)]
                        }
                        "bswap" => {
                            let nl = self.bswap64(hi);
                            let nh = self.bswap64(lo);
                            vec![nl, nh]
                        }
                        _ => {
                            let nl = self.rev64(hi);
                            let nh = self.rev64(lo);
                            vec![nl, nh]
                        }
                    };
                    return Some(Val::L(r));
                }
                let x = a[0];
                let x = if bits < 64 { self.and_k(x, (1i64 << bits) - 1) } else { x };
                let r = match name.as_str() {
                    "ctpop" => self.popcount64(x),
                    "ctlz" => {
                        // leading zeros in 64 bits, minus the bits above T
                        let c = self.clz64(x);
                        let k = self.iconst(64 - bits);
                        self.ibin(IOp::Sub, false, c, k)
                    }
                    "cttz" => {
                        // a sentinel bit at `bits` makes cttz(0) = bits
                        let x = if bits < 64 {
                            let s = self.iconst(1i64 << bits);
                            self.ibin(IOp::Or, false, x, s)
                        } else {
                            x
                        };
                        self.ctz64(x)
                    }
                    "bswap" => {
                        let y = self.bswap64(x);
                        if bits < 64 {
                            self.shr_k(y, 64 - bits)
                        } else {
                            y
                        }
                    }
                    _ => {
                        let y = self.rev64(x);
                        if bits < 64 {
                            self.shr_k(y, 64 - bits)
                        } else {
                            y
                        }
                    }
                };
                // results of type T are re-extended to T (counts are u32 and small)
                let r = if matches!(name.as_str(), "bswap" | "bitreverse") {
                    let d = self.f.vreg(Cls::I);
                    self.emit(Inst::Conv(Conv::IntToInt(it), d, r));
                    d
                } else {
                    r
                };
                Some(Val::L(vec![r]))
            }
            "catch_panic" => {
                // intrinsics_rt::catch_panic(f, data) -> bool: Rt::catch_panic runs f(data) under
                // a recovery point on this thread (1 = panicked: reported, stack reset here)
                if a.len() != 2 {
                    self.err(self.pos, "intrinsic `catch_panic` takes (f: fn(*mut u8), data: *mut u8)".to_string());
                    return Some(self.unit());
                }
                let r = self.f.vreg(Cls::I);
                let rt = self.u.rs.catch_panic;
                self.emit(Inst::Call(Callee::Host(rt), vec![a[0], a[1]], vec![r]));
                let z = self.f.vreg(Cls::I);
                self.emit(Inst::ICmpI(Cond::Ne, false, z, r, 0));
                Some(Val::L(vec![z]))
            }
            "cpuid" => {
                if a.len() != 2 {
                    self.err(self.pos, "intrinsic `cpuid` takes (leaf: u32, subleaf: u32)".to_string());
                    return Some(self.dummy_val(t));
                }
                let ds = [self.f.vreg(Cls::I), self.f.vreg(Cls::I), self.f.vreg(Cls::I), self.f.vreg(Cls::I)];
                self.emit(Inst::Cpuid(ds, a[0], a[1]));
                // A four-u32 tuple may use an indirect return; respect the layout.
                let dst = self.new_slot_for(t);
                let layout = self.layout(t);
                for (i, r) in ds.iter().enumerate() {
                    self.emit(Inst::Store(Mem::Int(4, false), dst, layout.fields[i] as i32, *r));
                }
                Some(self.load_val(dst, 0, t))
            }
            "xgetbv" => {
                if a.len() != 1 {
                    self.err(self.pos, "intrinsic `xgetbv` takes (index: u32)".to_string());
                    return Some(self.dummy_val(t));
                }
                let r = self.f.vreg(Cls::I);
                self.emit(Inst::Xgetbv(r, a[0]));
                Some(Val::L(vec![r]))
            }
            "fp_env_get" => {
                // intrinsics_rt::fp_env_get() -> u64: stmxcsr (x64) / mrs fpcr (arm64)
                let r = self.f.vreg(Cls::I);
                self.emit(Inst::FpEnv(false, r));
                Some(Val::L(vec![r]))
            }
            "fp_env_set" => {
                // intrinsics_rt::fp_env_set(x): ldmxcsr (x64) / msr fpcr (arm64)
                if a.len() != 1 {
                    self.err(self.pos, "intrinsic `fp_env_set` takes (x: u64)".to_string());
                    return Some(self.unit());
                }
                self.emit(Inst::FpEnv(true, a[0]));
                Some(self.unit())
            }
            "panicking" => {
                // intrinsics_rt::panicking() -> bool: Rt::panicking (this thread is unwinding)
                let r = self.f.vreg(Cls::I);
                let rt = self.u.rs.panicking;
                self.emit(Inst::Call(Callee::Host(rt), Vec::new(), vec![r]));
                Some(Val::L(vec![r]))
            }
            "rethrow" => {
                // intrinsics_rt::rethrow(): Rt::rethrow (the next panic continues a reported one)
                let rt = self.u.rs.rethrow;
                self.emit(Inst::Call(Callee::Host(rt), Vec::new(), Vec::new()));
                Some(self.unit())
            }
            "args" => {
                // intrinsics_rt::args() -> (usize, *const *const c_char): Rt::args (argc, argv)
                let (c, v) = (self.f.vreg(Cls::I), self.f.vreg(Cls::I));
                let rt = self.u.rs.args;
                self.emit(Inst::Call(Callee::Host(rt), Vec::new(), vec![c, v]));
                Some(Val::L(vec![c, v]))
            }
            "caller_location" => {
                // inside a #[track_caller] fn: the location its caller passed (the outermost
                // non-tracking call site of the chain); elsewhere this call site (Reference
                // attributes.codegen.track_caller)
                let r = self.caller_location_val();
                Some(Val::L(vec![r]))
            }
            _ => {
                let _ = t;
                self.err(self.pos, format!("unknown intrinsic `{}`", name));
                Some(self.unit())
            }
        }
    }

    // ------------------------------------------------------------ macros

    fn lower_mac(&mut self, e: ExprId, m: &MacCall, t: TyId) -> Val {
        let ast = self.ast();
        let name = match ast.segs(m.path.segs).last() {
            Some(s) => self.ident_text(s.name),
            None => "",
        };
        if name == "cfg" {
            // evaluated against the crate's cfg at compile time
            let prog = &self.u.prog;
            let krate = prog.file_crate[self.b.file as usize];
            let on = crate::cfg::eval_tokens(prog.src(self.b.file), m.toks.lo as usize, m.toks.hi as usize, &prog.crates[krate as usize].cfg);
            return Val::L(vec![self.iconst(on as i64)]);
        }
        if m.args == NO_MAC_ARGS {
            return self.unit();
        }
        let margs = ast.mac_arg(m.args);
        let mexprs = ast.list(margs.exprs);
        match name {
            // assembled unless `--no-asm` (typeck reports the block then)
            "asm" => self.lower_asm(margs),
            "assert" => {
                let c = mexprs[0];
                let ok = self.f.block();
                let bad = self.f.block();
                self.lower_cond(c, ok, bad);
                self.switch(bad);
                if mexprs.len() > 1 {
                    self.lower_fmt(e, &mexprs[1..]);
                } else {
                    let ce = ast.expr(c);
                    let msg = format!("assertion failed: {}", self.src_text(ce.lo, ce.hi));
                    self.fmt_lit(&msg);
                }
                self.panic_here("");
                self.switch(ok);
                self.unit()
            }
            "debug_assert" | "debug_assert_eq" | "debug_assert_ne" | "debug_assert_matches" => self.unit(),
            "assert_eq" | "assert_ne" => {
                let a = mexprs[0];
                let b2 = mexprs[1];
                let at = self.body_ty(a);
                let av = self.lower_expr(a);
                let bv = self.lower_expr(b2);
                // compare through references
                let (av2, bv2, ct) = self.deref_for_cmp(av.clone(), bv.clone(), at);
                let c = self.eq_vals(av2.clone(), bv2.clone(), ct);
                let ok = self.f.block();
                let bad = self.f.block();
                if name == "assert_eq" {
                    self.term(Term::Branch(c, ok, bad));
                } else {
                    self.term(Term::Branch(c, bad, ok));
                }
                self.switch(bad);
                let op = if name == "assert_eq" { "==" } else { "!=" };
                self.fmt_lit(&format!("assertion `left {} right` failed", op));
                if mexprs.len() > 2 {
                    self.fmt_lit(": ");
                    self.lower_fmt(e, &mexprs[2..]);
                }
                self.fmt_lit("\n  left: ");
                self.fmt_value(av2, ct, b'?');
                self.fmt_lit("\n right: ");
                self.fmt_value(bv2, ct, b'?');
                self.panic_here("");
                self.switch(ok);
                self.unit()
            }
            "print" | "println" | "eprint" | "eprintln" => {
                self.lower_fmt(e, mexprs);
                if name.ends_with("ln") {
                    self.fmt_lit("\n");
                }
                let stream = if name.starts_with('e') { 2 } else { 1 };
                let s = self.iconst(stream);
                let rt = self.u.rs.fmt_print;
                self.emit(Inst::Call(Callee::Host(rt), vec![s], Vec::new()));
                self.unit()
            }
            "panic" | "unreachable" | "todo" | "unimplemented" => {
                let prefix = match name {
                    "unreachable" => "internal error: entered unreachable code",
                    "todo" => "not yet implemented",
                    "unimplemented" => "not implemented",
                    _ => "",
                };
                if !prefix.is_empty() {
                    self.fmt_lit(prefix);
                    if !mexprs.is_empty() {
                        self.fmt_lit(": ");
                    }
                }
                self.lower_fmt(e, mexprs);
                if name == "panic" && mexprs.is_empty() {
                    self.fmt_lit("explicit panic");
                }
                self.panic_here("");
                self.dead();
                self.unit()
            }
            "assert_matches" => {
                // rustc core panicking::assert_matches_failed: the value in Debug and the
                // pattern's source text
                let (place, xt, ok, bad) = self.lower_macro_match(mexprs[0], margs.pat, margs.guard);
                self.switch(bad);
                let v = self.read_place(&place, xt);
                // the pattern's tokens: after the first top-level comma up to its `if` / comma
                let u = self.u;
                let src = u.prog.src(self.b.file);
                let mut depth = 0;
                let mut start = None;
                let mut end = m.toks.hi as usize;
                let mut i = m.toks.lo as usize;
                while i < m.toks.hi as usize {
                    match src.tok(i).kind {
                        crate::lexer::T::OpenParen | crate::lexer::T::OpenBracket | crate::lexer::T::OpenBrace => depth += 1,
                        crate::lexer::T::CloseParen | crate::lexer::T::CloseBracket | crate::lexer::T::CloseBrace => depth -= 1,
                        crate::lexer::T::Comma if depth == 0 => {
                            if start.is_none() {
                                start = Some(i + 1);
                            } else {
                                end = i;
                                break;
                            }
                        }
                        crate::lexer::T::KwIf if depth == 0 && start.is_some() => {
                            end = i;
                            break;
                        }
                        _ => {}
                    }
                    i += 1;
                }
                let pat_text = match start {
                    Some(s) if s < end => self.src_text(src.tok(s).lo, src.tok(end - 1).hi).into_owned(),
                    _ => String::new(),
                };
                self.fmt_lit("assertion `left matches right` failed");
                if mexprs.len() > 1 {
                    self.fmt_lit(": ");
                    self.lower_fmt(e, &mexprs[1..]);
                }
                self.fmt_lit("\n  left: ");
                self.fmt_value(v, xt, b'?');
                self.fmt_lit(&format!("\n right: {}", pat_text));
                self.panic_here("");
                self.switch(ok);
                self.unit()
            }
            "matches" => {
                let (_, _, ok, fail) = self.lower_macro_match(mexprs[0], margs.pat, margs.guard);
                let res = self.f.vreg(Cls::I);
                let join = self.f.block();
                self.switch(ok);
                self.emit(Inst::Iconst(res, 1));
                self.goto(join);
                self.switch(fail);
                self.emit(Inst::Iconst(res, 0));
                self.goto(join);
                self.switch(join);
                let _ = t;
                Val::L(vec![res])
            }
            _ => self.unit(),
        }
    }

    fn deref_for_cmp(&mut self, a: Val, b: Val, t: TyId) -> (Val, Val, TyId) {
        let mut a = a;
        let mut b = b;
        let mut t = t;
        while let TyKind::Ref(_, inner, _) = self.kind(t) {
            if self.unsized_ty(inner) {
                break;
            }
            let pa = self.to_leaves(a, t);
            let pb = self.to_leaves(b, t);
            a = self.read_place(&Place::Mem(pa[0], 0), inner);
            b = self.read_place(&Place::Mem(pb[0], 0), inner);
            t = inner;
        }
        (a, b, t)
    }

    fn panic_here(&mut self, msg: &str) {
        let site = self.u.site_sym(self.b.file, self.pos, msg);
        let s = self.sym_val(site);
        let rt = self.u.rs.panic_site;
        self.emit(Inst::Call(Callee::Host(rt), vec![s], Vec::new()));
        self.term(Term::Unreachable);
    }

    fn fmt_lit(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        // a literal piece: one copy per content, as every str literal
        let addr = self.u.lit_data(s.as_bytes());
        let addr = self.u.data_sym(addr);
        let p = self.f.vreg(Cls::I);
        self.emit(Inst::Addr(p, addr));
        let l = self.iconst(s.len() as i64);
        let z = self.iconst(0);
        let rt = self.u.rs.fmt_str;
        self.emit(Inst::Call(Callee::Host(rt), vec![p, l, z], Vec::new()));
    }

    /// Appends a formatted value to the runtime's format buffer.
    fn fmt_value_spec(&mut self, v: Val, t: TyId, spec: u64) {
        let mut v = v;
        let mut t = t;
        // format through references
        while let TyKind::Ref(_, inner, _) = self.kind(t) {
            if matches!(self.kind_ref(inner), TyKind::Str) {
                break;
            }
            let p = self.to_leaves(v, t);
            v = self.read_place(&Place::Mem(p[0], 0), inner);
            t = inner;
        }
        let sp = self.iconst(spec as i64);
        match self.kind(t) {
            TyKind::Int(_) | TyKind::Bool | TyKind::Char => {
                let it = self.int_ty(t);
                let r = self.to_leaves(v, t);
                let kind = match self.kind(t) {
                    TyKind::Bool => 3,
                    TyKind::Char => 4,
                    _ => {
                        if it.signed {
                            1
                        } else {
                            2
                        }
                    }
                };
                let k = self.iconst(kind | ((it.bits as i64) << 8));
                let rt = self.u.rs.fmt_int;
                self.emit(Inst::Call(Callee::Host(rt), vec![r[0], k, sp], Vec::new()));
            }
            TyKind::Float(_) => {
                let f64_ = self.is_f64(t);
                let r = self.to_leaves(v, t);
                let x = if f64_ {
                    r[0]
                } else {
                    let d = self.f.vreg(Cls::F64);
                    self.emit(Inst::Conv(Conv::F32ToF64, d, r[0]));
                    d
                };
                let k = self.iconst(if f64_ { 1 } else { 0 });
                let rt = self.u.rs.fmt_float;
                self.emit(Inst::Call(Callee::Host(rt), vec![x, k, sp], Vec::new()));
            }
            TyKind::Ref(_, inner, _) if matches!(self.kind_ref(inner), TyKind::Str) => {
                let r = self.to_leaves(v, t);
                let rt = self.u.rs.fmt_str;
                self.emit(Inst::Call(Callee::Host(rt), vec![r[0], r[1], sp], Vec::new()));
            }
            _ => {
                let s = crate::typeck::ty_to_string(false, &self.u.prog, &self.u.tcx, t);
                self.err(self.pos, format!("cannot format a value of type `{}` yet", s));
            }
        }
    }

    fn fmt_value(&mut self, v: Val, t: TyId, ty_char: u8) {
        let spec = FmtSpec { ty: ty_char, ..FmtSpec::default() }.pack();
        self.fmt_value_spec(v, t, spec);
    }


    fn lower_fmt(&mut self, e: ExprId, exprs: &[ExprId]) {
        if exprs.is_empty() {
            return;
        }
        let ast = self.ast();
        let fe = ast.expr(exprs[0]);
        let text = self.src_text(fe.lo, fe.hi);
        if !matches!(fe.kind, ExprKind::Lit(LitKind::Str) | ExprKind::Lit(LitKind::RawStr)) {
            // panic!(value) with a non-literal (2018 style): format it with Display
            let t = self.body_ty(exprs[0]);
            let v = self.lower_expr(exprs[0]);
            self.fmt_value(v, t, 0);
            return;
        }
        let s = unescape_str(&text);
        let pieces = parse_fmt(&s);
        // evaluate positional args once, in order
        let mut argv = Vec::new();
        for x in &exprs[1..] {
            let t = self.body_ty(*x);
            argv.push((self.lower_expr(*x), t));
        }
        let mut next = 0usize;
        for p in pieces {
            match p {
                FmtPiece::Lit(l) => self.fmt_lit(&l),
                FmtPiece::Arg { index, named, spec } => {
                    let (v, t) = match (index, named) {
                        (_, Some(n)) => {
                            let sy = self.u.prog.syms.get(&n).unwrap_or(u32::MAX);
                            match self.b.fmt_named.get(&(e.0, sy)).copied() {
                                Some(Res::Local(li)) => {
                                    let pl = self.local_place(li);
                                    let lt = self.monomorphize(self.b.locals[li as usize].ty);
                                    (self.read_place(&pl, lt), lt)
                                }
                                Some(Res::Def(d)) => {
                                    let ct = self.u.tcx.const_ty(&self.u.prog, d).unwrap_or(self.u.tcx.tys.error);
                                    (self.const_val(d, Vec::new(), ct), ct)
                                }
                                _ => continue,
                            }
                        }
                        (Some(i), None) => match argv.get(i) {
                            Some(x) => x.clone(),
                            None => continue,
                        },
                        (None, None) => {
                            let x = argv.get(next).cloned();
                            next += 1;
                            match x {
                                Some(x) => x,
                                None => continue,
                            }
                        }
                    };
                    self.fmt_value_spec(v, t, spec.pack());
                }
            }
        }
    }
}

fn ret_fits(l: &[Leaf]) -> bool {
    let mut ni = 0;
    let mut nf = 0;
    for x in l {
        if x.cls == Cls::I {
            ni += 1;
        } else {
            nf += 1;
        }
    }
    ni <= 2 && nf <= 2
}

/// Indices (pattern index, field index) for tuple-like patterns with `..`.
fn seq_indices(ast: &Ast, v: &[PatId], n: usize) -> Vec<(usize, u32)> {
    let mut rest = None;
    for (i, p) in v.iter().enumerate() {
        if let Pat::Rest = ast.pat(*p) {
            rest = Some(i);
        }
    }
    let mut out = Vec::new();
    match rest {
        None => {
            for i in 0..v.len() {
                out.push((i, i as u32));
            }
        }
        Some(r) => {
            for i in 0..r {
                out.push((i, i as u32));
            }
            let after = v.len() - r - 1;
            for j in 0..after {
                out.push((r + 1 + j, (n - after + j) as u32));
            }
        }
    }
    out
}

pub fn extend_const(v: i64, it: IntTy) -> i64 {
    match (it.bits, it.signed) {
        (8, true) => v as i8 as i64,
        (8, false) => v as u8 as i64,
        (16, true) => v as i16 as i64,
        (16, false) => v as u16 as i64,
        (32, true) => v as i32 as i64,
        (32, false) => v as u32 as i64,
        _ => v,
    }
}

unsafe fn read_raw(addr: u64, m: Mem) -> u64 {
    let p = addr as *const u8;
    match m {
        Mem::Int(1, _) => *p as u64,
        Mem::Int(2, _) => (p as *const u16).read_unaligned() as u64,
        Mem::Int(4, _) | Mem::F32 => (p as *const u32).read_unaligned() as u64,
        _ => (p as *const u64).read_unaligned(),
    }
}

fn strip_num(text: &str) -> String {
    let mut s = String::new();
    for c in text.chars() {
        if c != '_' {
            s.push(c);
        }
    }
    for suf in ["f32", "f64"] {
        if let Some(x) = s.strip_suffix(suf) {
            return x.to_string();
        }
    }
    // integer-typed suffixes never reach floats; strip anyway
    s
}

pub fn closure_map(_b: &Body) -> HashMap<u32, u32> {
    HashMap::default()
}

// ---------------------------------------------------------------- glue

/// Compiler-generated helper functions per type. GLUE_DROP: `fn(p: *mut T)` runs T's Drop
/// impl (if any) and then drops every field that needs it, in declaration order.
/// Static `core::panic::Location { file, line, col }` data (memoized per site).
pub fn location_const(u: &Unit, path: &str, line: u32, col: u32) -> relax_backend::rir::Sym {
    let a = location_addr(u, path, line, col);
    if a == 0 {
        return u.syms.intern(crate::syms::SymKind::Rt("null"), 0);
    }
    u.data_sym(a)
}

fn location_addr(u: &Unit, path: &str, line: u32, col: u32) -> u64 {
    let key = (path.to_string(), line, col);
    if let Some(&a) = u.locations.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return a;
    }
    let loc = u.tcx.lang_items.get(LangItem::PanicLocation);
    let lay = match loc {
        Some(d) => {
            let t = u.tcx.tys.intern(TyKind::Adt(d, crate::types::List::EMPTY, crate::types::RegionArgsId::EMPTY));
            u.layout_of(t)
        }
        None => return 0,
    };
    if lay.fields.len() != 3 {
        return 0;
    }
    let shown = crate::builtins::file_macro_path(path);
    let path = shown.as_str();
    // `file_bytes_with_nul` (rustc Location::file_as_c_str): the name plus its terminating nul
    let mut file_bytes = path.as_bytes().to_vec();
    file_bytes.push(0);
    let fa = u.lit_data(&file_bytes);
    let mut blob = vec![0u8; lay.size as usize];
    let put = |blob: &mut Vec<u8>, off: u32, b: &[u8]| {
        let o = off as usize;
        if o + b.len() <= blob.len() {
            blob[o..o + b.len()].copy_from_slice(b);
        }
    };
    put(&mut blob, lay.fields[0], &fa.to_le_bytes());
    put(&mut blob, lay.fields[0] + 8, &(file_bytes.len() as u64).to_le_bytes());
    put(&mut blob, lay.fields[1], &line.to_le_bytes());
    put(&mut blob, lay.fields[2], &col.to_le_bytes());
    // pointer words: the file name's bytes; the rest integers
    let mut words = vec![None; (blob.len() + 7) / 8];
    assert!(lay.fields[0] % 8 == 0, "Location's file field is not 8-aligned");
    words[(lay.fields[0] / 8) as usize] = Some(u.data_sym(fa));
    let a = u.data(&blob, lay.align.max(1) as u64, crate::syms::DataKind::Words(words));
    u.syms.set_data_name(a, format!("loc:{}:{}:{}", path, line, col));
    // another thread may have made this location meanwhile (the memo is not held while the
    // object is built): every user gets the first one -- one symbol per location, whatever the
    // compile order (one constant pool word; the copy made here stays unused)
    // identity state: the memo's key is the program's, not the compiler arena's
    let _k = crate::zone_alloc::kept();
    let key = key.clone();
    *u.locations.lock().unwrap_or_else(|e| e.into_inner()).entry(key).or_insert(a)
}

/// Does fn instance slot `id` take the implicit caller location (a #[track_caller] fn)?
pub fn takes_caller_loc(u: &Unit, id: u32) -> bool {
    let ahead = crate::jit::lower_ahead_fn_key(id);
    match ahead.as_ref().unwrap_or_else(|| &u.fns[id as usize].key) {
        FnKey::Inst(d, _) => u.tcx.fn_sig(&u.prog, *d).map_or(false, |s| s.track_caller),
        FnKey::Shim(ShimInstance { kind: ShimKind::ClosureOnce { track_caller, .. }, .. }) => *track_caller,
        FnKey::Shim(ShimInstance { kind: ShimKind::VTable(d), .. }) => u.tcx.fn_sig(&u.prog, *d).is_some_and(|s| s.track_caller),
        _ => false,
    }
}

/// The body of a provided crate's bound fn (provided.rs): Relax's own ABI on the outside, one
/// uniform C call of its shim inside. Each argument is stored to a stack slot (or passed where
/// the caller's copy already is, for arguments passed by address), the slot addresses go in an
/// array, the shim moves them out and writes the result to the result slot (or the caller's
/// result address). A shim that caught a panic returns 1: its text is read through the crate's
/// `__host_last_panic` and raised here as a Relax panic.
pub fn bridge_func(u: &Unit, d: DefId, name: String, shim: relax_backend::rir::Sym, last_panic: relax_backend::rir::Sym) -> Option<Func> {
    let s = u.tcx.fn_sig(&u.prog, d).cloned()?;
    let ps: Vec<TyId> = s.params.iter().map(|p| u.tcx.normalize_erasing_late_bound_regions(&u.prog, crate::tcx::TypingEnv::fully_monomorphized(&u.prog), *p)).collect();
    let ret = u.tcx.normalize_erasing_late_bound_regions(&u.prog, crate::tcx::TypingEnv::fully_monomorphized(&u.prog), s.ret);
    Some(bridge_func_sig(u, &ps, ret, name, shim, last_panic))
}

/// bridge_func for an explicit signature (a derived method of an opaque type).
pub fn bridge_func_sig(u: &Unit, ps: &[TyId], ret: TyId, name: String, shim: relax_backend::rir::Sym, last_panic: relax_backend::rir::Sym) -> Func {
    let mut f = Func::new(name, 0);
    let b = f.block();
    let rl = u.lay.of(&crate::layout::LayoutCx::new(&u.prog), &u.tcx, ret);
    let mut rets = Vec::new();
    let mut ret_leaves = Vec::new();
    let ret_addr = f.vreg(Cls::I);
    match &rl.leaves {
        Some(l) if ret_fits(l) => {
            for lf in l {
                f.rets.push(lf.cls);
                rets.push(f.vreg(lf.cls));
                ret_leaves.push(*lf);
            }
            let slot = f.slot(rl.size.max(1), rl.align.max(1));
            push(&mut f, b, Inst::SlotAddr(ret_addr, slot));
        }
        _ => {
            f.params.push(ret_addr);
        }
    }
    let mut addrs = Vec::new();
    let mut stores = Vec::new();
    for p in ps {
        let l = u.lay.of(&crate::layout::LayoutCx::new(&u.prog), &u.tcx, *p);
        match &l.leaves {
            Some(leaves) => {
                let slot = f.slot(l.size.max(1), l.align.max(1));
                let a = f.vreg(Cls::I);
                for lf in leaves {
                    let r = f.vreg(lf.cls);
                    f.params.push(r);
                    stores.push((a, *lf, r));
                }
                push(&mut f, b, Inst::SlotAddr(a, slot));
                addrs.push(a);
            }
            None => {
                let r = f.vreg(Cls::I);
                f.params.push(r);
                addrs.push(r);
            }
        }
    }
    for (a, lf, r) in stores {
        push(&mut f, b, Inst::Store(lf.mem, a, lf.off as i32, r));
    }
    let arr_slot = f.slot((addrs.len().max(1) * 8) as u32, 8);
    let arr = f.vreg(Cls::I);
    push(&mut f, b, Inst::SlotAddr(arr, arr_slot));
    for (i, a) in addrs.iter().enumerate() {
        push(&mut f, b, Inst::Store(Mem::Int(8, false), arr, (i * 8) as i32, *a));
    }
    let usize_ = u.tcx.tys.usize_;
    let status = f.vreg(Cls::I);
    let sig = crate::cabi::c_sig(u, &[usize_, usize_], u.tcx.tys.u32_, None, crate::types::Abi::C);
    push(&mut f, b, Inst::Call(Callee::CHost(shim, Box::new(sig)), vec![arr, ret_addr], vec![status]));
    let ok = f.block();
    let bad = f.block();
    f.blocks[b as usize].term = Term::Branch(status, bad, ok);
    for (lf, r) in ret_leaves.iter().zip(&rets) {
        push(&mut f, ok, Inst::Load(lf.mem, *r, ret_addr, lf.off as i32));
    }
    f.blocks[ok as usize].term = Term::Ret(rets);
    // the shim's panic text, raised as this thread's panic
    let out_slot = f.slot(16, 8);
    let out = f.vreg(Cls::I);
    let out_len = f.vreg(Cls::I);
    push(&mut f, bad, Inst::SlotAddr(out, out_slot));
    push(&mut f, bad, Inst::IBinI(IOp::Add, relax_backend::rir::I64, out_len, out, 8));
    let lp_sig = crate::cabi::c_sig(u, &[usize_, usize_], u.tcx.tys.unit, None, crate::types::Abi::C);
    push(&mut f, bad, Inst::Call(Callee::CHost(last_panic, Box::new(lp_sig)), vec![out, out_len], Vec::new()));
    let (mp, ml, z, site) = (f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I), f.vreg(Cls::I));
    push(&mut f, bad, Inst::Load(Mem::Int(8, false), mp, out, 0));
    push(&mut f, bad, Inst::Load(Mem::Int(8, false), ml, out, 8));
    push(&mut f, bad, Inst::Iconst(z, 0));
    push(&mut f, bad, Inst::Call(Callee::Host(u.rs.fmt_str), vec![mp, ml, z], Vec::new()));
    push(&mut f, bad, Inst::Iconst(site, u64::MAX as i64));
    push(&mut f, bad, Inst::Call(Callee::Host(u.rs.panic_site), vec![site], Vec::new()));
    f.blocks[bad as usize].term = Term::Unreachable;
    f
}

/// A function that calls slot `callee` with its own arguments and returns its result: the same
/// signature on both sides (provided_fmt_hash forwards an opaque type's Hash / Debug / Display
/// method to std's helper instance). Parameters and the result follow the leaf ABI
/// bridge_func_sig uses.
pub fn forward_func(u: &Unit, ps: &[TyId], ret: TyId, name: String, callee: u32) -> Func {
    let mut f = Func::new(name, 0);
    let b = f.block();
    let rl = u.lay.of(&crate::layout::LayoutCx::new(&u.prog), &u.tcx, ret);
    let mut args = Vec::new();
    let mut rets = Vec::new();
    match &rl.leaves {
        Some(l) if ret_fits(l) => {
            for lf in l {
                f.rets.push(lf.cls);
                rets.push(f.vreg(lf.cls));
            }
        }
        _ => {
            let r = f.vreg(Cls::I);
            f.params.push(r);
            args.push(r);
        }
    }
    for p in ps {
        let l = u.lay.of(&crate::layout::LayoutCx::new(&u.prog), &u.tcx, *p);
        match &l.leaves {
            Some(leaves) => {
                for lf in leaves {
                    let r = f.vreg(lf.cls);
                    f.params.push(r);
                    args.push(r);
                }
            }
            None => {
                let r = f.vreg(Cls::I);
                f.params.push(r);
                args.push(r);
            }
        }
    }
    push(&mut f, b, Inst::Call(Callee::Fn(callee), args, rets.clone()));
    f.blocks[b as usize].term = Term::Ret(rets);
    f
}

pub fn glue_func(u: &mut Unit, kind: u8, t: TyId) -> Func {
    if kind >= crate::jit::GLUE_CTOR {
        return ctor_fn(u, (kind - crate::jit::GLUE_CTOR) as u32, t);
    }
    if kind == crate::jit::GLUE_EXT_DROP {
        return ext_drop_fn(u, t);
    }
    if kind == crate::jit::GLUE_DROP
        && u.lay.is_unsized(&crate::layout::LayoutCx::new(&u.prog), &u.tcx, t)
    {
        // A drop_in_place shim takes the whole pointer, including a dyn
        // vtable or slice length. Use the same unsized drop lowering as the
        // intrinsic; the sized glue below only accepts a data pointer.
        let def = u.tcx.lang_items.get(LangItem::DropGlue).expect("drop glue lang item");
        let body = Body::empty(def, vec![t], u.prog.def(def).file, u.tcx.tys.unit);
        let id = u.fn_id(FnKey::Glue(kind, t));
        let mut l = Lcx::new(u, &body, fn_name(u, &FnKey::Glue(kind, t)), id);
        let p = l.f.vreg(Cls::I);
        let metadata = l.f.vreg(Cls::I);
        l.f.params.extend([p, metadata]);
        l.drop_unsized(p, metadata, t);
        l.term(Term::Ret(Vec::new()));
        l.finish();
        assert!(l.errors.is_empty(), "unsized drop glue: {:?}", l.errors);
        return l.f;
    }
    let name = fn_name(u, &FnKey::Glue(kind, t));
    let mut f = Func::new(name, 0);
    let entry = f.block();
    let p = f.vreg(Cls::I);
    f.params.push(p);
    let mut cur = entry;
    // a provided crate's opaque type: rustc's drop_in_place, through its drop shim
    let opaque_drop = match u.tcx.tys.kind(t) {
        TyKind::Adt(d, a, _) if a.is_empty() => u.provided_types.get(d).copied(),
        _ => None,
    };
    if kind == crate::jit::GLUE_DROP {
        match opaque_drop {
            Some(0) => {}
            Some(addr) => {
                let d = match u.tcx.tys.kind(t) {
                    TyKind::Adt(d, _, _) => *d,
                    _ => unreachable!(),
                };
                let addr = u.syms.intern(crate::syms::SymKind::Provided(format!("drop:{}", u.prog.def_path(d))), addr);
                let sig = crate::cabi::c_sig(u, &[u.tcx.tys.usize_], u.tcx.tys.unit, None, crate::types::Abi::C);
                push(&mut f, cur, Inst::Call(Callee::CHost(addr, Box::new(sig)), vec![p], Vec::new()));
            }
            // Like rustc's DropGlue(_, None), even an explicitly requested shim is
            // empty when no cleanup is needed. In particular, do not read an empty enum's tag.
            None if u.needs_drop_glue(t) => drop_glue_body(u, &mut f, &mut cur, p, t),
            None => {}
        }
    }
    f.blocks[cur as usize].term = Term::Ret(Vec::new());
    for b in f.blocks.iter_mut() {
        while b.pos.len() < b.insts.len() {
            b.pos.push(0);
        }
    }
    f
}

/// `fn(fields..) -> Adt` building variant `vi` (tuple structs: 0) of the fn type's return ADT.
fn ctor_fn(u: &mut Unit, vi: u32, t: TyId) -> Func {
    let name = fn_name(u, &FnKey::Glue(crate::jit::GLUE_CTOR + vi as u8, t));
    let mut f = Func::new(name, 0);
    let b = f.block();
    let (ps, ret) = match u.tcx.tys.kind(t).clone() {
        TyKind::FnPtr(ps, r, _, _) => (u.tcx.tys.list(ps).to_vec(), r),
        _ => (Vec::new(), u.tcx.tys.unit),
    };
    let rl = u.layout_of(ret);
    let fits = matches!(&rl.leaves, Some(l) if ret_fits(l));
    let sret = if fits {
        None
    } else {
        let p = f.vreg(Cls::I);
        f.params.push(p);
        Some(p)
    };
    let slot = f.slot(rl.size, rl.align);
    let base = f.vreg(Cls::I);
    push(&mut f, b, Inst::SlotAddr(base, slot));
    let is_enum = !rl.variant_fields.is_empty();
    let offs: Vec<u32> = if is_enum { rl.variant_fields.get(vi as usize).cloned().unwrap_or_default() } else { rl.fields.clone() };
    if let TyKind::Adt(d, _, _) = u.tcx.tys.kind(ret).clone() {
        if let Some(adt) = u.tcx.adts.get(&d) {
            if adt.is_enum {
                emit_store_disc(u, &mut f, b, 0, base, 0, &rl, &adt, vi);
            }
        }
    }
    for (i, p) in ps.iter().enumerate() {
        let pl = u.layout_of(*p);
        let off = offs.get(i).copied().unwrap_or(0) as i32;
        match &pl.leaves {
            Some(leaves) => {
                for lf in leaves {
                    let r = f.vreg(lf.cls);
                    f.params.push(r);
                    push(&mut f, b, Inst::Store(lf.mem, base, off + lf.off as i32, r));
                }
            }
            None => {
                let r = f.vreg(Cls::I);
                f.params.push(r);
                let dst = addr_off(&mut f, b, base, off as u32);
                if pl.size > 0 {
                    push(&mut f, b, Inst::Copy(dst, r, pl.size));
                }
            }
        }
    }
    let mut rets = Vec::new();
    match sret {
        Some(p) => {
            if rl.size > 0 {
                push(&mut f, b, Inst::Copy(p, base, rl.size));
            }
        }
        None => {
            for lf in rl.leaves.clone().unwrap_or_default() {
                f.rets.push(lf.cls);
                let r = f.vreg(lf.cls);
                push(&mut f, b, Inst::Load(lf.mem, r, base, lf.off as i32));
                rets.push(r);
            }
        }
    }
    f.blocks[b as usize].term = Term::Ret(rets);
    f
}

fn push(f: &mut Func, b: u32, i: Inst) {
    f.blocks[b as usize].insts.push(i);
    f.blocks[b as usize].pos.push(0);
}

fn push_at(f: &mut Func, b: u32, pos: u64, i: Inst) {
    f.blocks[b as usize].insts.push(i);
    f.blocks[b as usize].pos.push(pos);
}

/// Variant `vi`'s discriminant value.
pub fn variant_disc(u: &Unit, adt: &crate::tcx::AdtT, vi: u32) -> Option<i64> {
    Some(adt.discriminant_for_variant(&u.tcx, &u.prog, vi)?.signed_value() as i64)
}

/// The discriminant from the tag field's value `raw` (loaded with the tag's Mem): Direct tags
/// hold it; a niche encoding maps tag values to variants (rustc_codegen_ssa codegen_get_discr:
/// relative = tag - niche_start, in the niche variants' range when relative <= hi - lo).
/// Uninhabited layouts have no discriminant.
pub fn emit_decode_tag(u: &Unit, f: &mut Func, b: u32, pos: u64, raw: Vec<VReg>, l: &Layout, adt: &crate::tcx::AdtT) -> Option<Vec<VReg>> {
    use crate::layout::TagEnc;
    if l.uninhabited { return None; }
    Some(match l.enc {
        TagEnc::Direct => raw,
        TagEnc::Single(v) => {
            let d = f.vreg(Cls::I);
            let discr = adt.discriminant_for_variant(&u.tcx, &u.prog, v)?;
            push_at(f, b, pos, Inst::Iconst(d, discr.signed_value() as i64));
            if crate::types::Types::int_info(discr.ty).0 == 16 {
                let hi = f.vreg(Cls::I);
                push_at(f, b, pos, Inst::Iconst(hi, (discr.val >> 64) as i64));
                vec![d, hi]
            } else { vec![d] }
        }
        TagEnc::Niche { untagged, lo, hi, start } => {
            let bytes = match l.tag {
                Some((_, Mem::Int(n, _))) => n as u32,
                _ => 8,
            };
            // one dataless niche variant behind a narrow unsigned tag (Option<char>, Option<NonZeroU32>):
            // `(tag - start) & mask <= 0` is `tag & mask == start & mask`
            if hi == lo && lo != untagged && bytes < 8 && raw.len() == 1 {
                let inr = f.vreg(Cls::I);
                let mask = (1u64 << (bytes * 8)) - 1;
                let k = (start as u64 & mask) as i64;
                // the register may hold a sign-extended or dirty value: only the tag's bytes count
                let m = f.vreg(Cls::I);
                push_at(f, b, pos, Inst::IBinI(IOp::And, U64, m, raw[0], mask as i64));
                if k == k as i32 as i64 {
                    push_at(f, b, pos, Inst::ICmpI(Cond::Eq, false, inr, m, k));
                } else {
                    // the back ends' compare-immediate is a sign-extended imm32
                    let kr = f.vreg(Cls::I);
                    push_at(f, b, pos, Inst::Iconst(kr, k));
                    push_at(f, b, pos, Inst::ICmp(Cond::Eq, false, inr, m, kr));
                }
                let vidx = f.vreg(Cls::I);
                push_at(f, b, pos, Inst::Iconst(vidx, lo as i64));
                let by_index = (0..adt.variants.len() as u32).all(|v| variant_disc(u, adt, v) == Some(v as i64));
                let ud = f.vreg(Cls::I);
                push_at(f, b, pos, Inst::Iconst(ud, if by_index { untagged as i64 } else { variant_disc(u, adt, untagged)? }));
                if by_index {
                    let d = f.vreg(Cls::I);
                    push_at(f, b, pos, Inst::Sel(d, inr, vidx, ud));
                    return Some(vec![d]);
                }
                let k2 = f.vreg(Cls::I);
                push_at(f, b, pos, Inst::Iconst(k2, variant_disc(u, adt, lo)?));
                let d = f.vreg(Cls::I);
                push_at(f, b, pos, Inst::Sel(d, inr, k2, ud));
                return Some(vec![d]);
            }
            let ks = f.vreg(Cls::I);
            push_at(f, b, pos, Inst::Iconst(ks, start as u64 as i64));
            let rel = f.vreg(Cls::I);
            push_at(f, b, pos, Inst::IBin(IOp::Sub, U64, rel, raw[0], ks));
            let rel = if bytes < 8 {
                let m = f.vreg(Cls::I);
                push_at(f, b, pos, Inst::IBinI(IOp::And, U64, m, rel, ((1u64 << (bytes * 8)) - 1) as i64));
                m
            } else {
                rel
            };
            let inr = f.vreg(Cls::I);
            push_at(f, b, pos, Inst::ICmpI(Cond::Le, false, inr, rel, (hi - lo) as i64));
            // one niche variant: in range means exactly that variant
            let vidx = f.vreg(Cls::I);
            if hi == lo {
                push_at(f, b, pos, Inst::Iconst(vidx, lo as i64));
            } else {
                push_at(f, b, pos, Inst::IBinI(IOp::Add, U64, vidx, rel, lo as i64));
            }
            let by_index = (0..adt.variants.len() as u32).all(|v| variant_disc(u, adt, v) == Some(v as i64));
            let ud = f.vreg(Cls::I);
            push_at(f, b, pos, Inst::Iconst(ud, if by_index { untagged as i64 } else { variant_disc(u, adt, untagged)? }));
            if by_index {
                let d = f.vreg(Cls::I);
                push_at(f, b, pos, Inst::Sel(d, inr, vidx, ud));
                return Some(vec![d]);
            }
            // general discriminants: the niche variant's, else the untagged one's
            let mut d = ud;
            for v in lo..=hi {
                if v == untagged {
                    continue;
                }
                let c = f.vreg(Cls::I);
                push_at(f, b, pos, Inst::ICmpI(Cond::Eq, false, c, vidx, v as i64));
                let c2 = f.vreg(Cls::I);
                push_at(f, b, pos, Inst::IBin(IOp::And, U64, c2, c, inr));
                let k = f.vreg(Cls::I);
                push_at(f, b, pos, Inst::Iconst(k, variant_disc(u, adt, v)?));
                let nd = f.vreg(Cls::I);
                push_at(f, b, pos, Inst::Sel(nd, c2, k, d));
                d = nd;
            }
            vec![d]
        }
    })
}

/// The discriminant of the enum at `p + off` (layout `l`).
pub fn emit_load_disc(u: &Unit, f: &mut Func, b: u32, pos: u64, p: VReg, off: i32, l: &Layout, adt: &crate::tcx::AdtT) -> Option<Vec<VReg>> {
    if l.uninhabited { return None; }
    let mut raw = Vec::new();
    if let Some((toff, tmem)) = l.tag {
        let r = f.vreg(Cls::I);
        if matches!(tmem, Mem::Int(16, _)) {
            let hi = f.vreg(Cls::I);
            push_at(f, b, pos, Inst::Load(Mem::Int(8, false), r, p, off + toff as i32));
            push_at(f, b, pos, Inst::Load(Mem::Int(8, false), hi, p, off + toff as i32 + 8));
            raw.extend([r, hi]);
        } else {
            push_at(f, b, pos, Inst::Load(tmem, r, p, off + toff as i32));
            raw.push(r);
        }
    }
    emit_decode_tag(u, f, b, pos, raw, l, adt)
}

/// The tag store that marks the enum at `p + off` as variant `vi` (none for the untagged
/// variant of a niche encoding and for single-variant layouts).
pub fn emit_store_disc(u: &Unit, f: &mut Func, b: u32, pos: u64, p: VReg, off: i32, l: &Layout, adt: &crate::tcx::AdtT, vi: u32) {
    use crate::layout::TagEnc;
    let (toff, tmem) = match l.tag {
        Some(t) => t,
        None => return,
    };
    let v = match l.enc {
        TagEnc::Direct => match adt.discriminant_for_variant(&u.tcx, &u.prog, vi) {
            Some(discr) => discr.val,
            None => return,
        },
        TagEnc::Single(_) => return,
        TagEnc::Niche { untagged, lo, start, .. } => {
            if vi == untagged {
                return;
            }
            start.wrapping_add((vi - lo) as u128)
        }
    };
    let k = f.vreg(Cls::I);
    push_at(f, b, pos, Inst::Iconst(k, v as i64));
    if matches!(tmem, Mem::Int(16, _)) {
        let hi = f.vreg(Cls::I);
        push_at(f, b, pos, Inst::Iconst(hi, (v >> 64) as i64));
        push_at(f, b, pos, Inst::Store(Mem::Int(8, false), p, off + toff as i32, k));
        push_at(f, b, pos, Inst::Store(Mem::Int(8, false), p, off + toff as i32 + 8, hi));
    } else { push_at(f, b, pos, Inst::Store(tmem, p, off + toff as i32, k)); }
}

/// The discriminant of a constant's enum value whose tag field reads `raw` (sign-extended
/// per the tag's Mem).
pub fn decode_tag_const(u: &Unit, raw: i128, l: &Layout, adt: &crate::tcx::AdtT) -> Option<i128> {
    use crate::layout::TagEnc;
    if l.uninhabited { return None; }
    Some(match l.enc {
        TagEnc::Direct => crate::tcx::Discr { val: raw as u128, ty: adt.repr_int.unwrap_or(Prim::Isize) }.signed_value(),
        TagEnc::Single(v) => adt.discriminant_for_variant(&u.tcx, &u.prog, v)?.signed_value(),
        TagEnc::Niche { untagged, lo, hi, start } => {
            let bytes = match l.tag {
                Some((_, Mem::Int(n, _))) => n as u32,
                _ => 8,
            };
            let m: u128 = if bytes >= 16 { u128::MAX } else { (1u128 << (bytes * 8)) - 1 };
            let rel = (raw as u128).wrapping_sub(start) & m;
            let v = if rel <= (hi - lo) as u128 { lo + rel as u32 } else { untagged };
            adt.discriminant_for_variant(&u.tcx, &u.prog, v)?.signed_value()
        }
    })
}

/// A live-editable struct's extension (M10, hotreload.md section 13): the ext word's offset and
/// the fields edits added -- (field index, type, offset in the block), appended in the order the
/// edits added them so a field keeps its offset in every later version -- with the block's size
/// and alignment. The block starts with a 16-byte header: fields set (u32), capacity (u32),
/// alignment (u32).
pub(crate) fn ext_fields(u: &Unit, t: TyId) -> Option<(u32, Vec<(u32, TyId, u32)>, u32, u32)> {
    let l = u.layout_of(t);
    let ext = l.ext?;
    let (d, args) = match u.tcx.tys.kind(t) {
        TyKind::Adt(d, a, _) => (*d, a.clone()),
        _ => return None,
    };
    let adt = u.tcx.adts.get(&d)?;
    let mut out = Vec::new();
    let (mut off, mut align) = (16u32, 16u32);
    for fi in 0..adt.variants[0].fields.len() {
        if l.fields.get(fi) != Some(&u32::MAX) {
            continue;
        }
        let ft = u.tcx.field_ty(&u.prog, crate::tcx::TypingEnv::fully_monomorphized(&u.prog), d, 0, fi as u32, u.tcx.tys.list(args));
        let fl = u.layout_of(ft);
        off = crate::layout::round_up(off, fl.align.max(1));
        align = align.max(fl.align);
        out.push((fi as u32, ft, off));
        off += fl.size;
    }
    Some((ext, out, crate::layout::round_up(off.max(16), align), align))
}

/// A slot a live layout edit took the field away from (M10): renamed so no code names it.
pub(crate) fn is_dead_field(u: &Unit, name: u32) -> bool {
    name & 0x8000_0000 == 0 && u.prog.syms.str(name).starts_with("__relax_dead_")
}

/// Whether live-editable struct `t` has dead slots (code built now leaves them unset).
fn has_dead_slots(u: &Unit, t: TyId) -> bool {
    match u.tcx.tys.kind(t) {
        TyKind::Adt(d, _, _) => u.tcx.adts.get(d).map_or(false, |a| a.variants[0].fields.iter().any(|f| is_dead_field(u, f.name))),
        _ => false,
    }
}

/// Drop of a live-editable struct's extension block (GLUE_EXT_DROP; its argument is the ext
/// word's address): the fields it holds, then the block.
fn ext_drop_fn(u: &mut Unit, t: TyId) -> Func {
    let name = fn_name(u, &FnKey::Glue(crate::jit::GLUE_EXT_DROP, t));
    let mut f = Func::new(name, 0);
    let entry = f.block();
    let p = f.vreg(Cls::I);
    f.params.push(p);
    let word = f.vreg(Cls::I);
    push(&mut f, entry, Inst::Load(Mem::Int(8, false), word, p, 0));
    let zero = f.vreg(Cls::I);
    push(&mut f, entry, Inst::Iconst(zero, 0));
    let mut cur = entry;
    // dead slots (removed / retyped fields) of a value built before they died: dropped here
    if let (Some(ext_off), TyKind::Adt(d, args, _)) = (u.layout_of(t).ext, u.tcx.tys.kind(t).clone()) {
        let l = u.layout_of(t);
        let dead: Vec<(u32, TyId)> = u.tcx.adts.get(&d).map(|a| a.variants[0].fields.iter().enumerate().filter(|(i, fd)| is_dead_field(u, fd.name) && l.fields.get(*i).map_or(false, |o| *o != u32::MAX)).map(|(i, _)| (l.fields[i], u.tcx.field_ty(&u.prog, crate::tcx::TypingEnv::fully_monomorphized(&u.prog), d, 0, i as u32, u.tcx.tys.list(args)))).collect()).unwrap_or_default();
        let dead: Vec<(u32, TyId)> = dead.into_iter().filter(|(_, ft)| u.needs_drop_glue(*ft)).collect();
        if !dead.is_empty() {
            let one = f.vreg(Cls::I);
            push(&mut f, cur, Inst::Iconst(one, 1));
            let bit = f.vreg(Cls::I);
            push(&mut f, cur, Inst::IBin(IOp::And, U64, bit, word, one));
            let set = f.vreg(Cls::I);
            push(&mut f, cur, Inst::ICmp(Cond::Eq, false, set, bit, zero));
            let dr = f.block();
            let next = f.block();
            f.blocks[cur as usize].term = Term::Branch(set, dr, next);
            let base = f.vreg(Cls::I);
            push(&mut f, dr, Inst::IBinI(IOp::Add, U64, base, p, -(ext_off as i64)));
            for (off, ft) in dead {
                let a = addr_off(&mut f, dr, base, off);
                call_drop(u, &mut f, dr, a, ft);
            }
            f.blocks[dr as usize].term = Term::Jump(next);
            cur = next;
        }
    }
    let mask = f.vreg(Cls::I);
    push(&mut f, cur, Inst::Iconst(mask, !1i64));
    let blk = f.vreg(Cls::I);
    push(&mut f, cur, Inst::IBin(IOp::And, U64, blk, word, mask));
    let isnull = f.vreg(Cls::I);
    push(&mut f, cur, Inst::ICmp(Cond::Eq, false, isnull, blk, zero));
    let body = f.block();
    let out = f.block();
    f.blocks[cur as usize].term = Term::Branch(isnull, out, body);
    let mut cur = body;
    let cnt = f.vreg(Cls::I);
    push(&mut f, cur, Inst::Load(Mem::Int(4, false), cnt, blk, 0));
    if let Some((_, fields, _, _)) = ext_fields(u, t) {
        for (k, &(_, ft, off)) in fields.iter().enumerate() {
            if !u.needs_drop_glue(ft) {
                continue;
            }
            let kk = f.vreg(Cls::I);
            push(&mut f, cur, Inst::Iconst(kk, k as i64));
            let set = f.vreg(Cls::I);
            push(&mut f, cur, Inst::ICmp(Cond::Gt, false, set, cnt, kk));
            let dr = f.block();
            let next = f.block();
            f.blocks[cur as usize].term = Term::Branch(set, dr, next);
            let a = addr_off(&mut f, dr, blk, off);
            call_drop(u, &mut f, dr, a, ft);
            f.blocks[dr as usize].term = Term::Jump(next);
            cur = next;
        }
    }
    let rf: rt_fn_ty!(fn(u64)) = rt_ext_free;
    let fs = u.syms.intern(crate::syms::SymKind::Rt("ext_free"), rf as usize as u64);
    push(&mut f, cur, Inst::Call(Callee::Host(fs), vec![blk], Vec::new()));
    push(&mut f, cur, Inst::Store(Mem::Int(8, false), p, 0, zero));
    f.blocks[cur as usize].term = Term::Jump(out);
    f.blocks[out as usize].term = Term::Ret(Vec::new());
    for b in f.blocks.iter_mut() {
        while b.pos.len() < b.insts.len() {
            b.pos.push(0);
        }
    }
    f
}

rt_fn! {
    /// The extension block behind ext word `slot` (M10), allocated (zeroed) or grown to `size`
    /// (the fields it held keep their offsets); the caller sets the fields beyond its count.
    pub fn rt_ext_get(slot: u64, size: u64, align: u64) -> u64 {
        unsafe {
            let slot = slot as *mut u64;
            // bit 0 of the ext word is the value's dead-slot flag (lower.rs zero_ext)
            let flag = *slot & 1;
            let cur = *slot & !1;
            let align = align.max(16) as usize;
            if cur == 0 {
                let lay = std::alloc::Layout::from_size_align_unchecked(size as usize, align);
                let b = std::alloc::alloc_zeroed(lay);
                if b.is_null() {
                    std::alloc::handle_alloc_error(lay);
                }
                *(b as *mut u32).add(1) = size as u32;
                *(b as *mut u32).add(2) = align as u32;
                *slot = b as u64 | flag;
                return b as u64;
            }
            let b = cur as *mut u8;
            let cap = *(b as *const u32).add(1) as u64;
            if cap >= size {
                return cur;
            }
            let old_align = *(b as *const u32).add(2) as usize;
            let lay = std::alloc::Layout::from_size_align_unchecked(size as usize, align.max(old_align));
            let n = std::alloc::alloc_zeroed(lay);
            if n.is_null() {
                std::alloc::handle_alloc_error(lay);
            }
            std::ptr::copy_nonoverlapping(b, n, cap as usize);
            *(n as *mut u32).add(1) = size as u32;
            *(n as *mut u32).add(2) = align.max(old_align) as u32;
            std::alloc::dealloc(b, std::alloc::Layout::from_size_align_unchecked(cap as usize, old_align));
            *slot = n as u64 | flag;
            n as u64
        }
    }
}

rt_fn! {
    /// Frees an extension block (its fields were dropped or moved out); null is a no-op.
    pub fn rt_ext_free(blk: u64) {
        if blk == 0 {
            return;
        }
        unsafe {
            let b = blk as *mut u8;
            let cap = *(b as *const u32).add(1) as usize;
            let align = *(b as *const u32).add(2) as usize;
            std::alloc::dealloc(b, std::alloc::Layout::from_size_align_unchecked(cap, align));
        }
    }
}

fn addr_off(f: &mut Func, b: u32, base: VReg, off: u32) -> VReg {
    if off == 0 {
        return base;
    }
    let d = f.vreg(Cls::I);
    push(f, b, Inst::IBinI(IOp::Add, U64, d, base, off as i64));
    d
}

fn call_drop(u: &mut Unit, f: &mut Func, b: u32, addr: VReg, t: TyId) {
    if u.needs_drop_glue(t) {
        let id = u.fn_id(FnKey::Glue(crate::jit::GLUE_DROP, t));
        push(f, b, Inst::Call(Callee::Fn(id), vec![addr], Vec::new()));
    }
}

/// Calls `items` (drop functions with their argument) in order; a call that may unwind gets an
/// unwind edge to a pad calling the items after it, then Resume (rustc's drop elaboration: a
/// value's remaining fields still drop when one drop panics).
fn glue_drop_seq(f: &mut Func, cur: &mut u32, items: &[(Callee, VReg, bool)]) {
    for k in 0..items.len() {
        let (c, a, may) = (&items[k].0, items[k].1, items[k].2);
        push(f, *cur, Inst::Call(c.clone(), vec![a], Vec::new()));
        if may && k + 1 < items.len() {
            let pad = f.block();
            for (c2, a2, _) in &items[k + 1..] {
                push(f, pad, Inst::Call(c2.clone(), vec![*a2], Vec::new()));
            }
            f.blocks[pad as usize].term = Term::Resume;
            let next = f.block();
            f.blocks[*cur as usize].term = Term::Invoke(next, pad);
            *cur = next;
        }
    }
}

/// Destructor identity: rustc_middle/src/ty/util.rs:376; call selection: rustc_mir_transform/src/elaborate_drop.rs:973.
fn drop_glue_body(u: &mut Unit, f: &mut Func, cur: &mut u32, p: VReg, t: TyId) {
    let options = &u.prog.session.clone().options;
    let k = u.tcx.tys.kind(t).clone();
    match k {
        TyKind::Adt(d, args, _) => {
            let unwind = options.unwind;
            // the type's own Drop::drop(&mut self) first
            let mut user_drop: Option<(Callee, bool)> = None;
            if let (Some(dtor), Some(td)) = (u.tcx.adt_destructor(d), u.tcx.lang_items.get(LangItem::Drop)) {
                // Selection supplies the impl's substitutions; identity comes from adt_destructor.
                {
                    let trait_ref = relax_types::next_solver::TraitRef { def_id: td, args: vec![relax_types::consts::GenericArg::Type(t)] };
                    let selected = u.tcx.codegen_select_candidate(&u.prog, crate::tcx::TypingEnv::fully_monomorphized(&u.prog), trait_ref.clone()).unwrap_or_else(|error| panic!("established destructor dispatch failed for {trait_ref:?}: {error:?}"));
                    let relax_types::next_solver::select::ImplSource::UserDefined { args, .. } = selected else { panic!("destructor without impl") };
                    let mut iargs = Vec::new();
                    for arg in args { match arg {
                        relax_types::consts::GenericArg::Type(t) => iargs.push(t),
                        relax_types::consts::GenericArg::Const(c) => iargs.push(u.tcx.tys.intern(TyKind::Const(c))),
                        relax_types::consts::GenericArg::Lifetime(_) => {},
                    } }
                    let id = u.fn_id(FnKey::Inst(dtor.did, iargs));
                    // (a std impl unwinds only through what its type arguments own)
                    let may = unwind && u.drop_may_unwind(t);
                    user_drop = Some((Callee::Fn(id), may));
                }
            }
            let adt = match u.tcx.adts.get(&d) {
                Some(a) => a.clone(),
                None => return,
            };
            if adt.is_union || adt.is_enum {
                if let Some((c, _)) = user_drop.take() {
                    push(f, *cur, Inst::Call(c, vec![p], Vec::new()));
                }
            }
            if adt.is_union {
                return;
            }
            let l = u.layout_of(t);
            if !adt.is_enum {
                let mut items: Vec<(Callee, VReg, bool)> = Vec::new();
                if let Some((c, may)) = user_drop.take() {
                    items.push((c, p, may));
                }
                for (fi, fd) in adt.variants[0].fields.iter().enumerate() {
                    let ft = u.tcx.field_ty(&u.prog, crate::tcx::TypingEnv::fully_monomorphized(&u.prog), d, 0, fi as u32, u.tcx.tys.list(args));
                    // fields in the extension block (M10) drop with it, below; so do dead slots
                    // (a value built before their field went holds them, ext_drop_fn)
                    if l.fields.get(fi) == Some(&u32::MAX) || (l.ext.is_some() && is_dead_field(u, fd.name)) {
                        continue;
                    }
                    if u.needs_drop_glue(ft) {
                        let mut a = addr_off(f, *cur, p, l.fields[fi]);
                        if adt.repr_packed != 0 {
                            // a field of a packed struct may be misaligned: it is moved to an
                            // aligned temporary and dropped there (rustc add_moves_for_packed_drops)
                            let fl = u.layout_of(ft);
                            let s = f.slot(fl.size, fl.align);
                            let tmp = f.vreg(Cls::I);
                            push(f, *cur, Inst::SlotAddr(tmp, s));
                            push(f, *cur, Inst::Copy(tmp, a, fl.size));
                            a = tmp;
                        }
                        let id = u.fn_id(FnKey::Glue(crate::jit::GLUE_DROP, ft));
                        let may = unwind && u.drop_may_unwind(ft);
                        items.push((Callee::Fn(id), a, may));
                    }
                }
                if let Some(e) = l.ext {
                    let a = addr_off(f, *cur, p, e);
                    let id = u.fn_id(FnKey::Glue(crate::jit::GLUE_EXT_DROP, t));
                    items.push((Callee::Fn(id), a, unwind));
                }
                glue_drop_seq(f, cur, &items);
                return;
            }
            // the discriminant: the tag, or for a null-pointer niche whether the pointer word is
            // set (the dataful variant's disc when it is, the empty one's when it is null)
            if l.tag.is_none() && !matches!(l.enc, crate::layout::TagEnc::Single(_)) {
                return;
            }
            let Some(tag) = emit_load_disc(u, f, *cur, 0, p, 0, &l, &adt) else { return; };
            let join = f.block();
            for (vi, v) in adt.variants.iter().enumerate() {
                let mut any = false;
                for fi in 0..v.fields.len() {
                    let ft = u.tcx.field_ty(&u.prog, crate::tcx::TypingEnv::fully_monomorphized(&u.prog), d, vi as u32, fi as u32, u.tcx.tys.list(args));
                    if u.needs_drop_glue(ft) {
                        any = true;
                    }
                }
                if !any {
                    continue;
                }
                let c = f.vreg(Cls::I);
                let discr = adt.discriminant_for_variant(&u.tcx, &u.prog, vi as u32).expect("existing drop variant").signed_value() as u128;
                push(f, *cur, Inst::ICmpI(Cond::Eq, false, c, tag[0], discr as i64));
                let c = if tag.len() == 2 {
                    let hi = f.vreg(Cls::I); let both = f.vreg(Cls::I);
                    push(f, *cur, Inst::ICmpI(Cond::Eq, false, hi, tag[1], (discr >> 64) as i64));
                    push(f, *cur, Inst::IBin(IOp::And, U64, both, c, hi)); both
                } else { c };
                let yes = f.block();
                let no = f.block();
                f.blocks[*cur as usize].term = Term::Branch(c, yes, no);
                for fi in 0..v.fields.len() {
                    let ft = u.tcx.field_ty(&u.prog, crate::tcx::TypingEnv::fully_monomorphized(&u.prog), d, vi as u32, fi as u32, u.tcx.tys.list(args));
                    if u.needs_drop_glue(ft) {
                        let a = addr_off(f, yes, p, l.variant_fields[vi][fi]);
                        call_drop(u, f, yes, a, ft);
                    }
                }
                f.blocks[yes as usize].term = Term::Jump(join);
                *cur = no;
            }
            f.blocks[*cur as usize].term = Term::Jump(join);
            *cur = join;
        }
        TyKind::Tuple(v) => {
            let unwind = options.unwind;
            let l = u.layout_of(t);
            let mut items = Vec::new();
            for (i, ft) in u.tcx.tys.list(v).iter().enumerate() {
                if u.needs_drop_glue(*ft) {
                    let a = addr_off(f, *cur, p, l.fields[i]);
                    let id = u.fn_id(FnKey::Glue(crate::jit::GLUE_DROP, *ft));
                    let may = unwind && u.drop_may_unwind(*ft);
                    items.push((Callee::Fn(id), a, may));
                }
            }
            glue_drop_seq(f, cur, &items);
        }
        // a closure's state: its by-value captures (rustc: a closure is a struct of its upvars);
        // an FnOnce closure's captures only while their drop flag is set
        TyKind::Closure(file, ce, crate::types::ClosureArgs { closure_sig_as_fn_ptr_ty: sig, .. }) => {
            let storage = u.closure_storage(t);
            let once = storage.drop_flags;
            let fields = &storage.fields;
            let l = u.layout_of(t);
            let coro = u.tcx.coroutines.contains(&(file, ce));
            let nf = if coro { fields.len().saturating_sub(1) } else { fields.len() };
            if coro {
                // a coroutine suspended somewhere (state word 3 + k): its poll with a null
                // context runs the drops of that suspension point
                let w = f.vreg(Cls::I);
                let fo = l.fields.get(nf).copied().unwrap_or(0) as i32;
                let tag = Mem::Int(u.layout_of(fields[nf]).size as u8, false);
                f.blocks[*cur as usize].insts.push(Inst::Load(tag, w, p, fo));
                let c = f.vreg(Cls::I);
                f.blocks[*cur as usize].insts.push(Inst::ICmpI(Cond::Gt, false, c, w, 2));
                let yes = f.block();
                let join = f.block();
                f.blocks[*cur as usize].term = Term::Branch(c, yes, join);
                *cur = yes;
                if let Some(id) = u.closure_fn_id(t) {
                    let out = match u.tcx.tys.kind(sig).clone() {
                        TyKind::FnPtr(_, r, _, _) => r,
                        _ => u.tcx.tys.unit,
                    };
                    let pr = match u.tcx.lang_items.get(LangItem::Poll) {
                        Some(d) => u.tcx.tys.intern(TyKind::Adt(d, u.tcx.tys.mk_list(&[out]), crate::types::RegionArgsId::EMPTY)),
                        None => u.tcx.tys.unit,
                    };
                    let pl = u.layout_of(pr);
                    let z = f.vreg(Cls::I);
                    f.blocks[*cur as usize].insts.push(Inst::Iconst(z, 0));
                    let mut args = Vec::new();
                    let mut rets = Vec::new();
                    match &pl.leaves {
                        Some(lv) if ret_fits(lv) => {
                            for lf in lv {
                                rets.push(f.vreg(lf.cls));
                            }
                        }
                        _ => {
                            let sl = f.slot(pl.size, pl.align);
                            let a = f.vreg(Cls::I);
                            f.blocks[*cur as usize].insts.push(Inst::SlotAddr(a, sl));
                            args.push(a);
                        }
                    }
                    args.push(p);
                    args.push(z);
                    f.blocks[*cur as usize].insts.push(Inst::Call(Callee::Fn(id), args, rets));
                }
                f.blocks[*cur as usize].term = Term::Jump(join);
                *cur = join;
            }
            let n = if once { nf / 2 } else { nf };
            let precise = &storage.owned_fields;
            for i in 0..n {
                let ft = fields[i];
                if !u.needs_drop_glue(ft) {
                    continue;
                }
                let a = addr_off(f, *cur, p, l.fields[i]);
                // a precise capture owns only the fields it took (the rest stayed with the
                // enclosing body): (address, type) of what this state drops
                let parts: Vec<(VReg, TyId)> = match precise.get(i).cloned().flatten() {
                    Some(fs) => {
                        let sl = u.layout_of(ft);
                        let fts: Vec<TyId> = match u.tcx.tys.kind(ft).clone() {
                            TyKind::Adt(d, args, _) => u.tcx.adts.get(&d).and_then(|ad| ad.variants.first().map(|v| v.fields.iter().map(|x| u.tcx.tys.subst_erasing_regions(x.ty, u.tcx.tys.list(args))).collect())).unwrap_or_default(),
                            _ => Vec::new(),
                        };
                        fs.iter().filter_map(|&fi| {
                            let fty = *fts.get(fi as usize)?;
                            if !u.needs_drop_glue(fty) {
                                return None;
                            }
                            let off = *sl.fields.get(fi as usize)?;
                            Some((addr_off(f, *cur, a, off), fty))
                        }).collect()
                    }
                    None => vec![(a, ft)],
                };
                if once {
                    let flag = f.vreg(Cls::I);
                    f.blocks[*cur as usize].insts.push(Inst::Load(Mem::Int(1, false), flag, p, l.fields[n + i] as i32));
                    let yes = f.block();
                    let join = f.block();
                    f.blocks[*cur as usize].term = Term::Branch(flag, yes, join);
                    *cur = yes;
                    for (pa, pt) in &parts {
                        call_drop(u, f, *cur, *pa, *pt);
                    }
                    f.blocks[*cur as usize].term = Term::Jump(join);
                    *cur = join;
                } else {
                    for (pa, pt) in &parts {
                        call_drop(u, f, *cur, *pa, *pt);
                    }
                }
            }
        }
        TyKind::Array(e, n) => {
            let n = n.try_to_target_usize(&u.tcx.tys).expect("normalized array length");
            let el = u.layout_of(e);
            let stride = crate::layout::round_up(el.size, el.align);
            let unwind = options.unwind;
            let id = u.fn_id(FnKey::Glue(crate::jit::GLUE_DROP, e));
            let may = unwind && u.drop_may_unwind(e);
            let mut items = Vec::new();
            for i in 0..n as u32 {
                let a = addr_off(f, *cur, p, i * stride);
                items.push((Callee::Fn(id), a, may));
            }
            glue_drop_seq(f, cur, &items);
        }
        _ => {}
    }
}

/// A u128 as two u64 halves in the two integer result registers (C ABI).
#[repr(C)]
struct WideRet {
    lo: u64,
    hi: u64,
}

/// The address of the 128-bit division helper (its runtime symbol is interned at unit start).
pub fn divrem128_addr() -> u64 {
    let f: rt_fn_ty!(fn(u64, u64, u64, u64, u64) -> WideRet) = rt_divrem128;
    f as usize as u64
}

/// Scalar-only cast helpers, with the integer split to avoid a platform-specific i128 ABI.
pub fn wide_float_addrs() -> [u64; 2] {
    let f32_: rt_fn_ty!(fn(u64, u64, u64) -> f32) = rt_wide_f32;
    let f64_: rt_fn_ty!(fn(u64, u64, u64) -> f64) = rt_wide_f64;
    [f32_ as usize as u64, f64_ as usize as u64]
}

rt_fn! {
fn rt_wide_f32(signed: u64, lo: u64, hi: u64) -> f32 {
    let value = ((hi as u128) << 64) | lo as u128;
    if signed != 0 { (value as i128) as f32 } else { value as f32 }
}
}

rt_fn! {
fn rt_wide_f64(signed: u64, lo: u64, hi: u64) -> f64 {
    let value = ((hi as u128) << 64) | lo as u128;
    if signed != 0 { (value as i128) as f64 } else { value as f64 }
}
}

rt_fn! {
/// 128-bit quotient or remainder (mode bit 0: remainder, bit 1: signed). The caller has checked a
/// zero divisor and signed MIN / -1. (A host helper for now; candidate for an Rt entry.)
fn rt_divrem128(mode: u64, alo: u64, ahi: u64, blo: u64, bhi: u64) -> WideRet {
    let a = ((ahi as u128) << 64) | alo as u128;
    let b = ((bhi as u128) << 64) | blo as u128;
    let r: u128 = if mode & 2 != 0 {
        let (x, y) = (a as i128, b as i128);
        (if mode & 1 != 0 { x.wrapping_rem(y) } else { x.wrapping_div(y) }) as u128
    } else if mode & 1 != 0 {
        a % b
    } else {
        a / b
    };
    WideRet { lo: r as u64, hi: (r >> 64) as u64 }
}
}

/// Makes a lowered coroutine poll function a state machine in place (rustc's coroutine
/// transform and coroutine layout): what lives across a suspension point is saved in the
/// coroutine's own state after its upvars -- no allocation, and a pinned coroutine's saved
/// locals keep their addresses. Returns where the state ends and its alignment.
///
/// Saved at a suspension point (rustc: storage-live and maybe-initialized or borrowed there):
/// every value live across it on the paths its drop flags allow, and every stack slot whose
/// contents may be used after it (written before it, and addressed after it, or reachable from
/// a saved pointer, or escaped). A slot's own address is made again on resume, a flag whose
/// value the point knows is set again. Layout as rustc's: slots saved at one point only belong
/// to that point's variant (an awaited future always), the others are promoted to a prefix; the
/// variants overlap. A slot saved nowhere stays on the poll function's stack.
///
/// State word (rustc's discriminant): 0 unresumed, 1 returned, 2 poisoned (running: a panic
/// leaves it), 3 + k suspended at point k. A poll with a null context (the coroutine dropped)
/// runs the drops of its suspension point.
fn coroutine_transform(f: &mut Func, c: &Coro) -> Result<(u32, u32, u64), String> {
    use relax_backend::regalloc::{term_uses, uses_defs};
    // drop flags whose value is known where they are tested (a value moved out before a
    // suspension point: rustc's maybe-initialized analysis) -- the dead drop would keep the
    // value's slot saved across it
    let cp = CoroFlags::new(f);
    let (seen_g, known_g, inn_g) = cp.run(f, c, &[(0, None)]);
    for bi in 0..f.blocks.len() {
        if let (true, Some(t), Term::Branch(_, y, n)) = (seen_g[bi], known_g[bi], f.blocks[bi].term.clone()) {
            f.blocks[bi].term = Term::Jump(if t { y } else { n });
        }
    }
    // a branch whose arm does nothing (a drop flag tested for a drop that is a no-op) keeps
    // its condition live across suspension points for nothing
    for bi in 0..f.blocks.len() {
        if let Term::Branch(_, y, n) = f.blocks[bi].term {
            let empty_to = |x: u32, to: u32| f.blocks[x as usize].insts.is_empty() && matches!(f.blocks[x as usize].term, Term::Jump(j) if j == to);
            if y == n || empty_to(y, n) {
                f.blocks[bi].term = Term::Jump(n);
            } else if empty_to(n, y) {
                f.blocks[bi].term = Term::Jump(y);
            }
        }
    }
    let nv = f.vregs.len();
    let nb = f.blocks.len();
    let ns = f.slots.len();
    // points-to (rustc's borrowed-locals analysis, flow-insensitive): which slots a vreg may
    // point into, which slots' addresses a slot's memory may hold, and which escaped into
    // memory this function does not know (then any later access may reach them)
    let wd = (ns + 63) / 64;
    let mut pts = vec![0u64; nv * wd];
    let mut content = vec![0u64; ns * wd];
    let mut uset = vec![0u64; wd];
    let mut slot_defs: Vec<Option<u32>> = vec![None; nv];
    let mut other_def = vec![false; nv];
    // values that are never a pointer into this frame's memory a callee could store into
    let mut nosink = vec![false; nv];
    nosink[c.cx.0 as usize] = true;
    let (mut us, mut ds) = (Vec::new(), Vec::new());
    for b in &f.blocks {
        for i in &b.insts {
            match i {
                Inst::SlotAddr(d, s) => {
                    let d = d.0 as usize;
                    if slot_defs[d].map_or(false, |x| x != *s) {
                        other_def[d] = true;
                    }
                    slot_defs[d] = Some(*s);
                }
                _ => {
                    uses_defs(i, &mut us, &mut ds);
                    for d in &ds {
                        other_def[d.0 as usize] = true;
                    }
                }
            }
            if let Inst::Iconst(d, _) | Inst::Fconst(d, _, _) = i {
                nosink[d.0 as usize] = true;
            }
        }
    }
    // a vreg with a non-constant definition may be any pointer
    for b in &f.blocks {
        for i in &b.insts {
            match i {
                Inst::Iconst(..) | Inst::Fconst(..) => {}
                Inst::Mov(d, x) if *x == c.cx => {}
                _ => {
                    uses_defs(i, &mut us, &mut ds);
                    for d in &ds {
                        nosink[d.0 as usize] = false;
                    }
                }
            }
        }
    }
    let or = |dst: &mut [u64], src: &[u64]| -> bool {
        let mut ch = false;
        for (x, y) in dst.iter_mut().zip(src) {
            let n = *x | *y;
            if n != *x {
                *x = n;
                ch = true;
            }
        }
        ch
    };
    let empty = |s: &[u64]| s.iter().all(|w| *w == 0);
    let bits = |s: &[u64]| -> Vec<usize> {
        let mut v = Vec::new();
        for (wi, w) in s.iter().enumerate() {
            let mut w = *w;
            while w != 0 {
                v.push(wi * 64 + w.trailing_zeros() as usize);
                w &= w - 1;
            }
        }
        v
    };
    let mut changed = true;
    while changed {
        changed = false;
        for b in &f.blocks {
            for i in &b.insts {
                // what memory at the slots `base` points to may hold (unknown memory: U)
                let read_through = |pts: &[u64], content: &[u64], uset: &[u64], base: VReg| -> Vec<u64> {
                    let pb = &pts[base.0 as usize * wd..][..wd];
                    if empty(pb) {
                        return uset.to_vec();
                    }
                    let mut r = vec![0u64; wd];
                    for s in bits(pb) {
                        for w in 0..wd {
                            r[w] |= content[s * wd + w];
                        }
                    }
                    r
                };
                match i {
                    Inst::SlotAddr(d, s) => {
                        let x = d.0 as usize * wd + *s as usize / 64;
                        if pts[x] & (1 << (s % 64)) == 0 {
                            pts[x] |= 1 << (s % 64);
                            changed = true;
                        }
                    }
                    // a narrower value is never an address
                    Inst::Load(m, ..) | Inst::Store(m, ..) | Inst::AtomicLoad(m, ..) | Inst::AtomicStore(m, ..) if *m != Mem::Int(8, false) && *m != Mem::Int(8, true) => {}
                    Inst::IBin(_, it, ..) | Inst::IBinI(_, it, ..) if it.bits < 64 => {}
                    Inst::ICmp(..) | Inst::ICmpI(..) | Inst::FCmp(..) | Inst::FBin(..) | Inst::FUnary(..) => {}
                    Inst::Load(_, d, base, _) | Inst::VLoad(d, base, _) | Inst::AtomicLoad(_, _, d, base) => {
                        let r = read_through(&pts, &content, &uset, *base);
                        changed |= or(&mut pts[d.0 as usize * wd..][..wd], &r);
                    }
                    Inst::Store(_, base, _, src) | Inst::VStore(base, _, src) | Inst::AtomicStore(_, _, base, src) | Inst::AtomicRmw(_, _, _, _, base, src) | Inst::AtomicCas(_, _, _, _, base, _, src) => {
                        if let Inst::AtomicRmw(_, _, _, d, ..) | Inst::AtomicCas(_, _, _, d, ..) = i {
                            let r = read_through(&pts, &content, &uset, *base);
                            changed |= or(&mut pts[d.0 as usize * wd..][..wd], &r);
                        }
                        let ps = pts[src.0 as usize * wd..][..wd].to_vec();
                        let pb = pts[base.0 as usize * wd..][..wd].to_vec();
                        if empty(&pb) {
                            changed |= or(&mut uset, &ps);
                        } else {
                            for s in bits(&pb) {
                                changed |= or(&mut content[s * wd..][..wd], &ps);
                            }
                        }
                    }
                    Inst::Copy(dst, src, _) => {
                        // pointers into the source itself do not travel with its bytes: a
                        // value referenced from inside cannot be moved (borrowck), so a moved
                        // copy's self-pointers are never used
                        let mut r = read_through(&pts, &content, &uset, *src);
                        for w in 0..wd {
                            r[w] &= !pts[src.0 as usize * wd + w];
                        }
                        let pd = pts[dst.0 as usize * wd..][..wd].to_vec();
                        if empty(&pd) {
                            changed |= or(&mut uset, &r);
                        } else {
                            for s in bits(&pd) {
                                changed |= or(&mut content[s * wd..][..wd], &r);
                            }
                        }
                    }
                    Inst::Call(_, args, rets) => {
                        // a callee may store any pointer it reaches into any memory it reaches,
                        // and return one
                        let mut r = vec![0u64; wd];
                        let mut sink = false;
                        let mut srets = Vec::new();
                        for a in args {
                            let pa = &pts[a.0 as usize * wd..][..wd];
                            if c.call_private.contains(a) {
                                // the callee's own memory: what it holds flows, its address
                                // does not
                                if c.call_sret.contains(a) {
                                    srets.extend(bits(pa));
                                } else {
                                    for s in bits(pa) {
                                        for w in 0..wd {
                                            r[w] |= content[s * wd + w];
                                        }
                                    }
                                }
                                continue;
                            }
                            if empty(pa) && !nosink[a.0 as usize] {
                                sink = true;
                            }
                            for w in 0..wd {
                                r[w] |= pa[w];
                            }
                        }
                        if sink {
                            for w in 0..wd {
                                r[w] |= uset[w];
                            }
                        }
                        let mut grow = true;
                        while grow {
                            grow = false;
                            for s in bits(&r.clone()) {
                                grow |= or(&mut r, &content[s * wd..][..wd]);
                            }
                        }
                        if !empty(&r) {
                            for s in bits(&r).into_iter().chain(srets) {
                                changed |= or(&mut content[s * wd..][..wd], &r);
                            }
                            for d in rets {
                                changed |= or(&mut pts[d.0 as usize * wd..][..wd], &r);
                            }
                            if sink {
                                changed |= or(&mut uset, &r);
                            }
                        }
                    }
                    _ => {
                        // address arithmetic, copies, selects: a result may point where an
                        // operand does
                        uses_defs(i, &mut us, &mut ds);
                        let mut r = vec![0u64; wd];
                        for u in &us {
                            for w in 0..wd {
                                r[w] |= pts[u.0 as usize * wd + w];
                            }
                        }
                        if !empty(&r) {
                            for d in &ds {
                                changed |= or(&mut pts[d.0 as usize * wd..][..wd], &r);
                            }
                        }
                    }
                }
            }
        }
    }
    // blocks touching each slot (through any vreg that may point into it), and writing it
    let mut touched: Vec<Vec<u32>> = vec![Vec::new(); ns];
    let mut written: Vec<Vec<u32>> = vec![Vec::new(); ns];
    for (bi, b) in f.blocks.iter().enumerate() {
        let mut mark = vec![false; ns];
        let mut wmark = vec![false; ns];
        for i in &b.insts {
            // memory this instruction may write (a callee: all it reaches)
            let targets: Vec<VReg> = match i {
                Inst::Store(_, base, ..) | Inst::VStore(base, ..) | Inst::AtomicStore(_, _, base, _) | Inst::AtomicRmw(_, _, _, _, base, _) | Inst::AtomicCas(_, _, _, _, base, _, _) => vec![*base],
                Inst::Copy(dst, ..) => vec![*dst],
                Inst::Call(_, args, _) => args.clone(),
                _ => Vec::new(),
            };
            if !targets.is_empty() {
                let mut r = vec![0u64; wd];
                for t in &targets {
                    for w in 0..wd {
                        r[w] |= pts[t.0 as usize * wd + w];
                    }
                }
                if matches!(i, Inst::Call(..)) {
                    let mut grow = true;
                    while grow {
                        grow = false;
                        for s in bits(&r.clone()) {
                            grow |= or(&mut r, &content[s * wd..][..wd]);
                        }
                    }
                }
                for s in bits(&r) {
                    wmark[s] = true;
                }
            }
            uses_defs(i, &mut us, &mut ds);
            for v in us.iter().chain(ds.iter()) {
                for s in bits(&pts[v.0 as usize * wd..][..wd]) {
                    mark[s] = true;
                }
            }
        }
        term_uses(&b.term, &mut us);
        for v in &us {
            for s in bits(&pts[v.0 as usize * wd..][..wd]) {
                mark[s] = true;
            }
        }
        for s in 0..ns {
            if mark[s] {
                touched[s].push(bi as u32);
            }
            if wmark[s] {
                written[s].push(bi as u32);
            }
        }
    }
    // vreg liveness (backward, to a fixpoint)
    let words = (nv + 63) / 64;
    let mut gen = vec![0u64; nb * words];
    let mut kill = vec![0u64; nb * words];
    let mut used_in: Vec<Vec<u32>> = vec![Vec::new(); nv];
    for (bi, b) in f.blocks.iter().enumerate() {
        let g = bi * words;
        for i in &b.insts {
            uses_defs(i, &mut us, &mut ds);
            for u in &us {
                if used_in[u.0 as usize].last() != Some(&(bi as u32)) {
                    used_in[u.0 as usize].push(bi as u32);
                }
                let x = u.0 as usize;
                if kill[g + x / 64] & (1 << (x % 64)) == 0 {
                    gen[g + x / 64] |= 1 << (x % 64);
                }
            }
            for d in &ds {
                let x = d.0 as usize;
                kill[g + x / 64] |= 1 << (x % 64);
            }
        }
        term_uses(&b.term, &mut us);
        for u in &us {
            if used_in[u.0 as usize].last() != Some(&(bi as u32)) {
                used_in[u.0 as usize].push(bi as u32);
            }
            let x = u.0 as usize;
            if kill[g + x / 64] & (1 << (x % 64)) == 0 {
                gen[g + x / 64] |= 1 << (x % 64);
            }
        }
    }
    // liveness on the paths from a suspension point, with the flag branches its state decides
    // taken one way (rustc: a moved-out value is not live across it); a suspension point's
    // Pending return continues at its resume and cancel blocks
    let live_from = |seen: &[bool], known: &[Option<bool>]| -> Vec<u64> {
        let mut live_in = vec![0u64; nb * words];
        let mut changed = true;
        while changed {
            changed = false;
            for bi in (0..nb).rev() {
                if !seen[bi] {
                    continue;
                }
                let mut out = vec![0u64; words];
                let mut succs: Vec<u32> = match (known[bi], &f.blocks[bi].term) {
                    (Some(t), Term::Branch(_, y, n)) => vec![if t { *y } else { *n }],
                    (_, t) => t.succs().into_iter().collect(),
                };
                for &(pend, resume, cancel) in &c.suspends {
                    if pend as usize == bi {
                        succs.push(resume);
                        succs.push(cancel);
                    }
                }
                for t in succs {
                    for w in 0..words {
                        out[w] |= live_in[t as usize * words + w];
                    }
                }
                for w in 0..words {
                    let v = gen[bi * words + w] | (out[w] & !kill[bi * words + w]);
                    if v != live_in[bi * words + w] {
                        live_in[bi * words + w] = v;
                        changed = true;
                    }
                }
            }
        }
        live_in
    };
    let mut preds: Vec<Vec<u32>> = vec![Vec::new(); nb];
    for (bi, b) in f.blocks.iter().enumerate() {
        for t in b.term.succs() {
            preds[t as usize].push(bi as u32);
        }
    }
    let is_param = |v: usize| f.params.iter().any(|p| p.0 as usize == v);
    // the slot of each awaited future
    let mut await_slot: Vec<Option<usize>> = vec![None; ns];
    for &(a, k) in &c.await_slots {
        if let Some(s) = slot_defs[a.0 as usize] {
            await_slot[s as usize] = Some(k);
        }
    }
    // what each suspension point saves (rustc: storage-live and maybe-initialized or borrowed):
    // the slots a live pointer (other than the slot's own address, made again on resume) may
    // point into, the escaped ones touched before it, the ones written before it whose address
    // is used after it, and what their memory may point to
    // a slot's contents are needed after a point when some path from it reads them before
    // overwriting them whole (a call's result slot, a copy of the slot's size into it): per
    // block, the slots read before such a write (gen) and the ones written whole first (kill)
    let mut sgen = vec![0u64; nb * wd];
    let mut skill = vec![0u64; nb * wd];
    {
        let own = |v: &VReg| match slot_defs[v.0 as usize] {
            Some(s) if !other_def[v.0 as usize] => Some(s as usize),
            _ => None,
        };
        let reach_content = |start: Vec<u64>| -> Vec<u64> {
            let mut r = start;
            let mut grow = true;
            while grow {
                grow = false;
                for s in bits(&r.clone()) {
                    for w in 0..wd {
                        let n = r[w] | content[s * wd + w];
                        if n != r[w] {
                            r[w] = n;
                            grow = true;
                        }
                    }
                }
            }
            r
        };
        for (bi, b) in f.blocks.iter().enumerate() {
            let mut g = vec![0u64; wd];
            let mut k = vec![0u64; wd];
            for i in b.insts.iter().rev() {
                let mut reads = vec![0u64; wd];
                let mut whole: Option<usize> = None;
                let p = |v: &VReg| pts[v.0 as usize * wd..][..wd].to_vec();
                match i {
                    Inst::Load(_, _, base, _) | Inst::VLoad(_, base, _) | Inst::AtomicLoad(_, _, _, base) | Inst::AtomicRmw(_, _, _, _, base, _) | Inst::AtomicCas(_, _, _, _, base, _, _) => reads = p(base),
                    Inst::Copy(dst, src, n) => {
                        reads = p(src);
                        if let Some(s) = own(dst) {
                            if *n >= f.slots[s].size {
                                whole = Some(s);
                            }
                        }
                    }
                    Inst::Call(_, args, _) => {
                        let mut r = vec![0u64; wd];
                        for a in args {
                            if c.call_sret.contains(a) {
                                if let Some(s) = own(a) {
                                    whole = Some(s);
                                    continue;
                                }
                            }
                            let pa = p(a);
                            for w in 0..wd {
                                r[w] |= pa[w];
                            }
                        }
                        reads = reach_content(r);
                    }
                    _ => {}
                }
                if let Some(s) = whole {
                    k[s / 64] |= 1 << (s % 64);
                    g[s / 64] &= !(1 << (s % 64));
                }
                for w in 0..wd {
                    g[w] |= reads[w];
                    k[w] &= !reads[w];
                }
            }
            sgen[bi * wd..(bi + 1) * wd].copy_from_slice(&g);
            skill[bi * wd..(bi + 1) * wd].copy_from_slice(&k);
        }
    }
    let exposed_from = |seen: &[bool], known: &[Option<bool>]| -> Vec<u64> {
        let mut live_in = vec![0u64; nb * wd];
        let mut changed = true;
        while changed {
            changed = false;
            for bi in (0..nb).rev() {
                if !seen[bi] {
                    continue;
                }
                let mut out = vec![0u64; wd];
                let mut succs: Vec<u32> = match (known[bi], &f.blocks[bi].term) {
                    (Some(t), Term::Branch(_, y, n)) => vec![if t { *y } else { *n }],
                    (_, t) => t.succs().into_iter().collect(),
                };
                for &(pend, resume, cancel) in &c.suspends {
                    if pend as usize == bi {
                        succs.push(resume);
                        succs.push(cancel);
                    }
                }
                for t in succs {
                    for w in 0..wd {
                        out[w] |= live_in[t as usize * wd + w];
                    }
                }
                for w in 0..wd {
                    let v = sgen[bi * wd + w] | (out[w] & !skill[bi * wd + w]);
                    if v != live_in[bi * wd + w] {
                        live_in[bi * wd + w] = v;
                        changed = true;
                    }
                }
            }
        }
        live_in
    };
    let mut saved_at: Vec<Vec<usize>> = vec![Vec::new(); ns];
    let mut live_at: Vec<Vec<VReg>> = Vec::new();
    // flags whose value a suspension point knows: set again on resume instead of saved
    let mut const_at: Vec<Vec<(VReg, i64)>> = Vec::new();
    let reach = |from: &[u32], back: bool| -> Vec<bool> {
        let mut seen = vec![false; nb];
        let mut stack: Vec<u32> = from.to_vec();
        for b in from {
            seen[*b as usize] = true;
        }
        while let Some(b) = stack.pop() {
            let next: Vec<u32> = if back { preds[b as usize].clone() } else { f.blocks[b as usize].term.succs().into_iter().collect() };
            for n in next {
                if !seen[n as usize] {
                    seen[n as usize] = true;
                    stack.push(n);
                }
            }
        }
        seen
    };
    for (k, &(pend, resume, cancel)) in c.suspends.iter().enumerate() {
        let mut vs = Vec::new();
        let mut sv = vec![0u64; wd];
        let mut own_addr = vec![false; ns];
        let st: Option<Vec<i64>> = if seen_g[pend as usize] && cp.nt > 0 { Some(inn_g[pend as usize * cp.nt..(pend as usize + 1) * cp.nt].to_vec()) } else { None };
        let (seen_k, known_k, _) = cp.run(f, c, &[(resume, st.as_deref()), (cancel, st.as_deref())]);
        let live_in = live_from(&seen_k, &known_k);
        let exposed = exposed_from(&seen_k, &known_k);
        let mut consts = Vec::new();
        for v in 0..nv {
            let lv = |b: u32| live_in[b as usize * words + v / 64] & (1 << (v % 64)) != 0;
            let known_val = match (&st, cp.idx[v]) {
                (Some(st), x) if x != usize::MAX && st[x] != CF_ANY && st[x] != CF_UNSEEN => Some(st[x]),
                _ => None,
            };
            if let Some(x) = known_val {
                if used_in[v].iter().any(|b| seen_k[*b as usize]) {
                    consts.push((VReg(v as u32), x));
                }
                continue;
            }
            if (lv(resume) || lv(cancel)) && !is_param(v) {
                vs.push(VReg(v as u32));
                match slot_defs[v] {
                    Some(s) if !other_def[v] => own_addr[s as usize] = true,
                    _ => {
                        for w in 0..wd {
                            sv[w] |= pts[v * wd + w];
                        }
                    }
                }
            }
        }
        // the suspension is at the start of `pend`: what runs before it are its predecessors
        let before = reach(&preds[pend as usize].clone(), true);
        for s in 0..ns {
            let written_before = written[s].iter().any(|b| before[*b as usize]);
            let touched_before = touched[s].iter().any(|b| before[*b as usize]);
            let escaped = uset[s / 64] & (1 << (s % 64)) != 0;
            let read_after = (exposed[resume as usize * wd + s / 64] | exposed[cancel as usize * wd + s / 64]) & (1 << (s % 64)) != 0;
            if (escaped && touched_before) || (written_before && read_after) {
                sv[s / 64] |= 1 << (s % 64);
            }
        }
        let mut grow = true;
        while grow {
            grow = false;
            for s in bits(&sv.clone()) {
                grow |= or(&mut sv, &content[s * wd..][..wd]);
            }
        }
        for s in bits(&sv) {
            saved_at[s].push(k);
        }
        live_at.push(vs);
        const_at.push(consts);
    }
    // an awaited future is used only between its creation and its drop, around its own
    // suspension point; its poll's result is dead at it
    for s in 0..ns {
        if let Some(k) = await_slot[s] {
            saved_at[s] = vec![k];
        }
    }
    for &(lo, hi, k) in &c.poll_scratch {
        for s in lo..hi.min(ns as u32) {
            saved_at[s as usize].retain(|x| *x != k);
        }
    }
    // a vreg live across a point that only ever holds one saved slot's address is made again
    let remat = |v: usize| -> Option<u32> {
        match slot_defs[v] {
            Some(s) if !other_def[v] => Some(s),
            _ => None,
        }
    };
    // a vreg holding a parameter plus a constant (a capture's address in the state) is made
    // again on resume too
    let mut ndefs = vec![0u32; nv];
    let mut konst: Vec<Option<i64>> = vec![None; nv];
    for b in &f.blocks {
        for i in &b.insts {
            uses_defs(i, &mut us, &mut ds);
            for d in &ds {
                ndefs[d.0 as usize] += 1;
            }
            if let Inst::Iconst(d, x) = i {
                konst[d.0 as usize] = Some(*x);
            }
        }
    }
    let mut param_off: Vec<Option<(VReg, i64)>> = vec![None; nv];
    for b in &f.blocks {
        for i in &b.insts {
            let k1 = |x: &VReg| if ndefs[x.0 as usize] == 1 { konst[x.0 as usize] } else { None };
            let r = match i {
                Inst::Mov(d, p) if is_param(p.0 as usize) => Some((*d, *p, 0)),
                Inst::IBinI(IOp::Add, it, d, p, x) if it.bits == 64 && is_param(p.0 as usize) => Some((*d, *p, *x)),
                Inst::IBin(IOp::Add, it, d, p, k) if it.bits == 64 && is_param(p.0 as usize) && k1(k).is_some() => Some((*d, *p, k1(k).unwrap())),
                Inst::IBin(IOp::Add, it, d, k, p) if it.bits == 64 && is_param(p.0 as usize) && k1(k).is_some() => Some((*d, *p, k1(k).unwrap())),
                _ => None,
            };
            if let Some((d, p, x)) = r {
                if ndefs[d.0 as usize] == 1 && (i32::MIN as i64..=i32::MAX as i64).contains(&x) {
                    param_off[d.0 as usize] = Some((p, x));
                }
            }
        }
    }
    // rustc's eligibility (coroutine_saved_local_eligibility): a slot saved at one suspension
    // point only goes to that point's variant, unless its storage is live together with a slot
    // of another variant (then the one with more such conflicts is promoted); slots saved at
    // several points are promoted. An awaited future never conflicts: it is live only around
    // its own point.
    let nk = c.suspends.len();
    let mut variant_of: Vec<Option<usize>> = vec![None; ns];
    for s in 0..ns {
        if saved_at[s].len() == 1 || (await_slot[s].is_some() && !saved_at[s].is_empty()) {
            variant_of[s] = Some(saved_at[s][0]);
        }
    }
    let cand: Vec<usize> = (0..ns).filter(|&s| variant_of[s].is_some() && await_slot[s].is_none()).collect();
    if cand.len() > 1 {
        let mut vsucc: Vec<Vec<u32>> = f.blocks.iter().map(|b| b.term.succs().into_iter().collect()).collect();
        for &(pend, resume, cancel) in &c.suspends {
            vsucc[pend as usize].push(resume);
            vsucc[pend as usize].push(cancel);
        }
        let mut vpred: Vec<Vec<u32>> = vec![Vec::new(); nb];
        for (bi, ss) in vsucc.iter().enumerate() {
            for &t in ss {
                vpred[t as usize].push(bi as u32);
            }
        }
        let closure = |from: &[u32], next: &Vec<Vec<u32>>| -> Vec<bool> {
            let mut seen = vec![false; nb];
            let mut stack = from.to_vec();
            for b in from {
                seen[*b as usize] = true;
            }
            while let Some(b) = stack.pop() {
                for &n in &next[b as usize] {
                    if !seen[n as usize] {
                        seen[n as usize] = true;
                        stack.push(n);
                    }
                }
            }
            seen
        };
        // blocks where a slot's contents may be live: after a write, before an access
        let live: Vec<Vec<bool>> = cand
            .iter()
            .map(|&s| {
                let w = closure(&written[s], &vsucc);
                let r = closure(&touched[s], &vpred);
                (0..nb).map(|b| w[b] && r[b]).collect()
            })
            .collect();
        let n = cand.len();
        let mut conflict = vec![vec![false; n]; n];
        let mut count = vec![0usize; n];
        for i in 0..n {
            for j in i + 1..n {
                if variant_of[cand[i]] != variant_of[cand[j]] && (0..nb).any(|b| live[i][b] && live[j][b]) {
                    conflict[i][j] = true;
                    conflict[j][i] = true;
                    count[i] += 1;
                    count[j] += 1;
                }
            }
        }
        let mut promoted = vec![false; n];
        for i in 0..n {
            if promoted[i] {
                continue;
            }
            for j in 0..n {
                if conflict[i][j] && !promoted[j] && !promoted[i] {
                    let r = if count[i] > count[j] { i } else { j };
                    promoted[r] = true;
                }
            }
        }
        for i in 0..n {
            if promoted[i] {
                variant_of[cand[i]] = None;
            }
        }
    }
    // layout (offsets from the coroutine's start): the upvars and state word, the promoted
    // slots by decreasing alignment, then the variants overlapping, each by increasing
    // alignment (rustc: univariant of the prefix, StructKind::Prefixed for the variants)
    let mut align = 1u32;
    let mut slot_off: Vec<Option<u32>> = vec![None; ns];
    let mut off = c.up_size;
    let mut promoted: Vec<usize> = (0..ns).filter(|&s| !saved_at[s].is_empty() && variant_of[s].is_none()).collect();
    promoted.sort_by_key(|&s| std::cmp::Reverse(f.slots[s].align.max(1)));
    for s in promoted {
        let a = f.slots[s].align.max(1);
        align = align.max(a);
        off = (off + a - 1) / a * a;
        slot_off[s] = Some(off);
        off += f.slots[s].size;
    }
    // variant items: (slot or vreg cell, size, align)
    let mut items: Vec<Vec<(Option<usize>, Option<VReg>, u32, u32)>> = vec![Vec::new(); nk];
    for s in 0..ns {
        if let Some(k) = variant_of[s] {
            if k < nk {
                items[k].push((Some(s), None, f.slots[s].size, f.slots[s].align.max(1)));
            }
        }
    }
    // how a saved value is stored: as narrow as every definition of it allows (a byte loaded,
    // a flag) -- rustc saves the local, of its type's size
    let mut width: Vec<Option<(u8, bool)>> = vec![None; nv];
    let mut wide = vec![false; nv];
    for b in &f.blocks {
        for i in &b.insts {
            uses_defs(i, &mut us, &mut ds);
            let w = match i {
                Inst::Load(Mem::Int(n, sg), ..) => Some((*n, *sg)),
                Inst::Iconst(_, x) if (0..256).contains(x) => Some((1, false)),
                Inst::Iconst(_, x) if (0..65536).contains(x) => Some((2, false)),
                Inst::Iconst(_, x) if (0..1 << 32).contains(x) => Some((4, false)),
                Inst::ICmp(..) | Inst::ICmpI(..) | Inst::FCmp(..) => Some((1, false)),
                // narrow integers are kept extended in registers
                Inst::IBin(_, it, ..) | Inst::IBinI(_, it, ..) | Inst::INeg(it, ..) | Inst::INot(it, ..) | Inst::Conv(relax_backend::rir::Conv::IntToInt(it), ..) if it.bits < 64 => Some((((it.bits + 7) / 8).max(1) as u8, it.signed)),
                _ => None,
            };
            for d in &ds {
                let d = d.0 as usize;
                match (w, width[d]) {
                    (None, _) => wide[d] = true,
                    (Some(a), None) => width[d] = Some(a),
                    (Some(a), Some(b)) => {
                        if a.1 != b.1 && a.0 < 8 && b.0 < 8 {
                            wide[d] = true;
                        }
                        width[d] = Some((a.0.max(b.0), a.1 || b.1));
                    }
                }
            }
        }
    }
    let classes = f.vregs.clone();
    let cell_mem = |v: VReg| -> (Mem, u32) {
        let x = v.0 as usize;
        match classes[x] {
            Cls::V128 => (Mem::Int(8, false), 16),
            Cls::F32 => (Mem::F32, 4),
            Cls::F64 => (Mem::F64, 8),
            _ => match width[x] {
                Some((n, sg)) if !wide[x] && n < 8 => (Mem::Int(n, sg), n as u32),
                _ => (Mem::Int(8, false), 8),
            },
        }
    };
    for k in 0..nk {
        for &v in &live_at[k] {
            if remat(v.0 as usize).is_some() || param_off[v.0 as usize].is_some() {
                continue;
            }
            let sz = cell_mem(v).1;
            items[k].push((None, Some(v), sz, sz));
        }
    }
    let mut cell: Vec<Vec<(VReg, u32)>> = vec![Vec::new(); nk];
    let mut end = off;
    for k in 0..nk {
        items[k].sort_by_key(|x| x.3);
        let mut o = off;
        for &(s, v, sz, a) in &items[k] {
            align = align.max(a);
            o = (o + a - 1) / a * a;
            match (s, v) {
                (Some(s), _) => slot_off[s] = Some(o),
                (_, Some(v)) => cell[k].push((v, o)),
                _ => {}
            }
            o += sz;
        }
        end = end.max(o);
    }
    let size = end;
    // MAKEPAD_RUN_CORO_TRACE: the saved slots and values of each coroutine
    if std::env::var_os("MAKEPAD_RUN_CORO_TRACE").is_some() {
        for s in 0..ns {
            if !saved_at[s].is_empty() {
                eprintln!("coro {:?} slot {} size {} saved at {:?} variant {:?} offset {:?}", c.ct, s, f.slots[s].size, saved_at[s], variant_of[s], slot_off[s]);
            }
        }
        for k in 0..nk {
            eprintln!("coro {:?} point {} values {:?}", c.ct, k, cell[k]);
        }
        eprintln!("coro {:?} end {} align {}", c.ct, size, align);
    }
    let abs = |o: u32| o as i64;
    // saved slots live in the state
    for b in f.blocks.iter_mut() {
        for i in b.insts.iter_mut() {
            if let Inst::SlotAddr(d, sl) = *i {
                if let Some(o) = slot_off[sl as usize] {
                    *i = Inst::IBinI(IOp::Add, U64, d, c.env, abs(o));
                }
            }
        }
    }
    let pos0 = f.blocks[0].pos.first().copied().unwrap_or(0);
    let push = |b: &mut relax_backend::rir::Block, i: Inst| {
        b.insts.push(i);
        b.pos.push(pos0);
    };
    // suspensions: save the live values and the state before the Pending return; resuming or
    // cancelling restores them
    let mut resume_blocks = Vec::new();
    let mut cancel_blocks = Vec::new();
    for (k, &(pend, resume, cancel)) in c.suspends.iter().enumerate() {
        let mut pre = Vec::new();
        for &(v, o) in &cell[k] {
            pre.push(if f.vregs[v.0 as usize] == Cls::V128 { Inst::VStore(c.env, abs(o) as i32, v) } else { Inst::Store(cell_mem(v).0, c.env, abs(o) as i32, v) });
        }
        let st = f.vreg(Cls::I);
        pre.push(Inst::Iconst(st, k as i64 + 3));
        pre.push(Inst::Store(c.tag, c.env, c.frame_off, st));
        let pb = &mut f.blocks[pend as usize];
        let n = pre.len();
        let mut ins = pre;
        ins.extend(pb.insts.drain(..));
        pb.insts = ins;
        let mut ps = vec![pos0; n];
        ps.extend(pb.pos.drain(..));
        pb.pos = ps;
        for (target, list) in [(resume, &mut resume_blocks), (cancel, &mut cancel_blocks)] {
            let nbk = f.block();
            let mut b = relax_backend::rir::Block { insts: Vec::new(), pos: Vec::new(), term: Term::Jump(target), term_pos: pos0 };
            for &(v, x) in &const_at[k] {
                push(&mut b, Inst::Iconst(v, x));
            }
            for &v in &live_at[k] {
                match remat(v.0 as usize) {
                    Some(s) => match slot_off[s as usize] {
                        Some(o) => push(&mut b, Inst::IBinI(IOp::Add, U64, v, c.env, abs(o))),
                        // not saved here: this poll's own stack slot
                        None => push(&mut b, Inst::SlotAddr(v, s)),
                    },
                    None if param_off[v.0 as usize].is_some() => {
                        let (p, x) = param_off[v.0 as usize].unwrap();
                        push(&mut b, Inst::IBinI(IOp::Add, U64, v, p, x));
                    }
                    None => {
                        if let Some(&(_, o)) = cell[k].iter().find(|x| x.0 == v) {
                            push(&mut b, if f.vregs[v.0 as usize] == Cls::V128 { Inst::VLoad(v, c.env, abs(o) as i32) } else { Inst::Load(cell_mem(v).0, v, c.env, abs(o) as i32) });
                        }
                    }
                }
            }
            f.blocks[nbk as usize] = b;
            list.push(nbk);
        }
    }
    // completion (every other return) and the end of a cancel: returned
    let susp_ret: Vec<u32> = c.suspends.iter().map(|x| x.0).collect();
    let mark_returned = |f: &mut Func, bi: usize| {
        let one = f.vreg(Cls::I);
        let b = &mut f.blocks[bi];
        b.insts.push(Inst::Iconst(one, 1));
        b.insts.push(Inst::Store(c.tag, c.env, c.frame_off, one));
        b.pos.push(pos0);
        b.pos.push(pos0);
    };
    for bi in 0..nb {
        if matches!(f.blocks[bi].term, Term::Ret(_)) && !susp_ret.contains(&(bi as u32)) && bi as u32 != c.pending_ret {
            mark_returned(f, bi);
        }
    }
    if let Some(ce) = c.cancel_end {
        mark_returned(f, ce as usize);
    }
    // the dispatcher becomes the entry; the old entry continues at `start`
    let start = f.block();
    let old = std::mem::replace(&mut f.blocks[0], relax_backend::rir::Block { insts: Vec::new(), pos: Vec::new(), term: Term::Unreachable, term_pos: pos0 });
    f.blocks[start as usize] = old;
    for b in f.blocks.iter_mut() {
        b.term.map_succs(|t| if t == 0 { start } else { t });
    }
    let w = f.vreg(Cls::I);
    let not_started = f.block();
    let started = f.block();
    let begin = f.block();
    let not_done = f.block();
    let dispatch = f.block();
    push(&mut f.blocks[0], Inst::Load(c.tag, w, c.env, c.frame_off));
    let c0 = f.vreg(Cls::I);
    push(&mut f.blocks[0], Inst::ICmpI(Cond::Eq, false, c0, w, 0));
    f.blocks[0].term = Term::Branch(c0, not_started, started);
    // unresumed: dropped -> nothing to do here; polled -> run from the start
    f.blocks[not_started as usize].term = Term::Branch(c.cx, begin, c.pending_ret);
    {
        let running = f.vreg(Cls::I);
        let b = &mut f.blocks[begin as usize];
        push(b, Inst::Iconst(running, 2));
        push(b, Inst::Store(c.tag, c.env, c.frame_off, running));
        b.term = Term::Jump(start);
    }
    // returned: dropped -> nothing; polled -> panic
    let c1 = f.vreg(Cls::I);
    push(&mut f.blocks[started as usize], Inst::ICmpI(Cond::Eq, false, c1, w, 1));
    let done = f.block();
    f.blocks[started as usize].term = Term::Branch(c1, done, not_done);
    f.blocks[done as usize].term = Term::Branch(c.cx, c.after_done, c.pending_ret);
    // poisoned (a panic left it running): polled -> panic; dropped -> nothing (the unwinding
    // dropped what was live)
    let poisoned = f.block();
    let cz = f.vreg(Cls::I);
    push(&mut f.blocks[not_done as usize], Inst::ICmpI(Cond::Eq, false, cz, w, 2));
    f.blocks[not_done as usize].term = Term::Branch(cz, poisoned, dispatch);
    f.blocks[poisoned as usize].term = Term::Branch(c.cx, c.after_panic, c.pending_ret);
    // suspended at k: resume or cancel there
    let resume_d = f.block();
    let cancel_d = f.block();
    f.blocks[dispatch as usize].term = Term::Branch(c.cx, resume_d, cancel_d);
    for (head, list, running_mark) in [(resume_d, &resume_blocks, true), (cancel_d, &cancel_blocks, false)] {
        let mut cur = head;
        if running_mark {
            let r = f.vreg(Cls::I);
            push(&mut f.blocks[cur as usize], Inst::Iconst(r, 2));
            push(&mut f.blocks[cur as usize], Inst::Store(c.tag, c.env, c.frame_off, r));
        }
        for (k, &target) in list.iter().enumerate() {
            if k + 1 == list.len() {
                f.blocks[cur as usize].term = Term::Jump(target);
                break;
            }
            let ck = f.vreg(Cls::I);
            push(&mut f.blocks[cur as usize], Inst::ICmpI(Cond::Eq, false, ck, w, k as i64 + 3));
            let next = f.block();
            f.blocks[cur as usize].term = Term::Branch(ck, target, next);
            cur = next;
        }
        if list.is_empty() {
            f.blocks[cur as usize].term = Term::Unreachable;
        }
    }
    for b in f.blocks.iter_mut() {
        while b.pos.len() < b.insts.len() {
            b.pos.push(pos0);
        }
    }
    // the state's shape (M5): what each suspension point saves where
    let shape = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (nk, size, align).hash(&mut h);
        for s in 0..ns {
            if !saved_at[s].is_empty() {
                (&saved_at[s], f.slots[s].size, slot_off[s]).hash(&mut h);
            }
        }
        for k in 0..nk {
            for (_, o) in &cell[k] {
                (k, *o).hash(&mut h);
            }
        }
        h.finish()
    };
    Ok((size, align, shape))
}

/// Integer constants of drop-flag-like vregs (every definition an `Iconst`) per block, forward
/// from given starts (rustc's drop elaboration knows these statically): which branches are
/// decided on the paths from there. A suspension point continues at its resume and cancel
/// blocks with its state.
/// Drop elaboration at compile time (rustc_mir_transform/src/elaborate_drops.rs over
/// rustc_mir_dataflow's MaybeInitializedPlaces / MaybeUninitializedPlaces). Lowering gives every
/// move path that needs one a drop flag (a vreg set to 0 at entry, 1 where the path is
/// initialized, 0 where it is moved out or dropped) and tests it at every drop; this decides
/// the tests statically. Per flag a forward analysis keeps "may be 0" (maybe uninitialized)
/// and "may be 1" (maybe initialized); the "any field moved" ladders of an open drop
/// (drop_tree_top: `Or` of field flags) are evaluated the same way. A test whose flag is
/// known (definitely initialized: an unconditional drop; definitely uninitialized: no drop)
/// becomes a jump; only "maybe" tests keep the flag (rustc's DropStyle Static / Dead /
/// Conditional, Open per field). Then the flag writes no remaining test reads are deleted,
/// so a flag lives only where it is maybe -- usually nowhere.
/// `flags`: the flag vregs (each initialized to 0 by an entry instruction). Not for
/// coroutine bodies (their suspension edges are outside the CFG: CoroFlags).
fn elaborate_drop_flags(f: &mut Func, flags: &[VReg]) {
    use relax_backend::regalloc::{for_defs, for_uses, term_uses};
    if flags.is_empty() {
        return;
    }
    let nv = f.vregs.len();
    let nb = f.blocks.len();
    // tracked vregs: the flags and `Or`s of tracked vregs (ladders; within one block, after
    // their operands)
    let mut idx = vec![u32::MAX; nv];
    let mut nt = 0usize;
    for v in flags {
        if idx[v.0 as usize] == u32::MAX {
            idx[v.0 as usize] = nt as u32;
            nt += 1;
        }
    }
    for b in &f.blocks {
        for i in &b.insts {
            if let Inst::IBin(IOp::Or, _, d, a, c) = i {
                if idx[a.0 as usize] != u32::MAX && idx[c.0 as usize] != u32::MAX && idx[d.0 as usize] == u32::MAX {
                    idx[d.0 as usize] = nt as u32;
                    nt += 1;
                }
            }
        }
    }
    let w = (nt + 63) / 64;
    let get = |s: &[u64], k: usize| s[k / 64] >> (k % 64) & 1 != 0;
    let put = |s: &mut [u64], k: usize, on: bool| {
        if on {
            s[k / 64] |= 1 << (k % 64);
        } else {
            s[k / 64] &= !(1 << (k % 64));
        }
    };
    // state: [may0 words | may1 words]
    let transfer = |i: &Inst, st: &mut [u64]| {
        for_defs(i, |d| {
            let k = idx[d.0 as usize];
            if k == u32::MAX {
                return;
            }
            let k = k as usize;
            let (z, o) = match i {
                Inst::Iconst(_, 0) => (true, false),
                Inst::Iconst(_, 1) => (false, true),
                Inst::IBin(IOp::Or, _, _, a, c) if idx[a.0 as usize] != u32::MAX && idx[c.0 as usize] != u32::MAX => {
                    let (ka, kc) = (idx[a.0 as usize] as usize, idx[c.0 as usize] as usize);
                    (get(st, ka) && get(st, kc), get(st, w * 64 + ka) || get(st, w * 64 + kc))
                }
                _ => (true, true),
            };
            put(st, k, z);
            put(st, w * 64 + k, o);
        });
    };
    // reverse postorder
    let mut rpo = Vec::with_capacity(nb);
    {
        let mut seen = vec![false; nb];
        let mut stack: Vec<(u32, bool)> = vec![(0, false)];
        while let Some((b, done)) = stack.pop() {
            if done {
                rpo.push(b);
                continue;
            }
            if seen[b as usize] {
                continue;
            }
            seen[b as usize] = true;
            stack.push((b, true));
            for s in f.blocks[b as usize].term.succs() {
                if !seen[s as usize] {
                    stack.push((s, false));
                }
            }
        }
        rpo.reverse();
    }
    let reach_before: Vec<bool> = {
        let mut r = vec![false; nb];
        for &b in &rpo {
            r[b as usize] = true;
        }
        r
    };
    // forward: in-states, only feasible edges propagate (a known test takes one side)
    let sw = 2 * w;
    let mut inn = vec![0u64; nb * sw];
    let mut reached = vec![false; nb];
    reached[0] = true;
    for x in &mut inn[0..sw] {
        *x = !0;
    }
    let mut st = vec![0u64; sw];
    let mut changed = true;
    while changed {
        changed = false;
        for &b in &rpo {
            let b = b as usize;
            if !reached[b] {
                continue;
            }
            st.copy_from_slice(&inn[b * sw..(b + 1) * sw]);
            for i in &f.blocks[b].insts {
                transfer(i, &mut st);
            }
            let flow = |s: u32, st: &[u64], inn: &mut [u64], reached: &mut [bool], changed: &mut bool| {
                let s = s as usize;
                let to = &mut inn[s * sw..(s + 1) * sw];
                let mut ch = !reached[s];
                reached[s] = true;
                for (t, x) in to.iter_mut().zip(st) {
                    let n = *t | *x;
                    ch |= n != *t;
                    *t = n;
                }
                *changed |= ch;
            };
            match f.blocks[b].term {
                Term::Branch(c, y, n) if idx[c.0 as usize] != u32::MAX => {
                    let k = idx[c.0 as usize] as usize;
                    let (z, o) = (get(&st, k), get(&st, w * 64 + k));
                    if o {
                        let mut sy = st.clone();
                        put(&mut sy, k, false);
                        flow(y, &sy, &mut inn, &mut reached, &mut changed);
                    }
                    if z {
                        let mut sn = st.clone();
                        put(&mut sn, w * 64 + k, false);
                        flow(n, &sn, &mut inn, &mut reached, &mut changed);
                    }
                }
                ref t => {
                    for s in t.succs() {
                        flow(s, &st, &mut inn, &mut reached, &mut changed);
                    }
                }
            }
        }
    }
    // decided tests become jumps
    for &b in &rpo {
        let b = b as usize;
        if !reached[b] {
            continue;
        }
        if let Term::Branch(c, y, n) = f.blocks[b].term {
            if idx[c.0 as usize] == u32::MAX {
                continue;
            }
            st.copy_from_slice(&inn[b * sw..(b + 1) * sw]);
            for i in &f.blocks[b].insts {
                transfer(i, &mut st);
            }
            let k = idx[c.0 as usize] as usize;
            match (get(&st, k), get(&st, w * 64 + k)) {
                (false, true) => f.blocks[b].term = Term::Jump(y),
                (true, false) => f.blocks[b].term = Term::Jump(n),
                _ => {}
            }
        }
    }
    // blocks only the decided tests reached: gone
    for b in 0..nb {
        if reach_before[b] && !reached[b] {
            let blk = &mut f.blocks[b];
            blk.insts.clear();
            blk.pos.clear();
            blk.term = Term::Unreachable;
        }
    }
    // backward liveness of the tracked vregs; writes of dead ones (flag sets, ladder `Or`s)
    // are deleted
    let mut live_in = vec![0u64; nb * w];
    let mut live = vec![0u64; w];
    let mut tu = Vec::new();
    let mut changed = true;
    while changed {
        changed = false;
        for &b in rpo.iter().rev() {
            let b = b as usize;
            if !reached[b] {
                continue;
            }
            for x in live.iter_mut() {
                *x = 0;
            }
            for s in f.blocks[b].term.succs() {
                for (l, x) in live.iter_mut().zip(&live_in[s as usize * w..(s as usize + 1) * w]) {
                    *l |= *x;
                }
            }
            term_uses(&f.blocks[b].term, &mut tu);
            for v in &tu {
                let k = idx[v.0 as usize];
                if k != u32::MAX {
                    put(&mut live, k as usize, true);
                }
            }
            for i in f.blocks[b].insts.iter().rev() {
                for_defs(i, |d| {
                    let k = idx[d.0 as usize];
                    if k != u32::MAX {
                        put(&mut live, k as usize, false);
                    }
                });
                for_uses(i, |v| {
                    let k = idx[v.0 as usize];
                    if k != u32::MAX {
                        put(&mut live, k as usize, true);
                    }
                });
            }
            let to = &mut live_in[b * w..(b + 1) * w];
            if to != &live[..] {
                to.copy_from_slice(&live);
                changed = true;
            }
        }
    }
    for &b in &rpo {
        let b = b as usize;
        if !reached[b] {
            continue;
        }
        for x in live.iter_mut() {
            *x = 0;
        }
        for s in f.blocks[b].term.succs() {
            for (l, x) in live.iter_mut().zip(&live_in[s as usize * w..(s as usize + 1) * w]) {
                *l |= *x;
            }
        }
        term_uses(&f.blocks[b].term, &mut tu);
        for v in &tu {
            let k = idx[v.0 as usize];
            if k != u32::MAX {
                put(&mut live, k as usize, true);
            }
        }
        let blk = &mut f.blocks[b];
        let mut keep = vec![true; blk.insts.len()];
        for (j, i) in blk.insts.iter().enumerate().rev() {
            let dead = match i {
                Inst::Iconst(d, _) | Inst::IBin(IOp::Or, _, d, _, _) => {
                    let k = idx[d.0 as usize];
                    k != u32::MAX && !get(&live, k as usize)
                }
                _ => false,
            };
            if dead {
                keep[j] = false;
                continue;
            }
            for_defs(i, |d| {
                let k = idx[d.0 as usize];
                if k != u32::MAX {
                    put(&mut live, k as usize, false);
                }
            });
            for_uses(i, |v| {
                let k = idx[v.0 as usize];
                if k != u32::MAX {
                    put(&mut live, k as usize, true);
                }
            });
        }
        if keep.iter().any(|k| !k) {
            let mut j = 0;
            blk.insts.retain(|_| {
                j += 1;
                keep[j - 1]
            });
            let mut j = 0;
            blk.pos.retain(|_| {
                j += 1;
                keep[j - 1]
            });
        }
    }
}

struct CoroFlags {
    idx: Vec<usize>,
    nt: usize,
}

const CF_UNSEEN: i64 = i64::MIN;
const CF_ANY: i64 = i64::MIN + 1;

impl CoroFlags {
    fn new(f: &Func) -> CoroFlags {
        let nv = f.vregs.len();
        let mut flaglike = vec![true; nv];
        let (mut us, mut ds) = (Vec::new(), Vec::new());
        for b in &f.blocks {
            for i in &b.insts {
                match i {
                    Inst::Iconst(_, x) if *x != CF_UNSEEN && *x != CF_ANY => {}
                    _ => {
                        relax_backend::regalloc::uses_defs(i, &mut us, &mut ds);
                        for d in &ds {
                            flaglike[d.0 as usize] = false;
                        }
                    }
                }
            }
        }
        for p in &f.params {
            flaglike[p.0 as usize] = false;
        }
        let mut idx = vec![usize::MAX; nv];
        let mut nt = 0;
        for v in 0..nv {
            if flaglike[v] {
                idx[v] = nt;
                nt += 1;
            }
        }
        if nt * f.blocks.len().max(1) > 1 << 24 {
            // too big to track: nothing is known
            return CoroFlags { idx: vec![usize::MAX; nv], nt: 0 };
        }
        CoroFlags { idx, nt }
    }

    /// From `starts` (block, its state: a block's in-state from an earlier run, or all
    /// unknown): (blocks reached, decided branches, in-states).
    fn run(&self, f: &Func, c: &Coro, starts: &[(u32, Option<&[i64]>)]) -> (Vec<bool>, Vec<Option<bool>>, Vec<i64>) {
        let nb = f.blocks.len();
        let nt = self.nt;
        let idx = &self.idx;
        let mut inn = vec![CF_UNSEEN; nb * nt];
        let mut seen = vec![false; nb];
        let mut known: Vec<Option<bool>> = vec![None; nb];
        let mut work = Vec::new();
        for (b, st) in starts {
            let b = *b as usize;
            seen[b] = true;
            for k in 0..nt {
                inn[b * nt + k] = st.map_or(CF_ANY, |s| s[k]);
            }
            work.push(b as u32);
        }
        let meet = |a: i64, b: i64| if a == CF_UNSEEN { b } else if b == CF_UNSEEN || a == b { a } else { CF_ANY };
        while let Some(bi) = work.pop() {
            let bi = bi as usize;
            let mut st = inn[bi * nt..(bi + 1) * nt].to_vec();
            for i in &f.blocks[bi].insts {
                if let Inst::Iconst(d, x) = i {
                    if idx[d.0 as usize] != usize::MAX {
                        st[idx[d.0 as usize]] = *x;
                    }
                }
            }
            let mut succs: Vec<u32> = match &f.blocks[bi].term {
                Term::Branch(cv, y, n) if idx[cv.0 as usize] != usize::MAX && st[idx[cv.0 as usize]] != CF_ANY && st[idx[cv.0 as usize]] != CF_UNSEEN => {
                    let t = st[idx[cv.0 as usize]] != 0;
                    known[bi] = Some(t);
                    vec![if t { *y } else { *n }]
                }
                t => {
                    known[bi] = None;
                    t.succs().into_iter().collect()
                }
            };
            for &(pend, resume, cancel) in &c.suspends {
                if pend as usize == bi {
                    succs.push(resume);
                    succs.push(cancel);
                }
            }
            for sb in succs {
                let sb = sb as usize;
                let mut ch = !seen[sb];
                seen[sb] = true;
                for k in 0..nt {
                    let m = meet(inn[sb * nt + k], st[k]);
                    if m != inn[sb * nt + k] {
                        inn[sb * nt + k] = m;
                        ch = true;
                    }
                }
                if ch && !work.contains(&(sb as u32)) {
                    work.push(sb as u32);
                }
            }
        }
        (seen, known, inn)
    }
}

#[cfg(test)]
mod query_drop_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static FREES: AtomicUsize = AtomicUsize::new(0);
    rt_fn! {
        fn counted_ext_free(blk: u64) {
            if blk != 0 { FREES.fetch_add(1, Ordering::SeqCst); }
            rt_ext_free(blk);
        }
    }

    #[test]
    fn extension_blocks_are_freed_once_for_trivial_values_and_aggregates() {
        isolated_test!(extension_blocks_are_freed_once_for_trivial_values_and_aggregates);
        // the std sources of this checkout (a test binary's target dir need not be next to them)
        std::env::set_var("MAKEPAD_RUN_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
        struct LiveGuard { live: bool, root: u32 }
        impl Drop for LiveGuard {
            fn drop(&mut self) {
                crate::layout::LIVE_SESSION.store(self.live, Ordering::Relaxed);
                crate::layout::set_root_crate(self.root);
                crate::borrowck::event::set_app_scope(Vec::new());
            }
        }
        let dir = std::env::temp_dir().join(format!("relax-ext-drop-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("lib.rs");
        std::fs::write(&path, b"struct Plain { value: u64 }\n").unwrap();
        let (prog, core, krate) = crate::load_with_std(&std::sync::Arc::new(crate::Session::default()), path.to_str().unwrap(), "extension_drop", false).unwrap();
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(dir).unwrap();
        let mut tcx = crate::tcx::Tcx::new();
        tcx.collect(&prog);
        assert!(tcx.errors.is_empty(), "{:?}", tcx.errors);
        let mut plain = None;
        for i in 0..prog.defs.len() {
            let d = DefId(i as u32);
            if prog.def(d).krate == krate && prog.name(d) == "Plain" { plain = Some(d); }
        }
        let plain = tcx.tys.intern(TyKind::Adt(plain.unwrap(), tcx.tys.mk_list(&[]), crate::types::RegionArgsId::EMPTY));
        let aggregate = tcx.tys.intern(TyKind::Tuple(tcx.tys.mk_list(&[plain, plain])));
        let env = crate::tcx::TypingEnv::fully_monomorphized(&prog);
        for ty in [plain, aggregate] { assert!(!tcx.needs_drop(&prog, env, ty)); }
        let _live = LiveGuard {
            live: crate::layout::LIVE_SESSION.swap(true, Ordering::Relaxed),
            root: crate::layout::root_crate(),
        };
        crate::layout::set_root_crate(krate);
        crate::borrowck::event::set_app_scope(vec![prog.crates[krate as usize].root_path().to_string()]);
        let mut u = Unit::new(prog, tcx, core);
        let free: rt_fn_ty!(fn(u64)) = counted_ext_free;
        u.syms.intern(crate::syms::SymKind::Rt("ext_free"), free as usize as u64);
        let ext = u.layout_of(plain).ext.expect("live Plain has an extension word");
        FREES.store(0, Ordering::SeqCst);
        let mut total = 0;
        for ty in [plain, aggregate] {
            assert!(!u.tcx.needs_drop(&u.prog, env, ty));
            assert!(u.needs_drop_glue(ty));
            let layout = u.layout_of(ty);
            let mut value = vec![0u64; (layout.size as usize + 7) / 8];
            let base = value.as_mut_ptr() as u64;
            let offsets = if ty == plain { vec![ext] } else { vec![layout.fields[0] + ext, layout.fields[1] + ext] };
            for &off in &offsets { assert_ne!(rt_ext_get(base + off as u64, 16, 16), 0); }
            let id = u.fn_id(FnKey::Glue(crate::jit::GLUE_DROP, ty));
            let entry = u.entry(id);
            let result = unsafe { crate::jit::call_jit(&mut *u, entry, base) };
            assert!(result.is_ok());
            assert!(u.errors.is_empty(), "{:?}", u.errors);
            total += offsets.len();
            assert_eq!(FREES.load(Ordering::SeqCst), total);
            for off in offsets { assert_eq!(unsafe { *((base + off as u64) as *const u64) }, 0); }
            // A cleared extension word makes subsequent runtime cleanup a no-op.
            assert!(unsafe { crate::jit::call_jit(&mut *u, entry, base) }.is_ok());
            assert_eq!(FREES.load(Ordering::SeqCst), total);
        }
        assert_eq!(total, 3);
    }
    #[test]
    fn queries_during_thread_local_destruction() {
        isolated_test!(queries_during_thread_local_destruction);
        use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
        struct OnDrop { input: Arc<(crate::program::Program, crate::tcx::Tcx)>, success: Arc<AtomicBool> }
        impl Drop for OnDrop {
            fn drop(&mut self) {
                let (prog, tcx) = &*self.input;
                let ok = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    assert!(!tcx.needs_drop(prog, crate::tcx::TypingEnv::fully_monomorphized(&prog), tcx.tys.bool_));
                })).is_ok();
                self.success.store(ok, Ordering::Relaxed);
            }
        }
        thread_local! { static CLEANUP: std::cell::RefCell<Option<OnDrop>> = const { std::cell::RefCell::new(None) }; }
        let input = Arc::new((crate::program::Program::new(&Arc::new(crate::Session::default())), crate::tcx::Tcx::new()));
        let success = Arc::new(AtomicBool::new(false));
        let result = success.clone();
        std::thread::spawn(move || {
            CLEANUP.with(|slot| *slot.borrow_mut() = Some(OnDrop { input: input.clone(), success }));
            let (prog, tcx) = &*input;
            assert!(!tcx.needs_drop(prog, crate::tcx::TypingEnv::fully_monomorphized(&prog), tcx.tys.bool_));
        }).join().unwrap();
        assert!(result.load(Ordering::Relaxed), "query must remain callable from a TLS destructor");
    }

}

/// rustc_middle/src/ty/instance.rs: Display; identities include the method and args.
pub fn shim_name(prog: &crate::program::Program, tcx: &crate::tcx::Tcx, shim: &ShimInstance) -> String {
    let ty = |ty| crate::typeck::ty_to_string(true, prog, tcx, ty);
    let mut name = prog.def_path(shim.kind.def_id());
    name.push('<');
    for (i, &arg) in shim.args.iter().enumerate() {
        if i != 0 { name.push_str(", "); }
        name.push_str(&ty(arg));
    }
    name.push_str("> - ");
    name.push_str(&match shim.kind {
        ShimKind::FnPtr(_, t) => format!("shim({})", ty(t)),
        ShimKind::ClosureOnce { closure, track_caller, .. } => format!("once shim({}, track_caller={track_caller})", prog.def_path(closure)),
        ShimKind::FnPtrAsPtr(_, t) => format!("as_ptr shim({})", ty(t)),
        ShimKind::FnPtrFromPtr(_, t) => format!("from_ptr shim({})", ty(t)),
        ShimKind::Reify(_, reason) => format!("reify shim({reason:?})"),
        ShimKind::VTable(_) => "vtable shim".to_string(),
        ShimKind::Clone(..) | ShimKind::DropGlue(..) => unreachable!("type glue has its own key"),
    });
    name
}

/// ported from N:rustc_mir_transform/src/shim.rs @8d1a764
/// build_call_shim:813-987; build_fn_ptr_{as,from}_ptr_shim:1096-1194.
/// Round 23 maps these bodies onto RIR; closure storage is EXCEPTION(N4).
pub fn shim_func(u: &Unit, shim: &ShimInstance, id: u32) -> Result<Func, String> {
    let def = shim.kind.def_id();
    let signature = u.tcx.fn_item_sig(&u.prog, def).ok_or_else(|| "shim has no signature".to_string())?;
    let norm = |t| u.tcx.normalize_erasing_late_bound_regions(&u.prog, crate::tcx::TypingEnv::fully_monomorphized(&u.prog), u.tcx.tys.subst_erasing_regions(t, &shim.args));
    let mut ps = Vec::new();
    for &p in &signature.params { ps.push(norm(p)); }
    let ret = norm(signature.ret);
    if matches!(shim.kind, ShimKind::VTable(_)) { ps[0] = u.tcx.tys.intern(TyKind::Ptr(true, ps[0])); }
    let body = Body::empty(def, shim.args.clone(), u.prog.def(def).file, ret);
    let mut l = Lcx::new(u, &body, shim_name(&u.prog, &u.tcx, shim), id);
    // shim.rs:773-785: synthesized calls use the shim definition span.
    let definition = u.prog.def(def);
    l.pos = u.prog.files[definition.file as usize].ast.item(definition.item).lo;
    let layout = l.layout(ret);
    match &layout.leaves {
        Some(leaves) if ret_fits(leaves) => { for leaf in leaves { l.f.rets.push(leaf.cls); } }
        _ => { let p = l.f.vreg(Cls::I); l.f.params.push(p); l.f.sret = Some(p); l.sret = Some(p); }
    }
    let mut vals = Vec::new();
    for (index, &p) in ps.iter().enumerate() {
        let (regs, by_ptr) = if signature.header.abi == crate::types::Abi::RustCall && index + 1 == ps.len() { l.abi_tuple_param(p) } else { l.abi_param(p) };
        vals.push((if by_ptr { Val::M(regs[0], 0) } else { Val::L(regs) }, p));
    }
    if matches!(shim.kind, ShimKind::ClosureOnce { track_caller: true, .. })
        || matches!(shim.kind, ShimKind::VTable(_)) && signature.track_caller {
        let p = l.f.vreg(Cls::I); l.f.params.push(p); l.caller_loc = Some(p);
    }
    let result = match shim.kind {
        ShimKind::FnPtrAsPtr(_, ty) | ShimKind::FnPtrFromPtr(_, ty) => {
            assert!(matches!(u.tcx.tys.kind(ty), TyKind::FnPtr(..)));
            // The raw pointer cast and NonNull transmute preserve the pointer bits.
            Val::L(l.to_leaves(vals[0].0.clone(), vals[0].1))
        }
        ShimKind::FnPtr(method, callable) => {
            assert_eq!(vals.len(), 2);
            let once = matches!(u.tcx.lang_items.by_def.get(&u.prog.def(method).parent), Some(LangItem::FnOnce | LangItem::AsyncFnOnce));
            let receiver = if once { vals[0].0.clone() } else {
                let address = l.to_leaves(vals[0].0.clone(), vals[0].1)[0];
                l.read_place(&Place::Mem(address, 0), callable)
            };
            let tuple = vals[1].1;
            let TyKind::Tuple(inputs) = l.kind(tuple) else { panic!("FnPtr shim tuple argument") };
            let address = l.place_addr(&match vals[1].0.clone() { Val::L(r) => Place::Regs(r), Val::M(a, o) => Place::Mem(a, o) }, tuple);
            let mut call_args = Vec::new();
            for i in 0..u.tcx.tys.list(inputs).len() {
                let (place, ty) = l.project(&Place::Mem(address, 0), tuple, 0, i as u32);
                call_args.push((l.read_place(&place, ty), ty));
            }
            let callee = match l.kind(callable) {
                TyKind::FnPtr(inputs, _, header, _) => {
                    let ptr = l.to_leaves(receiver, callable)[0];
                    l.fn_ptr_callee(ptr, u.tcx.tys.list(inputs), ret, header, &mut call_args)
                }
                TyKind::FnDef(d, args, _) => {
                    let instance = u.tcx.resolve_instance_raw(&u.prog, crate::tcx::TypingEnv::fully_monomorphized(&u.prog), d, u.tcx.tys.list(args)).map_err(|e| format!("shim callee: {e:?}"))?;
                    let result = l.emit_instance(instance, call_args, ret);
                    l.ret_val(result, ret);
                    l.finish();
                    l.f.verify().map_err(|e| format!("shim RIR: {e}"))?;
                    return if let Some(error) = l.errors.first() { Err(error.clone()) } else { Ok(l.f) };
                }
                _ => panic!("FnPtr shim callable type"),
            };
            l.emit_call(callee, call_args, ret)
        }
        ShimKind::ClosureOnce { .. } => {
            assert_eq!(vals.len(), 2);
            let ct = vals[0].1;

            let address = l.place_addr(&match vals[0].0.clone() { Val::L(r) => Place::Regs(r), Val::M(a, o) => Place::Mem(a, o) }, ct);
            let tuple = vals[1].1;
            let tuple_address = l.place_addr(&match vals[1].0.clone() { Val::L(r) => Place::Regs(r), Val::M(a, o) => Place::Mem(a, o) }, tuple);
            let TyKind::Tuple(inputs) = l.kind(tuple) else { panic!("ClosureOnce shim tuple argument") };
            let mut call_args = Vec::new();
            call_args.push((Val::L(vec![address]), u.tcx.tys.usize_));
            for i in 0..u.tcx.tys.list(inputs).len() {
                let (place, ty) = l.project(&Place::Mem(tuple_address, 0), tuple, 0, i as u32);
                call_args.push((l.read_place(&place, ty), ty));
            }
            // shim.rs:947-987: own Self across the call, drop on return and unwind.
            let pad = if u.needs_drop_glue(ct) { Some(l.pad_for(&[DropAct::Temp(address, ct, None)])) } else { None };
            l.pad_fixed = Some(pad);
            let target = u.closure_fn_id(ct).expect("ClosureOnce closure slot");
            let result = l.emit_call(Callee::Fn(target), call_args, ret);
            l.pad_fixed = None;
            l.drop_at(address, ct);
            result
        }
        ShimKind::VTable(d) => {
            let address = l.to_leaves(vals[0].0.clone(), vals[0].1)[0];
            let self_ty = shim.args[0];
            vals[0] = (l.read_place(&Place::Mem(address, 0), self_ty), self_ty);
            let instance = crate::instance::Instance::expect_resolve(&u.tcx, &u.prog, crate::tcx::TypingEnv::fully_monomorphized(&u.prog), d, &shim.args);
            l.emit_instance(instance, vals, ret)
        }
        ShimKind::Reify(d, _) => {
            let instance = crate::instance::Instance::expect_resolve(&u.tcx, &u.prog, crate::tcx::TypingEnv::fully_monomorphized(&u.prog), d, &shim.args);
            l.emit_instance(instance, vals, ret)
        },
        ShimKind::Clone(..) | ShimKind::DropGlue(..) => unreachable!("type glue has its own key"),
    };
    l.ret_val(result, ret);
    l.finish();
    if let Some(error) = l.errors.first() { return Err(error.clone()); }
    l.f.verify().map_err(|e| format!("shim RIR: {e}"))?;
    Ok(l.f)
}

// N:rustc_middle/src/mir/terminator.rs: AssertKind::description.
fn arithmetic_overflow_message(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "attempt to add with overflow",
        BinOp::Sub => "attempt to subtract with overflow",
        BinOp::Mul => "attempt to multiply with overflow",
        BinOp::Shl => "attempt to shift left with overflow",
        BinOp::Shr => "attempt to shift right with overflow",
        _ => unreachable!(),
    }
}

/// The runtime helpers lowering interns on first use, by symbol name: a cached entry naming one links
/// before any lowering has run.
pub(crate) fn intern_lazy_rt(u: &mut Unit, name: &str) -> Option<relax_backend::rir::Sym> {
    let (kind, addr) = match name {
        "ext_free" => { let f: rt_fn_ty!(fn(u64)) = rt_ext_free; ("ext_free", f as usize as u64) }
        "ext_get" => { let f: rt_fn_ty!(fn(u64, u64, u64) -> u64) = rt_ext_get; ("ext_get", f as usize as u64) }
        "divrem128" => { let f: rt_fn_ty!(fn(u64, u64, u64, u64, u64) -> WideRet) = rt_divrem128; ("divrem128", f as usize as u64) }
        _ => return None,
    };
    Some(u.syms.intern(crate::syms::SymKind::Rt(kind), addr))
}

/// rustc's AbortUnwindingCalls for a body that cannot unwind (an `extern "C"` / `"system"` fn
/// called from C): a panic must not leave it. rustc_mir_transform/src/abort_unwinding_calls.rs:
/// a call that would unwind straight out of the body (UnwindAction::Continue) gets
/// UnwindAction::Terminate(Abi), and a cleanup pad's UnwindResume becomes UnwindTerminate(Abi),
/// so the frame's own cleanups run first. Here, on the lowered RIR: every call that is not
/// already an Invoke ends its block as one, with a terminate pad as its unwind edge;
/// `Term::Resume` becomes the terminate call. The terminate pad calls `rt_cannot_unwind`, which
/// reports and aborts as rustc's panic_cannot_unwind does. (Calls the optimiser learns cannot
/// unwind go back to plain calls: opt::demote_invokes.)
pub(crate) fn abort_unwinding_calls(f: &mut relax_backend::rir::Func, terminate: relax_backend::rir::Sym) {
    use relax_backend::rir::{Callee, Inst, Term};
    let may_unwind = |i: &Inst| match i {
        Inst::Call(Callee::Host(s), _, _) => *s != terminate,
        Inst::Call(Callee::CHost(_, sig) | Callee::CIndirect(_, sig) | Callee::CFn(_, sig), _, _) => sig.unwinds,
        // asm! without may_unwind never unwinds (Relax rejects may_unwind)
        Inst::Call(Callee::Asm(_), _, _) => false,
        Inst::Call(..) => true,
        _ => false,
    };
    let mut pad: Option<u32> = None;
    let mut work: Vec<usize> = (0..f.blocks.len()).collect();
    while let Some(bi) = work.pop() {
        if matches!(f.blocks[bi].term, Term::Resume) {
            let pos = f.blocks[bi].term_pos;
            f.blocks[bi].insts.push(Inst::Call(Callee::Host(terminate), Vec::new(), Vec::new()));
            f.blocks[bi].pos.push(pos);
            f.blocks[bi].term = Term::Unreachable;
            continue;
        }
        let len = f.blocks[bi].insts.len();
        let ends_invoke = matches!(f.blocks[bi].term, Term::Invoke(..));
        // the first plain call that may unwind (an Invoke's own call keeps its pad)
        let Some(i) = (0..len).find(|&i| !(ends_invoke && i + 1 == len) && may_unwind(&f.blocks[bi].insts[i])) else { continue };
        let pos = f.blocks[bi].pos[i];
        let p = *pad.get_or_insert_with(|| {
            let b = f.block();
            f.blocks[b as usize].insts.push(Inst::Call(Callee::Host(terminate), Vec::new(), Vec::new()));
            f.blocks[b as usize].pos.push(pos);
            f.blocks[b as usize].term = Term::Unreachable;
            f.blocks[b as usize].term_pos = pos;
            b
        });
        // bi: insts[..=i], then Invoke(rest, pad); rest: the remaining insts and bi's terminator
        let rest = f.block();
        let tail_insts = f.blocks[bi].insts.split_off(i + 1);
        let tail_pos = f.blocks[bi].pos.split_off(i + 1);
        let term = std::mem::replace(&mut f.blocks[bi].term, Term::Invoke(rest, p));
        let term_pos = std::mem::replace(&mut f.blocks[bi].term_pos, pos);
        let r = &mut f.blocks[rest as usize];
        r.insts = tail_insts;
        r.pos = tail_pos;
        r.term = term;
        r.term_pos = term_pos;
        work.push(rest as usize);
    }
}

rt_fn! {
/// A panic reached the edge of a function that cannot unwind (an `extern "C"` body): rustc's
/// panic_cannot_unwind -- the message, then abort.
fn rt_cannot_unwind() {
    if crate::jit::in_checked_const_eval() { crate::jit::leave_pending_panic(); }
    eprintln!("panic in a function that cannot unwind");
    std::process::abort();
}
}

rt_fn! {
/// A loop head of a checked program (Options::const_fuel).
fn rt_const_fuel() {
    crate::jit::const_fuel_tick();
}
}

pub fn const_fuel_addr() -> u64 {
    let f: rt_fn_ty!(fn()) = rt_const_fuel;
    f as usize as u64
}

/// How many times the function (every block, the entry initialisations, its parameters) writes `v`.
pub(crate) fn rir_writes(f: &Func, entry_inits: &[Inst], v: VReg) -> u32 {
    let mut n = f.params.iter().filter(|p| **p == v).count() as u32;
    for i in entry_inits.iter().chain(f.blocks.iter().flat_map(|b| b.insts.iter())) {
        i.defs(&mut |d| if d == v { n += 1; });
    }
    n
}

pub fn cannot_unwind_addr() -> u64 {
    let f: rt_fn_ty!(fn()) = rt_cannot_unwind;
    f as usize as u64
}

#[cfg(test)]
mod fold_soundness_tests {
    use super::*;

    /// RIR registers are not SSA: a register a loop assigns again is not a constant, however
    /// constant its first definition looks (`rir_writes` counts every block, the parameters).
    #[test]
    fn a_register_assigned_in_a_loop_is_written_more_than_once() {
        let mut f = Func::new("t".to_string(), 0);
        let x = f.vreg(Cls::I);
        let y = f.vreg(Cls::I);
        let c = f.vreg(Cls::I);
        let entry = f.block();
        let head = f.block();
        let body = f.block();
        f.blocks[entry as usize].insts.push(Inst::Iconst(x, 0));
        f.blocks[entry as usize].term = Term::Jump(head);
        f.blocks[head as usize].insts.push(Inst::ICmpI(Cond::Ne, false, c, x, 0));
        f.blocks[head as usize].term = Term::Branch(c, body, entry);
        f.blocks[body as usize].insts.push(Inst::Iconst(y, 1));
        f.blocks[body as usize].insts.push(Inst::Mov(x, y));
        assert_eq!(rir_writes(&f, &[], x), 2, "x: Iconst in the entry block, Mov in the loop");
        assert_eq!(rir_writes(&f, &[], y), 1);
        assert_eq!(rir_writes(&f, &[], c), 1);
        f.params.push(y);
        assert_eq!(rir_writes(&f, &[], y), 2, "a parameter is a definition");
        let entry_inits = [Inst::Iconst(c, 1)];
        assert_eq!(rir_writes(&f, &entry_inits, c), 2, "the entry initialisations count");
    }
}
