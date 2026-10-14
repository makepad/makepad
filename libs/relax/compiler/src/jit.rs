// ported from N:rustc_middle/src/ty/instance.rs @8d1a764
// ported from N:rustc_trait_selection/src/traits/vtable.rs @8d1a764
//! The live image: executable memory, the per-function patch table with lazy
//! stubs, the host runtime (formatting, panics, faults, hang watchdog) and the
//! compile-on-first-call path.

include!("rt_abi.rs");
use relax_types::next_solver::TraitRef;
use relax_types::instance::{Instance, InstanceKind, ShimKind};
use relax_types::consts::GenericArg;
use crate::lang_items::LangItem;
use crate::lower::{fn_name, Lcx};
use crate::program::{DefId, DefKind, Program};
use crate::tcx::Tcx;
use crate::typeck::{Body, Res};
use crate::types::{TyId, TyKind};
// arch backend dispatch (arm64 lane): both backends expose the same API; the OS services the
// runtime needs are in crate::os (one module per concern)
#[cfg(target_arch = "aarch64")]
use crate::arm64 as x64;
#[cfg(not(target_arch = "aarch64"))]
use crate::x64;
use crate::os;
use crate::owner;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum FnKey {
    Shim(ShimInstance),
    Inst(DefId, Vec<TyId>),
    /// (file, closure expr, parent fn slot)
    Closure(u32, u32, u32),
    /// compiler-generated helper for a type: (kind, type); GLUE_DROP = drop in place
    Glue(u8, TyId),
    /// the sandboxed twin of a slot (sandbox.rs): the same RIR compiled with every memory
    /// access confined to the calling module's arena, calls and fn values redirected to twins
    Sandbox(u32),
}

/// Round 23: the erased nightly Instance identity survives slot interning and caching.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct ShimInstance {
    pub kind: ShimKind,
    pub args: Vec<TyId>,
}

pub const GLUE_DROP: u8 = 0;
pub const GLUE_CLONE: u8 = 4;
/// `dyn Fn` entry for a callable without state (fn item, fn pointer, capture-free closure):
/// `fn(env, args..) -> R` forwarding to it

/// a #[track_caller] fn as a value: forwards with its definition site as the location

/// a live-editable struct's extension block (M10): drops the fields it holds and frees it
pub const GLUE_EXT_DROP: u8 = 3;
/// a tuple-struct / tuple-variant constructor used as a fn value: GLUE_CTOR + variant index,
/// for the constructor's fn pointer type `fn(fields..) -> Adt`
pub const GLUE_CTOR: u8 = 16;
/// a slot of the startup image whose identity is still encoded (cache/image.rs, `name_slot`), or
/// whose key did not decode: never built
pub const GLUE_UNNAMED: u8 = 15;

/// A slot. Heap classes (memlane zones, DESIGN-zones.md F): identity fields are KEPT (built and
/// grown under `zone_alloc::kept`, they survive a destroy of the compiler arena): key, name,
/// code/code_len, pc_map, file, history, compiled, patch_gen, refs, entry_ref*,
/// version, the flags, spec_state/spec_depth, build_ns/cg_ns. Compile-generation fields
/// live in the compiler arena and are forgotten at the swap: rir, rir_refs, build_err, spec,
/// calls, building, inflight (rir_refs and uncalled spec code are copied to the kept heap).
pub struct FnEntry {
    pub key: FnKey,
    /// Fixed after naming; an unnamed startup slot gets its whole name replaced on decode.
    pub name: Box<str>,
    pub code: u64,
    pub pc_map: PcMap,
    /// Allocated on the kept heap only when code history or an inlining caller is recorded.
    pub history: Option<Box<SlotHistory>>,
    /// optimised RIR (input to inlining into callers, tier-up, recompiles)
    /// shared: O3 builds snapshot callees' RIR and optimise off the unit
    pub rir: Option<std::sync::Arc<crate::rir::Func>>,
    /// One slot per site (duplicates matter). Kept when RIR is evicted/invalidated so
    /// a rebuild subtracts the old body's contributions before adding its new ones.
    rir_refs: Box<[u32]>,
    /// the last build's error (a failed build is not retried until the source changes:
    /// callers inlining it would otherwise rebuild it over and over)
    pub build_err: Option<Box<str>>,
    /// cpu ns of this slot's own RIR build (type check, lower, opt; nested builds excluded)
    /// and of its codegen -- MAKEPAD_RUN_COMPILE_REPORT
    pub build_ns: u64,
    pub cg_ns: u64,
    /// code a speculative build made ahead of the first call, installed by that call
    pub spec: Option<Box<SpecCode>>,
    /// the slots this fn's lowered body calls (before inlining; set by its preparation): a
    /// callee that calls its caller back is never inlined into it (inline_cycle)
    pub calls: Box<[u32]>,
    pub code_len: u32,
    pub file: u32,
    pub patch_gen: u32,
    /// Referencing Call(Fn) / FnAddr sites in every built O1 body, plus one
    /// permanent reference for an escaping entry address. Updated on the owner thread.
    pub refs: u32,
    /// bumped by every install and rollback: a O3 build of an older version is dropped
    pub version: u32,
    pub compiled: bool,
    /// entry() is also used by concurrent lowering (vtables), so it publishes the
    /// extra reference atomically; sync_entry_ref folds it into refs under the lock.
    entry_ref: AtomicBool,
    entry_ref_counted: bool,
    pub building: bool,
    /// a pipelined build is optimising / generating this slot off the unit (compile_split)
    pub inflight: bool,
    /// speculation: 0 none, 1 built ahead and not called (yet), 2 built ahead and called
    pub spec_state: u8,
    /// speculation depth this slot was queued at (u8::MAX: not queued)
    pub spec_depth: u8,
    /// Explicit trace/hint seed: use baseline even if its build starts after present.
    pub baseline_hint: AtomicBool,
    /// a Relax core fn called by `// relax: hot` code: compiled like it (marked_hot)
    pub hot_hint: AtomicBool,
    /// O3 code is installed (built or linked): O2 builds must not replace it
    pub o3: AtomicBool,
    /// its installed code is baseline code (note_base) the sampler has not found running yet
    pub base_code: AtomicBool,
}

/// Uninstalled speculative code; boxed so slots without it only retain one pointer.
#[derive(Clone)]
pub struct SpecCode {
    pub code: Vec<u8>,
    pub map: Vec<(u32, u64)>,
    pub relocs: Vec<crate::rir::RelocAt>,
    /// Whether the disk cache records it: false for code that came from the cache.
    pub record: bool,
}

/// Kept state needed only by slots with replaced code or inlining callers.
#[derive(Default)]
pub struct SlotHistory {
    /// previous code versions (for rollback), newest last
    pub prev: Vec<(u64, u32, PcMap, u32)>,
    /// code replaced by O3 code (frames may still run in it): for PC lookups only
    pub retired: Vec<(u64, u32, PcMap)>,
    /// slots that inlined this function (recompiled when it is patched)
    pub inlined_into: Vec<u32>,
}

impl FnEntry {
    /// Reading an untouched slot's history never allocates.
    pub fn history(&self) -> &SlotHistory {
        static EMPTY: SlotHistory = SlotHistory { prev: Vec::new(), retired: Vec::new(), inlined_into: Vec::new() };
        self.history.as_deref().unwrap_or(&EMPTY)
    }
}

pub struct Rt {
    pub fmt_str: u64,
    pub fmt_int: u64,
    pub fmt_float: u64,
    pub fmt_print: u64,
    pub panic_site: u64,
    pub panic_bounds: u64,
    pub str_eq: u64,
    pub fmod: u64,
    pub fmodf: u64,
    /// `panic_impl(msg, msg_len, file, file_len, line, col) -> !` (core::panicking)
    pub panic_impl: u64,
    /// `thread_run(entry: fn(*mut u8), arg) -> u64`: runs `entry(arg)` under a recovery point
    /// on the calling thread; 0 = returned, 1 = panicked/faulted (report printed)
    pub thread_run: u64,
    /// `catch_panic(f: fn(*mut u8), data) -> u64` (core::intrinsics_rt::catch_panic): runs
    /// `f(data)` under a recovery point on this thread (nested in any outer one); 1 = it
    /// panicked or faulted (report printed, the stack reset to here), 0 = returned
    pub catch_panic: u64,
    /// `args() -> (argc, argv)`: the program's C argv (NUL-terminated strings, argv[argc] = null)
    pub args: u64,
    pub memcpy: u64,
    /// libc memmove / memset (intrinsics_mem::copy / write_bytes)
    pub memmove: u64,
    pub memset: u64,
    pub fmt_push: u64,
    pub fmt_pop: u64,
    /// `Term::Resume`'s target: the panic trampoline in front of rt_resume_unwind
    pub resume: u64,
    /// land(regs, fp, sp, pc) glue: continues in a frame's landing stub
    pub land: u64,
    /// `panicking() -> u64`: 1 while this thread unwinds a panic (std::thread::panicking)
    pub panicking: u64,
    /// `rethrow()`: the next panic on this thread continues one already reported (resume_unwind)
    pub rethrow: u64,
}

/// `Rt` as symbols (syms.rs): what lowering and glue put into RIR.
#[derive(Clone, Copy)]
pub struct RtSyms {
    pub fmt_str: crate::rir::Sym,
    pub fmt_int: crate::rir::Sym,
    pub fmt_float: crate::rir::Sym,
    pub fmt_print: crate::rir::Sym,
    pub panic_site: crate::rir::Sym,
    pub panic_bounds: crate::rir::Sym,
    pub str_eq: crate::rir::Sym,
    pub fmod: crate::rir::Sym,
    pub fmodf: crate::rir::Sym,
    pub panic_impl: crate::rir::Sym,
    pub thread_run: crate::rir::Sym,
    pub catch_panic: crate::rir::Sym,
    pub args: crate::rir::Sym,
    pub memcpy: crate::rir::Sym,
    pub memmove: crate::rir::Sym,
    pub memset: crate::rir::Sym,
    pub fmt_push: crate::rir::Sym,
    pub fmt_pop: crate::rir::Sym,
    pub resume: crate::rir::Sym,
    pub land: crate::rir::Sym,
    pub panicking: crate::rir::Sym,
    pub rethrow: crate::rir::Sym,
}


pub struct Site {
    pub file: u32,
    pub pos: u32,
    pub msg: String,
}

#[cfg(not(target_arch = "aarch64"))]
const THUNK_SIZE: usize = 16;
#[cfg(not(target_arch = "aarch64"))]
const STUB_SIZE: usize = 32;
#[cfg(target_arch = "aarch64")]
const THUNK_SIZE: usize = x64::THUNK_SIZE;
#[cfg(target_arch = "aarch64")]
const STUB_SIZE: usize = x64::STUB_SIZE;

/// Reverse slot identities, maintained at slot creation (including lazy/background slots).
/// Keys never change when their installed code changes; old closure versions stay indexed.
#[derive(Default)]
pub(crate) struct ReloadSlots {
    defs: HashMap<DefId, Vec<u32>>,
    parents: HashMap<u32, Vec<u32>>,
    files: HashMap<u32, Vec<u32>>,
}
impl ReloadSlots {
    fn insert(&mut self, key: &FnKey, id: u32) {
        match key {
            FnKey::Inst(d, _) => self.defs.entry(*d).or_default().push(id),
            FnKey::Closure(file, _, parent) => {
                self.parents.entry(*parent).or_default().push(id);
                self.files.entry(*file).or_default().push(id);
            }
            _ => {}
        }
    }
}

#[path = "zone_scan.rs"]
pub(crate) mod zone_scan;
#[path = "zone_swap.rs"]
pub(crate) mod zone_swap;

pub(crate) struct ZoneTaken {
    last_front: Option<std::sync::Arc<FrontShared>>,
    main_compiles: Vec<MainCompile>,
    tier_queue: Vec<(u32, u8, u64, u64)>,
    tier_active: Vec<u32>,
    spec_q: std::collections::VecDeque<u32>,
    spec_built: Vec<(u32, u64)>,
    o2_queue: Vec<(usize, u32)>,
    o2_base: std::collections::VecDeque<(usize, u32)>,
    o2_seen: Option<std::collections::HashSet<(usize, u32)>>,
}

pub(crate) fn zone_take() -> ZoneTaken {
    macro_rules! take {
        ($name:ident => $empty:ident) => {{
            let mut guard = $name.lock().unwrap_or_else(|e| e.into_inner());
            let taken = std::mem::take(&mut *guard);
            #[cfg(test)]
            assert!(guard.$empty());
            taken
        }};
    }
    ZoneTaken {
        last_front: take!(LAST_FRONT => is_none),
        main_compiles: take!(MAIN_COMPILES => is_empty),
        tier_queue: take!(TIER_QUEUE => is_empty),
        tier_active: take!(TIER_ACTIVE => is_empty),
        spec_q: take!(SPEC_Q => is_empty),
        spec_built: take!(SPEC_BUILT => is_empty),
        o2_queue: take!(O2_QUEUE => is_empty),
        o2_base: take!(O2_BASE => is_empty),
        o2_seen: take!(O2_SEEN => is_none),
    }
}

pub(crate) fn zone_put_back(taken: ZoneTaken) {
    fn put<T>(slot: &relax_session::sync::Mutex<T>, old: T) {
        let mut guard = slot.lock().unwrap_or_else(|e| e.into_inner());
        std::mem::forget(std::mem::replace(&mut *guard, old));
    }
    put(&LAST_FRONT, taken.last_front);
    put(&MAIN_COMPILES, taken.main_compiles);
    put(&TIER_QUEUE, taken.tier_queue);
    put(&TIER_ACTIVE, taken.tier_active);
    put(&SPEC_Q, taken.spec_q);
    put(&SPEC_BUILT, taken.spec_built);
    put(&O2_QUEUE, taken.o2_queue);
    put(&O2_BASE, taken.o2_base);
    put(&O2_SEEN, taken.o2_seen);
}

/// Heap classes (memlane zones, DESIGN-zones.md F). KEPT (system heap; built and grown under
/// `zone_alloc::kept`; checked by MAKEPAD_RUN_ZONE_CHECK, zone_scan.rs): the Unit box, code tables,
/// fns/fn_map, consts, const_readers, body_deps, statics/static_spans, sites/site_ix, rt/rs,
/// syms, start_order, vtables, locations, lit_data, provided*, slots, missing_stubs, patch_log,
/// tier-1 state, stats, errors and the scalars/addresses. COMPILER generation (rebuilt or
/// refilled after the swap): prog, tcx, lay, bodies, old_bodies, coro_*, spec_cached, the
/// unwind/unpin/cycle/core-path memos, const_failed, cache, front, ctfe_fn_bodies, query_errors,
/// reload_*.
pub struct Unit {
    pub query_errors: relax_types::infer::diagnostics::DiagCtxt,
    /// retired (retire_unit): an auxiliary unit going away mid-run (the proc-macro world);
    /// dropping it leaves the process-wide machinery (background borrowck, O3) alone
    pub retired: bool,
    /// Short-lived jobs reuse their compile inputs until the entire unit is released.
    /// Reclaiming individual bodies there only adds rebuild work to the load path.
    pub keep_compile_state: bool,
    /// which slot's code holds a pc, and how its frames unwind (a leaf lock: panics, unwinding
    /// and faults read it on their own thread, without a unit call)
    pub code: std::sync::RwLock<CodeTables>,
    pub prog: Program,
    pub tcx: Tcx,
    /// provided crates (provided.rs): crate index -> its shims
    pub provided: Vec<(u32, crate::provided::ProvidedCrate)>,
    /// their opaque types: drop shim address (0: no drop)
    pub provided_types: HashMap<DefId, u64>,
    /// types whose values cross the provided-crate boundary (provided::crossing_types)
    pub provided_shared: std::collections::HashSet<TyId>,
    pub lay: crate::layout::Layouts,
    /// slots: created on any thread (fn_id from lowering), build state changed on the owner thread
    pub fns: crate::conc::AppendVec<FnEntry>,
    pub fn_map: crate::conc::MemoMap<FnKey, u32>,
    /// the reload index of slots by definition / parent / file: built from the slots on the
    /// first query (a reload), kept current from then on; a run without a reload never builds it
    reload_slots: relax_session::sync::Mutex<Option<ReloadSlots>>,
    pub(crate) reload_rir: Option<std::collections::HashSet<u32>>,
    pub(crate) reload_errors: std::collections::HashSet<u32>,
    pub(crate) reload_specs: std::collections::HashSet<u32>,
    /// serialises slot creation (one entry per key, dense ids)
    fn_create: relax_session::sync::Mutex<()>,
    pub consts: crate::fxhash::HashMap<(DefId, Vec<TyId>), u64>,
    /// hot reload: const -> the slots whose build read its value (folded into their code), so an
    /// edit of the const recompiles them (reload.rs)
    pub const_readers: relax_session::sync::Mutex<crate::fxhash::HashMap<DefId, Vec<u32>>>,
    /// per slot: the slots whose optimised RIR its RIR took in this run -- inlined bodies and
    /// callees answering nounwind true (a false answer assumes nothing of the body). Grows
    /// over rebuilds, never shrinks: code made from an earlier RIR names them too. The disk
    /// cache's dependency certificates are its transitive closure (cache.rs).
    pub body_deps: crate::fxhash::HashMap<u32, Vec<u32>>,
    pub statics: crate::fxhash::HashMap<DefId, u64>,
    /// the statics' storage by address: start -> (static, size), so a pointer into a static
    /// (`&S.field`, `&S[3]`) in constant data is that static's symbol plus an offset
    pub static_spans: std::collections::BTreeMap<u64, (DefId, u64)>,
    pub sites: relax_session::sync::Mutex<Vec<Site>>,
    /// one site per (file, position, message): every inlined copy of a panic location is one
    /// site and one symbol, whatever the compile order (one constant pool word)
    site_ix: relax_session::sync::Mutex<HashMap<(u32, u32, String), u64>>,
    pub rt: Rt,
    /// the runtime entries as symbols (what compiled code calls; see syms.rs)
    pub rs: RtSyms,
    /// every run-dependent value compiled code uses (syms.rs)
    pub syms: crate::syms::Syms,
    /// the disk cache of compiled code (cache.rs; MAKEPAD_RUN_CACHE)
    pub cache: Option<Box<crate::cache::CacheCell>>,
    /// slots installed in the first seconds after main, in order (the disk cache's startup
    /// manifest for the next run)
    pub start_order: Vec<u32>,
    pub bodies: crate::fxhash::HashMap<u32, std::sync::Arc<Body>>,
    /// hot reload: checked bodies of older file versions, by (slot, file): closures those
    /// versions made are lowered from them (put_body)
    pub old_bodies: crate::fxhash::HashMap<(u32, u32), std::sync::Arc<Body>>,
    /// coroutine poll functions lowered to find their state layout (coro_frame): the build of
    /// the slot takes this RIR, so the layout callers saw and the code agree
    pub coro_rir: crate::conc::MemoMap<u32, crate::rir::Func>,
    /// coroutine types whose body is being lowered for its layout (on the owner thread)
    pub coro_busy: relax_session::sync::Mutex<Vec<TyId>>,
    /// slots queued for speculation whose code the disk cache has: their worker links it and
    /// skips the type check ahead (a warm run checked ~11k bodies ahead it never used)
    pub spec_cached: crate::conc::MemoMap<u32, ()>,
    /// a coroutine that needed its own layout (a cycle): rustc E0733
    pub coro_cycle: AtomicBool,
    /// drop_may_unwind answers
    pub unwind_drop_cache: crate::conc::MemoMap<TyId, bool>,
    /// types whose drop_may_unwind / is_unpin answer is being computed, per thread
    /// (0 drop, 1 unwind, 2 unpin): a recursive type meets its own placeholder on its own thread only
    cycle_inprog: crate::conc::ShardMap<(u8, TyId, u32), ()>,
    /// (concrete type, dyn type) -> vtable address
    pub vtables: crate::conc::MemoMap<(TyId, TyId), u64>,
    /// static `Location` data per (file, line, col) (#[track_caller] arguments)
    pub locations: relax_session::sync::Mutex<crate::fxhash::HashMap<(String, u32, u32), u64>>,
    /// constants whose evaluation failed (reported once, not re-run per use)
    pub const_failed: std::collections::HashSet<(DefId, Vec<TyId>)>,
    pub core_path_cache: crate::conc::MemoMap<String, Option<DefId>>,
    /// `T: Unpin` answers (noalias on `&mut T` params needs it)
    unpin_cache: crate::conc::MemoMap<TyId, bool>,
    pub errors: Vec<String>,
    pub core_crate: u32,
    // memory
    exec_base: u64,
    exec_size: u64,
    /// where every slot's thunk, stub, table word and tier record are (segments, reserved as ids reach them)
    pub slots: Box<crate::rir::SlotTable>,
    lazy_entry: u64,
    pub enter: u64,
    pub leave: u64,
    /// bump allocators of code / read-only data / static memory (lock-free: lowering threads
    /// allocate too)
    code_next: AtomicU64,
    data_next: AtomicU64,
    /// (bytes, align) of str / byte-string / C-string literals and promoted constants -> their
    /// one read-only copy
    lit_data: relax_session::sync::Mutex<crate::fxhash::HashMap<(Vec<u8>, u64), u64>>,
    data_end: u64,
    /// immutable data range (consts, literals): loads from it fold at compile time
    pub ro_base: u64,
    rw_next: AtomicU64,
    rw_end: u64,
    pub opt: crate::opt::OptCfg,
    pub env: x64::Env,
    pub stats: Stats,
    pub patch_log: Vec<(u32, u32)>, // (slot, generation)
    pub patch_gen: u32,
    /// bytes of the per-thread block handed out by `tls_alloc`
    pub tls_next: u32,
    /// panicking stand-ins for foreign fns this host lacks, by symbol
    missing_stubs: relax_session::sync::Mutex<HashMap<String, u64>>,
    /// tier-up policy (None: O1 only) and its per-slot records / hot glue
    pub tier1: Option<Tier1Cfg>,
    /// O3 left calls to callees that had never run (ATTR_COLD): callee -> those O3
    /// functions, built again once the callee first runs (the path became warm)
    cold_users: HashMap<u32, Vec<u32>>,
    /// O3 rebuilds per slot for that reason (bounded)
    retiers: HashMap<u32, u8>,
    frequency_profiles: HashMap<u32, FrequencyProfile>,
    hot_entry: u64,
    /// state the type checks ahead share with this unit (no owner thread call needed to reach it)
    pub front: std::sync::Arc<FrontShared>,
    /// Instantiated const-fn bodies retained by the data evaluator at their declaration epoch.
    pub ctfe_fn_bodies: relax_session::sync::Mutex<crate::fxhash::HashMap<(DefId, Vec<TyId>), (u64, std::sync::Arc<Body>)>>,

    /// this unit's own address (boxed): shared lowering reaches the unit-changing paths
    /// (constant evaluation) through it on the owner thread, as the runtime does through UNIT
    self_ptr: UnitPtr,
}

#[derive(Default)]
pub struct Stats {
    pub inline_rules: [u32; crate::opt::RULE_NAMES.len()],
    pub compiled: u32,
    pub opt_ns: u64,
    pub rir_insts: u64,
    pub typeck_ns: u64,
    pub lower_ns: u64,
    pub codegen_ns: u64,
    pub code_bytes: u64,
    /// wall time spent inside outermost compile_slot calls (on the owner thread), and when the last one
    /// ended (ns since the unit's first compile request epoch, see COMPILE_EPOCH)
    pub compile_wall_ns: u64,
    pub last_compile_end_ns: u64,
    /// per 100 ms of wall time: (compiles ended, compile wall ns) -- the startup timeline
    pub timeline: Vec<(u32, u64)>,
    /// O3 (queued: TIER_QUEUED): functions installed, dropped (failed or outdated); cpu
    /// time the O3 thread spent on and off the unit (optimise + codegen)
    pub tier1_installed: u32,
    /// O3 rebuilds after a never-run callee ran
    pub tier1_retier: u32,
    pub tier1_dropped: u32,
    /// O3 code installed from the disk cache
    pub tier1_cached: u32,
    /// O3 drops by reason
    pub tier1_drop_why: std::collections::BTreeMap<&'static str, u32>,
    pub tier1_locked_ns: u64,
    pub tier1_free_ns: u64,
    /// constant initializers evaluated to data / compiled and run; cpu of the evaluations
    pub ctfe_data: u32,
    pub ctfe_code: u32,
    pub ctfe_ns: u64,
}

/// The code address tables (JIT_THREADS.md §2.10): which slot's code holds a pc and how its
/// frames unwind. A leaf lock: installs write it for a few instructions, panics, unwinding,
/// faults and the sampler read it on their own thread.
#[derive(Default)]
pub struct CodeTables {
    /// (code start, code len, slot) of every code body ever installed, in address order
    /// (emit_code allocates upward; sandbox code spaces lie below later unit code)
    pub index: Vec<(u64, u32, u32)>,
    /// unwind info (callee-saved register saves, frame size, landing stubs) by code start
    pub unwind: HashMap<u64, crate::rir::UnwindInfo>,
    /// unwind trailers of code installed from the startup image, read in place there (code
    /// start, trailer address and length; ascending): parsed when a frame of that code
    /// unwinds. `image_keep` keeps the image mapped.
    image_unwind: Vec<(u64, u64, u32)>,
    image_keep: Option<std::sync::Arc<PcFiles>>,
}

impl CodeTables {
    fn add(&mut self, at: u64, len: u32, id: u32, unwind: Option<crate::rir::UnwindInfo>) {
        // identity state (panics and faults walk it): the tables are the program's
        let _k = crate::zone_alloc::kept();
        if let Some(i) = unwind {
            let own = crate::rir::UnwindInfo { saved: i.saved.clone(), pads: i.pads.clone(), sp_off: i.sp_off };
            self.unwind.insert(at, own);
        }
        if self.index.last().map_or(true, |l| l.0 < at) {
            self.index.push((at, len, id));
        } else {
            let k = self.index.partition_point(|x| x.0 < at);
            self.index.insert(k, (at, len, id));
        }
    }

    fn reserve(&mut self, n: usize) {
        let _k = crate::zone_alloc::kept();
        self.index.reserve(n);
        self.image_unwind.reserve(n);
    }

    fn add_image_unwind(&mut self, at: u64, trailer: &'static [u8], pc_map: &PcMap) {
        let _k = crate::zone_alloc::kept();
        let e = (at, trailer.as_ptr() as u64, trailer.len() as u32);
        if self.image_unwind.last().map_or(true, |l| l.0 < at) {
            self.image_unwind.push(e);
        } else {
            let k = self.image_unwind.partition_point(|x| x.0 < at);
            self.image_unwind.insert(k, e);
        }
        if self.image_keep.is_none() {
            self.image_keep = pc_map.keep();
        }
    }

    /// The code body holding `pc`: (start, len, slot).
    pub fn entry_at(&self, pc: u64) -> Option<(u64, u32, u32)> {
        let k = self.index.partition_point(|x| x.0 <= pc);
        let e = *self.index.get(k.checked_sub(1)?)?;
        (pc < e.0 + e.1 as u64).then_some(e)
    }

    pub fn slot_at(&self, pc: u64) -> Option<u32> {
        self.entry_at(pc).map(|e| e.2)
    }

    /// The unwind info of the startup image code at `start` (its trailer, parsed now).
    fn image_unwind_at(&self, start: u64) -> Option<crate::rir::UnwindInfo> {
        let k = self.image_unwind.partition_point(|x| x.0 < start);
        let &(at, t, n) = self.image_unwind.get(k)?;
        if at != start {
            return None;
        }
        unsafe { crate::rir::read_unwind_info(t, n as u64) }
    }
}

// ------------------------------------------------------------ OS memory

pub use crate::clock::{cpu_ns, wall_ns};

/// Lowering runs on `&Unit` (phase B2): every Unit state it changes is behind interior
/// mutability, so the unit is Sync.
#[allow(dead_code)]
fn _assert_unit_sync() {
    fn s<T: Sync>() {}
    s::<Unit>();
}

unsafe fn write_bytes(addr: u64, b: &[u8]) {
    std::ptr::copy_nonoverlapping(b.as_ptr(), addr as *mut u8, b.len());
}

// ------------------------------------------------------------ thread-local blocks

/// Size of each thread's thread-local block (virtual; pages are touched on use).
const TLS_CAP: usize = 1 << 20;
/// Offset of the first thread-local variable: slot 0 is the domain base; slot 1 owns runtime
/// thread registrations and Relax std TLS values, drained before the block is freed.
const TLS_FIRST_FREE: u32 = 16;
static TLS_KEY: AtomicU64 = AtomicU64::new(u64::MAX);

extern "C" fn tls_dtor(p: *mut u8) {
    // pthread clears its key before calling us. Reinstall it while Relax std destructors run.
    os::tls::set(tls_key(), p);
    let registrations = unsafe { crate::domain_threads::finish(p as u64) };
    os::tls::set(tls_key(), std::ptr::null_mut());
    os::mem::free(p as u64, TLS_CAP);
    for (base, token) in registrations { crate::domain::thread_finished(base, token); }
}

/// The OS key holding each thread's block (one per process).
fn tls_key() -> u64 {
    let k = TLS_KEY.load(Ordering::Acquire);
    if k != u64::MAX {
        return k;
    }
    static CREATE: relax_session::sync::Mutex<()> = relax_session::sync::Mutex::new(());
    let _g = CREATE.lock().unwrap_or_else(|e| e.into_inner());
    if TLS_KEY.load(Ordering::Acquire) == u64::MAX {
        TLS_KEY.store(os::tls::key_create(tls_dtor), Ordering::Release);
    }
    TLS_KEY.load(Ordering::Acquire)
}

/// A short callback from a long-lived native worker gets a scoped app TLS block. In
/// particular, the live-edit watcher must not keep an old domain registered until it exits.
pub(crate) fn domain_callback<R>(base: u64, token: u64, f: impl FnOnce() -> R) -> R {
    let _kept = crate::zone_alloc::kept();
    let previous = tls_block();
    struct Restore(u64);
    impl Drop for Restore {
        fn drop(&mut self) {
            tls_dtor(os::tls::get(tls_key()));
            os::tls::set(tls_key(), self.0 as *mut u8);
            crate::domain::enter(unsafe { *(self.0 as *const u64) });
        }
    }
    os::tls::set(tls_key(), std::ptr::null_mut());
    let _restore = Restore(previous);
    crate::domain::relax_domain_enter(base, token);
    f()
}

thread_local! {
    static ALTSTACK: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Our fault handlers back on top if a library of the program replaced them (it stays chained
/// for signals that are not faults in JIT code). macOS; elsewhere nothing yet.
pub fn reassert_signals() {
    os::fault::reassert();
}

/// Gives the calling thread its fault stack (once), so a stack overflow in JIT code on a thread
/// the program started is still reported. Called on the thread's first lazy compile /
/// thread-local block and from the fault handler.
pub(crate) fn ensure_altstack() {
    if ALTSTACK.with(|a| a.replace(true)) {
        return;
    }
    os::fault::this_thread();
}

rt_fn! {
/// Allocates the calling thread's thread-local block (called by the TlsAddr slow path).
fn rt_tls_block() -> u64 {
    let _keep = os::proc::KeepLastError::new();
    ensure_altstack();
    let key = tls_key();
    let b = os::tls::get(key);
    if !b.is_null() {
        return b as u64;
    }
    let b = os::mem::alloc_rw(TLS_CAP);
    // a thread nothing assigned a domain to (one native code started, or the first use of a
    // thread JIT code started before its start put it in its parent's domain): the default
    unsafe { *(b as *mut u64) = crate::domain::default_base() };
    os::tls::set(key, b as *mut u8);
    b
}
}

/// What this compiler was built from (build.rs, build_stamp.rs): "fastrust <hash> relax <hash>",
/// content hashes of the makepad crates and of Relax's own crates and std it compiles; "unknown"
/// when built without the build script (the bring-up checker).
pub const PROVENANCE: &str = match option_env!("MAKEPAD_RUN_PROVENANCE") {
    Some(s) => s,
    None => "unknown",
};

// ------------------------------------------------------------ global runtime state

/// The unit reached by the lazy-compile entry and the runtime: set by `call_jit` and
/// kept afterwards, so threads that JIT code started keep reaching it.
pub(crate) static mut UNIT: *mut Unit = std::ptr::null_mut();
/// Tokens identify calls, never a process-wide arm/disarm generation.
static WATCHDOG_ID: AtomicU64 = AtomicU64::new(1);
struct WatchdogCall {
    token: u64,
    target: u64,
    deadline: Option<std::time::Instant>,
    parent: Option<std::sync::Arc<WatchdogCall>>,
    hit: AtomicBool,
}
static WATCHDOG_CALLS: relax_session::sync::Mutex<Vec<std::sync::Arc<WatchdogCall>>> = relax_session::sync::Mutex::new(Vec::new());
static WATCHDOG_WAKE: relax_session::sync::Condvar = relax_session::sync::Condvar::new();
crate::gen_local! {
    static WATCHDOG_STACK: std::cell::RefCell<Vec<std::sync::Arc<WatchdogCall>>>;
}
thread_local! {
    // The signal handler must not borrow a RefCell interrupted during enter/leave.
    // The stack and worker registry own the pointed-to call until this is reset.
    static WATCHDOG_CURRENT: std::cell::Cell<*const WatchdogCall> = const { std::cell::Cell::new(std::ptr::null()) };
    // A timeout caught by an inner guard propagates until its owning guard leaves.
    static WATCHDOG_EXPIRED: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Set when the app is asked to quit: its shutdown's waits are not frame hitches.
pub static QUIT_SEEN: AtomicBool = AtomicBool::new(false);

/// The main thread's longest wait for a unit call after the first present (ms), and what the
/// owner thread was serving when it began: a frame hitch (printed with the run summary).
pub fn main_post_present_wait() -> (f64, String) {
    (owner::MAIN_WAIT_MAX.load(Ordering::Relaxed) as f64 / 1e6, format!("behind {} work", owner::main_wait_behind()))
}

/// O3 drops by reason (on the owner thread).
unsafe fn drop_why(up: *mut Unit, why: &'static str) {
    crate::zone_alloc::kept_with(|| *(&mut *up).stats.tier1_drop_why.entry(why).or_insert(0) += 1);
}

/// A recursive #[relax(O3)] build with a larger stack frame is not recorded in the disk cache
/// (a warm launch would install it at the first call, before a deep recursion has run, and
/// multiply its frame by the depth: makepad_script_compute::lower::expr: 68 KB per level
/// overflowed a 2 MB kernel worker).
const TIER1_CACHE_FRAME_MAX: u32 = 16 * 1024;
/// Non-recursive O3 code is recorded up to this frame (one frame per call on any stack).
const TIER1_CACHE_FRAME_MAX_FLAT: u32 = 256 * 1024;
/// Installs before this wall time go to the startup manifest (run.rs sets it at main), as do
/// all installs before the first present.
pub static ORDER_UNTIL_NS: AtomicU64 = AtomicU64::new(0);
/// How many of those installs came before the first present.
pub static ORDER_PRESENT_LEN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Queues the previous run's startup manifest (disk cache) for the speculative workers, in the
/// order that run installed the slots: they link the cached code ahead of the main thread.
/// The keys are decoded on a thread of their own, a chunk per unit call.
pub unsafe fn spec_seed_manifest(up: *mut Unit) {
    let session = unsafe { (*std::ptr::addr_of!((*up).prog.session)).clone() };
    // after the first present: what the earlier run installed after its first present (the
    // slots before it were linked as this run's start called them)
    let list = owner::call_quick(owner::Class::App, || (&mut *up).cache.as_ref().map(|c| c.manifest[c.manifest_present..].to_vec()).unwrap_or_default());
    if list.is_empty() || !pipelined() {
        return;
    }
    struct P(*mut Unit);
    unsafe impl Send for P {}
    let p = P(up);
    { let _k = crate::zone_alloc::kept();
    let _ = std::thread::Builder::new().name("run-seed".to_string()).spawn(move || { let _z = crate::zone_alloc::compiler_thread();
        let runtime = &session.runtime;
        let p = p;
        let up = p.0;
        set_worker();
        lower_worker_priority(runtime);
        while startup_phase() {
            if SPEC_STOP.load(Ordering::SeqCst) { return; }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        for chunk in list.chunks(256) {
            owner::call(owner::Class::Background, || {
                let mut ids = Vec::with_capacity(chunk.len());
                for b in chunk {
                    if let Some(id) = crate::cache::slot_for_bytes_unit(up, b) {
                        ids.push(id);
                    }
                }
                spec_seed(up, &ids);
            });
        }
    });
    }
}

/// Installs one slot of the startup image (on the owner thread): as install_compiled does for code
/// from the disk cache, with the borrowck note collected in `notes` (sent after the start).
unsafe fn install_prelinked_slot(up: *mut Unit, s: crate::cache::PrelinkedSlot, gandalf: bool, notes: &mut Vec<u32>, arm: &mut Vec<(u32, u64)>) {
    let u = &mut *up;
    if u.fns[s.id as usize].compiled {
        return;
    }
    let tier1 = s.inlined.is_some();
    if let Some(inl) = &s.inlined {
        cached_tier1(up, s.id, inl);
    }
    if s.o2 {
        o2_cached(up, s.id);
    }
    if s.base {
        note_base(up, s.id);
    }
    let u = &mut *up;
    // the startup set of the next manifest (install_compiled's rule: before the first present)
    if u.fns[s.id as usize].patch_gen == 0 && (!FIRST_PRESENT.load(Ordering::Acquire) || wall_ns() < ORDER_UNTIL_NS.load(Ordering::Relaxed)) {
        crate::zone_alloc::kept_with(|| u.start_order.push(s.id));
        if !FIRST_PRESENT.load(Ordering::Acquire) {
            ORDER_PRESENT_LEN.store(u.start_order.len(), Ordering::Relaxed);
        }
    }
    u.install_prelinked(s.id, s.at, s.len, s.trailer, s.pc);
    // (perf maps and the cache trace name the slot: prelink_startup decoded the identities first)
    if perf_map_on() {
        let tier = if tier1 { " [O3]" } else if s.base { " [B]" } else if s.o2 { " [O2]" } else { " [O1]" };
        perf_map(s.at, s.len as usize, &format!("{}{}", perf_name(u, s.id), tier));
    }
    // an instance or a closure (image slots are never foreign fns: their code is not stored):
    // its call counter, armed with the others of the block (arm_counters)
    if s.kind <= 1 {
        arm.push((s.id, s.at));
    } else {
        u.frequency_profiles.remove(&s.id);
    }
    train_cached_tier1(u, s.id);
    crate::cache::trace_lookup(u, s.id, "prelink");
    crate::cache::trace_lookup(u, s.id, if tier1 { "hit1" } else { "hit" });
    if gandalf && s.kind == 0 {
        notes.push(s.id);
    }
}

/// The disk cache's startup image (cache.rs prelink): the code of the slots an earlier run of
/// the program installed before its first present, placed and linked as one block before main
/// and installed here -- instead of a key, a lookup, a link and an install per slot on its first
/// call. Slots whose crates changed, whose identities do not resolve or that have code already
/// stay lazy. MAKEPAD_RUN_PRELINK=0 turns it off. Returns the slots installed.
pub unsafe fn prelink_startup(up: *mut Unit) -> usize {
    if std::env::var_os("MAKEPAD_RUN_PRELINK").map_or(false, |v| v == "0") || std::env::var_os("MAKEPAD_RUN_HINTS_RECORD").is_some() {
        return 0;
    }
    let prelinked = owner::call(owner::Class::App, || {
        let usable = (&*up).cache.as_ref().map_or(false, |c| c.mode != crate::cache::Mode::Verify && c.startup_manifest_valid());
        if !usable {
            return None;
        }
        let options = (&*up).prog.session.clone().options.clone();
        let gandalf = crate::borrowck::lazy::gandalf_enabled(&options);
        let mut notes = Vec::new();
        // the identities of the image's slots are decoded after main started (complete_identities),
        // unless this run needs them all now: lazily skipped files (a key naming one does not decode
        // until it is loaded: decoded now, its slots stay lazy), perf maps, the cache trace and O3
        // profile replay (they name the slots as they install them)
        let eager = !(&*up).prog.skipped_files.is_empty() || perf_map_on() || crate::cache::trace_on() || t1prof_replay(&options);
        let n = crate::cache::prelink_unit(up, eager, &mut |u: &mut Unit, slots: Vec<crate::cache::PrelinkedSlot>| {
            u.code.write().unwrap_or_else(|e| e.into_inner()).reserve(slots.len());
            crate::zone_alloc::kept_with(|| u.start_order.reserve(slots.len()));
            let mut arm = Vec::with_capacity(slots.len());
            let up: *mut Unit = u;
            for s in slots {
                install_prelinked_slot(up, s, gandalf, &mut notes, &mut arm);
            }
            (&mut *up).arm_counters(&arm);
        });
        if IDS_PENDING.load(Ordering::Acquire) {
            struct P(*mut Unit);
            unsafe impl Send for P {}
            let p = P(up);
            { let _k = crate::zone_alloc::kept();
            let _ = std::thread::Builder::new().name("run-ids".to_string()).spawn(move || { let _z = crate::zone_alloc::compiler_thread();
                let p = p;
                let up = p.0;
                IDS_EXEMPT.with(|e| e.set(true));
                loop {
                    // (its turn like any holder: a waiting thread takes the unit next and decodes the
                    // rest itself)
                    let done = owner::call_quick(owner::Class::Background, || {
                        let left = IDS_PENDING.load(Ordering::Acquire);
                        let done = !left || crate::cache::complete_identities_unit(up, 512, true);
                        if left && done {
                            IDS_PENDING.store(false, Ordering::Release);
                            ids_report(up);
                        }
                        done
                    });
                    if done {
                        return;
                    }
                }
            });
            }
        }
        Some((n, notes))
    });
    let Some((n, notes)) = prelinked else { return 0 };
    // the background borrow checker learns of them after the start (each note computes a
    // verdict key: per slot work the start does not wait for)
    if !notes.is_empty() {
        struct P(*mut Unit);
        unsafe impl Send for P {}
        let p = P(up);
        { let _k = crate::zone_alloc::kept();
        let _ = std::thread::Builder::new().name("run-prelink-notes".to_string()).spawn(move || { let _z = crate::zone_alloc::compiler_thread();
            let p = p;
            let up = p.0;
            set_worker();
            while startup_phase() || !FIRST_PRESENT.load(Ordering::Acquire) && wall_ns() < ORDER_UNTIL_NS.load(Ordering::Relaxed) {
                if SPEC_STOP.load(Ordering::SeqCst) {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            for chunk in notes.chunks(256) {
                if SPEC_STOP.load(Ordering::SeqCst) {
                    return;
                }
                owner::call(owner::Class::Background, || {
                    for &id in chunk {
                        (&mut *up).note_first_borrowck(id);
                    }
                });
            }
        });
        }
    }
    n
}

/// The startup image's slots have identities still encoded (cache/image.rs): the owner decodes
/// the rest before it serves any request that is not quick, so every unit call sees every
/// slot's key (`ids_on_serve`); a background thread decodes them meanwhile, a chunk per call
/// (prelink_startup).
static IDS_PENDING: AtomicBool = AtomicBool::new(false);
static IDS_UNIT: std::sync::atomic::AtomicPtr<Unit> = std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());
/// who made the owner decode the rest (the verbose report)
static IDS_FORCED_BY: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(u8::MAX);
thread_local! {
    /// the owner is decoding them (a request it serves meanwhile does not start again)
    static IDS_EXEMPT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Before a request that is not quick (owner): the startup identities still encoded are
/// decoded first.
#[inline]
pub(crate) fn ids_on_serve() {
    if IDS_PENDING.load(Ordering::Acquire) && !IDS_EXEMPT.with(|e| e.get()) {
        IDS_FORCED_BY.store(owner::serving().0 as u8, Ordering::Relaxed);
        ids_complete();
    }
}

/// Decodes every startup identity still encoded (owner).
fn ids_complete() {
    let up = IDS_UNIT.load(Ordering::Acquire);
    if up.is_null() {
        return;
    }
    let was = IDS_EXEMPT.with(|e| e.replace(true));
    unsafe { crate::cache::complete_identities_unit(up, usize::MAX, false) };
    IDS_PENDING.store(false, Ordering::Release);
    IDS_EXEMPT.with(|e| e.set(was));
    ids_report(up);
}

/// Startup identities still encoded: true on the owner (the caller decodes what it needs);
/// elsewhere a unit call decodes them all first.
#[inline]
fn ids_pending_here() -> bool {
    if !IDS_PENDING.load(Ordering::Acquire) || IDS_EXEMPT.with(|e| e.get()) {
        return false;
    }
    if owner::is_owner() {
        return true;
    }
    owner::call(owner::Class::App, || ());
    false
}

/// Before slot `id` is built: its identity decoded, if it is a startup image slot's.
#[inline]
fn ids_name(up: *mut Unit, id: u32) {
    if ids_pending_here() {
        unsafe { crate::cache::name_unnamed_unit(up, id) };
    }
}

/// The startup image's slots were made with their identities encoded (cache/image.rs prelink,
/// a unit call): from now on a lookup by key or a build decodes them first.
pub(crate) fn ids_pending(up: *mut Unit) {
    IDS_UNIT.store(up, Ordering::Release);
    IDS_PENDING.store(true, Ordering::Release);
}

/// --verbose: how the startup identities were decoded (once, when the last is).
fn ids_report(up: *mut Unit) {
    let by = match IDS_FORCED_BY.load(Ordering::Relaxed) {
        u8::MAX => "the background decoder".to_string(),
        c => format!("a {} request", owner::CLASS_NAMES[c as usize & 7]),
    };
    unsafe { crate::cache::ids_report_unit(up, &by) };
}

/// O3 jobs that panicked outside their build's recovery (slot left at O1)
pub static TIER1_PANICS: AtomicU64 = AtomicU64::new(0);

/// fork() in a program run by Relax: the child has only the forking thread, and may go on
/// running JIT code (a lazy compile in the child). The loader hands programs its own `fork`
/// (loader_symbol): before the fork every compile job finishes or waits at its start, and the
/// owner thread parks between two requests (owner::fork_prepare); the parent lets both go on;
/// the child has no owner thread and no workers: its one thread runs every request itself,
/// pipelining off, the in-flight back ends (gone with their threads) forgotten.
///
/// Only the program's forks do this. A fork of the loader's own native code (a build script's
/// Command on Android, where std forks instead of posix_spawn; a provided crate starting a
/// process) execs at once and never compiles in the child, so it pauses nothing. A process-wide
/// pthread_atfork did, and deadlocked: a domain loader holds its compile job while it waits for
/// its build-script threads, and a script thread's fork waited for that job (Pixel WM: every
/// app not yet running hung at 'compiling: build scripts' on a cold cache).
#[cfg(unix)]
extern "C" fn relax_fork() -> i32 {
    fork_prepare();
    let pid = os::thread::fork();
    let err = os::proc::last_error();
    if pid == 0 {
        fork_child();
    } else {
        fork_parent();
    }
    os::proc::set_last_error(err);
    pid
}

#[cfg(unix)]
fn fork_prepare() {
    pause_compile_jobs(false);
    owner::fork_prepare();
}

#[cfg(unix)]
fn fork_parent() {
    owner::fork_parent();
    resume_compile_jobs(false);
}

#[cfg(unix)]
fn fork_child() {
    owner::fork_child();
    PIPELINE.store(false, Ordering::SeqCst);
    SPEC_STOP.store(true, Ordering::SeqCst);
    SPEC_Q.lock().unwrap_or_else(|e| e.into_inner()).clear();
    unsafe {
        if !UNIT.is_null() {
            for f in (&mut *UNIT).fns.iter_mut() {
                f.inflight = false;
            }
        }
    }
    // the child has no workers: nothing waits at the job gate, no job is running elsewhere
    TIER_STOP.store(true, Ordering::SeqCst);
    BG_ACTIVE.store(JOB_DEPTH.with(|d| (d.get() > 0) as u32), Ordering::SeqCst);
    BG_PAUSE.store(false, Ordering::SeqCst);
    BG_PAUSER.store(0, Ordering::SeqCst);
    end_pause_turn();
}


thread_local! {
    /// saved host context of this thread's innermost `enter` (sp); 0 = no recovery point
    static CTX: std::cell::UnsafeCell<[u64; 2]> = const { std::cell::UnsafeCell::new([0; 2]) };
    /// stack of format buffers: format!/write! to non-Formatter targets push one
    static FMT: std::cell::RefCell<Vec<String>> = std::cell::RefCell::new(vec![String::new()]);
    /// the last popped buffer (valid until the next pop)
    static FMT_POPPED: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
    static PANIC: std::cell::RefCell<Option<PanicInfo>> = const { std::cell::RefCell::new(None) };
    /// "file:line:col" of a panic raised through panic_impl (std's own panics)
    static PANIC_LOC: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

crate::gen_local! {
    /// source location of the last panic caught inside the compiler (one-line diagnostics)
    static ICE_LOC: std::cell::RefCell<String>;
}

/// Installs (once) a panic hook that stays silent for panics inside the compiler (they
/// become compile errors) and records their location; other panics print as usual.
fn install_ice_hook() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::SeqCst) {
        return;
    }
    let prev = std::panic::take_hook();
    // the hook lives as long as the process: the program's heap
    let _kept = crate::zone_alloc::kept();
    std::panic::set_hook(Box::new(move |info| {
        if owner::is_owner() {
            if let Some(l) = info.location() {
                let s = format!(" (at {}:{})", l.file(), l.line());
                ICE_LOC.with(|c| *c.borrow_mut() = s);
            }
            // MAKEPAD_RUN_ICE_BT=1: the compiler's own backtrace of an internal error
            if std::env::var_os("MAKEPAD_RUN_ICE_BT").is_some() {
                eprintln!("makepad-run ICE: {}\n{}", info, std::backtrace::Backtrace::force_capture());
            }
            return;
        }
        prev(info);
    }));
}

/// " (at src/file.rs:line)" of the last panic caught inside the compiler, once.
pub fn take_ice_location() -> String {
    ICE_LOC.with(|c| std::mem::take(&mut *c.borrow_mut()))
}

/// A fault at or just below the stack pointer: the stack's guard page was hit (frames
/// over a page probe their pages in order, so the guard is always touched first).
fn is_stack_fault(fault: u64, sp: u64) -> bool {
    fault < sp.wrapping_add(4096) && fault.wrapping_add(1 << 20) >= sp
}

/// A fault in JIT code on a thread without a recovery point: report and abort.
/// `makepad run`: a hardware fault in program code ends the process (as a rustc
/// build of the program would). Containing it (unwinding at the recovery point) skips the
/// cleanups of the frames between the fault and that point -- a fault has no landing pad -- so
/// RefCell borrows, locks and half-made values stay behind and later errors look unrelated.
/// `makepad live` / `makepad test` keep containing faults but mark the process poisoned.
pub static FAULTS_FATAL: AtomicBool = AtomicBool::new(false);

/// Set by the first contained fault: what happened where (reported with every later error).
static POISONED: relax_session::sync::Mutex<Option<String>> = relax_session::sync::Mutex::new(None);

/// The poisoned-state note, when a fault was contained earlier.
pub fn poisoned_note() -> Option<String> {
    POISONED.lock().ok().and_then(|g| g.clone())
}

/// A hardware fault in JIT code with a recovery point (from the signal handler, on the faulting thread).
/// Returns true when the process must end: the report is printed and the caller restores the
/// signal's default action and returns, so the faulting instruction faults again and the
/// process dies by the signal (exit status 128 + signal, as a rustc build's would). Otherwise
/// the fault is contained (the caller resumes at the recovery point) and the process is marked
/// poisoned.
pub(crate) unsafe fn fault_policy(label: &str, info: &PanicInfo) -> bool {
    let u = &*UNIT;
    let tname = os::thread::name(os::thread::current());
    let at = owner::call(owner::Class::Job, || info.frames.first().and_then(|f| u.pc_to_src(*f)).map(|(id, loc)| format!("{} ({})", u.fns[id as usize].name, loc)).unwrap_or_else(|| format!("{:#x}", info.frames.first().copied().unwrap_or(0))));
    // a guarded run-time module call (std::relax::guard): the fault is contained to the call
    // and reported; the caller marks the module failed
    if GUARD_DEPTH.with(|g| g.get()) > 0 {
        eprintln!("makepad: {} ({}, address {:#x}) inside a guarded module call on thread '{}' at {}: contained, the call returns an error", info.kind, label, info.fault_addr, tname, at);
        return false;
    }
    // (a fault in an app domain is that domain's crash: the others go on)
    if FAULTS_FATAL.load(Ordering::Relaxed) && !crate::domain::in_app_domain() {
        let r = owner::call(owner::Class::Job, || u.report(info));
        eprintln!("makepad: {} ({}, address {:#x}) in program code on thread '{}' at {}: the process ends (the fault may be the program's or a makepad run miscompile; a crash report follows)\n  report: {}", info.kind, label, info.fault_addr, tname, at, r);
        return true;
    }
    let note = crate::zone_alloc::kept_with(|| format!("state may be corrupt after a {} at {:#x} on thread '{}' in {} (the frames it unwound ran no cleanups); restart recommended", info.kind, info.fault_addr, tname, at));
    eprintln!("makepad: POISONED: {}", note);
    if let Ok(mut g) = POISONED.lock() {
        if g.is_none() {
            *g = Some(note);
        }
    }
    false
}

unsafe fn fatal_report(f: &os::fault::Fault) -> ! {
    let frames = collect_frames(f.fp, f.pc, false);
    let kind = if is_stack_fault(f.addr, f.sp) { "stack overflow" } else { "fault" };
    let info = PanicInfo { kind: kind.to_string(), message: format!("{} at {:#x}", f.label(), f.addr), site: u64::MAX, frames, fault_addr: f.addr, location: String::new() };
    let r = owner::call(owner::Class::Job, || (&*UNIT).report(&info));
    eprintln!("makepad: fault on a thread without a recovery point, aborting\n  report: {}", r);
    std::process::abort()
}

/// This thread's recovery context (stable address for the thread's lifetime).
fn ctx_ptr() -> *mut u64 {
    CTX.with(|c| c.get() as *mut u64)
}

#[derive(Clone)]
pub struct PanicInfo {
    pub kind: String,
    pub message: String,
    pub site: u64,
    pub frames: Vec<u64>,
    pub fault_addr: u64,
    /// "file:line:col" when the panic did not come from a compiler-known site
    pub location: String,
}

// ------------------------------------------------------------ host runtime functions

fn spec_pad(body: String, spec: u64, numeric: bool) -> String {
    let width = (spec & 0xffff) as usize;
    let align = ((spec >> 33) & 3) as u8;
    let zero = (spec >> 37) & 1 != 0;
    let fill = char::from_u32(((spec >> 48) & 0xffff) as u32).unwrap_or(' ');
    let n = body.chars().count();
    if n >= width {
        return body;
    }
    let pad = width - n;
    if zero && numeric {
        // zeros after sign / 0x prefix
        let (sign, rest) = if body.starts_with('-') || body.starts_with('+') { (body[..1].to_string(), body[1..].to_string()) } else { (String::new(), body.clone()) };
        let (pfx, digits) = if rest.starts_with("0x") || rest.starts_with("0b") || rest.starts_with("0o") { (rest[..2].to_string(), rest[2..].to_string()) } else { (String::new(), rest) };
        return format!("{}{}{}{}", sign, pfx, "0".repeat(pad), digits);
    }
    let fill_s = |k: usize| -> String {
        let mut s = String::new();
        for _ in 0..k {
            s.push(fill);
        }
        s
    };
    let align = if align == 0 { if numeric { 3 } else { 1 } } else { align };
    match align {
        1 => format!("{}{}", body, fill_s(pad)),
        2 => format!("{}{}{}", fill_s(pad / 2), body, fill_s(pad - pad / 2)),
        _ => format!("{}{}", fill_s(pad), body),
    }
}

rt_fn! {
fn rt_fmt_str(p: *const u8, len: u64, spec: u64) {
    let s = unsafe { std::slice::from_raw_parts(p, len as usize) };
    let s = String::from_utf8_lossy(s).into_owned();
    let ty = ((spec >> 40) & 0xff) as u8;
    let body = if ty == b'?' { format!("{:?}", s) } else { s };
    let out = if spec & 0xffff != 0 { spec_pad(body, spec, false) } else { body };
    fmt_append(&out);
}
}

rt_fn! {
fn rt_fmt_int(v: u64, kind: u64, spec: u64) {
    let bits = (kind >> 8) as u32;
    let k = kind & 0xff;
    let ty = ((spec >> 40) & 0xff) as u8;
    let alt = (spec >> 36) & 1 != 0;
    let plus = (spec >> 35) & 1 != 0;
    let mask: u64 = if bits >= 64 { u64::MAX } else { (1u64 << bits) - 1 };
    let body = match k {
        3 => (if v != 0 { "true" } else { "false" }).to_string(),
        4 => {
            let c = char::from_u32(v as u32).unwrap_or('?');
            if ty == b'?' {
                format!("{:?}", c)
            } else {
                c.to_string()
            }
        }
        _ => match ty {
            b'x' => {
                if alt {
                    format!("{:#x}", v & mask)
                } else {
                    format!("{:x}", v & mask)
                }
            }
            b'X' => format!("{:X}", v & mask),
            b'b' => format!("{:b}", v & mask),
            b'o' => format!("{:o}", v & mask),
            _ => {
                let s = if k == 1 { format!("{}", v as i64) } else { format!("{}", v) };
                if plus && !s.starts_with('-') {
                    format!("+{}", s)
                } else {
                    s
                }
            }
        },
    };
    let out = spec_pad(body, spec, k == 1 || k == 2);
    fmt_append(&out);
}
}

rt_fn! {
fn rt_fmt_float(v: f64, is64: u64, spec: u64) {
    let ty = ((spec >> 40) & 0xff) as u8;
    let has_prec = (spec >> 32) & 1 != 0;
    let prec = ((spec >> 16) & 0xffff) as usize;
    let plus = (spec >> 35) & 1 != 0;
    let body = if is64 != 0 {
        match (ty, has_prec) {
            (b'?', false) => format!("{:?}", v),
            (b'e', _) => format!("{:e}", v),
            (_, true) => format!("{:.*}", prec, v),
            _ => format!("{}", v),
        }
    } else {
        let x = v as f32;
        match (ty, has_prec) {
            (b'?', false) => format!("{:?}", x),
            (b'e', _) => format!("{:e}", x),
            (_, true) => format!("{:.*}", prec, x),
            _ => format!("{}", x),
        }
    };
    let body = if plus && !body.starts_with('-') { format!("+{}", body) } else { body };
    let out = spec_pad(body, spec, true);
    fmt_append(&out);
}
}

fn fmt_append(s: &str) {
    FMT.with(|f| {
        let mut v = f.borrow_mut();
        if v.is_empty() {
            v.push(String::new());
        }
        let n = v.len();
        v[n - 1].push_str(s);
    });
}

fn fmt_take_top() -> String {
    FMT.with(|f| {
        let mut v = f.borrow_mut();
        if v.is_empty() {
            v.push(String::new());
        }
        let n = v.len();
        std::mem::take(&mut v[n - 1])
    })
}

rt_fn! {
fn rt_fmt_push() {
    FMT.with(|f| f.borrow_mut().push(String::new()));
}
}

#[repr(C)]
pub struct StrRet {
    ptr: *const u8,
    len: u64,
}

impl StrRet {
    pub fn new(ptr: u64, len: u64) -> StrRet {
        StrRet { ptr: ptr as *const u8, len }
    }
    pub fn parts(&self) -> (u64, u64) {
        (self.ptr as u64, self.len)
    }
}

/// The address of a runtime entry by name (sandbox.rs wraps some of them).
pub fn rt_entry(name: &str) -> u64 {
    match name {
        "fmt_str" => rt_fmt_str as *const () as u64,
        "fmt_pop" => rt_fmt_pop as *const () as u64,
        "catch_panic" => rt_catch_panic as *const () as u64,
        _ => 0,
    }
}

/// Bytes per thunk (sandbox.rs lays out its own thunk table).
pub fn thunk_size() -> usize {
    THUNK_SIZE
}

/// This thread's thread-local block (allocated on first use).
pub fn tls_block() -> u64 {
    rt_tls_block()
}

rt_fn! {
fn rt_fmt_pop() -> StrRet {
    let s = FMT.with(|f| {
        let mut v = f.borrow_mut();
        let s = v.pop().unwrap_or_default();
        if v.is_empty() {
            v.push(String::new());
        }
        s
    });
    FMT_POPPED.with(|p| {
        *p.borrow_mut() = s;
        let b = p.borrow();
        StrRet { ptr: b.as_ptr(), len: b.len() as u64 }
    })
}
}

rt_fn! {
fn rt_fmt_print(stream: u64) {
    let s = fmt_take_top();
    use std::io::Write;
    if stream == 2 {
        let _ = std::io::stderr().write_all(s.as_bytes());
    } else {
        let _ = std::io::stdout().write_all(s.as_bytes());
    }
}
}

rt_fn! {
fn rt_str_eq(p1: *const u8, l1: u64, p2: *const u8, l2: u64) -> u64 {
    if l1 != l2 {
        return 0;
    }
    let a = unsafe { std::slice::from_raw_parts(p1, l1 as usize) };
    let b = unsafe { std::slice::from_raw_parts(p2, l2 as usize) };
    (a == b) as u64
}
}

rt_fn! {
fn rt_fmod(a: f64, b: f64) -> f64 {
    a % b
}
}
rt_fn! {
fn rt_fmodf(a: f32, b: f32) -> f32 {
    a % b
}
}

/// The frames of a stop as lookup PCs (`PanicInfo::frames`): the top one is `pc` itself
/// when it is the faulting/interrupted instruction (`pc_is_ret` false) and every return
/// address is taken minus one, as unwinders do -- a call that never returns can be the
/// last instruction of its function, and its return address is then the next function.
pub(crate) fn collect_frames(rbp: u64, pc: u64, pc_is_ret: bool) -> Vec<u64> {
    let mut frames = vec![pc.wrapping_sub(pc_is_ret as u64)];
    let mut bp = rbp;
    let u = unsafe { &*UNIT };
    // a stop inside C code (a libc abort/trap): walk its frames (frame pointers are kept on
    // both targets) up to the JIT code that called it
    let mut in_c = u.find_fn(frames[0]).is_none();
    let mut c_frames = 0;
    for _ in 0..64 {
        if bp == 0 || bp & 7 != 0 {
            break;
        }
        let ra = unsafe { *((bp + 8) as *const u64) }.wrapping_sub(1);
        if u.find_fn(ra).is_none() {
            c_frames += 1;
            if !in_c || c_frames > 16 {
                break;
            }
            let next = unsafe { *(bp as *const u64) };
            if next <= bp {
                break;
            }
            bp = next;
            continue;
        }
        in_c = false;
        frames.push(ra);
        bp = unsafe { *(bp as *const u64) };
    }
    frames
}

/// Reports a panic and leaves to the recovery point; with the register block of a panic
/// trampoline (`regs`), the frames' cleanup pads run on the way (unwind_walk).
fn raise_with(kind: &str, message: String, site: u64, fault: u64, rbp: u64, ret: u64, regs: *const u64) -> ! {
    let frames = collect_frames(rbp, ret, true);
    let location = PANIC_LOC.with(|l| std::mem::take(&mut *l.borrow_mut()));
    let info = PanicInfo { kind: kind.to_string(), message, site, frames, fault_addr: fault, location };
    if unsafe { *ctx_ptr() } == 0 {
        // no recovery point on this thread (a thread JIT code started, or a C callback on
        // a foreign thread): report and abort, as panic=abort does
        // (rustc: a panic escaping a function that cannot unwind -- an `extern "C"` thread
        // start routine, a C callback -- aborts with "panic in a function that cannot
        // unwind": core/src/panicking.rs panic_cannot_unwind, mir_transform
        // abort_unwinding_calls.rs). The message goes first, then the report.
        let r = owner::call(owner::Class::Job, || unsafe { (&*UNIT).report(&info) });
        eprintln!("makepad: {} on a thread without a recovery point: {}\n  panic in a function that cannot unwind, aborting\n  report: {}", kind, info.message, r);
        std::process::abort();
    }
    // makepad inspect: a watched run parks the panic here, stack intact, until a client decides
    // (a resume_unwind continues a panic inspect already parked: no second park)
    if !RETHROW.with(|c| c.replace(false)) {
        crate::inspect::events::on_panic(&info, !regs.is_null());
    }
    PANIC.with(|p| *p.borrow_mut() = Some(info));
    FMT.with(|f| {
        let mut v = f.borrow_mut();
        v.clear();
        v.push(String::new());
    });
    if !regs.is_null() {
        unsafe { unwind_walk(regs) }
    }
    CLEANING.with(|c| c.set(0));
    UNWINDING.with(|c| c.set(false));
    unsafe {
        let leave: rt_fn_ty!(fn(*mut u64, u64) -> !) = std::mem::transmute((&*UNIT).leave as usize);
        leave(ctx_ptr(), 1)
    }
}

/// Ends the run of the calling thread's `call_jit_in` without a panic report: it returns
/// `Err(PanicInfo { kind, message })` to its caller (a domain's `process::exit`, domain.rs). The
/// frames in between run no cleanups. A thread with no recovery point (native code called into
/// JIT code) cannot leave: it parks for good.
pub(crate) fn leave_thread(kind: &str, message: String) -> ! {
    if unsafe { *ctx_ptr() } == 0 {
        loop {
            std::thread::park();
        }
    }
    let info = PanicInfo { kind: kind.to_string(), message, site: u64::MAX, frames: Vec::new(), fault_addr: 0, location: String::new() };
    PANIC.with(|p| *p.borrow_mut() = Some(info));
    CLEANING.with(|c| c.set(0));
    UNWINDING.with(|c| c.set(false));
    unsafe {
        let leave: rt_fn_ty!(fn(*mut u64, u64) -> !) = std::mem::transmute((&*UNIT).leave as usize);
        leave(ctx_ptr(), 1)
    }
}

// Panic entries are reached through `panic_tramp`, which passes the JIT caller's frame
// (frame pointer, return address) as arguments 6 and 7 (x6/x7 on arm64, the first two
// stack arguments on x86-64), so entries keep up to six arguments of their own.

rt_fn! {
#[allow(clippy::too_many_arguments)]
fn rt_panic_site(site: u64, _a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, regs: *const u64, _a7: u64) {
    let mut msg = fmt_take_top();
    if msg.is_empty() {
        let u = unsafe { &*UNIT };
        let sites = u.sites.lock().unwrap_or_else(|e| e.into_inner());
        if (site as usize) < sites.len() {
            msg = sites[site as usize].msg.clone();
        }
    }
    raise_unwinding("panic", msg, site, regs)
}
}

rt_fn! {
#[allow(clippy::too_many_arguments)]
fn rt_panic_bounds(idx: u64, len: u64, site: u64, _a3: u64, _a4: u64, _a5: u64, regs: *const u64, _a7: u64) {
    let msg = format!("index out of bounds: the len is {} but the index is {}", len, idx);
    raise_unwinding("panic", msg, site, regs)
}
}

rt_fn! {
#[allow(clippy::too_many_arguments)]
fn rt_panic_impl(msg: *const u8, len: u64, file: *const u8, file_len: u64, line: u64, col: u64, regs: *const u64, _a7: u64) {
    let m = unsafe { String::from_utf8_lossy(std::slice::from_raw_parts(msg, len as usize)).into_owned() };
    let f = unsafe { String::from_utf8_lossy(std::slice::from_raw_parts(file, file_len as usize)).into_owned() };
    PANIC_LOC.with(|l| *l.borrow_mut() = format!("{}:{}:{}", f, line as u32, col as u32));
    raise_unwinding("panic", m, u64::MAX, regs)
}
}

// ---------------------------------------------------------------- unwinding
//
// A panic unwinds the JIT frames between it and the thread's recovery point (catch_unwind,
// the thread's start, `makepad test`'s per-test entry), running each frame's cleanup pad as
// rustc does (drops of the live locals and temporaries: MIR cleanup blocks, the Itanium
// unwinder's two phases folded into one walk, as panic=unwind with no catch filter needs).
// The walk starts from the register block the panic trampoline stored (frame pointer, return
// address, callee-saved registers); for each frame the code's unwind trailer
// (rir::unwind_info_words) names its landing stub for the call it is in, or where its
// prologue saved the caller's callee-saved registers. A landing stub runs with the frame's
// fp, sp and callee-saved registers restored; its pad ends in Term::Resume, which comes back
// here (rt_resume_unwind) and walks on from that frame. Past the last JIT frame the stack is
// cut back to the recovery point as before.

#[cfg(target_arch = "aarch64")]
const REG_WORDS: usize = 20;
#[cfg(not(target_arch = "aarch64"))]
const REG_WORDS: usize = 7;

/// Index of callee-saved register `r` (a trailer entry) in the register block.
#[cfg(target_arch = "aarch64")]
fn reg_slot(r: u32) -> Option<usize> {
    match r {
        19..=28 => Some(2 + (r - 19) as usize),
        0x108..=0x10f => Some(12 + (r - 0x108) as usize),
        _ => None,
    }
}
#[cfg(not(target_arch = "aarch64"))]
fn reg_slot(r: u32) -> Option<usize> {
    match r {
        3 => Some(2),
        12..=15 => Some(3 + (r - 12) as usize),
        _ => None,
    }
}

thread_local! {
    /// the recovery context (`*ctx_ptr()`, the sp `enter` saved) of the unwind whose cleanup
    /// is running on this thread; 0 = none. A panic raised while it runs, under the same
    /// recovery point, is a panic during cleanup: rustc aborts (panic_in_cleanup).
    static CLEANING: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// A panic from JIT code (panic trampoline): the report as `raise` makes it, then the frames'
/// cleanup pads run on the way to the recovery point.
fn raise_unwinding(kind: &str, message: String, site: u64, regs: *const u64) -> ! {
    let (rbp, ret) = unsafe { (*regs, *regs.add(1)) };
    let ctx = unsafe { *ctx_ptr() };
    if ctx != 0 && CLEANING.with(|c| c.get()) == ctx {
        let frames = collect_frames(rbp, ret, true);
        let location = PANIC_LOC.with(|l| std::mem::take(&mut *l.borrow_mut()));
        let info = PanicInfo { kind: kind.to_string(), message, site, frames, fault_addr: 0, location };
        let r = owner::call(owner::Class::Job, || unsafe { (&*UNIT).report(&info) });
        eprintln!("makepad: panic in a destructor during cleanup: {}\n  thread caused non-unwinding panic. aborting.\n  report: {}", info.message, r);
        std::process::abort();
    }
    UNWINDING.with(|c| c.set(true));
    raise_with(kind, message, site, 0, rbp, ret, regs)
}

rt_fn! {
/// Term::Resume (through the panic trampoline): the frame's cleanup pad is done; unwinding
/// goes on with its caller.
#[allow(clippy::too_many_arguments)]
fn rt_resume_unwind(_a0: u64, _a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, regs: *const u64, _a7: u64) {
    CLEANING.with(|c| c.set(0));
    unsafe { unwind_walk(regs) }
}
}

/// Walks the JIT frames from the register block `regs` (copied: it lives in the trampoline's
/// stack area, which a landing cuts away) to the first frame whose current call has a
/// landing stub, and continues there; past the last JIT frame, leaves to the recovery point.
unsafe fn unwind_walk(regs: *const u64) -> ! {
    let mut r = [0u64; REG_WORDS];
    std::ptr::copy_nonoverlapping(regs, r.as_mut_ptr(), REG_WORDS);
    let u = &*UNIT;
    let tables = u.code.read().unwrap_or_else(|e| e.into_inner());
    let mut target: Option<(u64, u64, u64)> = None;
    for _ in 0..100_000 {
        let (fp, ret) = (r[0], r[1]);
        if fp == 0 || fp & 7 != 0 {
            break;
        }
        // the threaded interpreter's call boundary: a frame with a landing that hands the unwind
        // to the interpreter (it runs the interpreted frames' cleanups, then resumes here)
        if let Some((land_pc, sp_below)) = relax_codegen::interp::unwind_boundary(ret) {
            target = Some((fp, fp - sp_below, land_pc));
            break;
        }
        let pc = ret.wrapping_sub(1);
        let Some((start, _, _)) = tables.entry_at(pc) else { break };
        let image_info;
        let info = match tables.unwind.get(&start) {
            Some(i) => i,
            None => match tables.image_unwind_at(start) {
                Some(i) => {
                    image_info = i;
                    &image_info
                }
                None => break,
            },
        };
        let off = (ret - start) as u32;
        if let Some(&(_, stub)) = info.pads.iter().find(|p| p.0 == off) {
            target = Some((fp, fp - info.sp_off as u64, start + stub as u64));
            break;
        }
        // this frame has no cleanup here: the caller's callee-saved registers come back from
        // where the prologue saved them, then the caller's frame
        for (k, reg) in info.saved.iter().enumerate() {
            if let Some(i) = reg_slot(*reg) {
                r[i] = *((fp - 8 * (k as u64 + 1)) as *const u64);
            }
        }
        r[1] = *((fp + 8) as *const u64);
        r[0] = *(fp as *const u64);
    }
    let land = u.rt.land;
    drop(tables);
    match target {
        Some((fp, sp, pc)) => {
            CLEANING.with(|c| c.set(*ctx_ptr()));
            // the block outlives this frame: land() reads it after sp moved up past it, so it
            // goes in a thread-local
            LAND_REGS.with(|b| {
                let p = b.get() as *mut u64;
                std::ptr::copy_nonoverlapping(r.as_ptr(), p, REG_WORDS);
            });
            let p = LAND_REGS.with(|b| b.get() as *mut u64 as u64);
            let f: rt_fn_ty!(fn(u64, u64, u64, u64) -> !) = std::mem::transmute(land as usize);
            f(p, fp, sp, pc)
        }
        None => {
            CLEANING.with(|c| c.set(0));
            UNWINDING.with(|c| c.set(false));
            let leave: rt_fn_ty!(fn(*mut u64, u64) -> !) = std::mem::transmute((&*UNIT).leave as usize);
            leave(ctx_ptr(), 1)
        }
    }
}

/// makepad inspect's restart / return: a frame's unwind landing ended the panic -- this thread
/// is no longer unwinding and its panic is dropped.
/// The message of the panic unwinding on this thread now (makepad inspect: a probe's panic).
pub(crate) fn take_panic_message() -> Option<String> {
    PANIC.with(|p| p.borrow().as_ref().map(|i| i.message.clone()))
}

pub(crate) fn abandon_unwind() {
    CLEANING.with(|c| c.set(0));
    UNWINDING.with(|c| c.set(false));
    PANIC.with(|p| *p.borrow_mut() = None);
}

rt_fn! {
/// std::thread::panicking(): this thread is unwinding (from the panic until the recovery point)
fn rt_panicking() -> u64 {
    UNWINDING.with(|c| c.get()) as u64
}
}

rt_fn! {
/// std::panic::resume_unwind: the panic that follows continues one already reported (and
/// parked, under inspect). Like rustc's resume_unwind it starts no new report: the unwind
/// goes on to the next catch.
fn rt_rethrow() {
    RETHROW.with(|c| c.set(true));
}
}

thread_local! {
    static RETHROW: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static UNWINDING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static LAND_REGS: std::cell::UnsafeCell<[u64; REG_WORDS]> = const { std::cell::UnsafeCell::new([0; REG_WORDS]) };
}

static PROGRAM_ARGS: relax_session::sync::Mutex<Vec<Vec<u8>>> = relax_session::sync::Mutex::new(Vec::new());
/// (argc, argv) built for each domain that asked, by block base (the process's own: dom0's)
static ARGV: relax_session::sync::Mutex<Vec<(u64, usize, usize)>> = relax_session::sync::Mutex::new(Vec::new());

/// Sets the program's argv (argv[0] first) before it starts; default: this process's args.
pub fn set_program_args(args: Vec<Vec<u8>>) {
    if let Ok(mut a) = PROGRAM_ARGS.lock() {
        // the program's argv outlives the compiler heap: rebuilt in the kept phase
        *a = crate::zone_alloc::kept_with(|| args.iter().map(|v| v.clone()).collect());
    }
}

#[repr(C)]
pub struct ArgsRet {
    argc: u64,
    argv: u64,
}

rt_fn! {
/// (argc, argv) as C sees them, built once (the strings and the array live forever).
fn rt_args() -> ArgsRet {
    // a domain started by `domain::start` has its own argv; any other thread the process's
    let base = crate::domain::current();
    let own = crate::domain::args_of(base);
    let key = if own.is_some() { base } else { 0 };
    let mut tab = ARGV.lock().unwrap_or_else(|e| e.into_inner());
    let (argc, argv) = match tab.iter().find(|x| x.0 == key) {
        Some(x) => (x.1, x.2),
        None => {
            let mut v = own.unwrap_or_else(|| PROGRAM_ARGS.lock().map(|a| a.clone()).unwrap_or_default());
            if v.is_empty() {
                v = os::proc::args_bytes();
            }
            let mut ptrs: Vec<*const u8> = Vec::new();
            for a in &v {
                let mut b = a.clone();
                b.push(0);
                ptrs.push(Box::leak(b.into_boxed_slice()).as_ptr());
            }
            let n = ptrs.len();
            ptrs.push(std::ptr::null());
            let argv = Box::leak(ptrs.into_boxed_slice()).as_ptr() as usize;
            tab.push((key, n, argv));
            (n, argv)
        }
    };
    ArgsRet { argc: argc as u64, argv: argv as u64 }
}
}

/// Entry of a thread the program starts: runs `entry(arg)` (a Relax fn pointer) with a
/// recovery point, so a panic or fault ends this thread with a report instead of the process.
/// `makepad run`: a compile error reached at run time is no panic -- rustc would have rejected
/// the program -- so it ends the process once reported, from any thread and through
/// catch_unwind (a spawned thread's error used to unwind that thread only, and a main thread
/// waiting on it hung). `makepad test` / `makepad live` keep it a per-test / recoverable failure.
pub static COMPILE_ERRORS_FATAL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// `makepad run --check-first` (opt-in, non-lazy Gandalf): every app fn is borrow-checked before
/// its first code is installed (Unit::refused_by_check); a rejected body gets the error stub.
/// Off by default: the run never waits for the checker, which reports in the background.
pub static CHECK_FIRST: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn fatal_compile_error(p: &PanicInfo, report: &str) {
    if COMPILE_ERRORS_FATAL.load(std::sync::atomic::Ordering::Relaxed) && p.message.starts_with("compile error in ") {
        eprintln!("makepad-run: {} -- the program cannot run past it (rustc rejects it)\n  report: {}", p.message, report);
        // an app domain's compile error is that domain's crash (its thread ends next); the others go on
        if crate::domain::crash_current(&p.message) {
            return;
        }
        std::process::exit(101);
    }
}

rt_fn! {
fn rt_thread_run(entry: u64, arg: u64) -> u64 {
    ensure_altstack();
    let u = unsafe { UNIT };
    // runs on the calling thread, in the domain it is in
    match unsafe { call_jit_in(u, crate::domain::current(), entry, arg) } {
        Ok(_) => 0,
        // `process::exit` on a thread of an app domain: it ended the domain (domain.rs), this thread ends quietly
        Err(p) if p.kind == crate::domain::EXIT_KIND => 0,
        Err(p) => {
            let r = owner::call(owner::Class::Job, || unsafe { (&*u).report(&p) });
            fatal_compile_error(&p, &r);
            eprintln!("makepad: thread {}: {}\n  report: {}", p.kind, p.message, r);
            1
        }
    }
}
}

rt_fn! {
/// core::intrinsics_rt::catch_panic: `f(data)` under a recovery point (std::panic::
/// catch_unwind, std::thread's start routine). The panic message itself was printed by
/// std's hook; the report adds where it happened.
fn rt_catch_panic(f: u64, data: u64) -> u64 {
    ensure_altstack();
    let u = unsafe { UNIT };
    // a recovery point inside running code: the domain of that code, not the unit's own
    match unsafe { call_jit_in(u, crate::domain::current(), f, data) } {
        Ok(_) => 0,
        Err(p) => {
            let r = owner::call(owner::Class::Job, || unsafe { (&*u).report(&p) });
            fatal_compile_error(&p, &r);
            // std's hook already printed the panic message (as rustc's does); add where
            eprintln!("  report: {}", r);
            1
        }
    }
}
}

rt_fn! {
fn rt_compile(slot: u64) -> u64 {
    // the program may read errno / GetLastError right after the call this compile interrupts
    // (a stub sits between a failing OS call and the fn that reads its error): the compiler's
    // own OS calls must not change it
    let _keep = os::proc::KeepLastError::new();
    ensure_altstack();
    unsafe {
        let _exec_call = profile_path().map(|_| crate::exec_profile::call(UNIT as usize, slot as u32));
        if profile_path().is_some() { (&*UNIT).exec_event(slot as u32, "call"); }
        compile_slot(UNIT, slot as u32)
    }
}
}

// ------------------------------------------------------------ compiler stack

// A compile runs on the program's thread (lazy compile from a stub), and constant evaluation
// runs program code inside it, which compiles again: `T::NUM + 1` over a type nested 1000 deep
// nests 1000 compiles. rustc runs the compiler on its own stack and grows it on demand
// (rustc_data_structures::stack::ensure_sufficient_stack: a 100 KiB red zone, new 1 MiB+
// segments via stacker); Relax does the same here, so neither deep constant evaluation nor a
// lazy compile reached near the end of a deep program recursion overflows the thread's stack.

/// Run the compiler on a fresh segment when less than this is left
const STACK_RED_ZONE: usize = 1 << 20;
/// Size of a fresh segment (virtual; pages are touched on use)
const STACK_SEGMENT: usize = 64 << 20;

thread_local! {
    /// lowest usable address of the stack this thread runs on now (0: not known yet)
    static STACK_LO: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Runs `f` on a fresh stack segment when this thread's stack is nearly used up.
fn ensure_sufficient_stack<R, F: FnOnce() -> R>(f: F) -> R {
    let here = &f as *const _ as usize;
    let mut lo = STACK_LO.with(|c| c.get());
    if lo == 0 {
        lo = os::thread::stack_lo();
        STACK_LO.with(|c| c.set(lo));
    }
    if lo == 0 || here.saturating_sub(lo) >= STACK_RED_ZONE {
        return f();
    }
    struct Job<F, R> {
        f: Option<F>,
        r: Option<std::thread::Result<R>>,
    }
    extern "C" fn run<F: FnOnce() -> R, R>(p: *mut u8) {
        let job = unsafe { &mut *(p as *mut Job<F, R>) };
        let f = job.f.take().unwrap();
        // a Rust panic must not unwind through the stack switch (no unwind info for it)
        job.r = Some(std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)));
    }
    let seg = match os::mem::stack_segment(STACK_SEGMENT) {
        Some(seg) => seg,
        None => return f(),
    };
    let mut job: Job<F, R> = Job { f: Some(f), r: None };
    STACK_LO.with(|c| c.set(seg.lo));
    unsafe { os::mem::call_on_segment(&seg, run::<F, R>, &mut job as *mut Job<F, R> as *mut u8) };
    STACK_LO.with(|c| c.set(lo));
    os::mem::free_segment(seg);
    match job.r.take().unwrap() {
        Ok(r) => r,
        Err(e) => std::panic::resume_unwind(e),
    }
}

/// Where the OS cannot run a handler on a looping thread (Windows: no signals), the watching
/// thread redirects it into this glue (x64::hang_tramp in front of rt_hang); 0 = none.
pub(crate) static HANG_TRAMP: AtomicU64 = AtomicU64::new(0);

rt_fn! {
    /// The hang watchdog's interrupt, run on the interrupted thread through HANG_TRAMP: the same
    /// decision as a watchdog signal; when it acts, the call unwinds to its recovery point
    /// (leave(ctx, 3)), else the thread continues where it was (return 0).
    fn rt_hang(regs: *const u64) -> u64 {
        let r = unsafe { std::slice::from_raw_parts(regs, 17) };
        let f = os::fault::Fault { kind: os::fault::Kind::Watchdog, sig: 0, pc: r[16], sp: regs as u64 + 136, fp: r[10], addr: 0 };
        match on_fault(&f, &os::fault::Hooks { prepare: &|| {}, chained: false, debug: &|| {} }) {
            os::fault::Action::Resume { pc, a0, a1 } => unsafe {
                let leave: rt_fn_ty!(fn(u64, u64) -> !) = std::mem::transmute(pc as usize);
                leave(a0, a1)
            },
            _ => 0,
        }
    }
}

/// Is `pc` in the unit's code memory (functions, stubs, glue)? Lock-free: the bounds are fixed
/// once the unit exists (os::fault's Windows handler asks before anything else).
pub(crate) fn in_jit_code(pc: u64) -> bool {
    unsafe { !UNIT.is_null() && pc >= (*UNIT).exec_base && pc - (*UNIT).exec_base < (*UNIT).exec_size }
}

/// The name of the function whose code holds `pc` ("-" when none; diagnostics, no lock).
pub(crate) fn fn_name_at(pc: u64) -> String {
    unsafe {
        if UNIT.is_null() {
            return "-".to_string();
        }
        let u = &*UNIT;
        u.find_fn(pc).map(|id| u.fns[id as usize].name.to_string()).unwrap_or("-".to_string())
    }
}

/// A fault or the watchdog's interrupt on this thread (os::fault's handler): what to do with it.
/// The code tables are read under their own leaf lock; reports are unit calls. A watchdog kick
/// on a thread inside the compiler (a unit call) is skipped.
pub(crate) fn on_fault(f: &os::fault::Fault, hooks: &os::fault::Hooks) -> os::fault::Action {
    if f.kind == os::fault::Kind::Watchdog && owner::is_owner() {
        return os::fault::Action::Ignore;
    }
    note_tombstone_fault(f);
    ensure_altstack();
    (hooks.prepare)();
    unsafe { on_fault_locked(f, hooks) }
}

/// A fault inside the address range of a destroyed compiler generation (memlane step 1b): a
/// stale pointer into the compiler arena. Says so on stderr (one lock-free table lookup; the
/// report that follows is the ordinary fault report).
fn note_tombstone_fault(f: &os::fault::Fault) {
    if std::env::var_os("MAKEPAD_RUN_ZONE_TRACE").is_some() {
        let sym = os::proc::symbol_at(f.pc).map_or_else(|| "?".to_string(), |s| s.0);
        eprintln!("makepad-run zone: fault: {} at {:#x}, pc {:#x} in {}, thread `{}`\n", f.label(), f.addr, f.pc, sym, std::thread::current().name().unwrap_or("?"));
    }
    if let Some(g) = crate::zone_alloc::compiler_tombstone_of(f.addr as usize) {
        eprintln!("makepad: use after compiler-zone destroy (generation {g}): {} at {:#x}, pc {:#x}", f.label(), f.addr, f.pc);
    }
}

unsafe fn on_fault_locked(f: &os::fault::Fault, hooks: &os::fault::Hooks) -> os::fault::Action {
    use os::fault::{Action, Kind};
    let (pc, fp, fault) = (f.pc, f.fp, f.addr);
    if f.kind == Kind::Watchdog {
        // hang watchdog: only act when JIT code is running and not inside the compiler
        if UNIT.is_null() || *ctx_ptr() == 0 || (&*UNIT).find_fn(pc).is_none() {
            return Action::Ignore;
        }
        // An enclosing timeout is already moving through inner guard cleanups. A
        // second expired timer must not replace its ownership before it reaches home.
        if WATCHDOG_EXPIRED.with(|t| t.get() != 0) {
            return Action::Ignore;
        }
        // Kicks carry no token on Unix. Validate against this thread's live deadline
        // chain, so a queued signal can never expire a later, still-in-budget call.
        let expired = WATCHDOG_CURRENT.with(|current| {
            let mut p = current.get();
            let now = std::time::Instant::now();
            let mut selected: *const WatchdogCall = std::ptr::null();
            while !p.is_null() {
                let call = &*p;
                if call.deadline.is_some_and(|d| now >= d) && !call.hit.load(Ordering::Acquire)
                    && (selected.is_null() || call.deadline < (*selected).deadline) {
                    selected = p;
                }
                p = call.parent.as_ref().map_or(std::ptr::null(), std::sync::Arc::as_ptr);
            }
            if selected.is_null() {
                return false;
            }
            (*selected).hit.store(true, Ordering::Release);
            WATCHDOG_EXPIRED.with(|t| t.set((*selected).token));
            true
        });
        if !expired {
            return Action::Ignore;
        }
        let u = &*UNIT;
        let frames = collect_frames(fp, pc, false);
        PANIC.with(|p| *p.borrow_mut() = Some(PanicInfo { kind: "hang".to_string(), message: "watchdog: the call did not return in time".to_string(), site: u64::MAX, frames, fault_addr: 0, location: String::new() }));
        return Action::Resume { pc: u.leave, a0: ctx_ptr() as u64, a1: 3 };
    }
    if !UNIT.is_null() {
        let u = &*UNIT;
        if *ctx_ptr() == 0 && u.find_fn(pc).is_some() {
            fatal_report(f);
        }
        // a fault at the recovery entry itself, or an instruction fetch fault in code memory
        // (the code is not executable): resuming would fault again forever -- report and end
        if pc == u.leave || (fault == pc && in_jit_code(pc)) {
            fatal_report(f);
        }
    }
    if UNIT.is_null() || *ctx_ptr() == 0 {
        // not ours: the handler we replaced (a library's), else the default action
        return Action::PassOn;
    }
    let u = &*UNIT;
    // a fault outside JIT code (inside a library the program called) goes to that library's
    // handler first when it installed one, as in a rustc build (Chromium handles its own)
    if hooks.chained && u.find_fn(pc).is_none() {
        return Action::PassOn;
    }
    let kind = match f.kind {
        Kind::Segv | Kind::Bus if is_stack_fault(fault, f.sp) => "stack overflow",
        Kind::Segv => "segfault",
        Kind::Bus => "bus error",
        Kind::Ill => "illegal instruction",
        Kind::Fpe => "arithmetic fault",
        Kind::Trap => "trap",
        _ => "signal",
    };
    (hooks.debug)();
    let frames = collect_frames(fp, pc, false);
    let label = f.label();
    let pinfo = PanicInfo { kind: kind.to_string(), message: format!("{} at {:#x}", label, fault), site: u64::MAX, frames, fault_addr: fault, location: String::new() };
    if fault_policy(&label, &pinfo) {
        // the default action: the instruction faults again and the process ends by the signal
        return Action::Die;
    }
    PANIC.with(|p| *p.borrow_mut() = Some(pinfo));
    // resume at `leave(ctx, 2)`
    Action::Resume { pc: u.leave, a0: ctx_ptr() as u64, a1: 2 }
}

/// JIT trampoline in front of a (never returning) host panic function: passes the caller's
/// frame (frame pointer, return address) as arguments 6 and 7 (thread-safe, no globals).
#[cfg(target_arch = "aarch64")]
fn panic_tramp(target: u64) -> Vec<u8> {
    x64::panic_tramp(target)
}

#[cfg(not(target_arch = "aarch64"))]
fn panic_tramp(target: u64) -> Vec<u8> {
    let mut b = Vec::new();
    // Entered by `call` (rsp = 8 mod 16, [rsp] = the JIT return address). Builds the register
    // block [rbp, return address, rbx, r12, r13, r14, r15] on the stack and passes its address
    // as argument 7 (SysV: the first stack argument): the target sees [rsp] = a return slot,
    // [rsp+8] = the block, [rsp+16] = 0, with rsp = 8 mod 16 as after a call. Never returns.
    // mov r10, [rsp]
    b.extend_from_slice(&[0x4c, 0x8b, 0x14, 0x24]);
    // push r15 ; push r14 ; push r13 ; push r12 ; push rbx ; push r10 ; push rbp
    b.extend_from_slice(&[0x41, 0x57, 0x41, 0x56, 0x41, 0x55, 0x41, 0x54, 0x53, 0x41, 0x52, 0x55]);
    // mov r11, rsp
    b.extend_from_slice(&[0x49, 0x89, 0xe3]);
    // push 0 ; push r11 ; push r11
    b.extend_from_slice(&[0x6a, 0x00, 0x41, 0x53, 0x41, 0x53]);
    // mov r11, imm64(target); jmp r11
    b.extend_from_slice(&[0x49, 0xbb]);
    b.extend_from_slice(&target.to_le_bytes());
    b.extend_from_slice(&[0x41, 0xff, 0xe3]);
    b
}

// ------------------------------------------------------------ the unit

crate::gen_local! {
    /// The memoized recursive type queries in progress on this thread (drop_may_unwind 1,
    /// is_unpin 2; outermost first) and the outermost entry a cycle was cut
    /// at (usize::MAX: none). An answer computed below a cut of an entry still in progress
    /// further out is provisional (it misses what that entry's other parts contribute) and is not
    /// memoized: memoizing it made the answer depend on which type was asked first, and with it
    /// the lowered code (Call vs Invoke, drop glue) -- O1 RIR that depended on build order.
    static CYC_AT: std::cell::RefCell<Vec<(u8, TyId)>>;
}
thread_local! {
    static CYC_CUT: std::cell::Cell<usize> = const { std::cell::Cell::new(usize::MAX) };
}

// try_with: CYC_AT has a destructor, and a thread's TLS destructors may run compiled code that
// is compiled right then (std's key_dtor): after CYC_AT is gone every answer counts as provisional.

/// A cycle met (kind, t) in progress: note how far out it reaches.
fn cyc_cut(kind: u8, t: TyId) {
    let at = CYC_AT.try_with(|m| m.borrow().iter().rposition(|x| *x == (kind, t)).unwrap_or(0)).unwrap_or(0);
    CYC_CUT.with(|c| c.set(c.get().min(at)));
}

/// Enters query (kind, t): its depth (usize::MAX: no record, the thread's TLS is being torn down).
fn cyc_enter(kind: u8, t: TyId) -> usize {
    CYC_AT
        .try_with(|m| {
            let mut m = m.borrow_mut();
            m.push((kind, t));
            m.len() - 1
        })
        .unwrap_or(usize::MAX)
}

/// Leaves the query at `depth`: whether its answer is provisional (not to be memoized).
fn cyc_leave(depth: usize) -> bool {
    if depth == usize::MAX {
        return true;
    }
    let _ = CYC_AT.try_with(|m| m.borrow_mut().truncate(depth));
    CYC_CUT.with(|c| {
        let cut = c.get();
        if cut >= depth {
            // a cut of this query itself (or none): its answer is final
            c.set(usize::MAX);
        }
        cut < depth
    })
}

/// A unit's data and statics parts of its reservation.
const UNIT_DATA_SIZE: u64 = 768 << 20;
pub(crate) const UNIT_RW_SIZE: u64 = 256 << 20;

// queries.rs:58-108: evaluate the resolved instance through the registered
// compiler provider; the final result is independent of its executable storage.
fn const_eval_to_valtree(context: usize, _env: crate::tcx::TypingEnv, def: DefId,
    args: &[relax_types::consts::GenericArg],
) -> relax_types::const_eval::EvalToValTreeResult {
    use relax_types::consts::GenericArg;
    use relax_types::const_eval::{ErrorHandled, evaluation_error};
    let u = unsafe { &*(context as *const Unit) };
    let mut compact = Vec::new();
    for arg in args {
        let ty = match *arg {
            GenericArg::Lifetime(_) => continue,
            GenericArg::Type(t) => t,
            GenericArg::Const(c) => u.tcx.tys.intern(TyKind::Const(c)),
        };
        if u.tcx.tys.flags(ty).contains(crate::types::TypeFlags::HAS_TY_PARAM) { return Err(ErrorHandled::TooGeneric); }
        compact.push(ty);
    }
    let ty = u.tcx.type_of(&u.prog, def).map_err(|_| evaluation_error())?;
    let ty = u.tcx.instantiate_ty(&u.prog, def, args, ty);
    // a unit call: the value, and whether this evaluation (not an earlier one of the same
    // build) deferred -- the flag belongs to the evaluation's own frame
    let (value, deferred) = owner::call(owner::Class::Job, || {
        let prev = DEFERRED_ABORT.with(|a| a.replace(false));
        let value = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let addr = unsafe { (&mut *u.self_ptr.0).const_addr(def, compact) }.ok_or_else(evaluation_error)?;
            crate::valtrees::const_to_valtree(u, addr, ty)
        }));
        // served nested and blocked by an earlier request: the request is posted again
        let value = match value {
            Err(p) if p.is::<crate::preempt::OwnerBusy>() => std::panic::resume_unwind(p),
            v => v,
        };
        let deferred = DEFERRED_ABORT.with(|a| a.get());
        DEFERRED_ABORT.with(|a| a.set(prev || deferred));
        (value, deferred)
    });
    match value {
        // The initializer needs a module the post-boot loader has not loaded yet: that is no
        // value and no evaluation error of the program. Nothing may cache or report it, so it
        // leaves this query (and every query above it) as a cancellation, and the build that
        // asked fails with DEFERRED_RETRY (build_rir); its outer compile loads and rebuilds.
        Ok(Err(_)) if deferred && BUILD_DEPTH.with(|d| d.get()) > 0 => std::panic::resume_unwind(Box::new(crate::preempt::NeedsLoad)),
        Ok(value) => value,
        Err(payload) => {
            if crate::preempt::is_cancelled(&*payload) { std::panic::resume_unwind(payload); }
            Err(evaluation_error())
        }
    }
}

impl Unit {
    pub(crate) fn flush_query_errors(&mut self, finished: bool) {
        if !self.errors.is_empty() { self.query_errors.note_reported_error(); }
        if finished { self.query_errors.flush_delayed(); }
        let (errors, warnings) = self.query_errors.drain_reported();
        for error in errors {
            let message = if error.cause.body_id == crate::program::NO_DEF { error.message }
                else { self.prog.record_error(self.prog.def(error.cause.body_id).file, error.cause.span, "typeck", error.message) };
            self.note_error(message);
        }
        for warning in warnings { eprintln!("warning: {}", warning.message); }
    }

    /// Records a build error (identity state: the message is the program's).
    pub(crate) fn note_error(&mut self, m: String) {
        let _k = crate::zone_alloc::kept();
        let m = m.clone();
        self.errors.push(m);
    }

    pub fn new(prog: Program, tcx: Tcx, core_crate: u32) -> Box<Unit> {
        owner::start();
        let query_errors = tcx.dcx.clone();
        let sess = prog.session.clone();
        // Restored files bypass load_file; seed their diagnostic identities too.
        if sess.runtime.profile {
            for file in 0..prog.file_paths.len() { prog.exec_profile_file(file as u32); }
        }
        let options = &sess.options;
        let _mem = crate::memstat::tag(crate::memstat::Tag::Unit);
        // One reservation, so code reaches everything of the unit pc-relatively (x64 rel32:
        // +-2 GB; arm64 adrp: +-4 GB): [code (glue, fns)][data][statics][symbol values]. Only
        // host / C symbols are outside it. The function slots (thunks, stubs, table words,
        // tier records) are in segments of their own, reserved as ids reach them: code calls
        // through full 64-bit addresses.
        // no-codegen mode: nothing below maps executable memory or writes code (os::mem refuses)
        let no_codegen = options.no_codegen;
        if no_codegen {
            if !cfg!(all(target_arch = "aarch64", target_vendor = "apple")) {
                panic!("makepad run: MAKEPAD_RUN_NO_CODEGEN is supported on arm64 Apple targets only");
            }
            os::mem::set_no_codegen();
        }
        // at exit: no executable memory that no image backs (anything that generated code);
        // MAKEPAD_RUN_EXEC_CHECK=1 runs the same check in JIT mode (where it must find the JIT)
        if no_codegen || std::env::var_os("MAKEPAD_RUN_EXEC_CHECK").is_some() {
            EXEC_CHECK.store(true, Ordering::Relaxed);
            #[cfg(target_os = "macos")]
            {
                extern "C" fn at_exit_check() {
                    exec_check();
                }
                extern "C" {
                    fn atexit(f: extern "C" fn()) -> i32;
                }
                static ONCE: std::sync::Once = std::sync::Once::new();
                ONCE.call_once(|| unsafe {
                    atexit(at_exit_check);
                });
            }
        }
        let exec_size: u64 = 768 << 20;
        let data_size: u64 = UNIT_DATA_SIZE;
        let rw_size: u64 = UNIT_RW_SIZE;
        let syms_size = (crate::syms::SYM_CAP * 8) as u64;
        let exec_base = os::mem::reserve(exec_size as usize, (data_size + rw_size + syms_size) as usize);
        let misc = exec_base;
        let data_base = exec_base + exec_size;
        let rw_base = data_base + data_size;
        let syms_base = rw_base + rw_size;
        // the symbol values are written outside the bump allocators (no-op where the OS overcommits)
        os::mem::commit(syms_base, syms_size as usize);
        // identity state below (the provided crates' shims, the slot table, the unit itself) is
        // built in the kept heap; the compile state (prog, tcx, layouts, front) stays as built
        let provided = crate::zone_alloc::kept_with(|| crate::provided::take_for(&prog));
        let (provided_types, opaque_layouts, check_layouts) = provided_type_tables(&prog, &provided);
        let provided_types = crate::zone_alloc::kept_clone(&provided_types);
        let provided_shared = {
            let ks: Vec<u32> = provided.iter().map(|(k, _)| *k).collect();
            if ks.is_empty() {
                Default::default()
            } else {
                let pt = &provided_types;
                crate::provided::crossing_types(&prog, &tcx, &ks, &|d| {
                    let k = prog.def(d).krate;
                    crate::provided::unbound_reason(&prog, &tcx, k, d, &|a| pt.contains_key(&a)).is_none() && {
                        let key = crate::provided::key(&prog, &tcx, d);
                        provided.iter().any(|(pk, pc)| *pk == k && pc.shims.contains_key(key.as_str()))
                    }
                })
            }
        };
        let provided_shared = crate::zone_alloc::kept_clone(&provided_shared);
        let slots = crate::zone_alloc::kept_with(|| {
            if no_codegen {
                Box::new(crate::rir::SlotTable::new(NC_STRIDE as u64, NC_STRIDE as u64, 8, TIER_REC as u64))
            } else {
                Box::new(crate::rir::SlotTable::new(THUNK_SIZE as u64, STUB_SIZE as u64, 8, TIER_REC as u64))
            }
        });
        // no-codegen: segment 0 is the precompiled thunk and stub pools and the static table
        // (no further segments)
        #[cfg(all(target_arch = "aarch64", target_vendor = "apple"))]
        if no_codegen {
            let g = static_glue::glue();
            let tier = os::mem::alloc_rw(static_glue::SLOTS * TIER_REC);
            slots.install(0, g.thunks, g.stubs, g.table, tier);
            relax_codegen::interp::set_static_thunks(g.thunks, NC_STRIDE as u64, static_glue::SLOTS as u64, g.table);
            relax_codegen::interp::set_tls_fn(static_glue::tls_block_c);
        }
        let slots_ptr = crate::rir::SlotsPtr(&*slots);
        let unit = Unit {
            prog,
            tcx,
            provided_shared,
            provided,
            provided_types,
            lay: { let mut l = crate::layout::Layouts::new(); l.opaque = opaque_layouts; l },
            fns: crate::conc::AppendVec::new_kept(u32::MAX as usize + 1),
            reload_slots: Default::default(),
            reload_rir: None,
            reload_errors: Default::default(),
            reload_specs: Default::default(),
            fn_map: crate::conc::MemoMap::new_kept(17),
            fn_create: Default::default(),
            consts: Default::default(),
            const_readers: Default::default(),
            body_deps: Default::default(),
            statics: Default::default(),
            static_spans: Default::default(),
            sites: Default::default(),
            site_ix: Default::default(),
            rs: RtSyms { fmt_str: crate::rir::Sym::new(0), fmt_int: crate::rir::Sym::new(0), fmt_float: crate::rir::Sym::new(0), fmt_print: crate::rir::Sym::new(0), panic_site: crate::rir::Sym::new(0), panic_bounds: crate::rir::Sym::new(0), str_eq: crate::rir::Sym::new(0), fmod: crate::rir::Sym::new(0), fmodf: crate::rir::Sym::new(0), panic_impl: crate::rir::Sym::new(0), thread_run: crate::rir::Sym::new(0), catch_panic: crate::rir::Sym::new(0), args: crate::rir::Sym::new(0), memcpy: crate::rir::Sym::new(0), memmove: crate::rir::Sym::new(0), memset: crate::rir::Sym::new(0), fmt_push: crate::rir::Sym::new(0), fmt_pop: crate::rir::Sym::new(0), resume: crate::rir::Sym::new(0), land: crate::rir::Sym::new(0), panicking: crate::rir::Sym::new(0), rethrow: crate::rir::Sym::new(0) },
            syms: crate::syms::Syms::new(syms_base, rw_base),
            cache: None,
            start_order: Vec::new(),
            rt: Rt { fmt_str: 0, fmt_int: 0, fmt_float: 0, fmt_print: 0, panic_site: 0, panic_bounds: 0, str_eq: 0, fmod: 0, fmodf: 0, panic_impl: 0, thread_run: 0, catch_panic: 0, args: 0, memcpy: 0, memmove: 0, memset: 0, fmt_push: 0, fmt_pop: 0, resume: 0, land: 0, panicking: 0, rethrow: 0 },
            bodies: Default::default(),
            old_bodies: Default::default(),
            coro_rir: Default::default(),
            coro_busy: Default::default(),
            spec_cached: Default::default(),
            coro_cycle: Default::default(),
            unwind_drop_cache: Default::default(),
            cycle_inprog: Default::default(),
            vtables: crate::conc::MemoMap::new_kept(16),
            locations: Default::default(),
            const_failed: std::collections::HashSet::new(),
            core_path_cache: crate::conc::MemoMap::with_bits(8),
            unpin_cache: Default::default(),
            query_errors,
            errors: Vec::new(),
            core_crate,
            exec_base,
            exec_size,
            slots,
            lazy_entry: 0,
            enter: 0,
            leave: 0,
            code_next: AtomicU64::new(misc),
            data_next: AtomicU64::new(data_base),
            lit_data: Default::default(),
            data_end: data_base + data_size,
            ro_base: data_base,
            // the first bytes stay unused: no domain symbol has the offset 0 (0 marks a static that failed)
            rw_next: AtomicU64::new(rw_base + 64),
            rw_end: rw_base + rw_size,
            opt: crate::opt::OptCfg::from_options(options),
            env: x64::Env { slots: slots_ptr, tls_key: 0, tls_slow: 0, resume: 0, opt_regs: false, syms: 0, sym_class: 0 },
            stats: Stats::default(),
            patch_log: Vec::new(),
            patch_gen: 0,
            // slot 0 of the block: the thread's domain static block (domain.rs)
            tls_next: TLS_FIRST_FREE,
            missing_stubs: Default::default(),
            tier1: {
                let t = Tier1Cfg::from_options(options);
                TIER1_ENABLED.store(t.is_some(), Ordering::Relaxed);
                t
            },
            retired: false,
            keep_compile_state: false,
            code: Default::default(),
            cold_users: HashMap::new(),
            retiers: HashMap::new(),
            frequency_profiles: HashMap::new(),
            hot_entry: 0,
            front: std::sync::Arc::new(FrontShared::new(&sess)),
            ctfe_fn_bodies: Default::default(),
            self_ptr: UnitPtr(std::ptr::null_mut()),
        };
        let mut u = crate::zone_alloc::kept_with(|| Box::new(unit));
        if no_codegen {
            #[cfg(all(target_arch = "aarch64", target_vendor = "apple"))]
            {
                let g = static_glue::glue();
                u.lazy_entry = g.lazy_entry;
                u.hot_entry = g.hot_entry;
                u.enter = g.enter;
                u.leave = g.leave;
            }
        } else {
            let lazy = x64::lazy_entry(rt_compile as *const () as usize as u64);
            u.lazy_entry = u.emit_code(&lazy);
            let hot = x64::lazy_entry(rt_hot as *const () as usize as u64);
            u.hot_entry = u.emit_code(&hot);
            let (el, leave_off) = x64::enter_leave();
            u.enter = u.emit_code(&el);
            u.leave = u.enter + leave_off as u64;
        }
        let ps = panic_tramp(rt_panic_site as *const () as usize as u64);
        let pb = panic_tramp(rt_panic_bounds as *const () as usize as u64);
        let pi = panic_tramp(rt_panic_impl as *const () as usize as u64);
        u.rt = Rt {
            fmt_str: rt_fmt_str as *const () as usize as u64,
            fmt_int: rt_fmt_int as *const () as usize as u64,
            fmt_float: rt_fmt_float as *const () as usize as u64,
            fmt_print: rt_fmt_print as *const () as usize as u64,
            panic_site: 0,
            panic_bounds: 0,
            str_eq: rt_str_eq as *const () as usize as u64,
            fmod: rt_fmod as *const () as usize as u64,
            fmodf: rt_fmodf as *const () as usize as u64,
            panic_impl: 0,
            thread_run: rt_thread_run as *const () as usize as u64,
            catch_panic: rt_catch_panic as *const () as usize as u64,
            args: rt_args as *const () as usize as u64,
            memcpy: os::mem::mem_fns().0,
            memmove: os::mem::mem_fns().1,
            memset: os::mem::mem_fns().2,
            fmt_push: rt_fmt_push as *const () as usize as u64,
            fmt_pop: rt_fmt_pop as *const () as usize as u64,
            resume: 0,
            land: 0,
            panicking: rt_panicking as *const () as usize as u64,
            rethrow: rt_rethrow as *const () as usize as u64,
        };
        if no_codegen {
            #[cfg(all(target_arch = "aarch64", target_vendor = "apple"))]
            {
                let g = static_glue::glue();
                u.rt.panic_site = g.panic_site;
                u.rt.panic_bounds = g.panic_bounds;
                u.rt.panic_impl = g.panic_impl;
                u.rt.resume = g.resume;
                u.rt.land = g.land;
            }
            let _ = (ps, pb, pi);
        } else {
            u.rt.panic_site = u.emit_code(&ps);
            u.rt.panic_bounds = u.emit_code(&pb);
            u.rt.panic_impl = u.emit_code(&pi);
            let rs = panic_tramp(rt_resume_unwind as *const () as usize as u64);
            u.rt.resume = u.emit_code(&rs);
            let ld = x64::land();
            u.rt.land = u.emit_code(&ld);
        }
        u.env.resume = u.rt.resume;
        if os::fault::NEEDS_HANG_TRAMP {
            HANG_TRAMP.store(u.emit_code(&crate::x64::hang_tramp(rt_hang as *const () as usize as u64)), Ordering::Relaxed);
        }
        u.env.syms = u.syms.vals;
        u.env.sym_class = u.syms.classes;
        u.rs = RtSyms {
            fmt_str: u.syms.intern(crate::syms::SymKind::Rt("fmt_str"), u.rt.fmt_str),
            fmt_int: u.syms.intern(crate::syms::SymKind::Rt("fmt_int"), u.rt.fmt_int),
            fmt_float: u.syms.intern(crate::syms::SymKind::Rt("fmt_float"), u.rt.fmt_float),
            fmt_print: u.syms.intern(crate::syms::SymKind::Rt("fmt_print"), u.rt.fmt_print),
            panic_site: u.syms.intern(crate::syms::SymKind::Rt("panic_site"), u.rt.panic_site),
            panic_bounds: u.syms.intern(crate::syms::SymKind::Rt("panic_bounds"), u.rt.panic_bounds),
            str_eq: u.syms.intern(crate::syms::SymKind::Rt("str_eq"), u.rt.str_eq),
            fmod: u.syms.intern(crate::syms::SymKind::Rt("fmod"), u.rt.fmod),
            fmodf: u.syms.intern(crate::syms::SymKind::Rt("fmodf"), u.rt.fmodf),
            panic_impl: u.syms.intern(crate::syms::SymKind::Rt("panic_impl"), u.rt.panic_impl),
            thread_run: u.syms.intern(crate::syms::SymKind::Rt("thread_run"), u.rt.thread_run),
            catch_panic: u.syms.intern(crate::syms::SymKind::Rt("catch_panic"), u.rt.catch_panic),
            args: u.syms.intern(crate::syms::SymKind::Rt("args"), u.rt.args),
            memcpy: u.syms.intern(crate::syms::SymKind::Rt("memcpy"), u.rt.memcpy),
            memmove: u.syms.intern(crate::syms::SymKind::Rt("memmove"), u.rt.memmove),
            memset: u.syms.intern(crate::syms::SymKind::Rt("memset"), u.rt.memset),
            fmt_push: u.syms.intern(crate::syms::SymKind::Rt("fmt_push"), u.rt.fmt_push),
            fmt_pop: u.syms.intern(crate::syms::SymKind::Rt("fmt_pop"), u.rt.fmt_pop),
            resume: u.syms.intern(crate::syms::SymKind::Rt("resume"), u.rt.resume),
            land: u.syms.intern(crate::syms::SymKind::Rt("land"), u.rt.land),
            panicking: u.syms.intern(crate::syms::SymKind::Rt("panicking"), u.rt.panicking),
            rethrow: u.syms.intern(crate::syms::SymKind::Rt("rethrow"), u.rt.rethrow),
        };
        crate::opt::set_mem_fns(u.rs.memcpy, u.rs.memmove);
        // runtime symbols lowering interns on first use: a cached entry may name them before any
        // function of this run was lowered (the cache links them by name)
        let _ = u.syms.intern(crate::syms::SymKind::Rt("divrem128"), crate::lower::divrem128_addr());
        let _ = u.syms.intern(crate::syms::SymKind::Rt("cannot_unwind"), crate::lower::cannot_unwind_addr());
        for (name, addr) in ["wide_f32", "wide_f64"].into_iter().zip(crate::lower::wide_float_addrs()) {
            let _ = u.syms.intern(crate::syms::SymKind::Rt(name), addr);
        }
        let _ = u.syms.intern(crate::syms::SymKind::Rt("null"), 0);
        // MAKEPAD_RUN_PERTURB: shift what code must not depend on (syms.rs reloc_sig)
        let n = crate::syms::perturb(options);
        if n > 0 {
            let _ = u.data(&vec![0x5a; (n * 72) as usize], 8, crate::syms::DataKind::Bytes);
            let _ = u.rw_data(n * 40, 8);
            for i in 0..n {
                let _ = u.site(0, i as u32, "perturb");
                let _ = u.syms.intern(crate::syms::SymKind::Foreign(format!("perturb{}", i)), 0);
                let _ = u.fn_id(FnKey::Closure(u32::MAX, i as u32, u32::MAX));
            }
        }
        // a call of a panic entry inlined into an invoked call's body unwinds into its pad
        crate::opt::set_unwind_hosts(vec![u.rs.panic_site, u.rs.panic_bounds, u.rs.panic_impl]);
        u.init_tls();
        // layouts normalize projections in field types (`struct P(<P as Flags>::Internal)`)
        // provided crates' trusted types: Relax's layout must be rustc's, else the type is used
        // through the crate's fns only (opaque) and the mismatch is reported
        if !check_layouts.is_empty() {
            let probe = crate::layout::Layouts::new();
            let mut demoted = Vec::new();
            for (d, size, align, path, offs) in check_layouts {
                let t = u.tcx.tys.intern(TyKind::Adt(d, crate::types::List::EMPTY, crate::types::RegionArgsId::EMPTY));
                let l = probe.of(&crate::layout::LayoutCx::new(&u.prog), &u.tcx, t);
                let offs_ok = offs.is_empty() || (offs.len() == l.fields.len() && offs.iter().zip(&l.fields).all(|(a, b)| *a == *b as u64));
                if l.size != size || l.align != align || !offs_ok {
                    eprintln!("makepad: provided type {}: rustc lays it out in {} bytes (align {}, field offsets {:?}), makepad run in {} (align {}, {:?}): opaque, used through the crate's fns only", path, size, align, offs, l.size, l.align, l.fields);
                    demoted.push((d, size, align));
                }
            }
            for (d, size, align) in demoted {
                u.lay.opaque.insert(d, (size, align));
                crate::zone_alloc::kept_with(|| u.provided_types.insert(d, 0));
            }
        }
        u.self_ptr = UnitPtr(&mut *u as *mut Unit);
        #[cfg(target_os = "android")]
        u.slots.set_callable_hook(c_callable_hook, u.self_ptr.0 as usize);
        u.tcx.const_eval_provider = Some(relax_types::const_eval::ConstEvalProvider {
            context: u.self_ptr.0 as usize, evaluate: const_eval_to_valtree,
        });
        set_shared_front(&mut u, core_crate);
        u.prog.body_memos.store(true, Ordering::Relaxed);
        u.tcx.publish_collected();
        install_ice_hook();
        ensure_altstack();
        os::fault::install();
        u
    }

    /// Whether `code` would link anywhere in the code area (O3 checks before installing).
    pub fn link_check(&self, code: &[u8], relocs: &[crate::rir::RelocAt]) -> bool {
        let mut c = code.to_vec();
        let lo = self.code_next.load(Ordering::Relaxed);
        crate::rir::link(&mut c, lo, relocs, &|r| self.env.reloc_value(r)).is_ok() && crate::rir::link(&mut c, self.exec_base + self.exec_size - 16, relocs, &|r| self.env.reloc_value(r)).is_ok()
    }

    /// Records code at `at` in the address-ordered index (sandbox code spaces lie below later
    /// unit code: inserted in order).
    pub(crate) fn index_code(&mut self, at: u64, len: u32, id: u32, unwind: Option<crate::rir::UnwindInfo>) {
        self.code.write().unwrap_or_else(|e| e.into_inner()).add(at, len, id, unwind);
    }

    /// Drops the index entries of code in [lo, hi) (a recycled sandbox code space).
    pub(crate) fn unindex_range(&mut self, lo: u64, hi: u64) {
        let mut t = self.code.write().unwrap_or_else(|e| e.into_inner());
        t.index.retain(|x| x.0 < lo || x.0 >= hi);
        t.unwind.retain(|a, _| *a < lo || *a >= hi);
    }

    /// Reserves code memory without writing it (sandbox code spaces).
    pub(crate) fn reserve_code(&self, len: u64) -> u64 {
        bump(&self.code_next, len, 1 << 16, self.exec_base + self.exec_size, "out of code memory")
    }

    /// Links code for `at` (the caller placed it there).
    pub(crate) fn link_at(&self, at: u64, code: &[u8], relocs: &[crate::rir::RelocAt]) -> Result<Vec<u8>, String> {
        let mut c = code.to_vec();
        crate::rir::link(&mut c, at, relocs, &|r| self.env.reloc_value(r))?;
        Ok(c)
    }

    /// The lazy stub of slot `id` (what its table word holds before its first compile).
    pub(crate) fn stub_of(&self, id: u32) -> u64 {
        self.slots.stub(id)
    }

    /// Code at a fresh address with its relocated fields linked for that address.
    /// no-codegen mode: the address a slot takes for `code` (which must be a plain jump with no
    /// relocations: an interpreter entry or a foreign function); other machine code is an error.
    fn no_codegen_target(&self, code: &[u8], relocs: &[crate::rir::RelocAt], what: &str) -> Result<u64, String> {
        if relax_codegen::interp::blob::is_blob(code) {
            let mut c = code.to_vec();
            crate::rir::link(&mut c, 0, relocs, &|r| self.env.reloc_value(r))?;
            return relax_codegen::interp::install_linked(&c, self.env.tls_slow, self.env.resume);
        }
        match relax_codegen::interp::redirect_target(code) {
            Some(t) if relocs.is_empty() => Ok(t),
            _ => Err(format!("makepad run: no-codegen mode: `{}` would need machine code ({} bytes)", what, code.len())),
        }
    }

    /// Code for an address that is not a slot (error stubs): written to code memory, or in
    /// no-codegen mode the target of its jump.
    fn emit_or_redirect(&self, code: &[u8], relocs: &[crate::rir::RelocAt], what: &str) -> Result<u64, String> {
        if self.prog.session.options.no_codegen || relax_codegen::interp::blob::is_blob(code) {
            self.no_codegen_target(code, relocs, what)
        } else {
            self.emit_linked(code, relocs)
        }
    }

    fn emit_linked(&self, code: &[u8], relocs: &[crate::rir::RelocAt]) -> Result<u64, String> {
        let at = bump(&self.code_next, code.len() as u64, 16, self.exec_base + self.exec_size - os::mem::CODE_TAIL, "out of code memory");
        let mut c = code.to_vec();
        crate::rir::link(&mut c, at, relocs, &|r| self.env.reloc_value(r))?;
        unsafe { os::mem::write_code(at, &c) };
        Ok(at)
    }

    /// A block of `len` bytes of code memory, 16-byte aligned, not written (the startup image:
    /// cache.rs prelink links its code for this address and writes it whole).
    pub(crate) fn code_block(&self, len: u64) -> u64 {
        let at = bump(&self.code_next, len, 16, self.exec_base + self.exec_size - os::mem::CODE_TAIL, "out of code memory");
        // filled whole right away (the startup image's code)
        os::mem::prefault(at, len as usize);
        at
    }

    fn emit_code(&self, b: &[u8]) -> u64 {
        let at = bump(&self.code_next, b.len() as u64, 16, self.exec_base + self.exec_size - os::mem::CODE_TAIL, "out of code memory");
        unsafe { os::mem::write_code(at, b) };
        at
    }

    /// Copies bytes into the data area; returns their address. `kind` says what the bytes
    /// hold (syms.rs: whether constant loads from them may fold, and to what).
    pub fn data(&self, b: &[u8], align: u64, kind: crate::syms::DataKind) -> u64 {
        // one byte more than the object: no object ends where the next begins, so a pointer one
        // past an object's end is never the next object's start (syms.rs provenance)
        let at = bump(&self.data_next, b.len() as u64 + 1, align, self.data_end, "out of data memory");
        unsafe { write_bytes(at, b) };
        self.syms.add_data(at, b.len() as u64, align.max(1), kind, false);
        at
    }

    /// `size` bytes of data memory aligned to `align`, for objects the caller places and
    /// registers itself (the startup image's data, cache/image.rs place_data).
    pub(crate) fn data_block(&self, size: u64, align: u64) -> u64 {
        let at = bump(&self.data_next, size, align, self.data_end, "out of data memory");
        os::mem::prefault(at, size as usize);
        at
    }

    /// A writable data object (a value promoted inside a static's initializer): its address;
    /// registered so pointers to it in constant data have a symbol (it never folds).
    pub fn rw_data_obj(&self, size: u64, align: u64, kind: crate::syms::DataKind) -> u64 {
        let at = bump(&self.rw_next, size + 1, align, self.rw_end, "out of static memory");
        self.syms.add_data(at, size, align.max(1), kind, true);
        at
    }

    /// The symbol of the data object at `addr` (an address `data` / `lit_data` returned).
    pub fn data_sym(&self, addr: u64) -> crate::rir::Sym {
        match self.syms.data_at(addr).or_else(|| self.syms.rw_syms.lock().unwrap_or_else(|e| e.into_inner()).get(&addr).copied()) {
            Some(s) => s,
            None => panic!("makepad: {:#x} is not the start of a data object (unsymbolised address)", addr),
        }
    }

    /// A string literal's bytes as a symbol.
    pub fn lit_sym(&self, b: &[u8]) -> crate::rir::Sym {
        let a = self.lit_data(b);
        self.data_sym(a)
    }

    /// A panic site as a symbol (its value is the site's index).
    pub fn site_sym(&self, file: u32, pos: u32, msg: &str) -> crate::rir::Sym {
        let i = self.site(file, pos, msg);
        self.syms.intern(crate::syms::SymKind::Site(i as u32), i)
    }

    /// A static's memory as a symbol.
    pub fn static_sym(&mut self, d: DefId) -> crate::rir::Sym {
        let a = self.static_addr(d);
        self.syms.intern(crate::syms::SymKind::Static(d), a)
    }

    /// The static containing `addr` (pointers to statics stored in constants), with the
    /// offset into it. Statics' storage has no gap between them (unlike data objects), so one
    /// past a static's end may be the next block's start (another static, a writable block a
    /// fn's code owns): such a pointer is not attributed.
    pub fn static_sym_at(&self, addr: u64) -> Option<crate::rir::Sym> {
        let (&start, &(d, size)) = self.static_spans.range(..=addr).next_back()?;
        if (addr != start && addr >= start + size) || self.statics.get(&d) != Some(&start) {
            return None;
        }
        let mut s = self.syms.intern(crate::syms::SymKind::Static(d), start);
        s.off = (addr - start) as i64;
        Some(s)
    }

    /// A C symbol (function or foreign static) as a symbol; a fn this host lacks is its
    /// panicking stand-in.
    pub fn foreign_sym(&self, d: DefId) -> Option<crate::rir::Sym> {
        let a = self.foreign_addr(d)?;
        let name = self.link_name(d);
        let missing = self.missing_stubs.lock().unwrap_or_else(|e| e.into_inner()).get(&name) == Some(&a);
        let k = if missing { crate::syms::SymKind::Missing(name) } else { crate::syms::SymKind::Foreign(name) };
        let s = self.syms.intern(k, a);
        crate::zone_alloc::kept_with(|| { self.syms.foreign_defs.lock().unwrap_or_else(|e| e.into_inner()).entry(s.id).or_insert(d); });
        Some(s)
    }

    /// The slot whose thunk is at `addr`.
    pub fn thunk_slot(&self, addr: u64) -> Option<u32> {
        let id = match self.slots.find(0, addr) {
            Some((id, 0)) => id,
            _ => return None,
        };
        if (id as usize) < self.fns.len() {
            // A thunk observed as a pointer (e.g. in constant data) escaped even
            // when no current RIR contains the FnAddr that originally made it.
            self.fns[id as usize].entry_ref.store(true, Ordering::Relaxed);
            Some(id)
        } else {
            None
        }
    }

    /// Constant-load folding for the optimiser (syms.rs ro_read).
    pub fn ro_reader(&self) -> Option<crate::opt::RoReader> {
        Some(crate::opt::RoReader::new(crate::syms::ro_read, self as *const Unit as usize))
    }

    /// The bytes of a string literal: one copy per content (rustc allocate_bytes_dedup for
    /// str, byte-string and C-string literals: `b"foo"` in two fns is one address).
    pub fn lit_data(&self, b: &[u8]) -> u64 {
        self.data_dedup(b, 1)
    }

    /// Read-only bytes at `align`, one copy per content and alignment (literals and promoted
    /// constants: rustc interns allocations by bytes + align). Keyed by both, so which copy a
    /// use gets never depends on what was compiled first (reusing a copy that happened to be
    /// aligned enough gave one constant two symbols in one build order and one in another).
    pub fn data_dedup(&self, b: &[u8], align: u64) -> u64 {
        let align = align.max(1);
        let mut m = self.lit_data.lock().unwrap_or_else(|e| e.into_inner());
        let key = (b.to_vec(), align);
        if let Some(&a) = m.get(&key) {
            return a;
        }
        let _k = crate::zone_alloc::kept();
        let key = key.clone();
        let a = self.data(b, align, crate::syms::DataKind::Bytes);
        m.insert(key, a);
        a
    }

    /// A fresh anonymous mutable block (promoted temporaries) as a symbol. `ty`: the value it
    /// holds, when lowering made it (a block linked from the disk cache is untyped): a constant
    /// value pointing into it can then store the block's bytes (syms.rs promoted_copy).
    pub fn rw_sym(&self, size: u64, align: u64, ty: Option<TyId>) -> crate::rir::Sym {
        let a = self.rw_data(size, align);
        let _k = crate::zone_alloc::kept();
        let k = {
            let mut b = self.syms.rw_blocks.lock().unwrap_or_else(|e| e.into_inner());
            b.push((size, align));
            b.len() as u32 - 1
        };
        if let Some(t) = ty {
            self.syms.rw_starts.lock().unwrap_or_else(|e| e.into_inner()).insert(a, (size, t));
        }
        let s = self.syms.intern(crate::syms::SymKind::Rw(k), a);
        self.syms.rw_syms.lock().unwrap_or_else(|e| e.into_inner()).insert(a, s);
        s
    }

    /// Mutable data (statics).
    pub fn rw_data(&self, size: u64, align: u64) -> u64 {
        bump(&self.rw_next, size, align, self.rw_end, "out of static memory")
    }

    /// Whether `[a, a + len)` is allocated writable static memory (statics, the writable
    /// blocks code owns).
    pub fn in_rw(&self, a: u64, len: u64) -> bool {
        a >= self.rw_end - UNIT_RW_SIZE && a.checked_add(len).is_some_and(|e| e <= self.rw_next.load(Ordering::Acquire))
    }

    pub fn ro_range(&self) -> (u64, u64) {
        (self.ro_base, self.data_next.load(Ordering::Acquire))
    }

    /// Bytes used of the unit's code, read-only data and static memory reservations (relax
    /// inspect's `memory`; atomics only, no owner): (code, data, statics).
    pub fn reserved_use(&self) -> (u64, u64, u64) {
        let code = self.code_next.load(Ordering::Relaxed).saturating_sub(self.exec_base);
        let data = self.data_next.load(Ordering::Relaxed).saturating_sub(self.ro_base);
        let rw = self.rw_next.load(Ordering::Relaxed).saturating_sub(self.rw_end - UNIT_RW_SIZE);
        (code, data, rw)
    }

    /// Reserves `size` bytes of every thread's thread-local block (zeroed per thread on
    /// first use); returns the offset for `Inst::TlsAddr`.
    pub fn tls_alloc(&mut self, size: u32, align: u32) -> u32 {
        let a = align.max(1);
        let at = (self.tls_next + a - 1) / a * a;
        if (at + size) as usize > TLS_CAP {
            panic!("thread-local block full");
        }
        self.tls_next = at + size;
        at
    }

    /// Thread-local blocks: one OS key per process holds each thread's block; compiled code
    /// reads it through the OS's fast path (os::tls::jit_offset), else calls tls_slow.
    fn init_tls(&mut self) {
        // no-codegen: no inline TLS fast path and no tls_slow glue; the interpreter asks
        // tls_block (static_glue::tls_block_c)
        if self.prog.session.options.no_codegen {
            self.env.tls_key = 0;
            self.env.tls_slow = 0;
            return;
        }
        let key = tls_key();
        let off = os::tls::jit_offset(key, &|b| self.emit_code(b));
        self.env.tls_key = off;
        self.env.tls_slow = self.emit_code(&x64::tls_slow(rt_tls_block as *const () as usize as u64, off));
    }

    pub fn site(&self, file: u32, pos: u32, msg: &str) -> u64 {
        let mut ix = self.site_ix.lock().unwrap_or_else(|e| e.into_inner());
        let key = (file, pos, msg.to_string());
        if let Some(&i) = ix.get(&key) {
            return i;
        }
        // identity state (panic sites are read by the program): the strings are the program's
        let _k = crate::zone_alloc::kept();
        let key = key.clone();
        let mut s = self.sites.lock().unwrap_or_else(|e| e.into_inner());
        s.push(Site { file, pos, msg: msg.to_string() });
        let i = s.len() as u64 - 1;
        ix.insert(key, i);
        i
    }

    /// Whether the monomorphic type `t` is Copy (rustc's Copy rules: primitives, shared
    /// references, raw and fn pointers, tuples / arrays of Copy, ADTs with a Copy impl; a
    /// closure only without captures here -- false is the safe answer for `is_copy` users).
    /// The anonymous const holding struct `d`'s declared default for field `field` (`field: T =
    /// expr`, or makepad's `#[rust(expr)]` in live sessions; parser derive.rs default_consts).
    pub fn field_default_const(&self, d: crate::program::DefId, field: u32) -> Option<crate::program::DefId> {
        if field & 0x8000_0000 != 0 {
            return None;
        }
        let ty = self.prog.syms.str(self.prog.def(d).name).trim_start_matches("r#").to_string();
        let cname = crate::parser::derive::default_const_name(&ty, None, self.prog.syms.str(field).trim_start_matches("r#"));
        let s = self.prog.syms.get(&cname)?;
        self.prog.lookup_in_scope(self.prog.def(d).scope, s, false)
    }

    /// Whether `t` has a `Default` impl (a field added live takes it in running values, M10).
    pub fn has_default(&self, t: TyId) -> bool {
        match self.core_path(&["default", "Default"]) {
            Some(td) => self.tcx.type_implements_trait(&self.prog, crate::tcx::TypingEnv::fully_monomorphized(&self.prog), TraitRef { def_id: td, args: vec![GenericArg::Type(t)] }).must_apply_modulo_regions(),
            None => false,
        }
    }

    pub fn is_copy(&self, t: TyId) -> bool {
        let Some(td) = self.tcx.lang_items.get(LangItem::Copy) else { return false; };
        self.tcx.type_implements_trait(&self.prog, crate::tcx::TypingEnv::fully_monomorphized(&self.prog), TraitRef { def_id: td, args: vec![GenericArg::Type(t)] }).must_apply_modulo_regions()
    }

    /// Whether the monomorphic type `t` is Unpin (an auto trait: every component is, and
    /// `PhantomPinned` is not). Uncertain cases (trait objects without `+ Unpin`, opaque types,
    /// closures over them) answer false: the safe side for noalias.
    pub fn is_unpin(&self, t: TyId) -> bool {
        let t = self.tcx.tys.erase_regions(t);
        if let Some(b) = self.unpin_cache.get(&t) {
            return b;
        }
        // a recursive type is Unpin unless something else says no
        let ik = (2u8, t, crate::conc::thread_id());
        if self.cycle_inprog.contains(&ik) {
            cyc_cut(2, t);
            return true;
        }
        self.cycle_inprog.insert(ik, ());
        let my_depth = cyc_enter(2, t);
        let r = match self.tcx.tys.kind(t).clone() {
            TyKind::Int(_) | TyKind::Float(_) | TyKind::Bool | TyKind::Char | TyKind::Never | TyKind::Str => true,
            TyKind::Ref(..) | TyKind::Ptr(..) | TyKind::FnPtr(..) | TyKind::FnDef(..) => true,
            TyKind::Tuple(v) => self.tcx.tys.list(v).iter().all(|x| self.is_unpin(*x)),
            TyKind::Array(e, _) | TyKind::Slice(e) => self.is_unpin(e),
            // an async block / fn body is !Unpin (rustc: it may hold references into itself)
            TyKind::Closure(file, ce, crate::types::ClosureArgs { tupled_upvars_ty: up, .. }) => !self.tcx.coroutines.contains(&(file, ce)) && self.is_unpin(up),
            TyKind::Adt(d, args, _) => {
                if self.prog.name(d) == "PhantomPinned" {
                    false
                } else {
                    let fields: Vec<TyId> = match self.tcx.adts.get(&d) {
                        Some(a) => a.variants.iter().flat_map(|v| v.fields.iter().map(|f| f.ty)).collect(),
                        None => Vec::new(),
                    };
                    let mut ok = true;
                    for f in fields {
                        let ft = self.tcx.tys.subst_erasing_regions(f, self.tcx.tys.list(args));
                        if !self.is_unpin(ft) {
                            ok = false;
                            break;
                        }
                    }
                    ok
                }
            }
            _ => false,
        };
        self.cycle_inprog.remove(&ik);
        if !cyc_leave(my_depth) {
            self.unpin_cache.insert(t, r);
        }
        r
    }

    /// Deferred diagnostic items and Relax runtime helpers, not language-item identities.
    pub fn core_path(&self, path: &[&str]) -> Option<DefId> {
        let key = path.join("::");
        if let Some(x) = self.core_path_cache.get(&key) {
            return x;
        }
        let prog = &self.prog;
        let mut cur = Some(prog.mods[prog.crates[self.core_crate as usize].root_mod as usize].def);
        for p in path {
            cur = match (cur, prog.syms.get(p)) {
                (Some(c), Some(s)) => match prog.lookup_in_container(c, s, true) {
                    Some(d) => Some(d),
                    None => prog.lookup_in_container(c, s, false),
                },
                _ => None,
            };
        }
        self.core_path_cache.insert(key, cur);
        cur
    }

    pub fn closure_storage_layout(&self, ty: TyId) -> crate::layout::Layout {
        loop {
            let lcx = crate::layout::LayoutCx::new(&self.prog);
            let layout = self.lay.closure_storage_layout(&lcx, &self.tcx, ty);
            if lcx.missing.borrow().is_empty() { return layout; }
            for ct in lcx.missing.into_inner() {
                if self.tcx.coro_frames.get(&ct).is_none() {
                    owner::call(owner::Class::Job, || unsafe { (&mut *self.self_ptr.0).coro_frame(ct) });
                }
            }
        }
    }

    pub fn closure_storage(&self, ty: TyId) -> crate::layout::ClosureStorage {
        self.lay.closure_storage(&self.prog, &self.tcx, ty)
    }

    /// The layout of `t`. A coroutine's state area comes from its lowered poll function: the
    /// coroutines `t` contains whose body was not lowered yet are lowered first (rustc computes
    /// a coroutine's layout from its optimized MIR the same way, on demand).
    pub fn layout_of(&self, t: TyId) -> &'static crate::layout::Layout {
        // rustc_ty_utils/src/layout.rs:63-75 @8d1a764: codegen layouts use
        // normalized types, including const aliases in static array lengths.
        let t = self.tcx.normalize_erasing_regions(&self.prog, crate::tcx::TypingEnv::fully_monomorphized(&self.prog), t);
        loop {
            let lcx = crate::layout::LayoutCx::new(&self.prog);
            let l = self.lay.of(&lcx, &self.tcx, t);
            if lcx.missing.borrow().is_empty() {
                return l;
            }
            for ct in lcx.missing.into_inner() {
                if self.tcx.coro_frames.get(&ct).is_none() {
                    // lowering a body changes the unit: a unit call
                    owner::call(owner::Class::Job, || unsafe { (&mut *self.self_ptr.0).coro_frame(ct) });
                }
            }
        }
    }

    /// Lowers coroutine `ct`'s poll function (once) and records its saved locals. Always
    /// records an answer: a body that does not lower gets none (its poll function's build error
    /// is what runs), a coroutine containing itself is E0733.
    fn coro_frame(&mut self, ct: TyId) {
        let ct = self.tcx.tys.erase_regions(ct);
        if self.tcx.coro_frames.get(&ct).is_some() {
            return;
        }
        if self.coro_busy.lock().unwrap_or_else(|e| e.into_inner()).contains(&ct) {
            self.coro_cycle.store(true, Ordering::Relaxed);
            self.tcx.coro_frames.insert(ct, (0, 1));
            return;
        }
        let id = match self.closure_fn_id(ct) {
            Some(id) => id,
            None => {
                self.tcx.coro_frames.insert(ct, (0, 1));
                return;
            }
        };
        // the owner's checked body (the poll function is lowered from it)
        if let FnKey::Closure(cfile, _, parent) = self.fns[id as usize].key.clone() {
            if !self.has_body(parent, cfile) {
                if let FnKey::Inst(d, args) = self.fns[parent as usize].key.clone() {
                    let _o = crate::clock::origin("coro-owner");
                    let body = crate::typeck::codegen_body(&self.prog, &self.tcx, d, &args);
                    if body.errors.is_empty() {
                        self.put_body(parent, body);
                    }
                }
            }
        }
        self.coro_busy.lock().unwrap_or_else(|e| e.into_inner()).push(ct);
        let up = self as *mut Unit;
        let r = unsafe { build_rir_lowered(up, id) };
        let u = unsafe { &mut *up };
        u.coro_busy.lock().unwrap_or_else(|e| e.into_inner()).retain(|x| *x != ct);
        match r {
            Ok(f) => u.coro_rir.insert(id, f),
            Err(e) => {
                if u.fns[id as usize].build_err.is_none() {
                    u.reload_errors.insert(id);
                    u.fns[id as usize].build_err = Some(e.into_boxed_str());
                }
            }
        }
        if u.tcx.coro_frames.get(&ct).is_none() {
            u.tcx.coro_frames.insert(ct, (0, 1));
        }
    }

    /// Runtime cleanup includes live-session extension allocations, even when the
    /// language needs_drop query is false. Walk owned fields so enclosing aggregates
    /// schedule the glue that emits GLUE_EXT_DROP / rt_ext_free in lower.rs.
    pub fn needs_drop_glue(&self, t: TyId) -> bool {
        let env = crate::tcx::TypingEnv::fully_monomorphized(&self.prog);
        if self.tcx.needs_drop(&self.prog, env, t) { return true; }
        if !crate::layout::LIVE_SESSION.load(Ordering::Relaxed) { return false; }
        let mut todo = vec![t];
        let mut seen = crate::fxhash::HashSet::default();
        while let Some(t) = todo.pop() {
            if !seen.insert(t) { continue; }
            match self.tcx.tys.kind(t) {
                TyKind::Adt(d, args, _) => {
                    let adt = &self.tcx.adts[d];
                    if adt.is_manually_drop() || adt.is_union() || adt.is_phantom_data() { continue; }
                    if self.tcx.tys.is_concrete(t) && self.layout_of(t).ext.is_some() { return true; }
                    for (vi, v) in adt.variants.iter().enumerate() {
                        for fi in 0..v.fields.len() {
                            todo.push(self.tcx.field_ty(&self.prog, env, *d, vi as u32, fi as u32, self.tcx.tys.list(*args)));
                        }
                    }
                }
                TyKind::Tuple(fields) => todo.extend_from_slice(self.tcx.tys.list(*fields)),
                TyKind::Array(elem, n) => { if n.try_to_target_usize(&self.tcx.tys) != Some(0) { todo.push(*elem); } }
                TyKind::Slice(elem) | TyKind::Closure(_, _, crate::types::ClosureArgs { tupled_upvars_ty: elem, .. }) => todo.push(*elem),
                _ => {}
            }
        }
        false
    }

    /// Whether dropping a `t` can unwind: some Drop impl in it outside Relax's std (core /
    /// alloc / std), or a trait object (its vtable's drop is unknown). std's Drop impls only
    /// free memory and drop the values they own (their type arguments): a Vec<String> drop
    /// cannot unwind, a Vec<UserType> drop can if UserType's Drop is user code. A drop that
    /// cannot unwind is a plain call (no cleanup pad behind it).
    pub fn drop_may_unwind(&self, t: TyId) -> bool {
        let t = self.tcx.tys.erase_regions(t);
        if let Some(b) = self.unwind_drop_cache.get(&t) {
            return b;
        }
        if !self.tcx.needs_drop(&self.prog, crate::tcx::TypingEnv::fully_monomorphized(&self.prog), t) {
            return false;
        }
        // recursive types: assume no while computing (a cycle only goes through pointers); answers
        // below such a cut are not memoized (cyc_leave)
        let ik = (1u8, t, crate::conc::thread_id());
        if self.cycle_inprog.contains(&ik) {
            cyc_cut(1, t);
            return false;
        }
        self.cycle_inprog.insert(ik, ());
        let my_depth = cyc_enter(1, t);
        let k = self.tcx.tys.kind(t).clone();
        let r = match k {
            TyKind::Adt(d, args, _) => {
                let imp = self.tcx.adt_destructor(d).map(|dtor| self.prog.def(dtor.did).parent);
                let user_impl = match imp {
                    Some(i) if i != crate::program::NO_DEF => {
                        let kr = self.prog.def(i).krate as usize;
                        let name = self.prog.syms.str(self.prog.crates[kr].name);
                        !matches!(name, "core" | "alloc" | "std")
                    }
                    Some(_) => true,
                    None => false,
                };
                if user_impl {
                    true
                } else {
                    let mut any = false;
                    if imp.is_some() {
                        // a std Drop impl drops what its type arguments own
                        for a in self.tcx.tys.list(args.clone()).iter().copied() {
                            if self.drop_may_unwind(a) {
                                any = true;
                            }
                        }
                    }
                    if let Some(adt) = self.tcx.adts.get(&d) {
                        if !adt.is_union {
                            for (vi, v) in adt.variants.iter().enumerate() {
                                for fi in 0..v.fields.len() {
                                    let ft = self.tcx.field_ty(&self.prog, crate::tcx::TypingEnv::fully_monomorphized(&self.prog), d, vi as u32, fi as u32, self.tcx.tys.list(args));
                                    if self.drop_may_unwind(ft) {
                                        any = true;
                                    }
                                }
                            }
                        }
                    }
                    any
                }
            }
            TyKind::Tuple(v) => {
                let mut any = false;
                for x in self.tcx.tys.list(v).iter().copied() {
                    if self.drop_may_unwind(x) {
                        any = true;
                    }
                }
                any
            }
            TyKind::Array(e, n) => n.try_to_target_usize(&self.tcx.tys) != Some(0) && self.drop_may_unwind(e),
            TyKind::Closure(_, _, crate::types::ClosureArgs { tupled_upvars_ty: up, .. }) => self.drop_may_unwind(up),
            _ => true,
        };
        self.cycle_inprog.remove(&ik);
        if !cyc_leave(my_depth) {
            self.unwind_drop_cache.insert(t, r);
        }
        r
    }

    /// The function of a closure type (its body is lowered from the owner instance's).
    pub fn closure_fn_id(&self, t: TyId) -> Option<u32> {
        if let TyKind::Closure(file, ce, crate::types::ClosureArgs { parent_args: owner, .. }) = self.tcx.tys.kind(t).clone() {
            if let TyKind::FnDef(d, args, _) = self.tcx.tys.kind(owner).clone() {
                let parent = self.fn_id(FnKey::Inst(d, self.tcx.tys.list(args).to_vec()));
                return Some(self.fn_id(FnKey::Closure(file, ce, parent)));
            }
        }
        None
    }

    /// vtable of `src` as `dy` (built once): [drop, size, align, methods...]
    pub fn vtable(&self, src: TyId, dy: TyId) -> Result<u64, String> {
        let src = self.tcx.tys.erase_regions(src);
        let dy = self.tcx.tys.erase_regions(dy);
        if let Some(a) = self.vtables.get(&(src, dy)) {
            return Ok(a);
        }
        let (td, targs) = match self.tcx.tys.kind(dy).clone() {
            TyKind::Dyn(td, targs, _, _, _, _, _) => (td, targs),
            _ => return Err("vtable for a non-dyn type".to_string()),
        };
        let l = self.layout_of(src);
        let mut words: Vec<u64> = Vec::new();
        // per word: the symbol of a pointer (thunks, supertrait vtables), None for integers
        let mut wsyms: Vec<Option<crate::rir::Sym>> = Vec::new();
        let drop = if self.needs_drop_glue(src) {
            let id = self.fn_id(FnKey::Glue(GLUE_DROP, src));
            self.entry(id)
        } else {
            0
        };
        words.push(drop);
        wsyms.push(self.thunk_slot(drop).map(|id| self.syms.intern(crate::syms::SymKind::Thunk(id), drop)));
        words.push(crate::layout::round_up(l.size, l.align) as u64);
        words.push(l.align as u64);
        wsyms.push(None);
        wsyms.push(None);
        // rustc_trait_selection/src/traits/vtable.rs:199-298.
        use relax_types::next_solver::vtable::VtblSegment;
        let mut compact = vec![src]; compact.extend_from_slice(self.tcx.tys.list(targs));
        let regions = self.tcx.tys.intern_region_args(&vec![crate::types::RegionId::ERASED; self.tcx.generic_indices_of(&self.prog, td).lifetimes.len()]);
        // rustc_middle/src/ty/fold.rs:321-326 @8d1a764: the object's
        // principal binder is instantiated before requesting vtable entries.
        let tr = self.tcx.instantiate_bound_regions_with_erased(relax_types::next_solver::TraitRef { def_id: td, args: self.tcx.complete_args(&self.prog, td, &compact, regions) });
        for segment in self.tcx.prepare_vtable_segments(&self.prog, tr) {
            let VtblSegment::TraitOwnEntries { trait_ref, emit_vptr } = segment else { continue };
            for method in self.tcx.own_existential_vtable_entries(&self.prog, trait_ref.def_id) {
                let indices = self.tcx.generic_indices_of(&self.prog, method);
                let mut args = trait_ref.args.clone();
                for arg in &indices.identity_args(&self.tcx.tys)[args.len()..] {
                    match arg {
                        relax_types::consts::GenericArg::Lifetime(_) => args.push(relax_types::consts::GenericArg::Lifetime(crate::types::RegionId::ERASED)),
                        _ => panic!("generic vtable method"),
                    }
                }
                if self.tcx.instantiate_and_check_impossible_clauses(&self.prog, method, &args) {
                    words.push(0); wsyms.push(None); continue;
                }
                let args = Instance::new_raw(method, args).compact_args(&self.tcx);
                let instance = Instance::expect_resolve_for_vtable(&self.tcx, &self.prog, method, &args);
                let id = self.instance_fn_id(instance);
                words.push(self.entry(id));
                wsyms.push(Some(self.syms.intern(crate::syms::SymKind::Thunk(id), self.entry(id))));
            }
            if emit_vptr {
                let args = Instance::new_raw(trait_ref.def_id, trait_ref.args).compact_args(&self.tcx);
                let super_dyn = self.tcx.tys.intern(TyKind::Dyn(trait_ref.def_id, self.tcx.tys.mk_list(&args[1..]), crate::types::List::EMPTY, crate::types::RegionArgsId::EMPTY, crate::types::RegionId::ERASED, 0, crate::types::List::EMPTY)
);
                let address = self.vtable(src, super_dyn)?;
                words.push(address); wsyms.push(Some(self.data_sym(address)));
            }
        }
        let mut bytes = Vec::with_capacity(words.len() * 8);
        for w in &words {
            bytes.extend_from_slice(&w.to_le_bytes());
        }
        let a = self.data(&bytes, 8, crate::syms::DataKind::Words(wsyms));
        self.syms.set_data_name(a, format!("vtable:{}:{}", crate::syms::stable_ty(self, src), crate::syms::stable_ty(self, dy)));
        // a racing builder of the same vtable: everyone uses the first (one symbol per vtable)
        let a = self.vtables.insert_first((src, dy), a);
        // real-time code calls through this trait-object type: the new vtable's methods are
        // compiled after the current request (realtime_drain)
        realtime_vtable(dy, a);
        Ok(a)
    }

    /// The reload index, built from the slots made so far on first use.
    fn reload_index(&self) -> relax_session::sync::MutexGuard<'_, Option<ReloadSlots>> {
        // fn_create first (fn_id's order): no slot is made while the index is built, and fn_id
        // indexes the slots it makes once the index exists
        let _create = self.fn_create.lock().unwrap_or_else(|e| e.into_inner());
        let mut g = self.reload_slots.lock().unwrap_or_else(|e| e.into_inner());
        if g.is_none() {
            let mut index = ReloadSlots::default();
            for id in 0..self.fns.len() {
                index.insert(&self.fns[id].key, id as u32);
            }
            *g = Some(index);
        }
        g
    }

    /// Round 23: the instance kind determines the permanent slot identity.
    pub fn instance_fn_id(&self, instance: Instance) -> u32 {
        let args = instance.compact_args(&self.tcx);
        match instance.def {
            InstanceKind::Item(d) if self.prog.def(d).kind == DefKind::Closure => {
                let (file, node) = self.prog.def_id_to_node_id[&d];
                let expr = match node {
                    crate::def_collector::NodeId::Closure(e) => e.0,
                    crate::def_collector::NodeId::AsyncBody(item) => {
                        let block = self.prog.fn_body(file, item).expect("coroutine body").expect("coroutine block");
                        crate::tcx::ASYNC_FN_BODY | block.0
                    }
                    _ => panic!("closure definition node"),
                };
                let mut owner = self.prog.def(d).parent;
                while self.prog.def(owner).kind == DefKind::Closure { owner = self.prog.def(owner).parent; }
                let parent_count = self.tcx.generic_indices_of(&self.prog, owner).types_consts.len();
                let parent = self.fn_id(FnKey::Inst(owner, args[..parent_count].to_vec()));
                self.fn_id(FnKey::Closure(file, expr, parent))
            }
            InstanceKind::Item(d) => self.fn_id(FnKey::Inst(d, args)),
            InstanceKind::Intrinsic(_) | InstanceKind::Virtual(..) => panic!("instance is lowered at its call site"),
            InstanceKind::Shim(ShimKind::DropGlue(_, _)) => self.fn_id(FnKey::Glue(GLUE_DROP, args[0])),
            InstanceKind::Shim(ShimKind::Clone(_, ty)) => self.fn_id(FnKey::Glue(GLUE_CLONE, ty)),
            InstanceKind::Shim(kind) => self.fn_id(FnKey::Shim(ShimInstance { kind, args })),
        }
    }

    /// Slot of a function instance (created with a lazy stub on first use).
    pub(crate) fn slots_of_defs(&self, defs: impl IntoIterator<Item = DefId>) -> Vec<u32> {
        let g = self.reload_index();
        let Some(index) = g.as_ref() else { return Vec::new() };
        let mut slots: Vec<_> = defs.into_iter().flat_map(|d| index.defs.get(&d).into_iter().flatten().copied()).collect();
        slots.sort_unstable();
        slots.dedup();
        slots
    }
    pub(crate) fn closures_of_slots(&self, parents: impl IntoIterator<Item = u32>) -> Vec<u32> {
        let g = self.reload_index();
        let Some(index) = g.as_ref() else { return Vec::new() };
        let mut slots: Vec<_> = parents.into_iter().flat_map(|p| index.parents.get(&p).into_iter().flatten().copied()).collect();
        slots.sort_unstable();
        slots.dedup();
        slots
    }
    pub(crate) fn closures_of_files(&self, files: &[u32]) -> Vec<u32> {
        let g = self.reload_index();
        let Some(index) = g.as_ref() else { return Vec::new() };
        files.iter().flat_map(|f| index.files.get(f).into_iter().flatten().copied()).collect()
    }

    /// A key as slots are keyed: all instance/glue identities (including O3 keys) use codegen types.
    fn slot_key_form(&self, mut key: FnKey) -> FnKey {
        match &mut key {
            FnKey::Shim(shim) => {
                for ty in &mut shim.args { *ty = self.tcx.tys.erase_regions(*ty); }
                match &mut shim.kind {
                    ShimKind::FnPtr(_, ty) | ShimKind::FnPtrAsPtr(_, ty) | ShimKind::FnPtrFromPtr(_, ty) => *ty = self.tcx.tys.erase_regions(*ty),
                    _ => {}
                }
            }
            FnKey::Inst(_, args) => args.iter_mut().for_each(|t| *t = self.tcx.tys.erase_regions(*t)),
            FnKey::Glue(_, t) => *t = self.tcx.tys.erase_regions(*t),
            FnKey::Closure(..) | FnKey::Sandbox(_) => {}
        }
        key
    }

    pub fn fn_id(&self, key: FnKey) -> u32 {
        let key = self.slot_key_form(key);
        if let Some(id) = lower_ahead_fn_id(self, &key) {
            return id;
        }
        if let Some(id) = self.fn_map.get(&key) {
            return id;
        }
        // a tuple constructor as a fn item (`let f = Some; f(1)`, reified ctors): the
        // constructor shim, keyed by its fn pointer type
        if let FnKey::Inst(d, args) = &key {
            let k = self.prog.def(*d).kind;
            if matches!(k, crate::program::DefKind::Struct | crate::program::DefKind::Variant) {
                if let Some(sig) = self.tcx.ctor_sigs.get(d) {
                    // (instantiated and normalized like any fn signature: field types declared
                    // through projections are their concrete types)
                    let ps: Vec<TyId> = sig.params.iter().map(|p| self.tcx.normalize_erasing_late_bound_regions(&self.prog, crate::tcx::TypingEnv::fully_monomorphized(&self.prog), self.tcx.tys.subst(*p, args))).collect();
                    let r = self.tcx.normalize_erasing_late_bound_regions(&self.prog, crate::tcx::TypingEnv::fully_monomorphized(&self.prog), self.tcx.tys.subst(sig.ret, args));
                    let fp = self.tcx.tys.intern(TyKind::FnPtr(self.tcx.tys.mk_list(&ps), r, sig.header, 0));
                    let vi = if k == crate::program::DefKind::Variant { self.prog.def(*d).sub } else { 0 };
                    let id = self.fn_id(FnKey::Glue(GLUE_CTOR + vi as u8, fp));
                    self.fn_map.insert(crate::zone_alloc::kept_clone(&key), id);
                    return id;
                }
            }
        }
        // a slot of the startup image whose identity is still encoded: found by the key's
        // encoding (and named now)
        if ids_pending_here() {
            if let Some(id) = unsafe { crate::cache::image_slot_by_key_unit(self.self_ptr.0, &key) } {
                return id;
            }
        } else if let Some(id) = self.fn_map.get(&key) {
            return id;
        }
        let _g = self.fn_create.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(id) = self.fn_map.get(&key) {
            return id;
        }
        let id = self.fns.len() as u32;
        self.ensure_slots(id, 1);
        // identity state: the slot's key and name (and the map's copy of the key) are the
        // program's, not the compiler arena's
        let (entry_key, map_key, name) = {
            let _k = crate::zone_alloc::kept();
            (key.clone(), key.clone(), fn_name(self, &key))
        };
        let file = self.key_file(&key);
        let stub_addr = self.slots.stub(id);
        let stub = x64::stub(id, self.lazy_entry);
        let slot_addr = self.slots.table(id);
        unsafe {
            // (no-codegen: the precompiled pools already hold slot `id`'s thunk and stub)
            if !self.prog.session.options.no_codegen {
                let thunk = x64::thunk(slot_addr, THUNK_SIZE);
                os::mem::write_code(stub_addr, &stub);
                os::mem::write_code(self.slots.thunk(id), &thunk);
            }
            *(slot_addr as *mut u64) = stub_addr;
        }
        self.slot_named(id, &key);
        self.fns.push(Self::new_entry(entry_key, name, file, stub_addr, stub.len() as u32));
        if let Some(index) = self.reload_slots.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            index.insert(&key, id);
        }
        self.fn_map.insert(map_key, id);
        id
    }

    fn exec_context(&self, demand: bool) -> crate::exec_census::Context {
        crate::exec_census::Context::new(self.front.census_id, demand)
    }

    fn exec_event(&self, id: u32, event: &str) {
        crate::exec_census::note(self.front.census_id, id, event);
    }

    /// Body ownership follows FnEntry::file (including closures), never the
    /// call site's file. Synthesized glue has no authored body.
    fn exec_note(&self, id: u32, bit: u32) {
        if !self.prog.session.runtime.profile { return; }
        let f = &self.fns[id as usize];
        match &f.key {
            FnKey::Inst(d, _) if matches!(self.prog.def(*d).kind,
                DefKind::Fn | DefKind::AssocFn | DefKind::Const | DefKind::AssocConst | DefKind::Static) => {}
            FnKey::Closure(..) => {}
            // Foreign declarations and synthesized constructors/glue have no
            // authored executable body in this file.
            _ => return,
        }
        if let Some(file) = self.prog.exec_profile_file(f.file) { file.note(bit); }
    }

    /// The source file of a slot keyed `key` (0: none).
    fn key_file(&self, key: &FnKey) -> u32 {
        match key {
            FnKey::Inst(d, _) => self.prog.def(*d).file,
            FnKey::Closure(f, _, _) => *f,
            FnKey::Shim(_) | FnKey::Glue(..) => 0,
            FnKey::Sandbox(b) if *b == u32::MAX => 0,
            FnKey::Sandbox(b) => self.fns[*b as usize].file,
        }
    }

    /// A new slot's entry: no code yet (its table word points at its lazy stub `stub_addr`).
    fn new_entry(key: FnKey, name: String, file: u32, stub_addr: u64, stub_len: u32) -> FnEntry {
        // Generated glue and sandbox entries are conservatively not local-linkage. A
        // definition's own entry/export markers are read only by single_ref (declared_entry):
        // most slots (the startup image's thousands) are never asked.
        let entry_ref = matches!(key, FnKey::Shim(_) | FnKey::Glue(..) | FnKey::Sandbox(_)) && !matches!(key, FnKey::Glue(GLUE_UNNAMED, _));
        let name = crate::zone_alloc::kept_with(|| name.into_boxed_str());
        FnEntry { key, name, code: stub_addr, code_len: stub_len, pc_map: PcMap::default(), file, history: None, compiled: false, patch_gen: 0, rir: None, refs: u32::from(entry_ref), rir_refs: Box::default(), entry_ref: AtomicBool::new(entry_ref), entry_ref_counted: entry_ref, building: false, build_err: None, build_ns: 0, cg_ns: 0, version: 0, inflight: false, spec: None, spec_state: 0, spec_depth: u8::MAX, baseline_hint: AtomicBool::new(false), hot_hint: AtomicBool::new(false), o3: AtomicBool::new(false), base_code: AtomicBool::new(false), calls: Box::default() }
    }

    /// Slot `id` has its key: the front's slot keys and its ahead check (before the slot is
    /// pushed or renamed: the ahead helpers read the key by id), the demand trace.
    fn slot_named(&self, id: u32, key: &FnKey) {
        if crate::exec_census::active() {
            let mut owner = key;
            while let FnKey::Closure(_, _, parent) | FnKey::Sandbox(parent) = owner {
                if *parent as usize >= self.fns.len() { break; }
                owner = &self.fns[*parent as usize].key;
            }
            let mut meta = if let FnKey::Inst(d, _) = owner {
                exec_metadata(&self.prog, *d, format!("{} [slot={id}]", fn_name(self, key)))
            } else {
                crate::exec_census::Metadata { name: fn_name(self, key), krate: "<generated>".into(), module: "<generated>".into(), initializer: "" }
            };
            if !matches!(key, FnKey::Inst(..)) { meta.initializer = ""; }
            let instance = match key { FnKey::Inst(d, a) => Some((d.0, a.iter().map(|t| t.0).collect())), _ => None };
            crate::exec_census::register(self.front.census_id, Some(id), instance, meta);
        }
        if crate::why::enabled() {
            if let FnKey::Inst(d, _) = key {
                let cause = self.prog.why_def(*d, "fn-demand");
                crate::why::event("fn-demand", &cause.file, cause.line, &cause.symbol, None);
            }
        }
        self.front.slot_keys.insert(id, key.clone());
        if self.front.helpers > 0 && startup_phase() {
            self.front.queue.lock().unwrap_or_else(|e| e.into_inner()).push_back(AheadWork::Slot(id));
            self.front.wake.notify_one();
        }
    }

    /// Makes slots `first..first + n` usable (the caller holds `fn_create`): reserves the
    /// segments that do not exist yet, and commits their entries' memory where the OS commits on
    /// demand (Windows). The ids are u32; the only limit below that is address space.
    fn ensure_slots(&self, first: u32, n: usize) {
        let last = first as u64 + n as u64 - 1;
        if last > u32::MAX as u64 {
            panic!("more than 4 billion function instances");
        }
        let (last_seg, _) = crate::rir::SlotTable::locate(last as u32);
        // no-codegen: segment 0 is precompiled and static (installed by Unit::new)
        if self.prog.session.options.no_codegen {
            if last_seg > 0 {
                panic!("makepad run: no-codegen mode: more than {} function slots (the precompiled thunk pool)", crate::rir::SlotTable::seg_slots(0));
            }
            return;
        }
        for seg in 0..=last_seg {
            if self.slots.has_seg(seg) {
                continue;
            }
            let count = crate::rir::SlotTable::seg_slots(seg);
            let code = count * (THUNK_SIZE + STUB_SIZE);
            let base = os::mem::reserve(code, count * (8 + TIER_REC));
            let thunk = base;
            let stub = thunk + (count * THUNK_SIZE) as u64;
            let table = base + code as u64;
            let tier = table + (count * 8) as u64;
            self.slots.install(seg, thunk, stub, table, tier);
        }
        let mut id = first as u64;
        while id <= last {
            let (seg, ix) = crate::rir::SlotTable::locate(id as u32);
            let run = (crate::rir::SlotTable::seg_slots(seg) - ix).min((last - id + 1) as usize) as u64;
            let id32 = id as u32;
            os::mem::commit(self.slots.thunk(id32), (run * THUNK_SIZE as u64) as usize);
            os::mem::commit(self.slots.stub(id32), (run * STUB_SIZE as u64) as usize);
            os::mem::commit(self.slots.table(id32), (run * 8) as usize);
            os::mem::commit(self.slots.tier(id32), (run * TIER_REC as u64) as usize);
            id += run;
        }
    }

    /// `n` new slots in one go, ids `first..first + n` (the startup image's, cache/image.rs):
    /// lazy stubs, no identity yet (GLUE_UNNAMED until `name_slot`). Returns `first`.
    pub(crate) fn new_slots(&self, n: usize) -> u32 {
        let _g = self.fn_create.lock().unwrap_or_else(|e| e.into_inner());
        let first = self.fns.len() as u32;
        if n == 0 {
            return first;
        }
        self.ensure_slots(first, n);
        let unnamed = FnKey::Glue(GLUE_UNNAMED, TyId(0));
        let stub_len = x64::stub(first, self.lazy_entry).len();
        if !self.prog.session.options.no_codegen { unsafe {
            // a run per segment: its slots are contiguous
            let mut id = first as usize;
            while id < first as usize + n {
                let (seg, ix) = crate::rir::SlotTable::locate(id as u32);
                let run = (crate::rir::SlotTable::seg_slots(seg) - ix).min(first as usize + n - id);
                let id0 = id as u32;
                os::mem::fill_code(self.slots.stub(id0), run * STUB_SIZE, |buf| {
                    for i in 0..run {
                        let b = x64::stub(id0 + i as u32, self.lazy_entry);
                        buf[i * STUB_SIZE..i * STUB_SIZE + b.len()].copy_from_slice(&b);
                    }
                });
                os::mem::fill_code(self.slots.thunk(id0), run * THUNK_SIZE, |buf| {
                    for i in 0..run {
                        let b = x64::thunk(self.slots.table(id0 + i as u32), THUNK_SIZE);
                        buf[i * THUNK_SIZE..(i + 1) * THUNK_SIZE].copy_from_slice(&b);
                    }
                });
                id += run;
            }
        } }
        // (the entries' memory mapped in one go: thousands of them)
        self.fns.reserve(first as usize + n, |p, len| os::mem::prefault(p as u64, len));
        for i in 0..n as u32 {
            let id = first + i;
            let stub_addr = self.slots.stub(id);
            unsafe { *(self.slots.table(id) as *mut u64) = stub_addr };
            self.fns.push(Self::new_entry(unnamed.clone(), String::new(), 0, stub_addr, stub_len as u32));
        }
        first
    }

    /// Names slot `id` (made by `new_slots`) `key` (on the owner thread): its name, file, the map.
    /// False when another slot has that key already: this one keeps it as a second slot of the
    /// function (its code and callers stay; the reload index finds both), the map the first.
    pub(crate) fn name_slot(&mut self, id: u32, key: FnKey) -> bool {
        let key = self.slot_key_form(key);
        let name = crate::zone_alloc::kept_with(|| fn_name(self, &key).into_boxed_str());
        let file = self.key_file(&key);
        self.slot_named(id, &key);
        let f = &mut self.fns[id as usize];
        f.key = crate::zone_alloc::kept_clone(&key);
        f.name = name;
        f.file = file;
        if let Some(index) = self.reload_slots.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            index.insert(&key, id);
        }
        let _g = self.fn_create.lock().unwrap_or_else(|e| e.into_inner());
        if self.fn_map.get(&key).is_some() {
            return false;
        }
        self.fn_map.insert(crate::zone_alloc::kept_clone(&key), id);
        true
    }

    /// Permanent callable address of a slot (survives patching).
    pub fn entry(&self, id: u32) -> u64 {
        // Includes vtables, runtime callbacks, exports, main/tests and cached thunk
        // symbols. Conservatively count even an address requested only for inspection.
        if id as usize >= self.fns.len() || !self.fns[id as usize].entry_ref.load(Ordering::Relaxed) {
            lower_ahead_needs_unit();
        }
        self.fns[id as usize].entry_ref.store(true, Ordering::Relaxed);
        self.slots.callable(id)
    }

    /// The slot's own thunk, never a stub: what a host that calls a function itself (not through
    /// a C library) is given, e.g. the JNI natives (run --exports).
    pub fn entry_direct(&self, id: u32) -> u64 {
        self.entry(id);
        self.slots.thunk(id)
    }

    fn sync_entry_ref(&mut self, id: u32) {
        let f = &mut self.fns[id as usize];
        if !f.entry_ref_counted && f.entry_ref.load(Ordering::Relaxed) {
            f.refs = f.refs.saturating_add(1);
            f.entry_ref_counted = true;
        }
    }

    /// All O1 RIR installations, including sandbox twins, pass through here.
    fn set_rir(&mut self, id: u32, func: crate::rir::Func) {
        if func.cabi.is_some() {
            self.fns[id as usize].entry_ref.store(true, Ordering::Relaxed);
        }
        let mut fn_addrs: HashMap<crate::rir::VReg, Vec<u32>> = HashMap::new();
        for i in func.blocks.iter().flat_map(|b| &b.insts) {
            if let crate::rir::Inst::FnAddr(v, c) = i { fn_addrs.entry(*v).or_default().push(*c); }
        }
        if !fn_addrs.is_empty() {
            let escape = |v| {
                if let Some(cs) = fn_addrs.get(&v) {
                    for c in cs { self.fns[*c as usize].entry_ref.store(true, Ordering::Relaxed); }
                }
            };
            let mut term_uses = Vec::new();
            for b in &func.blocks {
                for i in &b.insts {
                    // A local indirect call only uses the existing FnAddr site.
                    // Passing/storing/returning its value escapes; untracked aliases
                    // (including moves) conservatively escape too.
                    let local_call = match i {
                        crate::rir::Inst::Call(crate::rir::Callee::Indirect(v), args, _) if !args.contains(v) => Some(*v),
                        _ => None,
                    };
                    relax_codegen::regalloc::for_uses(i, |v| {
                        if Some(v) != local_call { escape(v); }
                    });
                }
                term_uses.clear();
                relax_codegen::regalloc::term_uses(&b.term, &mut term_uses);
                for v in &term_uses { escape(*v); }
            }
        }
        for i in func.blocks.iter().flat_map(|b| &b.insts) {
            let escaped = match i {
                crate::rir::Inst::Addr(_, s) => match self.syms.kind(s.id) {
                    Some(crate::syms::SymKind::Thunk(c)) => Some(c),
                    _ => None,
                },
                // C ABI references cannot be represented by the direct-Rust-call
                // oracle; conservatively give their slots an extra reference.
                crate::rir::Inst::Call(crate::rir::Callee::CFn(c, _), _, _) => Some(*c),
                _ => None,
            };
            if let Some(c) = escaped {
                self.fns[c as usize].entry_ref.store(true, Ordering::Relaxed);
                self.sync_entry_ref(c);
            }
        }
        let refs = func.blocks.iter().flat_map(|b| &b.insts).filter_map(|i| match i {
            crate::rir::Inst::Call(crate::rir::Callee::Fn(c), _, _) | crate::rir::Inst::FnAddr(_, c) => Some(*c),
            _ => None,
        }).collect::<Vec<_>>().into_boxed_slice();
        let old = std::mem::replace(&mut self.fns[id as usize].rir_refs, refs);
        for c in old {
            let n = &mut self.fns[c as usize].refs;
            // Once saturated, stay conservative even after a rebuild.
            if *n != u32::MAX { *n -= 1; }
        }
        for i in 0..self.fns[id as usize].rir_refs.len() {
            let c = self.fns[id as usize].rir_refs[i];
            self.sync_entry_ref(c);
            self.fns[c as usize].refs = self.fns[c as usize].refs.saturating_add(1);
        }
        self.sync_entry_ref(id);
        if self.fns[id as usize].rir.is_none() {
            if let Some(changed) = &mut self.reload_rir { changed.insert(id); }
        }
        self.fns[id as usize].rir = Some(std::sync::Arc::new(func));
        if let FnKey::Closure(_, _, parent) = self.fns[id as usize].key {
            drop_completed_body(self, parent);
        }
    }

    fn single_ref(&self, id: u32) -> bool {
        let f = &self.fns[id as usize];
        // Check the atomic too: a lowering thread may have just published an escape.
        f.refs == 1 && !f.entry_ref.load(Ordering::Relaxed) && !self.declared_entry(id)
    }

    /// Whether slot `id`'s definition is an entry by declaration (main, a foreign fn, a non-Rust
    /// ABI, an entry/export attribute): its address can be taken before any reference to it.
    fn declared_entry(&self, id: u32) -> bool {
        let FnKey::Inst(d, _) = &self.fns[id as usize].key else { return false };
        let def = self.prog.def(*d);
        let f = &self.prog.files[def.file as usize];
        self.prog.name(*d) == "main" || def.kind == DefKind::ForeignFn
            || self.tcx.fn_abi(&self.prog, *d).map_or(false, |abi| abi != crate::types::Abi::Rust)
            || f.ast.list(f.ast.item(def.item).attrs).iter().any(|a| {
                (a.toks.lo..a.toks.hi).any(|t| matches!(f.tok_text(t), "no_mangle" | "export_name" | "test" | "bench" | "start"))
            })
    }

    fn recursive_fn(&self, id: u32) -> bool {
        let mut seen = std::collections::HashSet::new();
        let mut work = self.fns[id as usize].calls.to_vec();
        while let Some(c) = work.pop() {
            if c == id { return true; }
            if seen.insert(c) { work.extend_from_slice(&self.fns[c as usize].calls); }
        }
        false
    }

    /// The slot whose code (current, earlier or retired) holds `pc` (no unit call needed).
    pub fn find_fn(&self, pc: u64) -> Option<u32> {
        self.code.read().unwrap_or_else(|e| e.into_inner()).slot_at(pc)
    }

    /// Foreign fns declared in Relax core's `intrinsics*` modules become instructions.
    pub fn is_intrinsic(&self, d: DefId) -> bool {
        let def = self.prog.def(d);
        def.krate == self.core_crate && self.prog.name(def.parent).starts_with("intrinsics")
    }

    /// Loads every library the program's extern blocks name with `#[link(name = ..)]`
    /// (frameworks on macOS), once: what the system linker would have linked.
    fn load_link_libs(&self) {
        // files load lazily (a platform file with its #[link] blocks may be parsed after an
        // earlier miss), so each call reads the files that came since the last
        static SCANNED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let from = SCANNED.swap(self.prog.files.len(), Ordering::SeqCst);
        for (fi, f) in self.prog.files.iter().enumerate().skip(from) {
            for it in &f.ast.items {
                // active blocks (cfg) are the ones whose items got definitions
                let active = match &it.kind {
                    // an empty block (link-only) counts unless it has a cfg we cannot see here
                    crate::ast::ItemKind::ForeignMod(r) if f.ast.row(*r).items.is_empty() => !f.ast.list(it.attrs).iter().any(|a| a.toks.hi > a.toks.lo && f.tok_text(a.toks.lo).starts_with("cfg")),
                    crate::ast::ItemKind::ForeignMod(r) => f.ast.list(f.ast.row(*r).items).iter().any(|si| self.prog.item_defs.contains_key(&(fi as u32, si.0))),
                    _ => false,
                };
                if !active {
                    continue;
                }
                for a in f.ast.list(it.attrs) {
                    if a.toks.hi <= a.toks.lo || f.tok_text(a.toks.lo) != "link" {
                        continue;
                    }
                    let mut name = String::new();
                    let mut framework = false;
                    let mut k = a.toks.lo;
                    while k + 2 < a.toks.hi {
                        let key = f.tok_text(k);
                        if f.tok_text(k + 1) == "=" {
                            let v = f.tok_text(k + 2).trim_matches('"').to_string();
                            if key == "name" {
                                name = v;
                            } else if key == "kind" && v == "framework" {
                                framework = true;
                            }
                        }
                        k += 1;
                    }
                    if name.is_empty() {
                        continue;
                    }
                    os::proc::load_library(&name, framework);
                }
            }
        }
    }

    /// Symbol name of a foreign item: `#[link_name = ".."]` or its own name.
    pub(crate) fn link_name(&self, d: DefId) -> String {
        let def = self.prog.def(d);
        let f = &self.prog.files[def.file as usize];
        for a in f.ast.list(f.ast.item(def.item).attrs) {
            if a.toks.hi >= a.toks.lo + 3 && f.tok_text(a.toks.lo) == "link_name" && f.tok_text(a.toks.lo + 1) == "=" {
                return f.tok_text(a.toks.lo + 2).trim_matches('"').to_string();
            }
        }
        self.prog.name(d).to_string()
    }

    /// A foreign address already available in this process. Reverse pointer attribution must
    /// not load new libraries or manufacture missing-function stubs just to search for a name.
    pub(crate) fn loaded_foreign_addr(&self, d: DefId) -> Option<u64> {
        let link = self.link_name(d);
        if let Some(a) = lookup_interposer(&link) {
            return Some(a);
        }
        if let Some(a) = loader_symbol(link.as_bytes()) {
            return Some(a as u64);
        }
        if let Some(s) = self.syms.find_foreign(&link) {
            return Some(self.syms.value(s));
        }
        let name = format!("{}\0", link);
        let p = unsafe { os::proc::find(name.as_ptr()) };
        if p.is_null() { None } else { Some(p as u64) }
    }

    /// Address of a foreign fn or static (dlsym; loads the program's #[link] libraries
    /// on a miss).
    pub fn foreign_addr(&self, d: DefId) -> Option<u64> {
        let link = self.link_name(d);
        // dlsym / GetProcAddress answer the symbols the loader provides to programs (loader_symbol) first
        if let Some(a) = lookup_interposer(&link) {
            return Some(a);
        }
        if let Some(a) = loader_symbol(link.as_bytes()) {
            return Some(a as u64);
        }
        let name = format!("{}\0", link);
        let mut p = unsafe { os::proc::find(name.as_ptr()) };
        // no-codegen mode: only what the host exports (no libraries loaded at run time); a
        // symbol it lacks is reported with its crate, and calling it panics
        if self.prog.session.options.no_codegen {
            if !p.is_null() {
                return Some(p as u64);
            }
            if self.prog.def(d).kind == DefKind::ForeignFn {
                static SAID: relax_session::sync::Mutex<Vec<String>> = relax_session::sync::Mutex::new(Vec::new());
                let mut said = SAID.lock().unwrap_or_else(|e| e.into_inner());
                if !said.contains(&link) {
                    let krate = self.prog.crates.get(self.prog.def(d).krate as usize).map(|c| self.prog.syms.str(c.name).to_string()).unwrap_or_default();
                    eprintln!("makepad run: no-codegen mode: crate `{}` uses extern `{}`, which the host binary does not export (calls to it panic)", krate, link);
                    crate::zone_alloc::kept_with(|| said.push(link.clone()));
                }
                return Some(self.missing_foreign_stub(d));
            }
            return None;
        }
        if p.is_null() {
            // Library initializers and their once-only load stay on the ordinary path.
            lower_ahead_needs_unit();
            self.load_link_libs();
            p = unsafe { os::proc::find(name.as_ptr()) };
        }
        if p.is_null() {
            p = unsafe { os::proc::find_system(name.as_ptr()) };
        }
        if !p.is_null() {
            return Some(p as u64);
        }
        if self.prog.def(d).kind == DefKind::ForeignFn {
            // a fn this host lacks (another platform's API behind a cfg the graph keeps):
            // calls compile, and panic with a one-line report only when one runs
            return Some(self.missing_foreign_stub(d));
        }
        None
    }

    /// Code that panics with "foreign function `x` is not available on this host" (one per
    /// symbol): loads its site into the first argument and tail-jumps to the panic
    /// trampoline, so the report names the JIT caller.
    fn missing_foreign_stub(&self, d: DefId) -> u64 {
        let name = self.link_name(d);
        let mut m = self.missing_stubs.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(&a) = m.get(&name) {
            return a;
        }
        let def = self.prog.def(d);
        let (file, pos) = (def.file, self.prog.files[def.file as usize].ast.item(def.item).lo);
        let site = self.site(file, pos, &format!("foreign function `{}` is not available on this host -- cfg the call out or link its library", name));
        // no-codegen: an interpreted stub makes the same call (the precompiled glue has no
        // per-site code)
        let a = if self.prog.session.options.no_codegen {
            relax_codegen::interp::const_call_entry(self.rt.panic_site, site)
        } else {
            let code = x64::jump_with_arg0(site, self.rt.panic_site);
            self.emit_code(&code)
        };
        crate::zone_alloc::kept_with(|| m.insert(name.clone(), a));
        a
    }

    /// const_addr from shared lowering: constant evaluation runs code and changes the unit, so
    /// it is a unit call.
    pub fn const_addr_shared(&self, d: DefId, args: Vec<TyId>) -> Option<u64> {
        owner::call(owner::Class::Job, || unsafe { (&mut *self.self_ptr.0).const_addr(d, args) })
    }

    /// static_addr from shared lowering (a unit call, as const_addr_shared).
    pub fn static_addr_shared(&self, d: DefId) -> u64 {
        owner::call(owner::Class::Job, || unsafe { (&mut *self.self_ptr.0).static_addr(d) })
    }

    pub fn const_addr(&mut self, d: DefId, mut args: Vec<TyId>) -> Option<u64> {
        args.iter_mut().for_each(|t| *t = self.tcx.tys.erase_regions(*t));
        let reader = BUILD_STACK.try_with(|s| s.borrow().last().copied()).ok().flatten();
        if let Some(r) = reader {
            note_const_reader(self, d, r);
        }
        // a trait's associated const: the impl's (or the trait's default) for these args
        let (d, mut args) = if self.prog.def(d).kind == DefKind::AssocConst && self.prog.def(self.prog.def(d).parent).kind == DefKind::Trait {
            {
                let instance = crate::instance::Instance::expect_resolve(&self.tcx, &self.prog, crate::tcx::TypingEnv::fully_monomorphized(&self.prog), d, &args);
                let InstanceKind::Item(def) = instance.def else { panic!("non-item associated const") };
                (def, instance.compact_args(&self.tcx))
            }
        } else {
            (d, args)
        };
        if let Some(r) = reader {
            note_const_reader(self, d, r);
        }
        args.iter_mut().for_each(|t| *t = self.tcx.tys.erase_regions(*t));
        if let Some(&a) = self.consts.get(&(d, args.clone())) {
            return Some(a);
        }
        // a constant that failed to evaluate is not re-run by every use (u64::MAX marks it)
        if self.const_failed.contains(&(d, args.clone())) {
            return None;
        }
        if let Some(a) = unsafe { crate::cache::const_value(self.self_ptr.0, d, &args) } {
            // the cached image is the shared read-only one: per domain where it reaches a static
            let a = crate::domain::rehome_const(self, a);
            self.put_const(d, args, a);
            return Some(a);
        }
        match self.eval_init(d, args.clone(), false) {
            Some(a) => {
                if let Some(mut cache) = self.cache.take() {
                    cache.record_const(self, d, &args, a);
                    self.cache = Some(cache);
                }
                // the cache keeps the shared read-only image; the unit uses the per-domain one
                // where the value reaches a static (domain.rs rehome_const)
                let a = crate::domain::rehome_const(self, a);
                self.put_const(d, args, a);
                Some(a)
            }
            None => {
                // a module the post-boot loader has still to load: evaluated again later
                if !DEFERRED_ABORT.with(|a| a.get()) { self.const_failed.insert((d, args)); }
                None
            }
        }
    }

    /// CTFE may use a reserved address for a cyclic static, but must propagate a
    /// deferred initializer. Otherwise a completed parent can hide the unfinished
    /// static from the retry after its missing module loads.
    pub(crate) fn ctfe_static_addr(&mut self, d: DefId) -> Option<u64> {
        let a = self.static_addr(d);
        (a != 0 && !DEFERRED_ABORT.with(|f| f.get())).then_some(a)
    }

    /// Memoises a constant's value; the key's types are built in the kept heap (the memo is
    /// identity state: it must outlive the compiler arena).
    pub(crate) fn put_const(&mut self, d: DefId, args: Vec<TyId>, a: u64) {
        let _k = crate::zone_alloc::kept();
        self.consts.insert((d, args.as_slice().to_vec()), a);
    }

    pub fn static_addr(&mut self, d: DefId) -> u64 {
        if let Some(&a) = self.statics.get(&d) {
            // its initializer needed a module the post-boot loader has loaded since: run it
            // again into the same storage (its symbol already names that address)
            if DEFERRED_STATICS.lock().unwrap_or_else(|e| e.into_inner()).remove(&d.0) {
                if self.run_init(d, Vec::new(), a).is_none() {
                    if DEFERRED_ABORT.with(|f| f.get()) { crate::zone_alloc::kept_with(|| DEFERRED_STATICS.lock().unwrap_or_else(|e| e.into_inner()).insert(d.0)); }
                    return a;
                }
                let sym = self.syms.intern(crate::syms::SymKind::Static(d), a);
                crate::sandbox::capture_initial_data(self, sym);
                crate::domain::materialize(self, sym);
            }
            return a;
        }
        // A static has its address before its value (Reference items/static-items: statics may
        // refer to each other's addresses, also cyclically): record the address first, then run
        // the initializer, so `static A: &T = &B; static B: &T = &A;` and `static mut S = &S as ..`
        // see the final addresses instead of recursing.
        let t = match self.tcx.const_ty(&self.prog, d) {
            Some(t) => t,
            None => {
                crate::zone_alloc::kept_with(|| self.statics.insert(d, 0));
                return 0;
            }
        };
        let l = self.layout_of(t);
        let buf = self.rw_data(l.size.max(1) as u64, l.align.max(8) as u64);
        crate::zone_alloc::kept_with(|| {
            self.statics.insert(d, buf);
            self.static_spans.insert(buf, (d, l.size as u64));
        });
        // Restore the initial value into this run's writable storage. The cached
        // object is an immutable snapshot taken immediately after pure CTFE,
        // never the static's later (possibly mutated) contents.
        let cached = unsafe { crate::cache::const_value(self.self_ptr.0, d, &[]) };
        if let Some(initial) = cached {
            unsafe { std::ptr::copy_nonoverlapping(initial as *const u8, buf as *mut u8, l.size as usize); }
        } else if self.run_init(d, Vec::new(), buf).is_none() {
            // needs a skipped module: evaluated again (same storage) once the outer compile
            // loaded it; the build using it fails meanwhile (DEFERRED_ABORT)
            if DEFERRED_ABORT.with(|a| a.get()) {
                crate::zone_alloc::kept_with(|| DEFERRED_STATICS.lock().unwrap_or_else(|e| e.into_inner()).insert(d.0));
                return buf;
            }
            crate::zone_alloc::kept_with(|| self.statics.insert(d, 0));
            return 0;
        } else {
            self.record_static(d, t, buf);
        }
        let sym = self.syms.intern(crate::syms::SymKind::Static(d), buf);
        crate::sandbox::capture_initial_data(self, sym);
        // every other live domain gets its own copy of the new static (none: nothing to do)
        crate::domain::materialize(self, sym);
        buf
    }

    fn eval_init(&mut self, d: DefId, args: Vec<TyId>, mutable: bool) -> Option<u64> {
        let t = self.tcx.type_of(&self.prog, d).ok()?;
        let t = self.tcx.tys.subst_erasing_regions(t, &args);
        let l = self.layout_of(t);
        let buf = if mutable {
            self.rw_data(l.size.max(1) as u64, l.align.max(8) as u64)
        } else {
            let b = self.data(&vec![0u8; l.size.max(1) as usize], l.align.max(8) as u64, crate::syms::DataKind::Typed(t));
            self.syms.set_data_name(b, const_name(self, d, &args));
            b
        };
        let r = self.run_init(d, args, buf);
        // a constant's undefined bytes are 0 whichever way its initializer ran
        if r.is_some() && !mutable {
            crate::ctfe::zero_undefined(self, t, buf);
            // Initializer compilation may have inspected the object while it was incomplete.
            self.syms.set_data_kind(buf, crate::syms::DataKind::Typed(t));
            crate::sandbox::capture_initial_data(self, self.data_sym(buf));
        }
        r
    }

    /// ctfe::eval_into. A constant it reads that waits for the post-boot loader (NeedsLoad,
    /// const_eval_to_valtree) means no CTFE value: the initializer's code is built instead,
    /// and that build defers (DEFERRED_RETRY) with the caller's storage kept (static_addr).
    fn ctfe_eval_into(&mut self, d: DefId, args: &[TyId], buf: u64) -> bool {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| crate::ctfe::eval_into(self, d, args, buf))) {
            Ok(ok) => ok,
            Err(p) if p.is::<crate::preempt::NeedsLoad>() => false,
            Err(p) => std::panic::resume_unwind(p),
        }
    }

    /// Runs the initializer of const/static `d` into `buf`.
    fn run_init(&mut self, d: DefId, args: Vec<TyId>, buf: u64) -> Option<u64> {
        let _census_context = self.exec_context(false);
        let options = &self.prog.session.clone().options;
        // Initializers outside the CTFE subset already have cached executable
        // code. Trying CTFE first would type-check them on every warm launch.
        // Verification mode still evaluates both implementations as before.
        if !crate::ctfe::check_mode(options)
            && self.cache.as_ref().is_some_and(|c| c.mode != crate::cache::Mode::Verify)
            && crate::reload::edited_crates().is_empty()
        {
            let id = self.fn_id(FnKey::Inst(d, args.clone()));
            let mut cache = self.cache.take();
            let cached = cache.as_mut().is_some_and(|c| c.has(self, id));
            self.cache = cache;
            if cached {
                return self.run_init_code(d, args, buf);
            }
        }
        // tables and other plain-data initializers: evaluated to bytes, no code (ctfe.rs)
        let t0 = cpu_ns();
        if crate::ctfe::check_mode(options) {
            let t = self.tcx.const_ty(&self.prog, d).map(|t| self.tcx.tys.subst_erasing_regions(t, &args));
            if let Some(t) = t {
                let l = self.layout_of(t);
                let tmp = self.rw_data(l.size.max(1) as u64, l.align.max(8) as u64);
                let ok = self.ctfe_eval_into(d, &args, tmp);
                self.stats.ctfe_ns += cpu_ns() - t0;
                if ok {
                    self.stats.ctfe_data += 1;
                    let id = self.fn_id(FnKey::Inst(d, args.clone()));
                    let entry = self.entry(id);
                    self.exec_note(id, crate::exec_profile::CALL);
                    self.exec_event(id, "call");
                    match unsafe { call_jit(self as *mut Unit, entry, buf) } {
                        Ok(_) if crate::ctfe::same_value(self, t, tmp, buf, 0) => return Some(buf),
                        Ok(_) => self.note_error(format!("CTFE MISMATCH in {}", self.prog.def_path(d))),
                        // Use the normal deferred-initialization retry and error path.
                        Err(_) => return self.run_init_code(d, args, buf),
                    }
                    return None;
                }
                // A failed check-mode evaluation already checked this initializer. Do not
                // repeat the same evaluation before falling back to its executable body.
                return self.run_init_code(d, args, buf);
            }
        }
        let ok = self.ctfe_eval_into(d, &args, buf);
        self.stats.ctfe_ns += cpu_ns() - t0;
        if ok {
            self.stats.ctfe_data += 1;
            return Some(buf);
        }
        self.run_init_code(d, args, buf)
    }

    /// Records static `d`'s initial value (of type `t`, just evaluated into `buf`, by CTFE or by
    /// running its initializer's code) as the value entry the next run copies into its storage
    /// instead of running the initializer: an immutable snapshot taken before any other code
    /// sees the static, keyed by the static and the fingerprints of the crates it is made of.
    fn record_static(&mut self, d: DefId, t: TyId, buf: u64) {
        if !self.cache.as_ref().is_some_and(|c| c.mode != crate::cache::Mode::ReadOnly) {
            return;
        }
        // Register the destination first, so self/cross-static references
        // serialize as symbols, not addresses of the snapshot copy.
        self.syms.intern(crate::syms::SymKind::Static(d), buf);
        let l = self.layout_of(t);
        let bytes = unsafe { std::slice::from_raw_parts(buf as *const u8, l.size as usize) }.to_vec();
        let initial = self.data(&bytes, l.align.max(8) as u64, crate::syms::DataKind::Typed(t));
        crate::ctfe::zero_undefined(self, t, initial);
        if let Some(mut cache) = self.cache.take() {
            cache.record_const(self, d, &[], initial);
            self.cache = Some(cache);
        }
    }

    fn run_init_code(&mut self, d: DefId, args: Vec<TyId>, buf: u64) -> Option<u64> {
        self.stats.ctfe_code += 1;
        if std::env::var_os("MAKEPAD_RUN_CTFE_TRACE").is_some() {
            eprintln!("ctfe: compiled {}", self.prog.def_path(d));
        }
        let id = self.fn_id(FnKey::Inst(d, args));
        let mut r;
        loop {
            let entry = self.entry(id);
            self.exec_note(id, crate::exec_profile::CALL);
            self.exec_event(id, "call");
            compile_watch(Some((&format!("running initializer {}", self.prog.def_path(d)), 0)));
            r = unsafe { call_jit(self as *mut Unit, entry, buf) };
            compile_watch(None);
            // a compile the code reached was cancelled (a nested request blocked by an earlier
            // one): no value and no error -- the request is cancelled here, in Rust frames
            if BUSY_PENDING.with(|b| b.replace(false)) {
                std::panic::resume_unwind(Box::new(crate::preempt::OwnerBusy));
            }
            // Evaluated outside any build (statics initialized before main, the prelink): no
            // outer compile above to retry it, so the post-boot loader loads what its build
            // missed right here, and it runs again.
            let deferred = matches!(&r, Err(p) if p.message.contains(DEFERRED_RETRY)) && BUILD_DEPTH.with(|d| d.get()) == 0;
            if !(deferred && unsafe { load_deferred(self as *mut Unit, id) }) { break; }
        }
        match r {
            Ok(_) => Some(buf),
            // in a build, or while linking cached code (the link fails, the slot is built, and
            // that build defers): not an error of the program
            Err(p) if p.message.contains(DEFERRED_RETRY) && (BUILD_DEPTH.with(|d| d.get()) > 0 || crate::cache::linking(self)) => {
                DEFERRED_ABORT.with(|a| a.set(true));
                None
            }
            // the initializer did not compile (its type check failed): that error is reported
            // already, and rustc reports no evaluation error for it
            Err(p) if p.message.starts_with("compile error in ") => None,
            Err(p) => {
                let m = format!("{}: E0080: {} [in {}]", self.ctfe_error_location(d, id, &p), ctfe_error_message(&p.message), self.prog.def_path(d));
                // rustc rejects a program whose constant or static cannot be evaluated (E0080);
                // `makepad run` ends here instead of running on with a zeroed or null value
                // (`makepad test` / `live` keep it recoverable, as for compile errors)
                if COMPILE_ERRORS_FATAL.load(Ordering::Relaxed) {
                    eprintln!("makepad-run: {} -- the program cannot run past it (rustc rejects it)", m);
                    if !crate::domain::crash_current(&m) {
                        std::process::exit(101);
                    }
                }
                // queries.rs:58-108: evaluation errors return to the caller;
                // const_eval_resolve turns this into Reported.
                self.note_error(m);
                None
            }
        }
    }

    /// Where a failed initializer of const/static `d` is reported (rustc E0080's span: the
    /// failing expression in the initializer): the panic's site when it is in `d`'s file, else
    /// the const item itself (the failure was in a const fn or std it called).
    fn ctfe_error_location(&self, d: DefId, init: u32, p: &PanicInfo) -> String {
        // the initializer's own frame: where it called the failing const fn (rustc's span; the
        // const fn's frames are its notes)
        for &pc in &p.frames {
            for at in [pc, pc.wrapping_sub(1)] {
                if self.find_fn(at) == Some(init) {
                    if let Some((_, loc)) = self.pc_to_src(at) { if loc.matches(':').count() >= 2 { return loc; } }
                }
            }
        }
        let def = self.prog.def(d);
        let site = self.sites.lock().unwrap_or_else(|e| e.into_inner()).get(p.site as usize).map(|st| (st.file, st.pos));
        match site {
            Some((f, pos)) if f == def.file => self.pos_str(f, pos),
            _ if !p.location.is_empty() && self.prog.file_paths.get(def.file as usize).is_some_and(|f| p.location.starts_with(&format!("{}:", f))) => p.location.clone(),
            _ => self.pos_str(def.file, self.prog.files[def.file as usize].ast.item(def.item).lo),
        }
    }

    /// Source position -> "file:line:col".
    pub fn pos_str(&self, file: u32, pos: u32) -> String {
        match (self.prog.files.get(file as usize), self.prog.file_paths.get(file as usize)) {
            (Some(f), Some(p)) => {
                let (l, c) = f.line_col(pos);
                format!("{}:{}:{}", p, l, c)
            }
            _ => "source position unknown".to_string(),
        }
    }

    /// Lookup PC (an instruction's address, or a return address minus one) -> (fn slot,
    /// source position of the instruction containing it)
    pub fn pc_to_src(&self, pc: u64) -> Option<(u32, String)> {
        let id = self.find_fn(pc)?;
        let f = &self.fns[id as usize];
        let (base, map) = if pc >= f.code && pc < f.code + f.code_len as u64 {
            (f.code, &f.pc_map)
        } else {
            let mut found = (f.code, &f.pc_map);
            for (c, l, m, _) in &f.history().prev {
                if pc >= *c && pc < *c + *l as u64 {
                    found = (*c, m);
                }
            }
            for (c, l, m) in &f.history().retired {
                if pc >= *c && pc < *c + *l as u64 {
                    found = (*c, m);
                }
            }
            found
        };
        let off = (pc - base) as u32;
        // no pc-map entry: the fn's file without a line (byte 0 would claim line 1:1)
        let Some(pos) = map.lookup(off) else {
            return Some((id, self.prog.file_paths.get(f.file as usize).cloned().unwrap_or_else(|| "source position unknown".to_string())));
        };
        Some((id, self.pos_str((pos >> 32) as u32, pos as u32)))
    }

    /// Structured JSON crash report for a panic/fault/hang.
    pub fn report(&self, p: &PanicInfo) -> String {
        let mut s = String::from("{");
        s.push_str(&format!("\"kind\":{:?},\"message\":{:?},\"build\":{:?}", p.kind, p.message, crate::jit::PROVENANCE));
        let site = self.sites.lock().unwrap_or_else(|e| e.into_inner()).get(p.site as usize).map(|st| (st.file, st.pos));
        if let Some((file, pos)) = site {
            s.push_str(&format!(",\"location\":{:?}", self.pos_str(file, pos)));
        } else if !p.location.is_empty() {
            s.push_str(&format!(",\"location\":{:?}", p.location));
        }
        s.push_str(",\"backtrace\":[");
        for (i, f) in p.frames.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            match self.pc_to_src(*f) {
                Some((id, loc)) => {
                    // the frame size of the code the pc is in (O3 when it is the current code of a
                    // slot that tiered up): a stack overflow's report then shows where the stack went
                    let fe = &self.fns[id as usize];
                    let t1 = !fe.history().retired.is_empty() && *f >= fe.code && *f < fe.code + fe.code_len as u64;
                    let fr = FRAMES.lock().unwrap_or_else(|e| e.into_inner()).get(id as usize).map_or(0, |x| if t1 { x.1 } else { x.0 });
                    s.push_str(&format!("{{\"fn\":{:?},\"at\":{:?},\"off\":{},\"patch_gen\":{},\"tier\":{},\"frame\":{}}}", fe.name, loc, f.wrapping_sub(fe.code), fe.patch_gen, t1 as u8, fr))
                }
                None => s.push_str(&format!("{{\"pc\":\"{:#x}\"}}", f)),
            }
        }
        if let Some(n) = poisoned_note() {
            s.push_str(&format!("],\"poisoned\":{:?},\"recent_patches\":[", n));
        } else {
            s.push_str("],\"recent_patches\":[");
        }
        let n = self.patch_log.len();
        let from = n.saturating_sub(5);
        for (i, (slot, gen)) in self.patch_log[from..].iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            s.push_str(&format!("{{\"fn\":{:?},\"gen\":{}}}", self.fns[*slot as usize].name, gen));
        }
        s.push_str("]}");
        s
    }

    /// Starts a patch group (one edit); its installs roll back together.
    /// Records slot `id`'s checked body (closures are lowered from it). A body of another file
    /// version (hot reload) moves to old_bodies, as the closures of that version still need it.
    /// Whether slot `id`'s checked body for file version `file` is recorded (u32::MAX: any).
    pub fn has_body(&self, id: u32, file: u32) -> bool {
        self.bodies.get(&id).map_or(false, |b| b.file == file || file == u32::MAX) || self.old_bodies.contains_key(&(id, file))
    }

    /// Slot `id`'s checked body for file version `file` (u32::MAX: the current one).
    pub fn body_of(&self, id: u32, file: u32) -> Option<&std::sync::Arc<Body>> {
        match self.bodies.get(&id) {
            Some(b) if b.file == file || file == u32::MAX => Some(b),
            _ => self.old_bodies.get(&(id, file)),
        }
    }

    pub fn put_body(&mut self, id: u32, body: std::sync::Arc<Body>) {
        if let Some(old) = self.bodies.remove(&id) {
            if old.file != body.file {
                self.old_bodies.insert((id, old.file), old);
            }
        }
        self.bodies.insert(id, body);
    }

    pub fn begin_patch(&mut self) -> u32 {
        self.patch_gen += 1;
        self.patch_gen
    }

    /// Installs new code for a slot, keeping the previous version for rollback.
    /// `gen` > 0 marks the install as part of patch group `gen`.
    /// Links `code` (emitted with its relocated fields zeroed) where it lands, then installs it.
    pub fn install(&mut self, id: u32, code: &[u8], relocs: &[crate::rir::RelocAt], pc_map: Vec<(u32, u64)>, gen: u32) -> Result<u64, String> {
        // `makepad run --check-first`: a body's first code is installed only once its checker
        // accepts it (a live patch, gen > 0, passed reload's gate instead). Off by default:
        // Gandalf checks lazily in the background and the JIT never waits for it.
        if gen == 0 && CHECK_FIRST.load(Ordering::Relaxed) && !self.fns[id as usize].compiled {
            if let Some(e) = self.refused_by_check(id) {
                return Ok(unsafe { install_error(self, id, &e) });
            }
        }
        self.install_unchecked(id, code, relocs, pc_map, gen)
    }

    /// check-before-run: whether slot `id` (an app fn instance about to get its first code) may
    /// run. Its owner's borrow-check verdict comes from this run, the disk cache or a synchronous
    /// check now (borrowck::lazy::check_before_run): `makepad run` never executes a body its
    /// checker rejects, whatever the background checker reached. Some(error) refuses it: the
    /// errors are printed here and the slot gets the error stub (the call ends the run).
    fn refused_by_check(&mut self, id: u32) -> Option<String> {
        let options = &self.prog.session.clone().options;
        if !crate::borrowck::lazy::gandalf_enabled(options) { return None; }
        let d = match &self.fns[id as usize].key {
            FnKey::Inst(d, _) if crate::borrowck::event::admit(&self.prog, self.prog.def(*d).krate) => *d,
            _ => return None,
        };
        crate::borrowck::body::CheckedBody::item(&self.prog, d)?;
        let mut c = self.cache.take();
        let key = c.as_mut().and_then(|c| c.borrowck_key(self, id));
        let cached = match (&c, key) {
            (Some(c), Some(k)) => c.borrowck_stored(k),
            _ => None,
        };
        self.cache = c;
        let epoch = self.tcx.collect_epoch.load(Ordering::SeqCst);
        if let Some(c) = &self.cache {
            if !self.front.owner_verdicts.contains_key(&epoch) {
                self.front.owner_verdicts.insert(epoch, std::sync::Arc::new(c.owner_verdicts(epoch)));
            }
        }
        let src: std::sync::Arc<dyn crate::borrowck::lazy::FrontSource> = std::sync::Arc::new(BorrowckFront(self.front.clone()));
        let events = crate::borrowck::lazy::check_before_run(&self.prog, &self.tcx, &src, d, epoch, key, cached);
        let first = events.first()?;
        for ev in &events { eprintln!("{}", ev.error.render()); }
        Some(format!("{} (the borrow checker rejects this body, so makepad run does not execute it)", first.error.one_line()))
    }

    fn install_unchecked(&mut self, id: u32, code: &[u8], relocs: &[crate::rir::RelocAt], pc_map: Vec<(u32, u64)>, gen: u32) -> Result<u64, String> {
        let _cost_install = crate::fncost::phase(6);
        let options = &self.prog.session.clone().options;
        // an interpreted function (its relocatable form) or, in no-codegen mode, a jump (a
        // foreign function's veneer): the slot takes the entry itself, nothing is written
        let interp = relax_codegen::interp::blob::is_blob(code);
        // the unwind trailer is kept beside the code, not in it
        let (n, info) = if interp { (code.len(), None) } else { crate::rir::split_unwind_info(code) };
        let code = &code[..n];
        let twin = matches!(self.fns[id as usize].key, FnKey::Sandbox(_));
        let redirect = if interp || options.no_codegen { Some(self.no_codegen_target(code, relocs, &self.fns[id as usize].name)?) } else { None };
        // a sandboxed twin's code goes to the sandbox code space (recycled as a whole)
        let at = match redirect {
            Some(t) => t,
            None => match if twin { crate::sandbox::emit_twin(self, code, relocs) } else { None } {
                Some(a) => a,
                None => self.emit_linked(code, relocs)?,
            },
        };
        if redirect.is_none() {
            self.index_code(at, code.len() as u32, id, info);
        }
        let f = &mut self.fns[id as usize];
        if twin && f.compiled {
            crate::sandbox::note_dead(f.code_len as u64);
        }
        if f.compiled {
            let old_map = std::mem::take(&mut f.pc_map);
            crate::zone_alloc::kept_with(|| f.history.get_or_insert_with(Default::default).prev.push((f.code, f.code_len, old_map, f.patch_gen)));
        } else if gen > 0 {
            // a patch group's first code for a slot: rolling it back restores the lazy stub
            // (length 0 marks it), so the slot builds again from the restored source
            crate::zone_alloc::kept_with(|| f.history.get_or_insert_with(Default::default).prev.push((f.code, 0, PcMap::default(), 0)));
        }
        f.code = at;
        f.code_len = code.len() as u32;
        f.pc_map = PcMap::new(&pc_map);
        let first = !f.compiled;
        f.compiled = true;
        f.version += 1;
        if gen > 0 {
            f.patch_gen = gen;
            crate::zone_alloc::kept_with(|| self.patch_log.push((id, gen)));
        }
        // borrowck lane: a fn instance's first code is checked lazily in the background
        if first && gen == 0 && crate::borrowck::lazy::gandalf_enabled(options) {
            self.note_first_borrowck(id);
        }
        unsafe {
            let slot = self.slots.table(id) as *const AtomicU64;
            (*slot).store(at, Ordering::SeqCst);
        }
        // its first run: O3 code that kept a call to it as never-run is built again
        if first && !self.cold_users.is_empty() {
            if let Some(users) = self.cold_users.remove(&id) {
                for t in users {
                    self.retier(t);
                }
            }
        }
        self.stats.code_bytes += code.len() as u64;
        self.exec_event(id, "install");
        if self.prog.session.runtime.profile {
            self.exec_note(id, if crate::exec_profile::is_call(self as *const Unit as usize, id) {
                crate::exec_profile::CALL
            } else { crate::exec_profile::AHEAD });
        }
        Ok(at)
    }

    /// borrowck lane: slot `id`'s first code (a fn instance) is checked lazily in the background,
    /// unless the disk cache has a verdict for its body.
    pub(crate) fn note_first_borrowck(&mut self, id: u32) {
        let options = &self.prog.session.clone().options;
        // only the running app's own crates are checked in the background (event::admit,
        // the app-crate rule of the live-edit gate): framework, dependency and std
        // instances are not queued, and their verdict keys are not computed
        if let Some((d, a)) = match &self.fns[id as usize].key {
            FnKey::Inst(d, a) if crate::borrowck::event::admit(&self.prog, self.prog.def(*d).krate) => Some((*d, a.clone())),
            _ => None,
        } {
            // a clean verdict for this body is in the disk cache: checked once ever
            let mut c = self.cache.take();
            let key = c.as_mut().and_then(|c| c.borrowck_key(self, id));
            let cached = match (&c, key) {
                (Some(c), Some(k)) => c.borrowck_stored(k),
                _ => None,
            };
            self.cache = c;
            if let Some(c) = &self.cache {
                let epoch = self.tcx.collect_epoch.load(Ordering::SeqCst);
                if !self.front.owner_verdicts.contains_key(&epoch) {
                    self.front.owner_verdicts.insert(epoch, std::sync::Arc::new(c.owner_verdicts(epoch)));
                }
            }
            trace_stored_verdict(&self.prog, d, "instance", cached.as_ref());
            let src: std::sync::Arc<dyn crate::borrowck::lazy::FrontSource> = std::sync::Arc::new(BorrowckFront(self.front.clone()));
            let epoch = self.tcx.collect_epoch.load(Ordering::SeqCst);
            crate::borrowck::lazy::note_program(options, &src, epoch);
            crate::borrowck::lazy::note_compiled(options, &src, crate::borrowck::lazy::NoteFn {
                def: d, args: a, cache_key: key, epoch, rank: None, cached,
            });
        }
    }

    /// Installs slot `id`'s first code from the startup image (cache.rs prelink placed and linked
    /// it at `at`): what `install` does for a first install outside a patch group, except the
    /// borrowck note (prelink_startup sends those in the background).
    pub(crate) fn install_prelinked(&mut self, id: u32, at: u64, len: u32, trailer: &'static [u8], pc_map: PcMap) {
        {
            let mut t = self.code.write().unwrap_or_else(|e| e.into_inner());
            if !trailer.is_empty() {
                t.add_image_unwind(at, trailer, &pc_map);
            }
            t.add(at, len, id, None);
        }
        let f = &mut self.fns[id as usize];
        f.code = at;
        f.code_len = len;
        f.pc_map = pc_map;
        f.compiled = true;
        f.version += 1;
        unsafe {
            let slot = self.slots.table(id) as *const AtomicU64;
            (*slot).store(at, Ordering::SeqCst);
        }
        if !self.cold_users.is_empty() {
            if let Some(users) = self.cold_users.remove(&id) {
                for t in users {
                    self.retier(t);
                }
            }
        }
        self.stats.code_bytes += len as u64;
        self.exec_event(id, "install");
        self.exec_note(id, crate::exec_profile::AHEAD);
    }


    /// makepad inspect: slot `id` gets back the code it ran before its install of generation
    /// `first` (the installs since then are popped off its saved versions).
    pub fn restore_before(&mut self, id: u32, first: u32) {
        let mut changed = false;
        loop {
            let f = &mut self.fns[id as usize];
            if f.patch_gen < first || f.patch_gen == 0 {
                break;
            }
            let Some(prev) = f.history.as_mut().and_then(|h| h.prev.pop()) else { break };
            f.code = prev.0;
            f.code_len = prev.1;
            f.pc_map = prev.2;
            f.patch_gen = prev.3;
            if prev.1 == 0 {
                f.compiled = false;
            }
            changed = true;
        }
        if changed {
            let f = &mut self.fns[id as usize];
            f.version += 1;
            let at = f.code;
            unsafe {
                let slot = self.slots.table(id) as *const AtomicU64;
                (*slot).store(at, Ordering::SeqCst);
            }
        }
    }

    /// Rolls back every slot installed by patch group `gen`. Their cached RIR is dropped,
    /// so later inlining recompiles them from source (the agent's next edit).
    pub fn rollback_gen(&mut self, gen: u32) -> Vec<u32> {
        let mut out = Vec::new();
        for id in 0..self.fns.len() {
            if self.fns[id].patch_gen != gen || gen == 0 {
                continue;
            }
            let f = &mut self.fns[id];
            let prev = match f.history.as_mut().and_then(|h| h.prev.pop()) {
                Some(p) => p,
                None => continue,
            };
            f.code = prev.0;
            f.code_len = prev.1;
            f.pc_map = prev.2;
            f.patch_gen = prev.3;
            f.rir = None;
            f.version += 1;
            if prev.1 == 0 {
                f.compiled = false;
            }
            unsafe {
                let slot = self.slots.table(id as u32) as *const AtomicU64;
                (*slot).store(prev.0, Ordering::SeqCst);
            }
            self.frequency_profiles.remove(&(id as u32));
            out.push(id as u32);
        }
        out
    }
}

/// Names `u` the runtime's unit without calling into it: code entered through an address the
/// host handed out (an exported function called from a thread of the host's, not through
/// `call_jit`) compiles its lazy stubs against it.
pub unsafe fn set_runtime_unit(u: *mut Unit) {
    UNIT = u;
}

/// Android: the address of a C-ABI function as a value is a stub in the host library
/// (os::proc::trampoline): system libraries check indirect calls against the code they know
/// (CFI: libaaudio crashed in __cfi_slowpath on a JIT address), and JIT memory is not any library's.
#[cfg(target_os = "android")]
fn c_callable_hook(ctx: usize, id: u32, thunk: u64) -> u64 {
    let u = unsafe { &*(ctx as *const Unit) };
    let c_abi = (id as usize) < u.fns.len()
        && match &u.fns[id as usize].key {
            FnKey::Inst(d, _) => u.tcx.fn_abi(&u.prog, *d).map_or(false, |abi| abi != crate::types::Abi::Rust),
            _ => false,
        };
    if c_abi { os::proc::trampoline(thunk) } else { thunk }
}

/// Calls JIT code `entry(arg)` under a fresh recovery context. No Rust borrow of the
/// unit may be alive across this call; `u` becomes the runtime's unit pointer.
pub unsafe fn call_jit(u: *mut Unit, entry: u64, arg: u64) -> Result<u64, PanicInfo> {
    // the unit's own domain (its compile domain: statics laid out, initializers run)
    call_jit_in(u, (&*u).syms.dom0(), entry, arg)
}

/// `call_jit` in the domain whose static block is at `domain` (domain.rs): the code runs on the
/// calling thread, whose domain comes back after the call.
pub unsafe fn call_jit_in(u: *mut Unit, domain: u64, entry: u64, arg: u64) -> Result<u64, PanicInfo> {
    // JIT code and the runtime helpers it calls allocate in the program's heap (zone_alloc.rs)
    let _kept = crate::zone_alloc::kept();
    // the runtime's unit for this call; the previous one comes back after it (a proc-macro call
    // into the macro world's unit must not leave the program's lazy stubs pointing there)
    let prev = UNIT;
    UNIT = u;
    let sampled = (&*u).tier1.is_some();
    if sampled {
        sampler_enter((&*u).exec_base, (&*u).exec_size);
    }
    // a nested call (into another unit's world) leaves the thread's domain as it found it
    let prev_domain = crate::domain::enter(domain);
    let r = call_jit_inner(u, entry, arg);
    crate::domain::enter(prev_domain);
    if sampled {
        sampler_leave();
    }
    if !prev.is_null() {
        UNIT = prev;
    }
    r
}

unsafe fn call_jit_inner(u: *mut Unit, entry: u64, arg: u64) -> Result<u64, PanicInfo> {
    let ctx = ctx_ptr();
    let saved_ctx = [*ctx, *ctx.add(1)];
    let enter: rt_fn_ty!(fn(u64, u64, *mut u64) -> u64) = std::mem::transmute((&*u).enter as usize);
    let r = enter(entry, arg, ctx);
    *ctx = saved_ctx[0];
    *ctx.add(1) = saved_ctx[1];
    if r == 0 {
        Ok(0)
    } else {
        let p = PANIC.with(|p| p.borrow_mut().take());
        Err(p.unwrap_or(PanicInfo { kind: "unknown".to_string(), message: String::new(), site: u64::MAX, frames: Vec::new(), fault_addr: 0, location: String::new() }))
    }
}

/// One worker serves all deadlines, including enclosing guards on the same thread.
/// No thread creation per guard, and no polling instructions in compiled code.
pub fn watchdog(ms: u64) -> u64 {
    static START: std::sync::Once = std::sync::Once::new();
    START.call_once(|| {
        { let _k = crate::zone_alloc::kept();
        std::thread::spawn(|| {
            let mut calls = WATCHDOG_CALLS.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                while calls.is_empty() {
                    calls = WATCHDOG_WAKE.wait(calls).unwrap_or_else(|e| e.into_inner());
                }
                let now = std::time::Instant::now();
                for call in calls.iter() {
                    if call.deadline.is_some_and(|d| now >= d) && !call.hit.load(Ordering::Acquire) {
                        os::thread::kick(call.target);
                    }
                }
                calls = WATCHDOG_WAKE.wait_timeout(calls, std::time::Duration::from_millis(5)).unwrap_or_else(|e| e.into_inner()).0;
            }
        });
        }
    });
    let call = crate::zone_alloc::kept_with(|| std::sync::Arc::new(WatchdogCall {
        token: WATCHDOG_ID.fetch_add(1, Ordering::Relaxed),
        target: os::thread::current(),
        deadline: if ms == 0 { None } else { std::time::Instant::now().checked_add(std::time::Duration::from_millis(ms)) },
        parent: WATCHDOG_STACK.with(|s| s.borrow().last().cloned()),
        hit: AtomicBool::new(false),
    }));
    let token = call.token;
    WATCHDOG_STACK.with(|s| s.borrow_mut().push(call.clone()));
    WATCHDOG_CURRENT.with(|c| c.set(std::sync::Arc::as_ptr(&call)));
    if call.deadline.is_some() {
        crate::zone_alloc::kept_with(|| WATCHDOG_CALLS.lock().unwrap_or_else(|e| e.into_inner()).push(call));
        WATCHDOG_WAKE.notify_one();
    }
    token
}

fn watchdog_remove(token: u64) {
    WATCHDOG_STACK.with(|s| {
        let mut stack = s.borrow_mut();
        if stack.last().is_some_and(|c| c.token == token) {
            WATCHDOG_CURRENT.with(|c| c.set(if stack.len() > 1 { std::sync::Arc::as_ptr(&stack[stack.len() - 2]) } else { std::ptr::null() }));
            stack.pop();
        }
    });
    WATCHDOG_CALLS.lock().unwrap_or_else(|e| e.into_inner()).retain(|c| c.token != token);
    WATCHDOG_EXPIRED.with(|t| { if t.get() == token { t.set(0); } });
}

pub fn watchdog_done() {
    if let Some(token) = WATCHDOG_STACK.with(|s| s.borrow().last().map(|c| c.token)) {
        watchdog_remove(token);
    }
}

/// Attribute methods and closures to their declaration module, not their callers.
fn exec_metadata(prog: &Program, d: DefId, name: String) -> crate::exec_census::Metadata {
    let def = prog.def(d);
    let module = &prog.mods[prog.mods[def.scope as usize].normal as usize];
    crate::exec_census::Metadata {
        name,
        krate: prog.syms.str(prog.crates[def.krate as usize].name).to_owned(),
        module: prog.def_path(module.def),
        initializer: match def.kind { DefKind::Static => "static-initializer", DefKind::Const | DefKind::AssocConst => "const-initializer", _ => "" },
    }
}

/// Diagnostic metadata is read only in a unit call (the slot table is the owner's).
unsafe fn cost_name(up: *mut Unit, id: u32) {
    if !crate::fncost::enabled() && !crate::warm::enabled() { return; }
    let f = &(&*up).fns[id as usize];
    crate::fncost::name(id, &f.name);
    if crate::warm::enabled() { crate::warm::name(id, &crate::syms::stable_fn_name(&*up, id)); }
    if let FnKey::Inst(d, args) = &f.key {
        crate::fncost::instance(id, d.0, &args.iter().map(|t| t.0).collect::<Vec<_>>());
    }
}

/// Compiles one slot (type check, lower, optimise, codegen) and installs it. Called
/// from the lazy stub on first call, and directly for eager compilation and patches.
pub unsafe fn compile_slot(up: *mut Unit, id: u32) -> u64 {
    let _z = crate::zone_alloc::compiler();
    // a slot of the startup image is built by its key
    ids_name(up, id);
    let _job = compile_job();
    ensure_sufficient_stack(|| compile_slot_on_stack(up, id))
}

/// The thread that runs the program's main (run.rs), and its compile intervals (wall ns, owner
/// thread waits included) for MAKEPAD_RUN_MAIN_COMPILE_LOG: app time vs compile time on the main thread.
pub static MAIN_TID: AtomicU64 = AtomicU64::new(0);
/// Functions compiled so far (= Stats::compiled), readable off the unit: progress
/// reports (run.rs `--report-to`) poll it while the owner thread is busy.
pub static COMPILED_FNS: AtomicU64 = AtomicU64::new(0);

/// Display-only snapshot. Never wait for a compiler or call the owner thread to draw
/// progress: a busy queue is reported as busy and sampled again on the next tick.
#[cfg(unix)]
pub(crate) fn compile_progress() -> (Option<usize>, u64) {
    let queued = (|| {
        let spec = SPEC_Q.try_lock().ok()?.len();
        let base = O2_QUEUE.try_lock().ok()?.len();
        let tier = TIER_QUEUE.try_lock().ok()?.len();
        Some(spec + base + tier)
    })();
    (queued, STARTUP_COMPILES.load(Ordering::Relaxed))
}
/// O3 installs (read off the unit: run.rs's monitor)
pub static TIER1_INSTALLS: AtomicU64 = AtomicU64::new(0);
static TIER1_FINISHED: AtomicU64 = AtomicU64::new(0);

/// Installation precedes releasing O3 reader pins. Wake reclamation at both boundaries,
/// including failed builds, so the last job cannot leave its inputs held until another build.
pub fn reclaim_generation() -> u64 {
    COMPILED_FNS.load(Ordering::Relaxed) + TIER1_INSTALLS.load(Ordering::Relaxed)
        + TIER1_FINISHED.load(Ordering::Relaxed) + BUILD_GEN.load(Ordering::Relaxed)
}
pub static MAIN_COMPILES: relax_session::sync::Mutex<Vec<MainCompile>> = relax_session::sync::Mutex::new(Vec::new());

/// One compile the main thread waited for (MAKEPAD_RUN_MAIN_COMPILE_LOG): wall interval, the slot,
/// how it was satisfied and where its wall time went (ns).
#[derive(Clone, Copy, Default)]
pub struct MainCompile {
    pub start: u64,
    pub end: u64,
    pub slot: u32,
    /// MAIN_HOW_*: the first way the slot was satisfied
    pub how: u8,
    /// typeck ahead off the unit, waiting for owner thread, front (typeck + lower on owner thread),
    /// back (opt + codegen off the unit), on owner thread after back end, waiting for another thread's build
    pub ns: [u64; 6],
    /// thread CPU of the front, the optimisation and the code generation (wall vs CPU: work vs
    /// waiting / descheduling)
    pub cpu: [u64; 3],
    /// RIR instructions entering optimisation (after the front's inlining) and entering codegen
    pub insts: [u32; 2],
}

pub const MAIN_HOW_NAMES: [&str; 6] = ["done", "cache", "spec", "inflight", "split", "plain"];

thread_local! {
    /// the main thread's compile being recorded (only between compile_slot_on_stack's ends)
    static MAIN_REC: std::cell::Cell<Option<MainCompile>> = const { std::cell::Cell::new(None) };
    /// owner: the record of the request being served (its requester's MAIN_REC; null: none)
    static MAIN_REC_AT: std::cell::Cell<*const std::cell::Cell<Option<MainCompile>>> = const { std::cell::Cell::new(std::ptr::null()) };
}

/// The compile record work on this thread adds to: the served request's requester's on the
/// owner, this thread's own elsewhere.
pub(crate) fn main_rec_target() -> *const () {
    let at = MAIN_REC_AT.try_with(|r| r.get()).unwrap_or(std::ptr::null());
    if !at.is_null() {
        return at as *const ();
    }
    MAIN_REC.try_with(|c| c as *const std::cell::Cell<Option<MainCompile>> as *const ()).unwrap_or(std::ptr::null())
}

/// Adds to the main thread's current record (no-op elsewhere).
fn main_rec(f: impl FnOnce(&mut MainCompile)) {
    let at = main_rec_target() as *const std::cell::Cell<Option<MainCompile>>;
    if at.is_null() {
        return;
    }
    // SAFETY: the requester whose record it is waits for this request (or it is this thread's)
    let c = unsafe { &*at };
    if let Some(mut r) = c.get() {
        f(&mut r);
        c.set(Some(r));
    }
}

fn main_how(how: u8) {
    main_rec(|r| {
        if r.how == u8::MAX {
            r.how = how;
        }
    });
}

/// Real-time threads lazily compiling (a lazy stub hit; each one is reported).
pub static RT_LAZY: AtomicU64 = AtomicU64::new(0);
pub static RT_LAZY_NS: AtomicU64 = AtomicU64::new(0);

/// The calling thread runs under a real-time policy, or MAKEPAD_RUN_RT_THREADS=name,.. names it.
/// Asked at each lazy compile, not cached: a thread's policy and name are set after it starts.
pub fn is_realtime_thread() -> bool {
    if os::thread::realtime() {
        return true;
    }
    static NAMES: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    let names = NAMES.get_or_init(|| crate::zone_alloc::kept_with(|| std::env::var("MAKEPAD_RUN_RT_THREADS").map(|v| v.split(',').filter(|n| !n.is_empty()).map(|n| n.to_string()).collect()).unwrap_or_default()));
    if names.is_empty() {
        return false;
    }
    let me = os::thread::name(os::thread::current());
    !me.is_empty() && names.iter().any(|n| me.contains(n.as_str()))
}

unsafe fn compile_slot_on_stack(up: *mut Unit, id: u32) -> u64 {
    // a real-time thread (audio callback) should never meet a lazy stub: the program marks its
    // real-time code (std::relax::realtime) and everything it reaches is compiled then, and
    // hot reload compiles what patched code reaches before installing it (reload.rs). One that
    // still does is reported with what it compiled and how long the thread stalled, and
    // everything that slot reaches is compiled at once, ahead of its next calls (and recorded
    // for the next run's mark).
    if !owner::is_owner() && is_realtime_thread() && !(&*up).fns[id as usize].compiled {
        let t = wall_ns();
        let a = compile_slot_on_stack_rt(up, id);
        let upn = up as usize;
        if realtime_on() {
            owner::post(owner::Class::Main, move || unsafe { realtime_reach(upn as *mut Unit, vec![id], "miss") });
        }
        let dt = wall_ns() - t;
        let n = RT_LAZY.fetch_add(1, Ordering::Relaxed);
        RT_LAZY_NS.fetch_add(dt, Ordering::Relaxed);
        // the first few and every long stall by name; the total at exit (run.rs summary)
        if n < 3 || dt >= 1_000_000 {
            let name = (&*up).fns[id as usize].name.clone();
            eprintln!("makepad: real-time thread `{}` stalled {:.2} ms compiling `{}` on its first call (a lazy compile on a real-time thread)", os::thread::name(os::thread::current()), dt as f64 / 1e6, name);
        }
        return a;
    }
    compile_slot_on_stack_rt(up, id)
}

unsafe fn compile_slot_on_stack_rt(up: *mut Unit, id: u32) -> u64 {
    let _census_context = (&*up).exec_context(crate::exec_census::active() && crate::exec_profile::is_call(up as usize, id));

    let cost_origin = if crate::fncost::enabled() && crate::clock::TYS_ORIGIN.try_with(|o| o.get() == "fncost-ahead").unwrap_or(false) { "ahead" } else { "lazy-on-call" };
    let _cost = crate::fncost::Record::new(id, 0, cost_origin);
    crate::fncost::event("call", id);
    let on_main = MAIN_TID.load(Ordering::Relaxed) == me() && !owner::is_owner();
    if on_main {
        MAIN_REC.with(|c| c.set(Some(MainCompile { start: wall_ns(), slot: id, how: u8::MAX, ..Default::default() })));
    }
    let r = compile_slot_on_stack_inner(up, id);
    if on_main {
        if let Some(mut rec) = MAIN_REC.with(|c| c.take()) {
            rec.end = wall_ns();
            MAIN_COMPILES.lock().unwrap().push(rec);
        }
    }
    r
}

unsafe fn compile_slot_on_stack_inner(up: *mut Unit, id: u32) -> u64 {
    // inside a unit call already (a constant's initializer running code, a nested build): the
    // whole build here, no split
    if owner::is_owner() {
        // (a cancellation must not unwind through the JIT frames above: the call gets a stub that
        // panics, and the Rust frame that ran the code -- run_init_code -- cancels the request)
        return match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| compile_slot_locked(up, id, false))) {
            Ok(Compiled::Code(a)) => a,
            Ok(Compiled::Back(_)) => unreachable!("a nested compile is never split"),
            Err(p) if p.is::<crate::preempt::OwnerBusy>() => {
                BUSY_PENDING.with(|b| b.set(true));
                let u = &mut *up;
                let (code, relocs) = error_stub(u, "the compiler's owner thread is busy with an earlier request (the request is cancelled)");
                u.emit_or_redirect(&code, &relocs, "error stub").unwrap_or(0)
            }
            Err(p) => std::panic::resume_unwind(p),
        };
    }
    let _startup = StartupCompile::foreground();
    let class = owner::program_class();
    // the slot's body (and its small callees') type check runs before its front: other threads
    // keep compiling meanwhile. Code linked ahead or found in the disk cache needs no front end:
    // that is tried first (in a unit call: a check without it saw no cache and type-checked
    // bodies -- parsing whole files' lazy bodies -- for code the cache had).
    {
        let r = owner::call(class, || {
            if (*up).cache.is_none() {
                return None;
            }
            let w0 = wall_ns();
            let r = ready_code_locked(up, id);
            if r.is_some() {
                if (&*up).prog.session.runtime.profile && crate::exec_profile::is_call(up as usize, id) { (&*up).exec_note(id, crate::exec_profile::CALL); }
                compile_wall_add(up, w0);
            }
            r
        });
        if let Some(a) = r {
            return a;
        }
    }
    let t = wall_ns();
    typeck_ahead_slot(up, id);
    main_rec(|r| r.ns[0] += wall_ns() - t);
    loop {
        let t = wall_ns();
        let front = owner::call(class, || {
            main_rec(|r| r.ns[1] += wall_ns() - t);
            let w0 = wall_ns();
            COMPILE_LAST.store(w0, Ordering::Relaxed);
            let r = compile_slot_locked(up, id, true);
            if let Compiled::Code(_) = r {
                compile_note(up, id, w0);
            }
            (r, w0)
        });
        let (work, w0) = match front {
            (Compiled::Code(a), _) => return a,
            (Compiled::Back(w), w0) => (w, w0),
        };
        let done = work.run();
        let t = wall_ns();
        let r = owner::call(owner::Class::Finish, || {
            let t1 = wall_ns();
            SPLIT_NS[2].fetch_add(t1 - t, Ordering::Relaxed);
            main_rec(|r| r.ns[4] += t1 - t);
            let r = split_finish(up, done);
            if let Split::Installed(_) = r {
                compile_note(up, id, w0);
            }
            r
        });
        match r {
            Split::Installed(a) => return a,
            Split::Restart => continue,
            Split::Spec | Split::Failed(_) => unreachable!("a lazy compile's finish installs"),
        }
    }
}

/// After an outer compile of slot `id` that began at `w0` installed it (unit call): the
/// execution profile's call and the compile-wall statistics.
unsafe fn compile_note(up: *mut Unit, id: u32, w0: u64) {
    if (&*up).prog.session.runtime.profile && crate::exec_profile::is_call(up as usize, id) && (&*up).fns[id as usize].compiled && (&*up).fns[id as usize].build_err.is_none() {
        (&*up).exec_note(id, crate::exec_profile::CALL);
    }
    compile_wall_add(up, w0);
}

/// Compile-wall statistics for one outer compile that began at `w0` (on the owner thread).
unsafe fn compile_wall_add(up: *mut Unit, w0: u64) {
    {
        let w1 = wall_ns();
        COMPILE_LAST.store(w1, Ordering::Relaxed);
        let u = &mut *up;
        u.stats.compile_wall_ns += w1 - w0;
        u.stats.last_compile_end_ns = w1;
        let b = (w1 / 100_000_000) as usize;
        if b < 6000 {
            if u.stats.timeline.len() <= b {
                crate::zone_alloc::kept_with(|| u.stats.timeline.resize(b + 1, (0, 0)));
            }
            u.stats.timeline[b].0 += 1;
            u.stats.timeline[b].1 += w1 - w0;
        }
    }
}

/// Monotonic wall clock in ns.

/// Slot `id` gets O3 code from the disk cache (on the owner thread, before its install): no
/// call counter (it is O3 already), and the slots its body inlined know it (a live patch of
/// one rebuilds it).
unsafe fn cached_tier1(up: *mut Unit, id: u32, inlined: &[u32]) {
    let u = &mut *up;
    u.fns[id as usize].o3.store(true, Ordering::Relaxed);
    if u.tier1.is_some() {
        let rec = tier_rec(&u.slots, id);
        (*rec.add(3)).store(T_QUEUED, Ordering::Release);
    }
    for &c in inlined {
        u.exec_event(c, "inline");
        if c != id && !u.fns[c as usize].history().inlined_into.contains(&id) {
            crate::zone_alloc::kept_with(|| u.fns[c as usize].history.get_or_insert_with(Default::default).inlined_into.push(id));
        }
    }
    u.stats.tier1_cached += 1;
}

/// Code that needs no compile (on the owner thread): already compiled, linked ahead from the disk
/// cache, found in the disk cache, or built ahead by a speculative worker. None: compile it.
unsafe fn ready_code_locked(up: *mut Unit, id: u32) -> Option<u64> {
    let u = &mut *up;
    if u.fns[id as usize].compiled {
        main_how(0);
        return Some(u.fns[id as usize].code);
    }
    SPEC_UNIT.store(up, Ordering::Release);
    // linked from the disk cache ahead by a speculative worker: install it as it is
    if u.fns[id as usize].spec.as_ref().is_some_and(|s| !s.record) && !matches!(u.cache.as_ref().map(|c| c.mode), Some(crate::cache::Mode::Verify)) {
        if let Some(spec) = u.fns[id as usize].spec.take() {
            let SpecCode { code, map, relocs, .. } = *spec;
            u.reload_specs.remove(&id);
            u.fns[id as usize].spec_state = 2;
            main_how(1);
            return Some(install_compiled(up, id, &code, map, &relocs, false));
        }
    }
    // the disk cache (cache.rs): linked code from an earlier run, no front end or back end
    let cached = cache_lookup(up, id);
    // A relocation may install this slot recursively even if a later relocation
    // fails. Reuse that installation regardless of the outer lookup's result.
    if u.fns[id as usize].compiled { return Some(u.fns[id as usize].code); }
    if let Some((code, map, crelocs)) = cached {
        main_how(1);
        u.reload_specs.remove(&id);
        u.fns[id as usize].spec = None;
        // MAKEPAD_RUN_CACHE=verify: compile it too and compare; the fresh code is the one installed
        if matches!(u.cache.as_ref().map(|c| c.mode), Some(crate::cache::Mode::Verify)) {
            if let Ok((fresh, fmap, relocs)) = compile_fn(up, id) {
                let u = &mut *up;
                let mut c = u.cache.take();
                let same = c.as_mut().map_or(true, |c| c.verify_entry(u, id, &fresh, &relocs));
                u.cache = c;
                if let Some(c) = u.cache.as_mut() {
                    if same { c.stats.verify_same += 1 } else { c.stats.verify_diff += 1 }
                }
                return Some(install_compiled(up, id, &fresh, fmap, &relocs, true));
            }
        }
        // linked from the cache: nothing to record; its cached callees get linked ahead
        let callees = (&mut *up).cache.as_mut().map(|c| std::mem::take(&mut c.last_callees)).unwrap_or_default();
        let t1 = (&mut *up).cache.as_mut().and_then(|c| c.last_tier1.take());
        if let Some(inl) = &t1 {
            cached_tier1(up, id, inl);
        }
        if (&mut *up).cache.as_mut().is_some_and(|c| std::mem::take(&mut c.last_o2)) {
            o2_cached(up, id);
        }
        let a = install_compiled(up, id, &code, map, &crelocs, false);
        spec_enqueue_cached(up, &callees, 0);
        return Some(a);
    }
    // built ahead by a speculative worker: the first call installs it
    if let Some(spec) = u.fns[id as usize].spec.take() {
        let SpecCode { code, map, relocs, record } = *spec;
        u.reload_specs.remove(&id);
        u.fns[id as usize].spec_state = 2;
        main_how(2);
        return Some(install_compiled(up, id, &code, map, &relocs, record));
    }
    None
}

/// A lazy compile's result: the code, or (outer and pipelined) the back end still to run off
/// the unit, on the requesting thread.
pub(crate) enum Compiled {
    Code(u64),
    Back(BackWork),
}

/// Compiles slot `id` (unit call). `outer`: the outermost compile of a program thread's request
/// (it may wait for an in-flight build, load deferred modules and split off its back end); a
/// nested one (a constant's initializer running code) builds everything here.
unsafe fn compile_slot_locked(up: *mut Unit, id: u32, outer: bool) -> Compiled {
    cost_name(up, id);
    if let Some(c) = ready_code_locked(up, id) {
        return Compiled::Code(c);
    }
    if relax_level(&*up, id) == 4 && !(&*up).fns[id as usize].inflight {
        if let Some(c) = compile_o3_always(up, id) {
            return Compiled::Code(c);
        }
    }
    let u = &mut *up;
    if u.fns[id as usize].inflight {
        if outer {
            // another thread optimises / generates it off the unit: wait for that build
            // instead of making a second one (its finish is served meanwhile)
            let w = wall_ns();
            { let _cost_wait = crate::fncost::phase(8); wait_inflight(up, id); }
            let w1 = wall_ns();
            SPLIT_NS[3].fetch_add(w1 - w, Ordering::Relaxed);
            main_how(3);
            main_rec(|r| r.ns[5] += w1 - w);
            return compile_slot_locked(up, id, outer);
        }
        // nested inside a compile (CTFE running code): build it here; the other build's
        // result is dropped when it finds the slot compiled
    }
    let is_foreign = match &u.fns[id as usize].key {
        FnKey::Inst(d, _) => u.prog.def(*d).kind == DefKind::ForeignFn || crate::sandbox::is_wrapper_crate(u.prog.def(*d).krate),
        // twins (and wrappers) take the plain path: their code is made from other slots' RIR
        FnKey::Sandbox(_) => true,
        _ => false,
    };
    main_how(if outer && !is_foreign && pipelined() { 4 } else { 5 });
    if outer && !is_foreign && pipelined() {
        // the unit is needed only for type check + lower: optimisation and code generation
        // run off it (on the requesting thread), so speculative front ends overlap them
        return match split_begin(up, id, false) {
            Begin::Back(w) => Compiled::Back(w),
            Begin::Done(Split::Installed(a)) => Compiled::Code(a),
            Begin::Done(Split::Spec | Split::Restart) => unreachable!(),
            // it looked into a module the selection skipped: load it, build again
            Begin::Done(Split::Failed(e)) if e.starts_with(DEFERRED_RETRY) && load_deferred(up, id) => compile_slot_locked(up, id, outer),
            // the process is ending on another thread: this request is never answered
            Begin::Done(Split::Failed(e)) if e == YIELDED => owner::park_for_exit(),
            Begin::Done(Split::Failed(e)) => Compiled::Code(install_error(up, id, &e)),
        };
    }
    // an internal compiler panic (front end or code generator) becomes this function's
    // compile error: the process never goes down for it (rt_compile is extern "C")
    let r = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| compile_fn(up, id))) {
        Ok(r) => r,
        Err(p) => {
            if p.is::<crate::preempt::OwnerBusy>() {
                std::panic::resume_unwind(p);
            }
            let msg = if let Some(s) = p.downcast_ref::<&str>() {
                s.to_string()
            } else if let Some(s) = p.downcast_ref::<String>() {
                s.clone()
            } else {
                "panic".to_string()
            };
            end_build(up, id);
            let u = &mut *up;
            Err(if let Some(fatal) = p.downcast_ref::<crate::infer::diagnostics::FatalError>() { fatal.0.clone() } else { format!("internal compiler error in {}: {}{}", u.fns[id as usize].name, msg, take_ice_location()) })
        }
    };
    Compiled::Code(match r {
        Ok((code, map, relocs)) => install_compiled(up, id, &code, map, &relocs, true),
        // it looked into a module the selection skipped: load it, build again
        Err(e) if outer && e.starts_with(DEFERRED_RETRY) && load_deferred(up, id) => return compile_slot_locked(up, id, outer),
        // nested (a constant's initializer run inside another build): nothing is installed;
        // the call fails with DEFERRED_RETRY and the outer compile retries after the load
        Err(e) if !outer && e.starts_with(DEFERRED_RETRY) => {
            let u = &mut *up;
            let (code, relocs) = error_stub(u, DEFERRED_RETRY);
            u.emit_or_redirect(&code, &relocs, "error stub").unwrap_or(0)
        }
        Err(e) => install_error(up, id, &e),
    })
}

/// Installs a slot's compiled code (on the owner thread): perf map, O3 counter, and the
/// speculative builds of its callees.
/// Links and installs a slot's code (`relocs` are the code's own; `record`: the disk cache
/// stores it -- false for code that came from the cache).
unsafe fn install_compiled(up: *mut Unit, id: u32, code: &[u8], map: Vec<(u32, u64)>, relocs: &[crate::rir::RelocAt], record: bool) -> u64 {
    let _cost_install = crate::fncost::phase(6);
    // check-before-run (Unit::install), before the code is recorded: a refused body's code
    // never reaches the disk cache or the startup image (a warm start would run it unchecked)
    if CHECK_FIRST.load(Ordering::Relaxed) && (&*up).fns[id as usize].patch_gen == 0 && !(&*up).fns[id as usize].compiled {
        if let Some(e) = (&mut *up).refused_by_check(id) {
            return install_error(up, id, &e);
        }
    }
    // built while an earlier request's slot was open (it was left out): no cache record
    if record && TAINTED.with(|t| t.get()) { crate::cache::trace_lookup(&*up, id, "norecord:tainted"); }
    let record = record && !TAINTED.with(|t| t.get());
    // a `#[relax(O2)]` fn's O1 code is not the cache's entry for it: the O2 worker's code is
    let o2 = record && marked_hot(&*up, id);
    let record = record && !o2;
    // baseline code (built this run, or a TIER_BASE entry linked from the cache): the sampler
    // may find it running (note_base)
    let base = if record {
        (&*up).fns[id as usize].rir.as_ref().is_some_and(|r| r.baseline)
    } else {
        (&mut *up).cache.as_mut().is_some_and(|c| c.base_linked.remove(&id))
    };
    {
        {
            let u = &mut *up;
            if record {
                if let Some(mut c) = u.cache.take() {
                    c.record(u, id, code, relocs, &map);
                    u.cache = Some(c);
                }
            }
            let u = &mut *up;
            // the startup set: everything installed before the first present, then what the
            // first seconds after main install
            let before_present = !FIRST_PRESENT.load(Ordering::Acquire);
            if (before_present || wall_ns() < ORDER_UNTIL_NS.load(Ordering::Relaxed)) && u.fns[id as usize].patch_gen == 0 && !u.fns[id as usize].compiled {
                crate::zone_alloc::kept_with(|| u.start_order.push(id));
                // MAKEPAD_RUN_PRESENT_FILES=1: the source file of every fn installed before the first
                // present (what startup executes, per file)
                if before_present && present_files() {
                    if let FnKey::Inst(d, _) = &u.fns[id as usize].key {
                        eprintln!("pre-present-fn: {:?}", u.prog.file_paths[u.prog.def(*d).file as usize]);
                    }
                }
                if before_present {
                    ORDER_PRESENT_LEN.store(u.start_order.len(), Ordering::Relaxed);
                }
            }
            let a = match u.install_unchecked(id, code, relocs, map, 0) {
                Ok(a) => a,
                Err(e) => return install_error(up, id, &format!("link: {}", e)),
            };
            if perf_map_on() {
                // the code's level: cached O3 / baseline / O2 (cached or marked) / O1
                let tier = if u.fns[id as usize].o3.load(Ordering::Relaxed) {
                    " [O3]"
                } else if base {
                    " [B]"
                } else if O2_SEEN.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|m| m.contains(&(up as usize, id))) {
                    " [O2]"
                } else {
                    " [O1]"
                };
                perf_map(a, code.len(), &format!("{}{}", perf_name(u, id), tier));
            }
            u.arm_counter(id, a);
            if base {
                note_base(up, id);
            }
            if o2 {
                o2_enqueue(up, id);
                // #[relax(O3)]: its O3 build queued now, at the lowest priority (no samples, no
                // call rate); the O3 threads start after the first present
                if relax_level(u, id) >= 3 && u.tier1.is_some() {
                    queue_marked(u, id);
                }
            }
            // Cached O3 code also gets one opportunity to train this run.
            if !record {
                train_cached_tier1(u, id);
            }
            note_entered(&*up, id);
            spec_enqueue(up, id, 0);
            drop_completed_body(&mut *up, id);
            if (*tier_rec(&u.slots, id).add(3)).load(Ordering::Acquire) == T_REPLAY {
                (*tier_rec(&u.slots, id).add(2)).load(Ordering::Relaxed)
            } else { a }
        }
    }
}


/// A slot's name in the perf map: impl methods' names lack the self type, so the source line and
/// the type arguments are added.
fn perf_name(u: &Unit, id: u32) -> String {
    let mut name = u.fns[id as usize].name.to_string();
    if let FnKey::Inst(d, args) = &u.fns[id as usize].key {
        let def = u.prog.def(*d);
        let pf = &u.prog.files[def.file as usize];
        let (line, _) = pf.line_col(pf.ast.item(def.item).lo);
        name = format!("{}@{}", name, line);
        if !args.is_empty() {
            let a: Vec<String> = args.iter().map(|t| crate::typeck::ty_to_string(false, &u.prog, &u.tcx, *t)).collect();
            name = format!("{}<{}>", name, a.join(", "));
        }
    }
    name
}

/// Cached O3 code (queued as O3 already) gets one opportunity to train this run.
unsafe fn train_cached_tier1(u: &mut Unit, id: u32) {
    if (*tier_rec(&u.slots, id).add(3)).load(Ordering::Acquire) == T_QUEUED
        && !u.prog.session.options.no_block_freq && !t1prof_replay(&u.prog.session.options) {
        crate::zone_alloc::kept_with(|| { u.frequency_profiles.entry(id).or_default(); });
    }
}

/// Records that slot `id`'s RIR took in the RIR of `deps` (Unit::body_deps).
pub(crate) fn note_body_deps(u: &mut Unit, id: u32, deps: impl Iterator<Item = u32>) {
    let mut v: Vec<u32> = deps.filter(|&c| c != id).collect();
    if v.is_empty() {
        return;
    }
    let _k = crate::zone_alloc::kept();
    let e = u.body_deps.entry(id).or_default();
    v.retain(|c| !e.contains(c));
    e.extend(v);
}

fn note_const_reader(u: &Unit, d: DefId, slot: u32) {
    let mut g = u.const_readers.lock().unwrap_or_else(|e| e.into_inner());
    let _k = crate::zone_alloc::kept();
    let v = g.entry(d).or_default();
    if v.last() != Some(&slot) && !v.contains(&slot) {
        v.push(slot);
    }
}

/// Hot reload (reload.rs): installs the first code of a slot as a lazy compile would (cache
/// record, O3 counter, speculation of its callees).
pub(crate) unsafe fn install_first(up: *mut Unit, id: u32, code: &[u8], map: Vec<(u32, u64)>, relocs: &[crate::rir::RelocAt]) -> u64 {
    install_compiled(up, id, code, map, relocs, true)
}

/// Hot reload: a slot whose build failed panics when called (as a lazy compile's would).
pub(crate) unsafe fn install_failed(up: *mut Unit, id: u32, e: &str) -> u64 {
    install_error(up, id, e)
}

/// Hot reload: installs a patched slot's code as part of patch group `gen` (kept for rollback),
/// with a fresh O3 counter.
pub(crate) unsafe fn install_patched(up: *mut Unit, id: u32, code: &[u8], map: Vec<(u32, u64)>, relocs: &[crate::rir::RelocAt], gen: u32) -> Result<u64, String> {
    let u = &mut *up;
    let a = u.install(id, code, relocs, map, gen)?;
    if matches!(u.fns[id as usize].key, FnKey::Sandbox(_)) {
        u.rearm_sandbox_counter(id, a);
    } else {
        u.arm_counter(id, a);
    }
    Ok(a)
}

/// `makepad check`: every failed build (slot, full error), collected while the check compiles.
pub static CHECK_FAILURES: relax_session::sync::Mutex<Option<Vec<(u32, String)>>> = relax_session::sync::Mutex::new(None);

/// A slot whose build failed: the error is reported and the slot panics when called.
unsafe fn install_error(up: *mut Unit, id: u32, e: &str) -> u64 {
    if let Some(v) = CHECK_FAILURES.lock().unwrap_or_else(|p| p.into_inner()).as_mut() {
        v.push((id, e.to_string()));
    }
    {
        {
            let u = &mut *up;
            let mut name = u.fns[id as usize].name.to_string();
            // the instance a generic body failed for (its args decide most such errors)
            if let FnKey::Inst(_, args) = &u.fns[id as usize].key {
                if !args.is_empty() && !name.contains('<') {
                    let a: Vec<String> = args.iter().map(|t| crate::typeck::ty_to_string(false, &u.prog, &u.tcx, *t)).collect();
                    name = format!("{}<{}>", name, a.join(", "));
                }
            }
            // a startup image slot whose key does not decode is not the program's error: no
            // identity to rebuild, only the stub that reports it if the slot is ever called
            if !matches!(u.fns[id as usize].key, FnKey::Glue(GLUE_UNNAMED, _)) {
                u.note_error(format!("compile error in {}: {}", name, e));
            }
            // a body that failed to compile panics when called, with the error's first line
            let msg = format!("compile error in {}: {}", name, e.lines().next().unwrap_or(""));
            let (code, relocs) = error_stub(u, &msg);
            u.install_unchecked(id, &code, &relocs, Vec::new(), 0).expect("error stub links")
        }
    }
}

/// MAKEPAD_RUN_PERF_MAP=1: append `addr size name [tier]` (tier: B / O1 / O2 / O3) to /tmp/perf-<pid>.map so `perf report` names
/// JIT code (Linux perf's JIT symbol convention).
pub(crate) fn perf_map_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("MAKEPAD_RUN_PERF_MAP").is_ok())
}

fn perf_map(addr: u64, len: usize, name: &str) {
    static MAP: std::sync::OnceLock<Option<relax_session::sync::Mutex<std::fs::File>>> = std::sync::OnceLock::new();
    let f = MAP.get_or_init(|| {
        if std::env::var("MAKEPAD_RUN_PERF_MAP").is_err() {
            return None;
        }
        std::fs::File::create(format!("/tmp/perf-{}.map", std::process::id())).ok().map(relax_session::sync::Mutex::new)
    });
    if let Some(m) = f {
        use std::io::Write;
        if let Ok(mut g) = m.lock() {
            let _ = writeln!(g, "{:x} {:x} {}", addr, len, name);
        }
    }
}

fn error_stub(u: &mut Unit, msg: &str) -> (Vec<u8>, Vec<crate::rir::RelocAt>) {
    let options = &u.prog.session.clone().options;
    let mut f = crate::rir::Func::new("error".to_string(), 0);
    let b = f.block();
    let addr = u.data(msg.as_bytes(), 1, crate::syms::DataKind::Bytes);
    let addr = u.data_sym(addr);
    let p = f.vreg(crate::rir::Cls::I);
    let l = f.vreg(crate::rir::Cls::I);
    let z = f.vreg(crate::rir::Cls::I);
    let s = f.vreg(crate::rir::Cls::I);
    let blk = &mut f.blocks[b as usize];
    blk.insts.push(crate::rir::Inst::Addr(p, addr));
    blk.insts.push(crate::rir::Inst::Iconst(l, msg.len() as i64));
    blk.insts.push(crate::rir::Inst::Iconst(z, 0));
    blk.insts.push(crate::rir::Inst::Call(crate::rir::Callee::Host(u.rs.fmt_str), vec![p, l, z], Vec::new()));
    blk.insts.push(crate::rir::Inst::Iconst(s, u64::MAX as i64));
    blk.insts.push(crate::rir::Inst::Call(crate::rir::Callee::Host(u.rs.panic_site), vec![s], Vec::new()));
    for _ in 0..blk.insts.len() {
        blk.pos.push(0);
    }
    blk.term = crate::rir::Term::Unreachable;
    x64::compile(options, &f, &u.env).map(|c| (c.code, c.relocs)).unwrap_or_default()
}

fn why_function(u: &Unit, id: u32, phase: &str) -> Option<crate::why::Scope> {
    if !crate::why::enabled() { return None; }
    let cause = match &u.fns[id as usize].key {
        FnKey::Inst(d, _) => u.prog.why_def(*d, phase),
        _ => crate::why::Cause::new("<generated>", 0, phase, &u.fns[id as usize].name),
    };
    crate::why::event(phase, &cause.file, cause.line, &cause.symbol, None);
    Some(crate::why::enter(cause))
}
fn why_compile(u: &Unit, id: u32) -> Option<crate::why::Scope> { why_function(u, id, "compile") }

/// Builds (or reuses) a slot's optimised RIR, then generates machine code.
pub unsafe fn compile_fn(up: *mut Unit, id: u32) -> Result<(Vec<u8>, Vec<(u32, u64)>, Vec<crate::rir::RelocAt>), String> {
    let _census_context = (&*up).exec_context(crate::exec_census::active() && crate::exec_profile::is_call(up as usize, id));

    let _why = why_compile(&*up, id);
    let _cost = crate::fncost::Record::ensure(id, 0, "ahead");
    cost_name(up, id);
    let session = unsafe { (*std::ptr::addr_of!((*up).prog.session)).clone() };
    let options = &session.options;
    let runtime = &session.runtime;
    // a foreign fn reached through its slot (a fn-pointer value made before the front end
    // used the loader address directly, a vtable or generic path): a veneer to the symbol
    // the loader resolves for direct calls, so it behaves exactly like the C function
    if let FnKey::Inst(d, _) = (&*up).fns[id as usize].key.clone() {
        if (&*up).prog.def(d).kind == crate::program::DefKind::ForeignFn {
            let u = &mut *up;
            return match u.foreign_addr(d) {
                // a jump to the C address (the cache neither records nor looks up foreign fns)
                Some(a) => { let _warm = crate::warm::Scope::slot(id, "foreign veneer emission"); let code = x64::jump_to(a); crate::fncost::code(code.len()); Ok((code, Vec::new(), Vec::new())) },
                None => Err(format!("unknown foreign function `{}` -- link its library (#[link])", u.prog.name(d))),
            };
        }
    }
    // a sandboxed twin: its base slot's RIR, transformed (sandbox.rs)
    let twin_func = match (&*up).fns[id as usize].key {
        FnKey::Sandbox(base) => {
            let f = crate::sandbox::twin_rir(up, id, base)?;
            // O3 must start from confined RIR, never from the base slot's host RIR.
            (&mut *up).set_rir(id, f.clone());
            Some(f)
        },
        _ => None,
    };
    let tb = cpu_ns();
    if twin_func.is_none() {
        crate::tui::item_begin(&(&*up).fns[id as usize].name);
        let built = build_rir(up, id);
        crate::tui::item_end();
        built?;
    }
    let u = &mut *up;
    let t0 = cpu_ns();
    // plain compile: front + optimisation (with the inline candidates' builds) count as front
    main_rec(|r| r.cpu[0] += t0 - tb);
    let rir = twin_func.as_ref().unwrap_or_else(|| u.fns[id as usize].rir.as_ref().unwrap());
    let cg = tier0_codegen_rir(options, rir, u.opt.fold, u.opt.layout);
    let func = &cg;
    // without O3 these are the only compiles: allocate thoroughly
    let env_full;
    let env = if func.baseline || (TIER1_ENABLED.load(Ordering::Relaxed) && !func.full) {
        &u.env
    } else {
        let e = &u.env;
        env_full = x64::Env { slots: e.slots, tls_key: e.tls_key, tls_slow: e.tls_slow, resume: e.resume, opt_regs: true, syms: e.syms, sym_class: e.sym_class };
        &env_full
    };
    compile_watch(Some((&format!("codegen {}", func.name), crate::opt::inst_count(func))));
    let mps = crate::memstat::peak_start();
    let c = crate::memstat::tagged(crate::memstat::Tag::Codegen, || {
        let _scratch = crate::zone_alloc::scratch(runtime);
        x64::compile(options, func, env)
    });
    crate::memstat::note_big("codegen", &func.name, mps, || cg_shape(func));
    compile_watch(None);
    let c = c?;
    note_frame(id, c.frame_size, slot_bytes(func), false);
    code_sig(id, func, &c.code, c.alloc_sig);
    crate::syms::reloc_sig(u, || crate::syms::stable_fn_name(u, id), &c.code, &c.relocs, &c.pc_map);
    dump_o1(func, &c);
    let n = crate::opt::inst_count(func) as u32;
    let t3 = cpu_ns();
    main_rec(|r| {
        r.cpu[2] += t3 - t0;
        r.insts[1] += n;
    });
    u.stats.codegen_ns += cpu_ns() - t0;
    u.fns[id as usize].cg_ns = cpu_ns() - t0;
    u.stats.compiled += 1;
    COMPILED_FNS.fetch_add(1, Ordering::Relaxed);
    Ok((c.code, c.pc_map, c.relocs))
}

/// makepad inspect's instrumented builds: slot `id` built again from its RIR with `edit` applied
/// (probe calls added), then installed as part of patch group `gen` -- `rollback_gen(gen)`
/// restores the code it replaced. Nothing is recorded in the disk cache and the slot's RIR stays
/// the plain one (O3 and O2 installs skip instrumented slots: inspect::instrument::active).
pub(crate) unsafe fn install_instrumented(up: *mut Unit, id: u32, gen: u32, base: Option<crate::rir::Func>, edit: &mut dyn FnMut(&mut crate::rir::Func, &Unit)) -> Result<u64, String> {
    let session = unsafe { (*std::ptr::addr_of!((*up).prog.session)).clone() };
    let options = &session.options;
    let runtime = &session.runtime;
    match (&*up).fns[id as usize].key.clone() {
        FnKey::Sandbox(_) => return Err("a sandboxed twin cannot be instrumented".into()),
        FnKey::Inst(d, _) if (&*up).prog.def(d).kind == crate::program::DefKind::ForeignFn => return Err("a foreign fn has no body".into()),
        _ => {}
    }
    if (&*up).fns[id as usize].rir.is_none() {
        build_rir(up, id)?;
    }
    let u = &mut *up;
    let mut f: crate::rir::Func = match base {
        Some(f) => f,
        None => {
            let Some(rir) = u.fns[id as usize].rir.clone() else { return Err("no RIR for this slot".into()) };
            (*rir).clone()
        }
    };
    edit(&mut f, u);
    let cg = tier0_codegen_rir(options, &f, u.opt.fold, u.opt.layout);
    let c = {
        let _scratch = crate::zone_alloc::scratch(runtime);
        x64::compile(options, &cg, &u.env)
    }?;
    note_frame(id, c.frame_size, slot_bytes(&cg), false);
    u.install(id, &c.code, &c.relocs, c.pc_map, gen)
}

/// makepad inspect's by-name probes: slot `id`'s body lowered again with probe calls at `points`
/// (lower.rs ProbePoint), as baseline RIR (no inlining of user fns: the probe calls stay calls
/// for probes.rs to guard). The slot's own RIR is untouched. What each point found comes back.
pub(crate) unsafe fn relower_with_probes(up: *mut Unit, id: u32, points: Vec<crate::lower::ProbePoint>) -> Result<(crate::rir::Func, Vec<(u32, Result<(), String>)>), String> {
    let session = unsafe { (*std::ptr::addr_of!((*up).prog.session)).clone() };
    let options = &session.options;
    let _ = crate::lower::PROBE_POINTS.try_with(|p| *p.borrow_mut() = points);
    let _ = crate::lower::PROBE_RESULTS.try_with(|p| p.borrow_mut().clear());
    let lowered = build_rir_lowered(up, id);
    let _ = crate::lower::PROBE_POINTS.try_with(|p| p.borrow_mut().clear());
    let results = crate::lower::PROBE_RESULTS.try_with(|p| std::mem::take(&mut *p.borrow_mut())).unwrap_or_default();
    let lowered = lowered?;
    let (prep, _) = prepare_rir_from(up, id, false, Some(lowered), RirMode::Baseline)?;
    let (func, _) = optimize_rir(options, id, prep)?;
    Ok((func, results))
}

/// Stack frame bytes of each slot's latest code (O1, O3): the compile report's frame
/// columns (kernel-worker threads have 2 MiB stacks; Relax frames larger than rustc's
/// overflowed them)
pub static FRAMES: relax_session::sync::Mutex<Vec<(u32, u32)>> = relax_session::sync::Mutex::new(Vec::new());

fn note_frame(id: u32, frame: u32, slots: u32, tier1: bool) {
    let _k = crate::zone_alloc::kept();
    let mut g = FRAMES.lock().unwrap_or_else(|e| e.into_inner());
    if g.len() <= id as usize {
        g.resize(id as usize + 1, (0, 0));
    }
    let mut h = FRAME_SLOTS.lock().unwrap_or_else(|e| e.into_inner());
    if h.len() <= id as usize {
        h.resize(id as usize + 1, (0, 0));
    }
    if tier1 {
        g[id as usize].1 = frame;
        h[id as usize].1 = slots;
    } else {
        g[id as usize].0 = frame;
        h[id as usize].0 = slots;
    }
}

/// bytes of a function's stack slots (aggregates, address-taken locals) after slot colouring;
/// the rest of a frame is spill slots, saved registers and call staging
fn slot_bytes(f: &crate::rir::Func) -> u32 {
    f.slots.iter().map(|s| (s.size + s.align - 1) / s.align * s.align).sum()
}

/// slot bytes of each slot's latest code (O1, O3) next to FRAMES
pub static FRAME_SLOTS: relax_session::sync::Mutex<Vec<(u32, u32)>> = relax_session::sync::Mutex::new(Vec::new());

/// MAKEPAD_RUN_CODE_SIG=<file>: one line per compiled function (its stable name -- instance type
/// arguments included, syms::stable_fn_name --, code bytes, RIR hash + allocation signature):
/// back-end refactors compare these between builds for equal output, and two runs of one
/// program compare them for build-order independence (tools/hrcheck/fe/sigcmp.py).
/// A slot's name in MAKEPAD_RUN_CODE_SIG output: its stable name, with the DefId of an instance (two
/// inherent impls in one module share a def path; DefIds are stable for one source tree).
fn sig_name(u: &Unit, id: u32) -> String {
    let mut n = crate::syms::stable_fn_name(u, id);
    if (id as usize) < u.fns.len() {
        if let FnKey::Inst(d, _) = &u.fns[id as usize].key {
            n.push_str(&format!("@{}", d.0));
        }
    }
    n
}

fn sig_dir() -> Option<std::ffi::OsString> {
    static D: std::sync::OnceLock<Option<std::ffi::OsString>> = std::sync::OnceLock::new();
    D.get_or_init(|| crate::zone_alloc::kept_with(|| std::env::var_os("MAKEPAD_RUN_CODE_SIG_DIR"))).clone()
}

fn code_sig(id: u32, f: &crate::rir::Func, code: &[u8], alloc: u64) {
    static F: std::sync::OnceLock<Option<relax_session::sync::Mutex<std::fs::File>>> = std::sync::OnceLock::new();
    if let Some(m) = F.get_or_init(|| {
        let f = std::env::var("MAKEPAD_RUN_CODE_SIG").ok().and_then(|p| std::fs::File::create(p).ok()).map(relax_session::sync::Mutex::new);
        crate::regalloc::SIG_ON.store(f.is_some(), Ordering::Relaxed);
        f
    }) {
        use std::io::Write;
        // the optimised RIR, absolute addresses (which differ between builds) masked
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        let d = canon_slots(f.dump());
        let d = match crate::syms::unit() {
            Some(u) => crate::syms::canon_dump(u, &d),
            None => d,
        };
        if let Ok(w) = std::env::var("MAKEPAD_RUN_DUMP_CANON") {
            if f.name.ends_with(w.as_str()) {
                eprintln!("== CANON {}\n{}", f.name, d);
            }
        }
        let b = d.as_bytes();
        let mut i = 0;
        while i < b.len() {
            let mut j = i;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            if j > i {
                if j - i <= 6 {
                    for &c in &b[i..j] {
                        h = (h ^ c as u64).wrapping_mul(0x100_0000_01b3);
                    }
                }
                i = j;
            } else {
                h = (h ^ b[i] as u64).wrapping_mul(0x100_0000_01b3);
                i += 1;
            }
        }
        let name = match crate::syms::unit() {
            Some(u) => sig_name(u, id),
            None => f.name.clone(),
        };
        // MAKEPAD_RUN_CODE_SIG_DIR=<dir>: each signed RIR too, as <dir>/<name hash>-<RIR hash>.rir (first line
        // the stable name): two runs' dirs diff function by function
        if let Some(dir) = sig_dir() {
            let nh = name.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, c| (h ^ c as u64).wrapping_mul(0x100_0000_01b3));
            let _ = std::fs::write(std::path::Path::new(&dir).join(format!("{:016x}-{:016x}.rir", nh, h)), format!("{}\n{}", name, d));
        }
        let _ = writeln!(m.lock().unwrap(), "{}\t{}\t{:016x}{:016x}", name, code.len(), h, alloc);
    }
}

/// Miscompile bisection knobs (read once): MAKEPAD_RUN_NOINL=<pats> builds the matching callers
/// without user-code inlining, MAKEPAD_RUN_NOINL_CALLEE=<pats> never inlines the matching callees.
/// A pattern list is comma-separated name substrings, or `hash:M/R` (name hash % M == R).
fn noinl_env(options: &crate::options::Options) -> (&Option<String>, &Option<String>) {
    (&options.noinl, &options.noinl_callee)
}

/// MAKEPAD_RUN_DUMP_RIR=<name suffix>: optimised RIR of matching fns to stderr (also inline-only
/// callees, which MAKEPAD_RUN_DUMP never sees); MAKEPAD_RUN_INL_TRACE=<substring>: "inlined X into Y".
/// MAKEPAD_RUN_DUMP_RIR matching: a name suffix, or `~text` for a name containing text (closures:
/// `~{closure#1292}@` -- their position suffix changes between builds)
fn dump_rir_match(name: &str, w: &str) -> bool {
    match w.strip_prefix('~') {
        Some(t) => name.contains(t),
        None => name.ends_with(w),
    }
}

fn debug_env() -> &'static (Option<String>, Option<String>) {
    static E: std::sync::OnceLock<(Option<String>, Option<String>)> = std::sync::OnceLock::new();
    E.get_or_init(|| crate::zone_alloc::kept_with(|| (std::env::var("MAKEPAD_RUN_DUMP_RIR").ok(), std::env::var("MAKEPAD_RUN_INL_TRACE").ok())))
}

/// Bisection knobs for the block-order passes (relayout, rotate_loops): MAKEPAD_RUN_NO_LAYOUT_FN=<pats>
/// skips them for matching fns, MAKEPAD_RUN_LAYOUT_ONLY=<pats> runs them only for matching fns.
fn layout_for(options: &crate::options::Options, n: &str) -> bool {
    let (no, only) = (&options.no_layout_fn, &options.layout_only);
    if let Some(p) = no {
        if name_matches(n, p) {
            return false;
        }
    }
    if let Some(p) = only {
        return name_matches(n, p);
    }
    true
}

/// MAKEPAD_RUN_NOHINT=<pats>: core callees that do not get the implicit #[inline] (bisection)
fn nohint_env(options: &crate::options::Options) -> Option<&str> {
    options.nohint.as_deref()
}

fn name_matches(n: &str, pat: &str) -> bool {
    if let Some(rest) = pat.strip_prefix("hash:") {
        let h = n.bytes().fold(0u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
        let mut it = rest.split('/');
        let m: u32 = it.next().and_then(|x| x.parse().ok()).unwrap_or(1).max(1);
        let r: u32 = it.next().and_then(|x| x.parse().ok()).unwrap_or(0);
        return h % m == r;
    }
    pat.split(',').any(|p| n.contains(p))
}

fn inline_source_wanted(u: &Unit, id: u32, full: bool) -> bool {
    let options = &u.prog.session.options;
    let attr = match u.fns[id as usize].key {
        FnKey::Inst(d, _) => inline_attrs(&u.prog, d),
        _ => 0,
    };
    let src = src_size(u, id);
    crate::simd::covered(options, &u.fns[id as usize].name)
        || attr & crate::opt::ATTR_ALWAYS != 0
        || src <= inline_src_toks(options, u.tier1.is_some() && !full)
        || (attr & crate::opt::ATTR_HINT != 0 && src <= INLINE_HINT_SRC_TOKS)
}

/// Source size of a slot's function in tokens (cheap pre-filter for inlining: comments, doc
/// text and long names do not count).
pub fn src_size(u: &Unit, id: u32) -> u32 {
    let (file, lo, hi) = match &u.fns[id as usize].key {
        FnKey::Inst(d, _) => {
            let def = u.prog.def(*d);
            if !matches!(def.kind, DefKind::Fn | DefKind::AssocFn) {
                return u32::MAX;
            }
            let it = u.prog.files[def.file as usize].ast.item(def.item);
            (def.file, it.lo, it.hi)
        }
        FnKey::Closure(file, ce, _) if *ce & crate::tcx::ASYNC_FN_BODY != 0 => {
            let b = u.prog.files[*file as usize].ast.block(crate::ast::BlockId(*ce & !crate::tcx::ASYNC_FN_BODY));
            (*file, b.lo, b.hi)
        }
        FnKey::Sandbox(_) => return u32::MAX,
        FnKey::Closure(file, ce, _) => {
            let e = u.prog.files[*file as usize].ast.expr(crate::ast::ExprId(*ce));
            (*file, e.lo, e.hi)
        }
        FnKey::Shim(_) | FnKey::Glue(..) => return u32::MAX,
    };
    let toks = &u.prog.files[file as usize].toks;
    let a = toks.partition_point(|t| t.lo < lo);
    let b = toks.partition_point(|t| t.lo < hi);
    (b - a) as u32
}

/// Inline attributes of callee slot `c` for the inliner, None when it may not be inlined:
/// #[inline]/#[inline(always)], rule 4 (core instances made for a closure / fn item), and
/// with `core_hint` the implicit #[inline] of Relax core's functions (real std marks its
/// small methods #[inline]). User code only with `inline_all`.
fn callee_attrs(prog: &crate::program::Program, tcx: &crate::tcx::Tcx, fns: &crate::conc::AppendVec<FnEntry>, core_crate: u32, c: u32, inline_all: bool, core_hint: bool) -> Option<u8> {
    let options = &prog.session.clone().options;
    // a sandboxed module's code is never inlined into host code (it runs only as twins)
    if crate::sandbox::is_sandboxed_crate_key(prog, &fns[c as usize].key) {
        return None;
    }
    Some(match &fns[c as usize].key {
        FnKey::Inst(d, args) => {
            let core = prog.def(*d).krate == core_crate;
            if !inline_all && !core || inline_never(prog, *d) {
                return None;
            }
            let mut a = inline_attrs(prog, *d);
            // core's small fns (real std marks them #[inline]) and, at O3, user fns too:
            // LLVM inlines small same-crate fns by cost, #[inline] or not
            if core_hint && !nohint_env(options).map_or(false, |p| name_matches(&fns[c as usize].name, p)) {
                a |= crate::opt::ATTR_HINT;
            }
            if core && args.iter().any(|t| has_closure(tcx, *t, 0)) {
                a |= crate::opt::ATTR_CHAIN;
            }
            if core && crate::simd::covered(options, &fns[c as usize].name) {
                a |= crate::opt::ATTR_ALWAYS;
            }
            a
        }
        // a closure body is usually called from the one adapter instance made for it; with
        // the core hint (O3) it inlines like #[inline]
        FnKey::Closure(..) if core_hint => crate::opt::ATTR_HINT,
        FnKey::Shim(_) | FnKey::Glue(..) | FnKey::Closure(..) => 0,
        // twins are never inlined into anything (their callers are twins built from RIR)
        FnKey::Sandbox(_) => return None,
    })
}

/// Relax core's implicit #[inline] at O1: on while O3 is off (O1 is then the
/// only optimisation), off with O3 (hot functions get it there; cold code compiles
/// cheaper). MAKEPAD_RUN_CORE_HINT=0/1 overrides.
fn tier0_core_hint(options: &crate::options::Options, tier1: bool) -> bool {
    options.core_hint.unwrap_or(!tier1)
}

/// A closure or fn item among a type's arguments (iterator adapters, combinators made for a
/// closure: `Map<Filter<Iter<T>, {closure}>, {closure}>`, `Option::map::<_, {closure}>`).
fn has_closure(tcx: &crate::tcx::Tcx, t: TyId, depth: u32) -> bool {
    use crate::types::TyKind;
    if depth > 4 {
        return false;
    }
    match tcx.tys.kind(t) {
        TyKind::Closure(..) | TyKind::FnDef(..) => true,
        TyKind::Adt(_, args, _) | TyKind::Tuple(args) => tcx.tys.list(*args).iter().any(|a| has_closure(tcx, *a, depth + 1)),
        TyKind::Ref(_, x, _) | TyKind::Ptr(_, x) | TyKind::Array(x, _) | TyKind::Slice(x) => has_closure(tcx, *x, depth + 1),
        _ => false,
    }
}

/// `#[inline(always)]` -> ATTR_ALWAYS, `#[inline]` -> ATTR_HINT; `#[inline(never)]` -> 0
/// (inline_never).
pub(crate) fn inline_attrs(prog: &crate::program::Program, d: DefId) -> u8 {
    let def = prog.def(d);
    let f = &prog.files[def.file as usize];
    let it = f.ast.item(def.item);
    let mut r = 0;
    for a in f.ast.list(it.attrs) {
        if a.toks.hi > a.toks.lo && f.tok_text(a.toks.lo) == "inline" {
            if (a.toks.lo..a.toks.hi).any(|i| f.tok_text(i) == "never") {
                return 0;
            }
            r |= crate::opt::ATTR_HINT;
            for i in a.toks.lo..a.toks.hi {
                if f.tok_text(i) == "always" {
                    r |= crate::opt::ATTR_ALWAYS;
                }
            }
        }
    }
    r
}

/// `#[inline(never)]`: never inlined (rustc's inliner refuses InlineAttr::Never).
pub(crate) fn inline_never(prog: &crate::program::Program, d: DefId) -> bool {
    let def = prog.def(d);
    let f = &prog.files[def.file as usize];
    let it = f.ast.item(def.item);
    f.ast.list(it.attrs).iter().any(|a| a.toks.hi > a.toks.lo && f.tok_text(a.toks.lo) == "inline" && (a.toks.lo..a.toks.hi).any(|i| f.tok_text(i) == "never"))
}

/// A function on cold paths (speculation skips it and the blocks that call it): `#[cold]`
/// (rustc's attribute: the slow paths of Vec / HashMap growth, panics), diverging (returns
/// `!`), or a panic / failure / formatting function by name.
fn is_cold_fn(u: &Unit, c: u32) -> bool {
    if let FnKey::Inst(d, _) = &u.fns[c as usize].key {
        if is_cold_def(&u.prog, &u.tcx, *d) {
            return true;
        }
    }
    is_cold_name(&u.fns[c as usize].name)
}

/// `#[cold]` or diverging (is_cold_fn).
fn is_cold_def(prog: &Program, tcx: &Tcx, d: DefId) -> bool {
    let def = prog.def(d);
    let f = &prog.files[def.file as usize];
    let it = f.ast.item(def.item);
    f.ast.list(it.attrs).iter().any(|a| a.toks.hi > a.toks.lo && f.tok_text(a.toks.lo) == "cold")
        || tcx.fn_sig(prog, d).map_or(false, |s| s.ret == tcx.tys.never)
}

/// A panic / failure / formatting function by name (is_cold_fn).
fn is_cold_name(n: &str) -> bool {
    n.contains("panic") || n.contains("_failed") || n.contains("::fmt") || n.contains("fmt::") || n.contains("abort")
}

/// Source tokens up to which a callee's RIR is built so it can be considered for inlining;
/// #[inline] callees up to INLINE_HINT_SRC_TOKS; MAKEPAD_RUN_INLINE_SRC overrides the first.
const INLINE_SRC_TOKS_DEFAULT: u32 = 100;
/// with O3 on, O1 prepares only the most obvious callees (hot code gets more there)
const INLINE_SRC_TOKS_WITH_TIER1: u32 = 60;
/// O3 builds callees up to this size for inlining
const TIER1_SRC_TOKS: u32 = 300;
const TIER1_SINGLE_SRC_TOKS: u32 = 4000;
const INLINE_HINT_SRC_TOKS: u32 = 200;

/// MAKEPAD_RUN_O1_INLINE=0: O1 inlines nothing (the former behaviour).
fn base_inline_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("MAKEPAD_RUN_O1_INLINE").map_or(true, |v| v != "0"))
}

/// The `#[relax(..)]` level (makepad-relax) of slot `id`'s fn, on the fn itself or on its impl
/// block: 2 = O2, 3 = O3, 4 = O3 always; 0 = none. Closures follow their enclosing fn; Relax core
/// fns called by marked code, and slots found hot (o2_hot), are O2 (hot_hint).
fn relax_level(u: &Unit, id: u32) -> u8 {
    let l = relax_mark(u, id);
    if l == 0 && u.fns[id as usize].hot_hint.load(Ordering::Relaxed) {
        return 2;
    }
    l
}

fn relax_mark(u: &Unit, id: u32) -> u8 {
    let d = match &u.fns[id as usize].key {
        FnKey::Inst(d, _) => *d,
        FnKey::Closure(_, _, parent) if *parent != id && (*parent as usize) < u.fns.len() => return relax_level(u, *parent),
        _ => return 0,
    };
    let def = u.prog.def(d);
    if !matches!(def.kind, DefKind::Fn | DefKind::AssocFn) {
        return 0;
    }
    let level_of = |d: DefId| -> u8 {
        let def = u.prog.def(d);
        let f = &u.prog.files[def.file as usize];
        let text = |k: usize| f.tok_text(k as u32).to_string();
        for a in f.ast.list(f.ast.item(def.item).attrs) {
            let (lo, hi) = (a.toks.lo as usize, a.toks.hi as usize);
            if hi > lo + 1 && f.tok_text(lo as u32) == "relax" {
                if let Some(l) = crate::builtins::relax_level_toks(&text, &f.toks, lo + 1, hi) {
                    return l;
                }
            }
        }
        0
    };
    let mut l = level_of(d);
    if l == 0 && def.parent.0 != u32::MAX && (def.parent.0 as usize) < u.prog.defs.len() && u.prog.def(def.parent).kind == DefKind::Impl {
        l = level_of(def.parent);
    }
    l
}

thread_local! {
    /// compile_tier1 builds the first code of a `#[relax(O3, always)]` slot (compile_o3_always)
    static O3_FIRST: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static O3_FIRST_AT: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

fn o3_first() -> bool {
    O3_FIRST.try_with(|c| c.get()).unwrap_or(false)
}

/// `#[relax(O3, always)]`: the first call of slot `id` builds its O3 code on the calling thread
/// (on the owner thread) and runs nothing lower; the wait is logged. None: the O3 build failed (the
/// caller compiles it as usual). The code is the slot's cache entry (record_o3), so the wait
/// happens cold or after an edit only.
unsafe fn compile_o3_always(up: *mut Unit, id: u32) -> Option<u64> {
    if (&*up).tier1.is_none() {
        return None;
    }
    let w0 = wall_ns();
    if build_rir_mode(up, id, RirMode::Normal).is_err() {
        return None;
    }
    O3_FIRST.with(|c| c.set(true));
    O3_FIRST_AT.with(|c| c.set(0));
    let ok = compile_tier1(up, id);
    O3_FIRST.with(|c| c.set(false));
    let at = O3_FIRST_AT.with(|c| c.get());
    let u = &*up;
    eprintln!("makepad-run O3 always: {} waited {:.1} ms for its O3 build{}", u.fns[id as usize].name, (wall_ns() - w0) as f64 / 1e6, if ok && at != 0 { "" } else { " (failed: compiled as usual)" });
    (ok && at != 0 && u.fns[id as usize].compiled).then_some(at)
}

thread_local! {
    /// this thread builds O2 code (o2_thread)
    static O2_BUILD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// ... before the first present: from RIR that exists, no front ends under the lock
    static O2_SNAPSHOT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// O2 slots built from snapshots (or skipped) before the first present: built in full after it
static O2_LATER: relax_session::sync::Mutex<Vec<(usize, u32)>> = relax_session::sync::Mutex::new(Vec::new());

/// The first present released the O2 worker's full builds.
fn o2_release_later() {
    let later = std::mem::take(&mut *O2_LATER.lock().unwrap_or_else(|e| e.into_inner()));
    if !later.is_empty() {
        O2_QUEUE.lock().unwrap_or_else(|e| e.into_inner()).extend(later);
        O2_WAKE.notify_one();
    }
}

fn o2_build() -> bool {
    O2_BUILD.try_with(|c| c.get()).unwrap_or(false)
}

/// `#[relax(O2)]` slots waiting for their O2 build: (unit, slot)
static O2_QUEUE: relax_session::sync::Mutex<Vec<(usize, u32)>> = relax_session::sync::Mutex::new(Vec::new());
/// slots ever queued (each is built once; live patches rebuild through the usual paths)
static O2_SEEN: relax_session::sync::Mutex<Option<std::collections::HashSet<(usize, u32)>>> = relax_session::sync::Mutex::new(None);
static O2_WAKE: relax_session::sync::Condvar = relax_session::sync::Condvar::new();
static O2_THREAD: std::sync::Once = std::sync::Once::new();
/// O2 builds installed, their worker CPU ns (front + back end), dropped builds, the main
/// thread's unit-lock wait ns while the O2 worker held it, and the number of such waits
pub static O2_STATS: [AtomicU64; 5] = [AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0)];

fn o2_trace() -> bool {
    static T: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *T.get_or_init(|| std::env::var_os("MAKEPAD_RUN_O2_TRACE").is_some())
}

/// Slot `id`'s code came from an O2 cache entry: no O2 build for it this run.
pub(crate) unsafe fn o2_cached(up: *mut Unit, id: u32) {
    O2_SEEN.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(Default::default).insert((up as usize, id));
}

/// Slot `id` ran hot (an O3 build queued by its calls or samples): it gets an O2 build too, the
/// slot's cache entry, so later launches link optimised code for it from the first call instead
/// of the O1 code their start runs until O3 replaces it (O3 code is not cached). On the owner thread.
unsafe fn o2_hot(up: *mut Unit, id: u32) {
    let u = &*up;
    if (id as usize) >= u.fns.len() || matches!(u.fns[id as usize].key, FnKey::Glue(..) | FnKey::Sandbox(_)) || u.cache.is_none() {
        return;
    }
    u.fns[id as usize].hot_hint.store(true, Ordering::Relaxed);
    o2_enqueue(up, id);
}

/// Queues slot `id` (marked O2 or higher) for its O2 build on the O2 worker, once.
unsafe fn o2_enqueue(up: *mut Unit, id: u32) {
    if !o2_on() {
        return;
    }
    {
        let mut seen = O2_SEEN.lock().unwrap_or_else(|e| e.into_inner());
        if !seen.get_or_insert_with(Default::default).insert((up as usize, id)) {
            return;
        }
    }
    O2_QUEUE.lock().unwrap_or_else(|e| e.into_inner()).push((up as usize, id));
    O2_WAKE.notify_one();
    o2_start(up);
}

/// MAKEPAD_RUN_O2=0: no O2 builds (marked fns run O1, baseline code stays; A/B)
fn o2_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("MAKEPAD_RUN_O2").map_or(true, |v| v != "0"))
}

unsafe fn o2_start(up: *mut Unit) {
    // (kept: the thread outlives the compiler generation)
    let runtime = crate::zone_alloc::kept_clone(&(&*up).prog.session.runtime);
    O2_THREAD.call_once(move || {
        { let _k = crate::zone_alloc::kept();
        let _ = std::thread::Builder::new().name("run-O2".into()).spawn(move || { let _z = crate::zone_alloc::compiler_thread(); o2_thread(&runtime) });
        }
    });
}

/// Baseline slots the sampler found running (a sample in the slot or one of its SAMPLE_CALLERS
/// callers): their O2 builds -- `hit` until the startup releases its workers after the first
/// present (startup_main), at once after.
struct BaseSlots {
    released: bool,
    hit: Vec<(usize, u32)>,
}
static BASE: relax_session::sync::Mutex<BaseSlots> = relax_session::sync::Mutex::new(BaseSlots { released: false, hit: Vec::new() });
/// baseline slots waiting for their O2 build: served when O2_QUEUE is empty
static O2_BASE: relax_session::sync::Mutex<std::collections::VecDeque<(usize, u32)>> = relax_session::sync::Mutex::new(std::collections::VecDeque::new());

/// Slot `id` runs baseline code -- built so the cold start compiles fast, but every launch that
/// links it from the cache runs it again (unoptimised RIR, no inlining of user fns: a warm
/// Calculator's first frame ran ~2x native in it). Baseline code the sampler finds running gets
/// an O2 build that replaces the code and the cache entry (record_o2); the rest (most startup
/// code runs for microseconds) stays as it is. Unit lock held.
unsafe fn note_base(up: *mut Unit, id: u32) {
    (&*up).fns[id as usize].base_code.store(true, Ordering::Relaxed);
}

/// The sampler found slot `id` running (unit lock held): baseline code gets its O2 build.
unsafe fn base_sampled(up: *mut Unit, id: u32) {
    if !(&*up).fns[id as usize].base_code.swap(false, Ordering::Relaxed) {
        return;
    }
    {
        let mut g = BASE.lock().unwrap_or_else(|e| e.into_inner());
        if !g.released {
            crate::zone_alloc::kept_with(|| g.hit.push((up as usize, id)));
            return;
        }
    }
    o2_base(up, id);
}

/// The startup released its workers (after the first present): the baseline slots sampled so far
/// get their O2 builds. Unit lock held.
unsafe fn release_base(up: *mut Unit) {
    let hit = {
        let mut g = BASE.lock().unwrap_or_else(|e| e.into_inner());
        g.released = true;
        std::mem::take(&mut g.hit)
    };
    for (unit, id) in hit {
        if unit == up as usize {
            o2_base(up, id);
        }
    }
}

/// Queues baseline slot `id` for its O2 build (once; not glue or sandbox twins, as o2_hot).
unsafe fn o2_base(up: *mut Unit, id: u32) {
    let u = &*up;
    if (id as usize) >= u.fns.len() || matches!(u.fns[id as usize].key, FnKey::Glue(..) | FnKey::Sandbox(_)) || u.cache.is_none() || !o2_on() {
        return;
    }
    {
        let mut seen = O2_SEEN.lock().unwrap_or_else(|e| e.into_inner());
        if !seen.get_or_insert_with(Default::default).insert((up as usize, id)) {
            return;
        }
    }
    u.fns[id as usize].hot_hint.store(true, Ordering::Relaxed);
    {
        // under O2_QUEUE's lock: the worker checks both queues under it before it waits
        let _q = O2_QUEUE.lock().unwrap_or_else(|e| e.into_inner());
        O2_BASE.lock().unwrap_or_else(|e| e.into_inner()).push_back((up as usize, id));
        O2_WAKE.notify_one();
    }
    o2_start(up);
}

/// The O2 worker: builds `#[relax(O2)]` fns at O2 at low priority, ahead of their first call
/// where it can (the marked callees of every O2 build are queued behind it) and over their O1
/// code otherwise. The program's threads never wait for it: a call before the O2 code is
/// installed runs O1 code compiled on demand as usual. Its code is the slot's ordinary cache
/// entry, so warm launches link O2 code at the first call.
fn o2_thread(runtime: &crate::options::RuntimeOptions) {
    ensure_altstack();
    lower_worker_priority(runtime);
    loop {
        let (unit, id) = {
            let mut q = O2_QUEUE.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                if !q.is_empty() {
                    break q.remove(0);
                }
                if let Some(x) = O2_BASE.lock().unwrap_or_else(|e| e.into_inner()).pop_front() {
                    break x;
                }
                q = O2_WAKE.wait(q).unwrap_or_else(|e| e.into_inner());
            }
        };
        let _job = compile_job();
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            let begin = owner::call(owner::Class::Background, || {
                let up = SPEC_UNIT.load(Ordering::Acquire);
                if SPEC_STOP.load(Ordering::SeqCst) || up.is_null() || up as usize != unit {
                    return None;
                }
                o2_begin(up, id).map(|w| (up, w))
            });
            if let Some((up, w)) = begin {
                let done = w.run();
                owner::call(owner::Class::Finish, || o2_finish(up, done));
            }
        }));
        if r.is_err() {
            O2_STATS[2].fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// One O2 build, in three parts: its front (`o2_begin`, a unit call), the optimisation and code
/// generation off the unit (`O2Work::run`), the install (`o2_finish`, a unit call); nothing is marked in flight, so a program thread needing
/// one of these slots meanwhile builds it itself (O1) and never waits for this one.
/// Before the first present (snapshot): only slots whose front end a program thread already ran,
/// inlining only candidates that have RIR -- the lock is held just to copy RIR, never for type
/// checking or lowering -- installed but not recorded, and built again in full after the
/// present. After it (full): inline candidates' front ends run here, and the code is the slot's
/// cache entry, so warm launches link full O2 code at the first call.
unsafe fn o2_begin(up: *mut Unit, id: u32) -> Option<O2Work> {
    let _census_context = (&*up).exec_context(false);
    let _cost = crate::fncost::Record::new(id, 0, "O2");
    let t0 = cpu_ns();
    let epoch = PATCH_EPOCH.load(Ordering::SeqCst);
    let snapshot = !FIRST_PRESENT.load(Ordering::Acquire);
    let f = &(&*up).fns[id as usize];
    // O3 code installed: an O2 build after the present is still the slot's cache entry
    if f.build_err.is_some() || matches!(f.key, FnKey::Sandbox(_)) || (snapshot && f.o3.load(Ordering::Relaxed)) {
        O2_STATS[2].fetch_add(1, Ordering::Relaxed);
        return None;
    }
    if snapshot && (f.building || f.inflight || f.rir.as_ref().map_or(true, |r| r.full)) {
        // no RIR to copy yet (or a build open): its full build after the present
        crate::zone_alloc::kept_with(|| O2_LATER.lock().unwrap_or_else(|e| e.into_inner()).push((up as usize, id)));
        return None;
    }
    // its back end runs off the unit for a lazy compile now: that build first (its finish is
    // served meanwhile)
    wait_inflight(up, id);
    let f = &(&*up).fns[id as usize];
    let lowered = f.rir.as_ref().map(|r| (**r).clone());
    if begin_build(up, id).is_err() {
        O2_STATS[2].fetch_add(1, Ordering::Relaxed);
        return None;
    }
    let depth0 = BUILD_STACK.try_with(|s| s.borrow().len()).unwrap_or(0);
    O2_BUILD.with(|c| c.set(true));
    O2_SNAPSHOT.with(|c| c.set(snapshot));
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| prepare_rir_from(up, id, true, lowered, RirMode::Normal)));
    O2_BUILD.with(|c| c.set(false));
    O2_SNAPSHOT.with(|c| c.set(false));
    if r.is_err() {
        let open: Vec<u32> = BUILD_STACK.try_with(|s| s.borrow()[depth0.min(s.borrow().len())..].to_vec()).unwrap_or_default();
        for x in open.into_iter().rev() {
            end_build(up, x);
        }
    }
    end_build(up, id);
    let prep = match r {
        Ok(Ok((p, _))) if p.func.full => p,
        // gave way to a program thread's request: again, behind the queue
        Ok(Err(e)) if e == YIELDED => {
            O2_QUEUE.lock().unwrap_or_else(|e| e.into_inner()).push((up as usize, id));
            return None;
        }
        Err(p) if p.is::<crate::preempt::OwnerBusy>() => std::panic::resume_unwind(p),
        _ => {
            O2_STATS[2].fetch_add(1, Ordering::Relaxed);
            return None;
        }
    };
    let u = &*up;
    let session = (*std::ptr::addr_of!(u.prog.session)).clone();
    let e = &u.env;
    let env = x64::Env { slots: e.slots, tls_key: e.tls_key, tls_slow: e.tls_slow, resume: e.resume, opt_regs: true, syms: e.syms, sym_class: e.sym_class };
    let (fold, layout) = (u.opt.fold, u.opt.layout);
    Some(O2Work { id, prep, epoch, snapshot, session, env, fold, layout, t0 })
}

/// An O2 build's optimisation and code generation (no unit state).
struct O2Work {
    id: u32,
    prep: Prep,
    epoch: u64,
    snapshot: bool,
    session: std::sync::Arc<crate::Session>,
    env: x64::Env,
    fold: bool,
    layout: bool,
    t0: u64,
}

struct O2Done {
    id: u32,
    epoch: u64,
    snapshot: bool,
    t0: u64,
    built: std::thread::Result<Result<(crate::rir::Func, Vec<(u32, usize)>, x64::Compiled), String>>,
    done: Vec<Deferred>,
}

impl O2Work {
    fn run(self) -> O2Done {
        let O2Work { id, prep, epoch, snapshot, session, env, fold, layout, t0 } = self;
        let options = &session.options;
        let mut done: Vec<Deferred> = Vec::new();
        let built = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let (func, inlined) = optimize_tree(options, id, prep, &mut done)?;
            let cg = tier0_codegen_rir(options, &func, fold, layout);
            let _scratch = crate::zone_alloc::scratch(&session.runtime);
            let c = x64::compile(options, &cg, &env)?;
            Ok::<_, String>((func, inlined, c))
        }));
        O2Done { id, epoch, snapshot, t0, built, done }
    }
}

/// An O2 build's install (unit call): the inline candidates' RIR, the cache record, the code.
unsafe fn o2_finish(up: *mut Unit, d: O2Done) {
    let O2Done { id, epoch, snapshot, t0, built, done } = d;
    let stale = PATCH_EPOCH.load(Ordering::SeqCst) != epoch;
    if !stale {
        // the inline candidates' RIR, where no program thread built it meanwhile
        for (c, r) in done {
            if let Ok((f, inl)) = r {
                let x = &(&*up).fns[c as usize];
                if x.rir.is_none() && !x.building && x.build_err.is_none() {
                    store_rir(up, c, f, inl, 0);
                }
            }
        }
    }
    let u = &mut *up;
    let c = match built {
        Ok(Ok((func, inlined, c))) if !stale && {
            let n = if relax_codegen::interp::blob::is_blob(&c.code) { c.code.len() } else { crate::rir::split_unwind_info(&c.code).0 };
            u.link_check(&c.code[..n], &c.relocs)
        } => {
            // O3 starts from this RIR
            store_rir(up, id, func, inlined, 0);
            c
        }
        _ => {
            O2_STATS[2].fetch_add(1, Ordering::Relaxed);
            return;
        }
    };
    let u = &mut *up;
    if snapshot {
        crate::zone_alloc::kept_with(|| O2_LATER.lock().unwrap_or_else(|e| e.into_inner()).push((up as usize, id)));
    } else if let Some(mut cache) = u.cache.take() {
        cache.record_o2(u, id, &c.code, &c.relocs, &c.pc_map);
        u.cache = Some(cache);
    }
    if u.fns[id as usize].o3.load(Ordering::Relaxed) || crate::inspect::instrument::active(id) {
        O2_STATS[2].fetch_add(1, Ordering::Relaxed);
        return;
    }
    let first = !u.fns[id as usize].compiled;
    let len = c.code.len();
    match u.install(id, &c.code, &c.relocs, c.pc_map, 0) {
        Ok(a) => {
            if first {
                u.arm_counter(id, a);
            }
            if perf_map_on() {
                perf_map(a, len, &format!("{} [O2]", u.fns[id as usize].name));
            }
            O2_STATS[0].fetch_add(1, Ordering::Relaxed);
            let dt = cpu_ns() - t0;
            O2_STATS[1].fetch_add(dt, Ordering::Relaxed);
            if o2_trace() {
                eprintln!("makepad-run O2: {} installed{} ({}, {} bytes, {:.2} ms cpu)", u.fns[id as usize].name, if first { " ahead of its first call" } else { " over O1" }, if snapshot { "snapshot" } else { "full, cached" }, len, dt as f64 / 1e6);
            }
        }
        Err(_) => {
            O2_STATS[2].fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Marked `#[relax(O2)]` or higher: compiled at O2 from its first build, never O1.
fn marked_hot(u: &Unit, id: u32) -> bool {
    relax_level(u, id) >= 2
}

/// Source-size bound (tokens) of baseline inline candidates.
const BASE_INLINE_SRC_TOKS: u32 = 40;

/// A callee a baseline caller may inline: a Relax core fn instance small in source.
fn base_inline_wanted(u: &Unit, c: u32) -> bool {
    match &u.fns[c as usize].key {
        FnKey::Inst(d, _) => u.prog.def(*d).krate == u.core_crate && src_size(u, c) <= BASE_INLINE_SRC_TOKS,
        _ => false,
    }
}

fn inline_src_toks(options: &crate::options::Options, tier1: bool) -> u32 {
    options.inline_src.unwrap_or(if tier1 { INLINE_SRC_TOKS_WITH_TIER1 } else { INLINE_SRC_TOKS_DEFAULT })
}

crate::gen_local! {
    /// CPU time of the builds nested in each running build of this thread (innermost last):
    /// phases count exclusive time, without the callees and CTFE compiles they wait for
    static BUILD_CHILD: std::cell::RefCell<Vec<u64>>;
    /// slots this thread is building (begin_build .. end_build), innermost last
    static BUILD_STACK: std::cell::RefCell<Vec<u32>>;
}
thread_local! {
    /// a speculative worker: its fronts may suspend between bodies (prepare_rir)
    static IS_WORKER: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Nested build time so far inside the current build.
unsafe fn nested_build_ns() -> u64 {
    // try_with: a thread's TLS destructors (std's key_dtor) can compile after this thread's
    // Rust thread-locals are gone
    BUILD_CHILD.try_with(|c| c.borrow().last().copied().unwrap_or(0)).unwrap_or(0)
}

fn child_push() {
    let _ = BUILD_CHILD.try_with(|c| c.borrow_mut().push(0));
}

/// Ends a timed build of `dt` ns: returns its nested part and charges `dt` to the parent.
fn child_pop(dt: u64) -> u64 {
    BUILD_CHILD
        .try_with(|c| {
            let mut c = c.borrow_mut();
            let nested = c.pop().unwrap_or(0);
            if let Some(top) = c.last_mut() {
                *top += dt;
            }
            nested
        })
        .unwrap_or(0)
}

fn me() -> u64 {
    os::thread::current()
}

/// Marks a background driver thread (stats: its type checks ahead count as workers').
fn set_worker() {
    IS_WORKER.with(|w| w.set(true));
}

/// The calling thread's id (os thread id).
pub fn current_thread_id() -> u64 {
    me()
}

/// Slot `id` is open in this request's frame (its build is on the owner's stack above the
/// request's start): a build of it now is recursion.
fn open_here(id: u32) -> bool {
    BUILD_STACK.try_with(|s| s.borrow().contains(&id)).unwrap_or(false)
}

/// Starts building slot `id`. Err: this request is building it already (recursion). Open in an
/// earlier request lower on the owner's stack (this one was served inside its wait): this
/// request is cancelled (owner::busy_if_nested), so the slot never gets an error stub that
/// depends on that timing.
unsafe fn begin_build(up: *mut Unit, id: u32) -> Result<(), String> {
    let f = &mut (&mut *up).fns[id as usize];
    if f.building {
        // open in an earlier request lower on the owner's stack: this nested one is cancelled
        if !open_here(id) {
            owner::busy_if_nested();
        }
        return Err("recursive build".to_string());
    }
    f.building = true;
    let _ = BUILD_STACK.try_with(|s| s.borrow_mut().push(id));
    Ok(())
}

/// Waits (owner) until slot `id`'s back end, running off the unit, is finished: its Finish is
/// served meanwhile. A request served nested inside an earlier request's wait does not wait:
/// the back end may be that request's own (a patch's batch), which cannot end before this one.
unsafe fn wait_inflight(up: *mut Unit, id: u32) {
    if !(&*up).fns[id as usize].inflight {
        return;
    }
    // a slot this thread's own batch put in flight (compile_batch_off_lock): its back end starts
    // only after all fronts, so a later front of the same batch builds what it needs itself
    if BATCH_FLIGHT.with(|b| b.borrow().contains(&id)) {
        return;
    }
    owner::busy_if_nested();
    owner::pump_wait(&|| !(&*up).fns[id as usize].inflight);
}

crate::gen_local! {
    /// Slots the running compile_batch_off_lock on this thread marked in flight during its fronts.
    static BATCH_FLIGHT: std::cell::RefCell<Vec<u32>>;
}

/// Bumped whenever a build ends (reclaim_generation).
static BUILD_GEN: AtomicU64 = AtomicU64::new(0);

unsafe fn end_build(up: *mut Unit, id: u32) {
    let open = BUILD_STACK.try_with(|s| {
        let mut s = s.borrow_mut();
        match s.iter().rposition(|x| *x == id) {
            Some(i) => {
                s.truncate(i);
                true
            }
            None => false,
        }
    });
    if open.unwrap_or(false) {
        (&mut *up).fns[id as usize].building = false;
    }
    BUILD_GEN.fetch_add(1, Ordering::Release);
}

/// A served request's frame (JIT_THREADS.md §2.12): the per-request compile state of the
/// owner thread, fresh for every request it serves and restored after (also nested inside
/// another request's wait, also when the request panics).
pub(crate) struct Frame {
    build_stack: Vec<u32>,
    build_child: Vec<u64>,
    build_depth: u32,
    deferred: bool,
    o2_build: bool,
    o2_snapshot: bool,
    o3_first: bool,
    o3_first_at: u64,
    cyc_at: Vec<(u8, TyId)>,
    cyc_cut: usize,
    missing: (Vec<(u32, String)>, usize),
    rec: *const std::cell::Cell<Option<MainCompile>>,
    tainted: bool,
    taint_built: Vec<(usize, u32)>,
    busy: bool,
}

thread_local! {
    /// this request left out an inline candidate open in an earlier request lower on the
    /// owner's stack: its builds depend on timing (owner frames)
    static TAINTED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
crate::gen_local! {
    /// (unit, slot) whose RIR such a request stored: dropped when it ends (its code stays)
    static TAINT_BUILT: std::cell::RefCell<Vec<(usize, u32)>>;
}
thread_local! {
    /// a compile reached from JIT code in this request was cancelled (OwnerBusy) and got a
    /// panicking stub: the request is cancelled once a Rust frame can unwind
    static BUSY_PENDING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Enters a fresh frame for a request whose requester's compile record is at `rec`.
pub(crate) fn frame_enter(rec: *const ()) -> Frame {
    Frame {
        build_stack: BUILD_STACK.with(|s| std::mem::take(&mut *s.borrow_mut())),
        build_child: BUILD_CHILD.with(|c| std::mem::take(&mut *c.borrow_mut())),
        build_depth: BUILD_DEPTH.with(|d| d.replace(0)),
        deferred: DEFERRED_ABORT.with(|a| a.replace(false)),
        o2_build: O2_BUILD.with(|c| c.replace(false)),
        o2_snapshot: O2_SNAPSHOT.with(|c| c.replace(false)),
        o3_first: O3_FIRST.with(|c| c.replace(false)),
        o3_first_at: O3_FIRST_AT.with(|c| c.replace(0)),
        cyc_at: CYC_AT.with(|m| std::mem::take(&mut *m.borrow_mut())),
        cyc_cut: CYC_CUT.with(|c| c.replace(usize::MAX)),
        missing: crate::program::missing_save(),
        rec: MAIN_REC_AT.with(|r| r.replace(rec as *const std::cell::Cell<Option<MainCompile>>)),
        tainted: TAINTED.with(|t| t.replace(false)),
        taint_built: TAINT_BUILT.with(|b| std::mem::take(&mut *b.borrow_mut())),
        busy: BUSY_PENDING.with(|b| b.replace(false)),
    }
}

/// Leaves a request's frame: the state of the frame below it comes back. True: a compile in it
/// was cancelled and nothing turned that into the request's cancellation yet (its result is not
/// to be used).
pub(crate) fn frame_leave(f: Frame) -> bool {
    // builds a panic left open (backstop: their bookkeeping normally closes them)
    for id in BUILD_STACK.with(|s| std::mem::take(&mut *s.borrow_mut())) {
        let up = IDS_UNIT.load(Ordering::Acquire);
        let unit = unsafe { UNIT };
        for u in [unit, up] {
            if !u.is_null() && (id as usize) < unsafe { (&*u).fns.len() } {
                unsafe { (&mut *u).fns[id as usize].building = false };
                break;
            }
        }
    }
    BUILD_STACK.with(|s| *s.borrow_mut() = f.build_stack);
    BUILD_CHILD.with(|c| *c.borrow_mut() = f.build_child);
    BUILD_DEPTH.with(|d| d.set(f.build_depth));
    DEFERRED_ABORT.with(|a| a.set(f.deferred));
    O2_BUILD.with(|c| c.set(f.o2_build));
    O2_SNAPSHOT.with(|c| c.set(f.o2_snapshot));
    O3_FIRST.with(|c| c.set(f.o3_first));
    O3_FIRST_AT.with(|c| c.set(f.o3_first_at));
    CYC_AT.with(|m| *m.borrow_mut() = f.cyc_at);
    CYC_CUT.with(|c| c.set(f.cyc_cut));
    crate::program::missing_restore(f.missing);
    MAIN_REC_AT.with(|r| r.set(f.rec));
    // what a tainted request built is not kept for others to inline
    for (unit, id) in TAINT_BUILT.with(|b| std::mem::replace(&mut *b.borrow_mut(), f.taint_built)) {
        let u = unsafe { &mut *(unit as *mut Unit) };
        if !u.fns[id as usize].building {
            u.fns[id as usize].rir = None;
        }
    }
    TAINTED.with(|t| t.set(f.tainted));
    BUSY_PENDING.with(|b| b.replace(f.busy))
}

/// Amount released at settle. Counted bytes are live-heap deltas; without heap
/// accounting RIR uses the structural estimate (which excludes nested instruction operands).
#[derive(Default)]
pub struct SettleDrops {
    pub fns: usize,
    pub rir: u64,
    pub typeck: u64,
    pub lower: u64,
    pub spec: u64,
    pub max_sweep_ns: u64,
    pub front_pending: bool,
    retired_rir: Vec<std::sync::Arc<crate::rir::Func>>,
}

impl SettleDrops {
    /// Slots have relinquished ownership under the unit lock. Destroy their potentially
    /// large instruction graphs after unlocking, so a frame's lazy call need not wait.
    pub fn release_rir(&mut self) {
        let before = crate::memstat::live_total();
        if !crate::memstat::enabled() {
            self.rir += self.retired_rir.iter().filter(|r| std::sync::Arc::strong_count(r) == 1)
                .map(|r| crate::memreport::rir_bytes(r)).sum::<u64>();
        }
        self.retired_rir.clear();
        if crate::memstat::enabled() { self.rir += freed_since(before); }
    }
}

static DONE_RIR_FNS: AtomicU64 = AtomicU64::new(0);
static DONE_RIR_BYTES: AtomicU64 = AtomicU64::new(0);
static DONE_BODY_BYTES: AtomicU64 = AtomicU64::new(0);

pub fn take_completed_drops() -> SettleDrops {
    SettleDrops {
        fns: DONE_RIR_FNS.swap(0, Ordering::Relaxed) as usize,
        rir: DONE_RIR_BYTES.swap(0, Ordering::Relaxed),
        typeck: DONE_BODY_BYTES.swap(0, Ordering::Relaxed),
        ..Default::default()
    }
}

/// The cache recorder and speculation have consumed the installed slot. Backends retain
/// Arc snapshots; queued/active O3 jobs and a nested O1 build still pin this slot.
/// Current closure owners can be rechecked; old file versions must survive hot reload.
fn drop_completed_body(u: &mut Unit, id: u32) {
    if u.keep_compile_state || !u.prog.session.runtime.settle_drop || RT_KEEP.with(|k| k.get()) { return; }
    let f = &u.fns[id as usize];
    if !f.compiled || f.building || f.inflight { return; }
    // Most installed slots have no owned body, or are still reusable inline inputs.
    // Avoid touching tier queues when there is nothing this installation can release.
    let drop_rir = f.rir.is_some() && !inline_source_wanted(u, id, false);
    let drop_body = u.bodies.get(&id).is_some_and(|b| u.prog.def(b.def).file == b.file
        && b.closure_sigs.keys().all(|ce| {
            u.fn_map.get(&FnKey::Closure(b.file, *ce, id)).is_some_and(|child| {
                let child = &u.fns[child as usize];
                child.compiled || child.rir.is_some()
            })
        }));
    if !drop_rir && !drop_body { return; }
    let queue = TIER_QUEUE.lock().unwrap_or_else(|e| e.into_inner());
    let active = TIER_ACTIVE.lock().unwrap_or_else(|e| e.into_inner());
    if queue.iter().any(|x| x.0 == id) || active.contains(&id) { return; }
    let before = crate::memstat::live_total();
    // Inline candidates still have startup readers, including callers not lowered yet.
    // Their source-driven eligibility is exactly the inliner's, not a new size heuristic.
    let rir = if drop_rir { u.fns[id as usize].rir.take() } else { None };
    if let Some(r) = rir {
        let bytes = if std::sync::Arc::strong_count(&r) == 1 { crate::memreport::rir_bytes(&r) } else { 0 };
        drop(r);
        DONE_RIR_FNS.fetch_add(1, Ordering::Relaxed);
        DONE_RIR_BYTES.fetch_add(if crate::memstat::enabled() { freed_since(before) } else { bytes }, Ordering::Relaxed);
    }
    let before = crate::memstat::live_total();
    if drop_body {
        u.bodies.remove(&id);
    }
    DONE_BODY_BYTES.fetch_add(freed_since(before), Ordering::Relaxed);
}

fn freed_since(before: i64) -> u64 {
    before.saturating_sub(crate::memstat::live_total()).max(0) as u64
}

/// Settle (the run is quiet after first present): drop the type side's solver cache and the
/// query values nothing holds, which a query derives again on demand. Caller holds the unit
/// lock; FRONT_RW excludes the off-lock checkers. None when a checker is busy (retried).
pub unsafe fn retire_type_side(up: *mut Unit) -> Option<(usize, usize)> {
    // A settle-swap run waits (the owner serves while it does) for the readers that overlap
    // without a gap in a big app (Scope cold: ahead checkers and the lazy file loader), which
    // try_write never wins; writers waiting make new checks stop at their next point.
    let _front = if zone_swap::enabled() { front_write_within(150)? } else { FRONT_RW.try_write().ok()? };
    Some((*up).tcx.retire_settled())
}

/// Reclaim rebuildable compile state. Caller holds the unit lock, outside any compilation.
/// FRONT_RW excludes off-lock typeck before memo nodes can be freed. Suspended fronts pin
/// their own inputs, not the entire unit. MemoMap::get returns owned clones; no node borrow
/// crosses a front's unit-lock yield.
pub unsafe fn settle_drop(up: *mut Unit) -> SettleDrops {
    let session = unsafe { (*std::ptr::addr_of!((*up).prog.session)).clone() };
    let options = &session.options;
    let u = &mut *up;
    let started = wall_ns();
    let mut drops = take_completed_drops();
    // Freeze the pending/active sets together; the worker moves queue -> active under q.
    let queue = TIER_QUEUE.lock().unwrap_or_else(|e| e.into_inner());
    let active = TIER_ACTIVE.lock().unwrap_or_else(|e| e.into_inner());
    let mut pinned: std::collections::HashSet<u32> = queue.iter().map(|x| x.0).chain(active.iter().copied()).collect();
    // Queued jobs have not enumerated their inline snapshots yet. Their known RIR
    // dependency graph is still pending input too; dropping it would make each queued
    // caller recheck shared callees. The edges survive slot RIR eviction (warm rebuilds).
    let mut pending: Vec<u32> = pinned.iter().copied().collect();
    while let Some(id) = pending.pop() {
        for &callee in &u.fns[id as usize].rir_refs {
            if pinned.insert(callee) { pending.push(callee); }
        }
    }
    for (id, f) in u.fns.iter().enumerate() {
        if f.building {
            pinned.insert(id as u32);
            // prepare_rir records its callees before yielding between their builds, and
            // snapshots their RIR only after that loop. Keep those not-yet-snapshotted inputs.
            pinned.extend(f.calls.iter().copied());
        }
    }
    for mut id in pinned.iter().copied().collect::<Vec<_>>() {
        while let FnKey::Closure(_, _, parent) = u.fns[id as usize].key {
            if parent == u32::MAX { break; }
            pinned.insert(parent);
            id = parent;
        }
    }
    for (id, f) in u.fns.iter_mut().enumerate() {
        if f.compiled && !f.inflight && !pinned.contains(&(id as u32)) {
            // Arc snapshots remain valid even after the slot drops its ownership.
            if let Some(r) = f.rir.take() {
                drops.fns += 1;
                drops.retired_rir.push(r);
            }
        }
    }
    let fns = &u.fns;
    let eligible = |id: u32| {
        let f = &fns[id as usize];
        f.compiled && !f.inflight && f.rir.is_none() && !pinned.contains(&id)
    };
    let mut defs = crate::fxhash::HashSet::default();
    for (id, f) in u.fns.iter().enumerate() {
        if eligible(id as u32) {
            if let FnKey::Inst(d, _) = &f.key { defs.insert(*d); }
        }
    }
    // A polymorphic body is shared by all instances: keep it if any instance is pinned.
    for (id, f) in u.fns.iter().enumerate() {
        if !eligible(id as u32) {
            if let FnKey::Inst(d, _) = &f.key { defs.remove(d); }
        }
    }
    let before = crate::memstat::live_total();
    // Current bodies can be checked again. Old file versions cannot: closures still refer to
    // their original expression IDs, so keep old_bodies and any old-version current body.
    let fns = &u.fns;
    let prog = &u.prog;
    u.bodies.retain(|id, b| {
        let f = &fns[*id as usize];
        !(f.compiled && !f.inflight && f.rir.is_none() && !pinned.contains(id) && prog.def(b.def).file == b.file)
    });
    u.bodies.shrink_to_fit();
    *u.ctfe_fn_bodies.lock().unwrap_or_else(|e| e.into_inner()) = Default::default();
    drops.typeck += freed_since(before);
    // RIR and checked body ownership belong to the unit, not the off-lock front.
    // A busy checker only postpones shared memo-node reclamation.
    let Ok(_front) = FRONT_RW.try_write() else {
        drops.front_pending = true;
        drops.max_sweep_ns = wall_ns().saturating_sub(started);
        return drops;
    };
    let before = crate::memstat::live_total();
    // Lcx itself is stack-scoped and already gone. These are its retained memo results.
    u.coro_rir.retain_exclusive(|id, _| !eligible(*id));
    u.unwind_drop_cache.retain_exclusive(|_, _| false);
    u.unpin_cache.retain_exclusive(|_, _| false);
    drops.lower = freed_since(before);
    drop(active);
    drop(queue);
    // Force the existing TTL sweep now, under our reentrant unit lock; never evict young
    // speculative code or change what MAKEPAD_RUN_SPEC_TTL_MS=0 means.
    let before = crate::memstat::live_total();
    expire_spec(options);
    drops.spec = freed_since(before);
    drops.max_sweep_ns = wall_ns().saturating_sub(started);
    drops
}

/// Type-checks, lowers and optimises a slot's function into `fns[id].rir`. An internal
/// compiler panic becomes this function's compile error (Relax itself never goes down on bad
/// input).
/// Instances whose type arguments nest deeper than this are not built: polymorphic recursion
/// that grows its types (`convert::<Wrap<S>>()` inside `convert::<S>`) never ends otherwise
/// (rustc: "reached the recursion limit while instantiating", recursion_limit = 128).
pub const INSTANCE_DEPTH_LIMIT: u32 = 128;

pub unsafe fn build_rir(up: *mut Unit, id: u32) -> Result<(), String> {
    build_rir_mode(up, id, RirMode::Tier0)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RirMode {
    Tier0,
    Baseline,
    Normal,
}

/// O3 consumes normal O1 RIR, including its small inline candidates. A
/// baseline machine-code body stays installed until hotness replaces it; only
/// the saved optimization input is upgraded here, reusing the existing lowering.
unsafe fn build_rir_mode(up: *mut Unit, id: u32, mode: RirMode) -> Result<(), String> {
    if let FnKey::Inst(_, args) = &(&*up).fns[id as usize].key {
        let u = &*up;
        if args.iter().any(|t| u.tcx.tys.depth(*t) > INSTANCE_DEPTH_LIMIT) {
            return Err(format!("reached the recursion limit while instantiating `{}` (type arguments nested deeper than {})", u.fns[id as usize].name, INSTANCE_DEPTH_LIMIT));
        }
    }
    // record where an internal panic happened (no stderr noise; the report carries it)
    static ICE_AT: relax_session::sync::Mutex<String> = relax_session::sync::Mutex::new(String::new());
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(|info| {
        if let Some(l) = info.location() {
            if let Ok(mut g) = ICE_AT.lock() {
                *g = crate::zone_alloc::kept_with(|| format!("{}:{}", l.file().rsplit('/').next().unwrap_or(""), l.line()));
            }
        }
    }));
    let depth0 = BUILD_STACK.try_with(|s| s.borrow().len()).unwrap_or(0);
    let child0 = BUILD_CHILD.try_with(|c| c.borrow().len()).unwrap_or(0);
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| build_rir_unguarded(up, id, mode)));
    std::panic::set_hook(prev);
    if r.is_err() {
        // the unwound builds' bookkeeping
        let open: Vec<u32> = BUILD_STACK.try_with(|s| s.borrow()[depth0.min(s.borrow().len())..].to_vec()).unwrap_or_default();
        for x in open.into_iter().rev() {
            end_build(up, x);
        }
        let _ = BUILD_CHILD.try_with(|c| c.borrow_mut().truncate(child0));
    }
    match r {
        Ok(r) => r,
        Err(p) => {
            if crate::preempt::is_preempted(&*p) || p.is::<crate::preempt::OwnerBusy>() { std::panic::resume_unwind(p); }
            // a constant it evaluated needs a module not loaded yet (const_eval_to_valtree)
            if p.is::<crate::preempt::NeedsLoad>() { return Err(DEFERRED_RETRY.to_string()); }
            let msg = if let Some(s) = p.downcast_ref::<&str>() {
                s.to_string()
            } else if let Some(s) = p.downcast_ref::<String>() {
                s.clone()
            } else {
                "panic".to_string()
            };
            let u = &mut *up;
            let at = ICE_AT.lock().map(|g| g.clone()).unwrap_or_default();
            Err(if let Some(fatal) = p.downcast_ref::<crate::infer::diagnostics::FatalError>() { fatal.0.clone() } else { format!("internal compiler error in {} (makepad run {}): {} -- makepad run bug, report it", u.fns[id as usize].name, at, msg) })
        }
    }
}

unsafe fn build_rir_unguarded(up: *mut Unit, id: u32, mode: RirMode) -> Result<(), String> {
    let normal = mode == RirMode::Normal;
    let mut lowered = None;
    {
        let u = &mut *up;
        if let Some(r) = &u.fns[id as usize].rir {
            let to_o2 = normal && o2_build() && !r.full;
            if !normal || !(r.baseline || to_o2) || u.fns[id as usize].building || u.fns[id as usize].inflight {
                return Ok(());
            }
            lowered = Some((**r).clone());
        }
        if let Some(e) = &u.fns[id as usize].build_err {
            return Err(e.to_string());
        }
    }
    if let FnKey::Sandbox(base) = (&*up).fns[id as usize].key {
        if normal { build_rir_mode(up, base, RirMode::Normal)?; }
        let f = crate::sandbox::twin_rir(up, id, base)?;
        (&mut *up).set_rir(id, f);
        return Ok(());
    }
    if lowered.is_none() && (&*up).fns[id as usize].rir.is_some() {
        return Ok(());
    }
    if let Some(e) = &(&*up).fns[id as usize].build_err {
        return Err(e.to_string());
    }
    begin_build(up, id)?;
    // `building` stays set until the RIR is stored: inlining builds callees, which may lead
    // back here (derived impls of mutually recursive types)
    // exclusive build time: builds nest (inlining builds callees)
    child_push();
    let t0 = cpu_ns();
    let res = build_rir_body(up, id, lowered, mode);
    let dt = cpu_ns() - t0;
    let nested = child_pop(dt);
    (&mut *up).fns[id as usize].build_ns = dt.saturating_sub(nested);
    end_build(up, id);
    res
}

unsafe fn build_rir_body(up: *mut Unit, id: u32, lowered: Option<crate::rir::Func>, mode: RirMode) -> Result<(), String> {
    let _census_context = (&*up).exec_context(false);
    let _census = crate::exec_census::Record::new(id, 0, "ahead", true);
    let session = unsafe { (*std::ptr::addr_of!((*up).prog.session)).clone() };
    let options = &session.options;
    let prep = prepare_rir_from(up, id, false, lowered, mode)?;
    let t0 = cpu_ns();
    let (func, inlined) = optimize_rir(options, id, prep.0)?;
    store_rir(up, id, func, inlined, (cpu_ns() - t0) + prep.1);
    Ok(())
}

/// What the optimisation of one function needs, taken under the unit lock: its lowered RIR,
/// the options, the inlinable callees' RIR (shared), the read-only data range. With it,
/// `optimize_rir` runs without the lock (pipelined compiles, compile_split).
pub struct Prep {
    func: crate::rir::Func,
    cfg: crate::opt::OptCfg,
    callees: Vec<(u32, std::sync::Arc<crate::rir::Func>, u8)>,
    ro: Option<crate::opt::RoReader>,
    relayout: bool,
    name: String,
    /// inline candidates whose own optimisation is deferred to the back end (split compiles):
    /// (slot, inline attributes if inlinable, their preparation)
    deferred: Vec<(u32, Option<u8>, Prep)>,
}

/// Type check + lower `id`, and build the RIR of the callees O1 may inline (under the
/// lock). Returns the preparation and the cpu ns of inline preparation outside nested builds.
unsafe fn prepare_rir(up: *mut Unit, id: u32, defer: bool) -> Result<(Prep, u64), String> {
    prepare_rir_from(up, id, defer, None, RirMode::Tier0)
}

unsafe fn prepare_rir_from(up: *mut Unit, id: u32, defer: bool, lowered: Option<crate::rir::Func>, mode: RirMode) -> Result<(Prep, u64), String> {
    let _census_context = (&*up).exec_context(crate::exec_census::active() && crate::exec_profile::is_call(up as usize, id));

    let _why = why_function(&*up, id, "lower-fn");
    let _cost = crate::fncost::Record::ensure(id, 0, "ahead");
    cost_name(up, id);
    let _cost_opt = crate::fncost::phase(4);
    let session = unsafe { (*std::ptr::addr_of!((*up).prog.session)).clone() };
    let options = &session.options;
    // Freeze the mode before lowering, which may release the lock or cross present.
    // (real-time code is never baseline: it runs against a device's deadline)
    let baseline = mode == RirMode::Baseline || mode == RirMode::Tier0 && !o2_build() && !RT_KEEP.with(|k| k.get()) && match options.t0_baseline {
        crate::options::Tier0Baseline::Off => false,
        crate::options::Tier0Baseline::Startup => startup_phase() || (&*up).fns[id as usize].baseline_hint.load(Ordering::Relaxed),
        crate::options::Tier0Baseline::Always => true,
    };
    let r = match lowered { Some(f) => Ok(f), None => build_rir_inner(up, id) };
    if let Err(e) = &r {
        (&mut *up).reload_errors.insert(id);
        (&mut *up).fns[id as usize].build_err = Some(e.as_str().into());
    }
    let mut func = r?;
    crate::fncost::size(true, &func);
    // Keep the wrapper's sandbox calls in the saved RIR, not just its O1 codegen
    // copy. O3 and callers inlining this wrapper must also call the twins; base
    // slots of module functions are RIR sources, not patchable host implementations.
    if let FnKey::Inst(d, _) = &(&*up).fns[id as usize].key {
        if crate::sandbox::is_wrapper_crate((&*up).prog.def(*d).krate) {
            crate::sandbox::redirect_wrapper(up, &mut func);
        }
    }
    let u = &mut *up;
    func.baseline = baseline;
    // O2: a `#[relax(O2)]` fn built by the O2 worker (o2_thread) compiles as O1 does without
    // O3; its on-demand builds on the program's threads stay O1
    let full = !baseline && o2_build() && marked_hot(u, id);
    func.full = full;
    let mut cfg = u.opt;
    if baseline {
        // baseline code inlines only Relax core's tiniest callees (base_inline_wanted); user
        // fns, closures and glue stay calls
        cfg.inline = cfg.inline && base_inline_on();
        cfg.inline_all = false;
    }
    if let Some(pat) = noinl_env(options).0.as_deref() {
        // `all:<pats>`: no inlining at all in the matching callers (core builtins too)
        let (all, pat) = match pat.strip_prefix("all:") {
            Some(p) => (true, p),
            None => (false, pat),
        };
        if name_matches(&u.fns[id as usize].name, pat) {
            cfg.inline_all = false;
            if all {
                cfg.inline = false;
            }
        }
    }
    let mut nested_ns = 0u64;
    let t0 = cpu_ns();
    let mut snap = Vec::new();
    let mut deferred = Vec::new();
    let mut skipped: Vec<(u32, &str)> = Vec::new();
    // the callees chosen from the source above: only they are inline candidates -- not every
    // callee that happens to have RIR already (built for another caller, or called first), which
    // made O1 code depend on what was compiled before
    let mut wanted: Vec<u32> = Vec::new();
    // the lowered body's direct callees, kept for the callers' cycle check (inline_cycle)
    let mut callees = Vec::new();
    for b in &func.blocks {
        for i in &b.insts {
            if let crate::rir::Inst::Call(crate::rir::Callee::Fn(c), _, _) = i {
                if *c != id && !callees.contains(c) {
                    callees.push(*c);
                }
            }
        }
    }
    (&mut *up).fns[id as usize].calls = callees.as_slice().into();
    // inline candidates come from warm blocks only: a callee called only on a panic / error
    // path stays a call and its RIR is not built for it
    let warm: Vec<u32> = if cfg.inline {
        let cold = cold_blocks(&*up, &func);
        let mut w = Vec::new();
        for (bi, b) in func.blocks.iter().enumerate() {
            if cold[bi] {
                continue;
            }
            for i in &b.insts {
                if let crate::rir::Inst::Call(crate::rir::Callee::Fn(c), _, _) = i {
                    if !w.contains(c) {
                        w.push(*c);
                    }
                }
            }
        }
        w
    } else { Vec::new() };
    let inline_all = cfg.inline_all;
    let noinl_callee = noinl_env(options).1.as_deref();
    let core_hint = full || tier0_core_hint(options, (&*up).tier1.is_some());
    if full {
        // the Relax core helpers hot code calls (index, next, push...) are as hot: their RIR,
        // inlined here, and their own code are built the same way
        let u = &*up;
        for &c in &callees {
            if matches!(&u.fns[c as usize].key, FnKey::Inst(d, _) if u.prog.def(*d).krate == u.core_crate) {
                u.fns[c as usize].hot_hint.store(true, Ordering::Relaxed);
            }
        }
    }
    if cfg.inline {
        // callees small enough in source get their RIR built (bottom-up)
        for c in &callees {
            if !warm.contains(c) {
                continue;
            }
            // a background front (speculation, O2) gives way between bodies to a program
            // thread's request: it ends here and is queued again (the candidates built so far
            // keep their RIR)
            if defer && owner::serving().0 == owner::Class::Background && spec_yield(options) && owner::urgent() {
                SPEC_YIELDS.fetch_add(1, Ordering::Relaxed);
                return Err(YIELDED.to_string());
            }
            // the process is ending on another thread: this front ends here (its requester
            // then waits until the process is gone)
            if defer && owner::exit_tid() != 0 && !owner::serving_exit() {
                return Err(YIELDED.to_string());
            }
            if let FnKey::Closure(file, _, parent) = (&*up).fns[*c as usize].key {
                if !(&*up).has_body(parent, file) {
                    let _ = ensure_parent_body(up, parent, file, 0);
                }
            }
            let u = &mut *up;
            // O1 decides from the source before preparing a callee (its RIR build is the
            // cost: typeck + lower + opt of a body that may never run on its own): small bodies,
            // #[inline] ones and closures (the closures of iterator chains and combinators,
            // called from the adapter instance made for them). User code only with inline_all.
            let builtin = match &u.fns[*c as usize].key {
                FnKey::Inst(d, _) => u.prog.def(*d).krate == u.core_crate,
                // a closure body is lowered from its parent's checked body: only once that exists
                FnKey::Closure(cf, _, parent) => u.has_body(*parent, *cf),
                FnKey::Shim(_) | FnKey::Glue(..) => true,
                FnKey::Sandbox(_) => continue,
            };
            if !builtin && (!cfg.inline_all || matches!(u.fns[*c as usize].key, FnKey::Closure(..))) {
                continue;
            }
            if baseline && !base_inline_wanted(u, *c) {
                continue;
            }
            // refused callees (#[inline(never)], sandboxed, user code without inline_all,
            // MAKEPAD_RUN_NOINL) are not built for inlining: decided before the build, not after
            let want = inline_source_wanted(u, *c, full)
                && callee_attrs(&u.prog, &u.tcx, &u.fns, u.core_crate, *c, inline_all, core_hint).is_some()
                && !noinl_callee.is_some_and(|p| name_matches(&u.fns[*c as usize].name, p));
            if want && O2_SNAPSHOT.try_with(|c| c.get()).unwrap_or(false) {
                // a snapshot O2 build makes no front end under the lock: candidates with RIR only
                if u.fns[*c as usize].rir.is_some() {
                    wanted.push(*c);
                }
                continue;
            }
            if want {
                wanted.push(*c);
            }
            // a callee whose back end runs off the unit right now: wait for it (its finish is
            // served meanwhile), so what O1 inlines does not depend on timing (O1 RIR must be a
            // function of the source alone: the disk cache's keys and O3's inputs assume it)
            if want && u.fns[*c as usize].rir.is_none() && u.fns[*c as usize].inflight && u.fns[*c as usize].build_err.is_none() {
                wait_inflight(up, *c);
            }
            let u = &mut *up;
            // open: this request's own recursion leaves it out; open in an earlier request lower
            // on the owner's stack (this one was served inside its wait) it is left out too, and
            // what this request builds is not kept (no RIR for others to inline, no cache
            // record: it depends on that timing)
            if want && u.fns[*c as usize].rir.is_none() && u.fns[*c as usize].building && !open_here(*c) {
                TAINTED.with(|t| t.set(true));
            }
            let open = u.fns[*c as usize].building;
            if want && sig_dir().is_some() && u.fns[*c as usize].rir.is_none() {
                let why = if open { "own-open" } else if u.fns[*c as usize].build_err.is_some() { "err" } else { "" };
                if !why.is_empty() {
                    skipped.push((*c, why));
                }
            }
            if want && u.fns[*c as usize].rir.is_none() && !open && u.fns[*c as usize].build_err.is_none() {
                let tn = cpu_ns();
                if baseline {
                    // Keep the caller's frozen tier even if another thread presents while
                    // lowering. Already available higher-tier RIR is reused above.
                    let _ = build_rir_mode(up, *c, RirMode::Baseline);
                } else if defer {
                    // only the callee's front end now; its optimisation runs with the caller's
                    // back end, off the lock
                    let r = split_front(up, *c);
                    if matches!(&r, Err(e) if e == YIELDED) {
                        return Err(YIELDED.to_string());
                    }
                    if sig_dir().is_some() {
                        match &r {
                            Ok(Front::Codegen(_)) => skipped.push((*c, "split:codegen")),
                            Err(_) => skipped.push((*c, "split:err")),
                            _ => {}
                        }
                    }
                    if let Ok(Front::Opt(p, _)) = r {
                        let u = &*up;
                        // built (its RIR is stored) but not inlined when it calls this fn back
                        let attrs = if inline_cycle(u, id, *c) { None } else { callee_attrs(&u.prog, &u.tcx, &u.fns, u.core_crate, *c, inline_all, core_hint) };
                        if attrs.is_none() && sig_dir().is_some() && inline_cycle(u, id, *c) {
                            skipped.push((*c, "cycle"));
                        }
                        deferred.push((*c, attrs, p));
                    }
                } else {
                    let _ = build_rir(up, *c);
                }
                nested_ns += cpu_ns() - tn;
            }
            if want && !baseline && (&*up).fns[*c as usize].rir.as_ref().is_some_and(|r| r.baseline) {
                let tn = cpu_ns();
                let _ = build_rir_mode(up, *c, RirMode::Normal);
                nested_ns += cpu_ns() - tn;
            }
        }
        let u = &mut *up;
        for &c in &callees {
            if !wanted.contains(&c) {
                continue;
            }
            let attrs = match callee_attrs(&u.prog, &u.tcx, &u.fns, u.core_crate, c, inline_all, core_hint) {
                Some(a) => a,
                None => continue,
            };
            if let Some(p) = noinl_callee {
                if name_matches(&u.fns[c as usize].name, p) {
                    continue;
                }
            }
            if u.fns[c as usize].rir.is_some() && inline_cycle(u, id, c) {
                if sig_dir().is_some() {
                    skipped.push((c, "cycle"));
                }
                continue;
            }
            if let Some(r) = &u.fns[c as usize].rir {
                snap.push((c, r.clone(), attrs));
            }
        }
    }
    let u = &*up;
    // MAKEPAD_RUN_CODE_SIG_DIR: the inline candidates this preparation had (ready RIR / deferred), per
    // build, for order-independence diffs (tools/hrcheck/fe/rirdiff.py)
    if let Some(dir) = sig_dir() {
        let me = sig_name(u, id);
        let nh = me.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, c| (h ^ c as u64).wrapping_mul(0x100_0000_01b3));
        let mut t = String::new();
        let mut c: Vec<String> = snap.iter().map(|x| format!("ready {}", sig_name(u, x.0))).collect();
        c.extend(deferred.iter().map(|x| format!("deferred {}", sig_name(u, x.0))));
        c.extend(skipped.iter().map(|x| format!("skip-{} {}", x.1, sig_name(u, x.0))));
        c.sort();
        for l in c {
            t.push_str(&l);
            t.push('\n');
        }
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(std::path::Path::new(&dir).join(format!("{:016x}.prep", nh))) {
            let _ = write!(f, "== {}\n{}", me, t);
        }
    }
    let name = u.fns[id as usize].name.to_string();
    let relayout = layout_for(options, &name);
    let prep_ns = (cpu_ns() - t0).saturating_sub(nested_ns);
    Ok((Prep { func, cfg, callees: snap, ro: u.ro_reader(), relayout, name, deferred }, prep_ns))
}

/// Callee `c` calls caller `id` back directly (mutual recursion: derived impls of mutually
/// recursive types, drop glue). Neither is inlined into the other, whichever is built first:
/// before, the one built inside the other's open build left its caller out ("own-open") and the
/// other inlined it, so O1 code followed the compile order (counter: PartialEq::eq of
/// ScriptPodTypeData inlined ScriptPodTy's in 2 of 8 runs; cache verify gate,
/// gc::mark_transient_bodies). Longer cycles still fall back to the open-build rule.
fn inline_cycle(u: &Unit, id: u32, c: u32) -> bool {
    u.fns[c as usize].calls.contains(&id)
}

/// Inlining + the passes after it, from a preparation alone (no unit access: callable
/// without the lock). Returns the optimised RIR and the inlined call sites (callee, rule).
/// MAKEPAD_RUN_BACKEND_BENCH=<reps> (makepad check): every function's RIR as it enters the opt
/// pipeline (after inlining) is kept, and the back end (opt pipeline, address modes, loop
/// rotation, code generation) is replayed on copies at the end -- a back-end timing free of
/// the front end and of machine noise from the compile order.
fn backend_bench_on() -> bool {
    static B: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *B.get_or_init(|| std::env::var_os("MAKEPAD_RUN_BACKEND_BENCH").is_some())
}
static BENCH_CAPTURE: relax_session::sync::Mutex<Vec<(crate::rir::Func, crate::opt::OptCfg, Option<crate::opt::RoReader>, bool)>> = relax_session::sync::Mutex::new(Vec::new());

/// Replays the captured back ends `reps` times; prints the best rep's cpu ms, per 1k fns
/// and by function size (instructions entering the pipeline).
pub unsafe fn backend_bench(up: *mut Unit) {
    let session = unsafe { (*std::ptr::addr_of!((*up).prog.session)).clone() };
    let options = &session.options;
    let reps: usize = std::env::var("MAKEPAD_RUN_BACKEND_BENCH").ok().and_then(|v| v.parse().ok()).unwrap_or(3);
    let fns = std::mem::take(&mut *BENCH_CAPTURE.lock().unwrap());
    if fns.is_empty() {
        return;
    }
    let u = &*up;
    let full = !TIER1_ENABLED.load(Ordering::Relaxed);
    let env = x64::Env { slots: u.env.slots, tls_key: u.env.tls_key, tls_slow: u.env.tls_slow, resume: u.env.resume, opt_regs: full, syms: u.env.syms, sym_class: u.env.sym_class };
    let bucket = |f: &crate::rir::Func| -> usize {
        match f.blocks.iter().map(|b| b.insts.len()).sum::<usize>() {
            0..=15 => 0,
            16..=63 => 1,
            64..=255 => 2,
            256..=1023 => 3,
            _ => 4,
        }
    };
    // MAKEPAD_RUN_BACKEND_BENCH_SIZE=lo-hi: only functions entering with lo..hi instructions
    let range: (usize, usize) = std::env::var("MAKEPAD_RUN_BACKEND_BENCH_SIZE").ok().and_then(|v| {
        let (a, b) = v.split_once('-')?;
        Some((a.parse().ok()?, b.parse().ok()?))
    }).unwrap_or((0, usize::MAX));
    let fns: Vec<_> = fns.into_iter().filter(|x| (range.0..range.1).contains(&x.0.blocks.iter().map(|b| b.insts.len()).sum::<usize>())).collect();
    let mut best: Option<(u64, u64, [u64; 5], [u64; 5])> = None;
    for _ in 0..reps {
        let (mut opt_ns, mut cg_ns, mut by, mut cnt) = (0u64, 0u64, [0u64; 5], [0u64; 5]);
        for (f0, cfg, ro, relayout) in &fns {
            let k = bucket(f0);
            let t0 = cpu_ns();
            let mut f = f0.clone();
            opt_pipeline(options, &mut f, *ro, cfg, *relayout, full, &|c| f0.nounwind_inputs.iter().find(|x| x.0 == c).is_some_and(|x| x.1));
            if cfg.fold {
                crate::opt::addr_modes(&mut f);
            }
            if cfg.layout && *relayout {
                crate::opt::rotate_loops(&mut f);
            }
            let t1 = cpu_ns();
            let _ = x64::compile(options, &f, &env);
            let t2 = cpu_ns();
            opt_ns += t1 - t0;
            cg_ns += t2 - t1;
            by[k] += t2 - t0;
            cnt[k] += 1;
        }
        if best.map_or(true, |b| opt_ns + cg_ns < b.0 + b.1) {
            best = Some((opt_ns, cg_ns, by, cnt));
        }
    }
    let (o, c, by, cnt) = best.unwrap();
    let n = fns.len() as f64;
    eprintln!(
        "makepad check: back end replay ({} fns, best of {}): opt {:.0} ms + codegen {:.0} ms = {:.0} ms, {:.1} ms per 1k fns; by entering size (<16,<64,<256,<1k,more) ms {:?} fns {:?}",
        fns.len(),
        reps,
        o as f64 / 1e6,
        c as f64 / 1e6,
        (o + c) as f64 / 1e6,
        (o + c) as f64 / 1e6 / n * 1000.0,
        by.iter().map(|x| x / 1_000_000).collect::<Vec<_>>(),
        cnt
    );
}

fn optimize_rir(options: &crate::options::Options, id: u32, prep: Prep) -> Result<(crate::rir::Func, Vec<(u32, usize)>), String> {
    let _census_opt = crate::exec_census::phase(4);
    let Prep { func, cfg, callees, ro, relayout, name, deferred } = prep;
    debug_assert!(deferred.is_empty());
    if func.baseline {
        let mut func = func;
        let mut inlined = Vec::new();
        if cfg.inline && !callees.is_empty() {
            let get = |c: u32| -> Option<(*const crate::rir::Func, u8)> { callees.iter().find(|x| x.0 == c).map(|x| (&*x.1 as *const crate::rir::Func, x.2)) };
            let ti = crate::opt::pass_clock();
            // Candidates can be baseline bodies too: conservatively keep their unwind edges.
            inlined = crate::opt::inline_calls(options, &mut func, id, &get, &|_| false, &crate::opt::InlineLimits::baseline());
            crate::opt::pass_time(5, ti);
        }
        baseline_rir(options, &mut func, ro);
        // No callee facts were queried: missing nounwind answers conservatively mean false.
        verify_pass(&func, "baseline");
        if let Some(w) = debug_env().0.as_deref() {
            if dump_rir_match(&name, w) { eprintln!("{}", func.dump()); }
        }
        return Ok((func, inlined));
    }
    let queries = crate::nounwind::QueryCx::new(&callees);
    let mut capture = None;
    let result = queries.optimize(id, func, &mut |func, nounwind| {
        let mut inlined = Vec::new();
        if cfg.inline {
            let get = |c: u32| -> Option<(*const crate::rir::Func, u8)> { callees.iter().find(|x| x.0 == c).map(|x| (&*x.1 as *const crate::rir::Func, x.2)) };
            let ti = crate::opt::pass_clock();
            inlined = crate::opt::inline_calls(options, func, id, &get, nounwind, &crate::opt::InlineLimits::tier0(options));
            crate::opt::pass_time(5, ti);
        }
        static HIST_ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if backend_bench_on() {
            capture = Some(func.clone());
        }
        let t_dbg = if *HIST_ON.get_or_init(|| std::env::var_os("MAKEPAD_RUN_OPT_HIST").is_some()) { cpu_ns() } else { 0 };
        let full = !TIER1_ENABLED.load(Ordering::Relaxed) || func.full;
        opt_pipeline(options, func, ro, &cfg, relayout, full, nounwind);
        if t_dbg != 0 {
            let n: usize = func.blocks.iter().map(|b| b.insts.len()).sum();
            let bucket = match n { 0..=15 => 0, 16..=63 => 1, 64..=255 => 2, 256..=1023 => 3, 1024..=4095 => 4, _ => 5 };
            crate::opt::HIST[bucket].fetch_add(cpu_ns() - t_dbg, Ordering::Relaxed);
            crate::opt::HIST[6 + bucket].fetch_add(1, Ordering::Relaxed);
        }
        if let Some(w) = debug_env().0.as_deref() {
            if dump_rir_match(&name, w) {
                eprintln!("{}", func.dump());
            }
        }
        let tv = crate::opt::pass_clock();
        let v = func.verify();
        crate::opt::pass_time(13, tv);
        if let Err(e) = v {
            return Err(format!("RIR verify after opt: {}\n{}", e, func.dump()));
        }
        Ok(inlined)
    });
    if let (Some(mut input), Ok((optimized, _))) = (capture, &result) {
        input.nounwind_inputs = optimized.nounwind_inputs.clone();
        BENCH_CAPTURE.lock().unwrap().push((input, cfg, ro, relayout));
    }
    result
}

/// A deferred inline candidate's back-end result, stored under the lock afterwards.
type Deferred = (u32, Result<(crate::rir::Func, Vec<(u32, usize)>), String>);

/// optimize_rir after optimising the deferred inline candidates (bottom-up), which then
/// inline like candidates that were ready. Their results come back for store_rir.
fn optimize_tree(options: &crate::options::Options, id: u32, mut prep: Prep, done: &mut Vec<Deferred>) -> Result<(crate::rir::Func, Vec<(u32, usize)>), String> {
    let _cost = crate::fncost::Record::ensure(id, 0, "ahead");
    crate::fncost::size(true, &prep.func);
    let _cost_opt = crate::fncost::phase(4);
    let _mem = crate::memstat::tag(crate::memstat::Tag::Opt);
    for (c, attrs, p) in std::mem::take(&mut prep.deferred) {
        match optimize_tree(options, c, p, done) {
            Ok((f, inl)) => {
                if let Some(a) = attrs {
                    let f = std::sync::Arc::new(f);
                    prep.callees.push((c, f.clone(), a));
                    done.push((c, Ok(((*f).clone(), inl))));
                } else {
                    done.push((c, Ok((f, inl))));
                }
            }
            Err(e) => done.push((c, Err(e))),
        }
    }
    optimize_rir(options, id, prep)
}

/// Slots of a preparation's deferred candidates (marked in flight while off the lock).
fn deferred_slots(prep: &Prep, out: &mut Vec<u32>) {
    for (c, _, p) in &prep.deferred {
        out.push(*c);
        deferred_slots(p, out);
    }
}

/// Records an optimised RIR (under the lock): inlining links, stats, the slot's RIR.
unsafe fn store_rir(up: *mut Unit, id: u32, func: crate::rir::Func, inlined: Vec<(u32, usize)>, opt_ns: u64) {
    let u = &mut *up;
    note_body_deps(u, id, inlined.iter().map(|x| x.0).chain(func.nounwind_inputs.iter().filter(|x| x.1).map(|x| x.0)));
    for (c, rule) in inlined {
        u.exec_event(c, "inline");
        if let Some(w) = debug_env().1.as_deref() {
            if u.fns[c as usize].name.contains(w) {
                eprintln!("inlined {} into {}", u.fns[c as usize].name, u.fns[id as usize].name);
            }
        }
        u.stats.inline_rules[rule] += 1;
        if !u.fns[c as usize].history().inlined_into.contains(&id) {
            crate::zone_alloc::kept_with(|| u.fns[c as usize].history.get_or_insert_with(Default::default).inlined_into.push(id));
        }
    }
    u.stats.opt_ns += opt_ns;
    u.stats.rir_insts += crate::opt::inst_count(&func) as u64;
    u.set_rir(id, func);
    if TAINTED.with(|t| t.get()) {
        TAINT_BUILT.with(|b| b.borrow_mut().push((up as usize, id)));
    }
}

/// The passes after inlining (O1 and O3; `ro`: the read-only data loads fold from).
fn pipeline_variant(options: &crate::options::Options) -> u32 {
    options.pipe
}

/// `full`: the second SROA round (after the first simplify) -- O3, or O1 when it is
/// the only tier; with O3 on, O1 runs one round (counter opt CPU -20%)
/// MAKEPAD_RUN_COMPILE_WATCH=<seconds>: a watcher thread prints every optimisation that has been
/// running longer than that (function, instructions, thread) -- finds a compile that does not
/// finish without a debugger.
pub fn compile_watch(what: Option<(&str, usize)>) {
    match &what {
        Some((n, _)) => crate::tui::item_begin(n),
        None => crate::tui::item_end(),
    }
    static ON: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    let secs = *ON.get_or_init(|| {
        let s = std::env::var("MAKEPAD_RUN_COMPILE_WATCH").ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
        if s > 0 {
            { let _k = crate::zone_alloc::kept();
            std::thread::spawn(move || loop {
                std::thread::sleep(std::time::Duration::from_secs(s));
                let g = WATCH.lock().unwrap_or_else(|e| e.into_inner());
                let steps = WATCH_STEPS.load(Ordering::Relaxed);
                if steps == WATCH_PREV.swap(steps, Ordering::Relaxed) {
                    // no progress at all: ask the thread that made the last step where it is
                    WATCH_DUMP.store(true, Ordering::SeqCst);
                    let owner = owner::owner_tid();
                    let n = WATCH_ROUND.fetch_add(1, Ordering::Relaxed);
                    let target = if n % 2 == 1 && owner != 0 { owner } else { WATCH_LAST.load(Ordering::Relaxed) };
                    eprintln!("makepad-run watch: owner thread {:#x} serving {}, last stepping thread {:#x}; asking {:#x}", owner, owner::serving_note(), WATCH_LAST.load(Ordering::Relaxed), target);
                    os::thread::kick(target);
                }
                eprintln!("makepad-run watch: {} steps so far; now: {}", WATCH_STEPS.load(Ordering::Relaxed), g.iter().map(|x| format!("{:?} {} ({} s)", x.0, x.1 .0, x.1 .2.elapsed().as_secs())).collect::<Vec<_>>().join(" | "));
                for (t, (n, k, at, pt)) in g.iter() {
                    if at.elapsed().as_secs() >= s {
                        eprintln!("makepad-run watch: {:?} optimising {} ({} insts) for {} s", t, n, k, at.elapsed().as_secs());
                        if at.elapsed().as_secs() >= 2 * s {
                            WATCH_DUMP.store(true, Ordering::SeqCst);
                            os::thread::kick(*pt);
                        }
                    }
                }
            });
            }
        }
        s
    });
    if secs > 0 {
        WATCH_LAST.store(os::thread::current(), Ordering::Relaxed);
        // the OS thread id: std::thread::current() panics during TLS teardown (key_dtor compiles)
        let me = os::thread::current();
        let mut g = WATCH.lock().unwrap_or_else(|e| e.into_inner());
        match what {
            Some((name, insts)) => {
                let _k = crate::zone_alloc::kept();
                WATCH_STEPS.fetch_add(1, Ordering::Relaxed);
                let e = (name.to_string(), insts, std::time::Instant::now(), os::thread::current());
                match g.iter_mut().find(|x| x.0 == me) {
                    Some(x) => x.1 = e,
                    None => g.push((me, e)),
                }
            }
            None => g.retain(|x| x.0 != me),
        }
    }
}
static WATCH: relax_session::sync::Mutex<Vec<(u64, (String, usize, std::time::Instant, u64))>> = relax_session::sync::Mutex::new(Vec::new());
/// set while the watcher signals a stuck thread for its pc (the watchdog signal's handler)
pub(crate) static WATCH_DUMP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static WATCH_STEPS: AtomicU64 = AtomicU64::new(0);
static WATCH_PREV: AtomicU64 = AtomicU64::new(u64::MAX);
static WATCH_LAST: AtomicU64 = AtomicU64::new(0);
static WATCH_ROUND: AtomicU64 = AtomicU64::new(0);

/// Lowering already produces legal RIR for both backends. Only oversized stack
/// aggregates require rewriting here: constructing them through temporary copies
/// can overflow application threads' stacks even before their code becomes hot.
fn baseline_rir(options: &crate::options::Options, func: &mut crate::rir::Func, ro: Option<crate::opt::RoReader>) {
    if !func.slots.iter().any(|s| s.size >= BIG_SLOT) {
        return;
    }
    // Drop-flag branches and address copies obscure aggregate destinations. Simplify
    // only these exceptional bodies, without CSE, inlining, SROA or loop passes. (The passes
    // move SlotAddrs: the slots are no longer `born`.)
    for s in func.slots.iter_mut() {
        s.born = false;
    }
    crate::opt::simplify(options, func, ro, true, false);
    for _ in 0..3 {
        let mut changed = crate::opt::forward_slot_dests(func);
        changed |= crate::opt::forward_slot_dests_x(options, func, BIG_SLOT);
        changed |= crate::opt::forward_slot_copies(func);
        if !changed { break; }
        crate::opt::simplify(options, func, ro, true, false);
    }
    crate::opt::compact_slots(func);
}

/// Baseline codegen borrows the saved RIR directly. Normal O1 keeps its
/// addressing/layout rewrites separate from the immutable O3 input.
fn tier0_codegen_rir<'a>(options: &crate::options::Options, rir: &'a crate::rir::Func, fold: bool, rotate: bool) -> std::borrow::Cow<'a, crate::rir::Func> {
    if rir.baseline { return std::borrow::Cow::Borrowed(rir); }
    let mut cg = rir.clone();
    if fold { crate::opt::addr_modes(&mut cg); }
    if rotate && layout_for(options, &cg.name) { crate::opt::rotate_loops(&mut cg); }
    std::borrow::Cow::Owned(cg)
}

fn opt_pipeline(options: &crate::options::Options, func: &mut crate::rir::Func, ro: Option<crate::opt::RoReader>, cfg: &crate::opt::OptCfg, relayout: bool, full: bool, nounwind: &dyn Fn(u32) -> bool) {
    let _mem = crate::memstat::tag(crate::memstat::Tag::Opt);
    compile_watch(Some((&func.name, crate::opt::inst_count(func))));
    let mps = crate::memstat::peak_start();
    opt_pipeline_inner(options, func, ro, cfg, relayout, full, nounwind);
    crate::memstat::note_big("opt", &func.name, mps, || cg_shape(func));
    compile_watch(None);
}

fn opt_pipeline_inner(options: &crate::options::Options, func: &mut crate::rir::Func, ro: Option<crate::opt::RoReader>, cfg: &crate::opt::OptCfg, relayout: bool, full: bool, nounwind: &dyn Fn(u32) -> bool) {
    verify_pass(func, "input (lowering / inlining)");
    // MAKEPAD_RUN_DUMP_INPUT=<name suffix>: the RIR entering the optimiser
    if let Ok(w) = std::env::var("MAKEPAD_RUN_DUMP_INPUT") {
        if func.name.ends_with(w.as_str()) {
            eprintln!("{}", func.dump());
        }
    }
    // calls of callees learnt not to unwind, and empty pads: plain calls
    crate::opt::demote_invokes(func, nounwind);
    if cfg.sroa {
        crate::opt::forward_const_slots(func, ro);
        verify_pass(func, "forward_const_slots");
    }
    if cfg.fold {
        if pipeline_variant(options) >= 1 {
            // SROA on the lowered code first: one simplify (with CSE) then sees the scalars
            let t = crate::opt::pass_clock();
            if full && cfg.sroa {
                crate::opt::forward_slot_copies(func);
                verify_pass(func, "forward_slot_copies");
                crate::opt::forward_slot_loads(func);
                verify_pass(func, "forward_slot_loads");
            }
            let s1 = cfg.sroa && crate::opt::sroa(func);
            crate::opt::pass_time(8, t);
            verify_pass(func, "sroa");
            if s1 {
                let t = crate::opt::pass_clock();
                crate::opt::live_dce(func);
                crate::opt::pass_time(9, t);
                verify_pass(func, "live_dce");
            }
            crate::opt::simplify(options, func, ro, cfg.imm, cfg.cse);
            verify_pass(func, "simplify");
            // big values moved through temporaries (a struct literal with a large array, then
            // Box::new): build them in place, at every tier -- these frames overflow threads'
            // stacks, not only O3's
            // Rounds: a chain of whole copies (slot -> temp -> temp -> result buffer, as lowering
            // moves a struct literal through `Self { .. }`, a let and the return) collapses one
            // link per forwarding, and the drop-flag tests the moves left behind fold only once
            // the copies sit in one block (Score: SamplerBackend::new kept its 108 KB value in a
            // slot, copied into the result buffer, on the CoreAudio IO thread's 512 KB stack)
            for _ in 0..3 {
                if !(cfg.sroa && func.slots.iter().any(|s| s.size >= BIG_SLOT)) {
                    break;
                }
                let mut ch = crate::opt::forward_slot_dests(func);
                verify_pass(func, "forward_slot_dests");
                ch |= crate::opt::forward_slot_dests_x(options, func, BIG_SLOT);
                verify_pass(func, "forward_slot_dests_x");
                if !full {
                    ch |= crate::opt::forward_slot_copies(func);
                    verify_pass(func, "forward_slot_copies");
                }
                if !ch {
                    break;
                }
                crate::opt::simplify(options, func, ro, cfg.imm, false);
                verify_pass(func, "simplify");
            }
            let t = crate::opt::pass_clock();
            // copy prop has merged the slot address copies: chains the first pass could not
            // follow
            let mut fw = cfg.sroa && crate::opt::forward_const_slots(func, ro);
            verify_pass(func, "forward_const_slots");
            if full && cfg.sroa {
                fw |= crate::opt::forward_slot_copies(func);
                verify_pass(func, "forward_slot_copies");
                fw |= crate::opt::forward_slot_loads(func);
                verify_pass(func, "forward_slot_loads");
            }
            let s2 = full && pipeline_variant(options) == 1 && cfg.sroa && crate::opt::sroa(func);
            crate::opt::pass_time(8, t);
            verify_pass(func, "sroa");
            if s2 {
                let t = crate::opt::pass_clock();
                crate::opt::live_dce(func);
                crate::opt::pass_time(9, t);
                verify_pass(func, "live_dce");
            }
            if s2 || fw {
                crate::opt::simplify(options, func, ro, cfg.imm, false);
                verify_pass(func, "simplify");
            }
            let t = crate::opt::pass_clock();
            let l = cfg.licm && crate::opt::licm(func);
            crate::opt::pass_time(10, t);
            verify_pass(func, "licm");
            if l {
                crate::opt::simplify(options, func, ro, cfg.imm, false);
                verify_pass(func, "simplify");
            }
        } else {
            crate::opt::simplify(options, func, ro, cfg.imm, cfg.cse);
            verify_pass(func, "simplify");
            if cfg.sroa && crate::opt::sroa(func) {
                verify_pass(func, "sroa");
                // the cells' entry zeros that a write always precedes are dead by liveness only
                crate::opt::live_dce(func);
                verify_pass(func, "live_dce");
                crate::opt::simplify(options, func, ro, cfg.imm, cfg.cse);
                verify_pass(func, "simplify");
            }
            if cfg.licm && crate::opt::licm(func) {
                verify_pass(func, "licm");
                crate::opt::simplify(options, func, ro, cfg.imm, cfg.cse);
                verify_pass(func, "simplify");
            }
        }
    }
    let ts = crate::opt::pass_clock();
    if cfg.fold {
        crate::opt::sink_consts(func);
        verify_pass(func, "sink_consts");
    }
    crate::opt::pass_time(6, ts);
    let ts = crate::opt::pass_clock();
    if cfg.layout && relayout {
        crate::opt::relayout(func);
        verify_pass(func, "relayout");
    }
    crate::opt::pass_time(7, ts);
    // slots with disjoint lifetimes share stack where the backend lays out the frame
    // (opt::slot_layout, every tier)
    crate::opt::compact_slots(func);
}

/// slots from this size up make O1 forward big copies too
const BIG_SLOT: u32 = 4096;

/// MAKEPAD_RUN_VERIFY_PASSES=1: verify the RIR after every pass (names the pass that broke it).
fn verify_pass(func: &crate::rir::Func, pass: &str) {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ON.get_or_init(|| std::env::var("MAKEPAD_RUN_VERIFY_PASSES").is_ok()) {
        if let Err(e) = func.verify() {
            eprintln!("makepad: RIR broken by {}: {}\n{}", pass, e, func.dump());
        }
    }
}

/// MAKEPAD_RUN_TRACE=typeck:<fn substring>,lower:<fn substring>: print each matching phase start
/// (a debugger-free way to see where the front end is).
fn trace_on() -> bool {
    static T: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *T.get_or_init(|| std::env::var("MAKEPAD_RUN_TRACE").is_ok())
}

fn trace(phase: &str, name: &str) {
    static SPEC: std::sync::OnceLock<Vec<(String, String)>> = std::sync::OnceLock::new();
    let spec = SPEC.get_or_init(|| {
        let _k = crate::zone_alloc::kept();
        let mut v = Vec::new();
        if let Ok(s) = std::env::var("MAKEPAD_RUN_TRACE") {
            for part in s.split(',') {
                if let Some((p, f)) = part.split_once(':') {
                    v.push((p.to_string(), f.to_string()));
                }
            }
        }
        v
    });
    for (p, f) in spec {
        if p == phase && name.contains(f.as_str()) {
            eprintln!("trace: {} {}", phase, name);
        }
    }
}

/// Lowers slot `id` without installing anything (boundary.rs audit).
pub unsafe fn lower_for_audit(up: *mut Unit, id: u32) -> Result<(), String> {
    build_rir_inner(up, id).map(|_| ())
}

/// Canonical, uninlined RIR for deterministic sandbox metering. Never take the
/// tier-dependent optimized base-slot RIR as the meter's input.
pub(crate) unsafe fn sandbox_source_rir(up: *mut Unit, id: u32) -> Result<crate::rir::Func, String> {
    build_rir_inner(up, id)
}

/// The skipped modules (Rust file selection) failed builds looked into: the post-boot loader
/// loads them at the next outer compile (load_deferred).
/// (outermost build on the failing thread, (skipped module, name)): misses of failed builds.
static DEFERRED_MISSING: relax_session::sync::Mutex<Vec<(u32, (u32, String))>> = relax_session::sync::Mutex::new(Vec::new());

/// The message of a build that needs a skipped module: no error stub is installed for it,
/// its callers' builds fail too, and the outer compile loads the module and builds again.
/// Statics whose initializer failed for a module not loaded yet (by DefId): their storage
/// stays; static_addr runs the initializer again.
static DEFERRED_STATICS: relax_session::sync::Mutex<std::collections::BTreeSet<u32>> = relax_session::sync::Mutex::new(std::collections::BTreeSet::new());

/// Whether static `d`'s initializer is waiting for a module the post-boot loader has still to
/// load (its storage holds no value yet). A cached entry naming it is not linked: the slot is
/// built instead, and that build defers (DEFERRED_ABORT). Outside any build the deferral flag
/// is cleared here, the failed link being the whole of its effect.
pub(crate) fn static_deferred(d: DefId) -> bool {
    let deferred = DEFERRED_STATICS.lock().unwrap_or_else(|e| e.into_inner()).contains(&d.0);
    if deferred && BUILD_DEPTH.with(|b| b.get()) == 0 {
        DEFERRED_ABORT.with(|a| a.set(false));
    }
    deferred
}

pub const DEFERRED_RETRY: &str = "needs a module not loaded yet (the post-boot loader retries)";

thread_local! {
    /// A build on this thread ran into code that needs a skipped module (a static whose
    /// initializer failed for it): the builds up to the outer compile fail and are retried.
    static DEFERRED_ABORT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Builds in progress on this thread (a static evaluated outside any build cannot defer).
    static BUILD_DEPTH: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// One build on this thread (build_rir_inner): it starts with no miss notes of its own;
/// BUILD_DEPTH, the enclosing work's miss notes (a nested build runs inside another build's
/// lowering or type check) and its DEFERRED_ABORT come back when the build ends, also when it
/// unwinds (an internal error, NeedsLoad).
struct BuildFrame { prev_abort: bool, outer_missing: Option<(Vec<(u32, String)>, usize)> }
impl BuildFrame {
    fn enter() -> Self {
        BUILD_DEPTH.with(|d| d.set(d.get() + 1));
        BuildFrame { prev_abort: DEFERRED_ABORT.with(|a| a.replace(false)), outer_missing: Some(crate::program::missing_save()) }
    }
}
impl Drop for BuildFrame {
    fn drop(&mut self) {
        if let Some(outer) = self.outer_missing.take() { crate::program::missing_restore(outer); }
        let _ = DEFERRED_ABORT.try_with(|a| a.set(self.prev_abort || a.get()));
        let _ = BUILD_DEPTH.try_with(|d| d.set(d.get() - 1));
    }
}

unsafe fn build_rir_inner(up: *mut Unit, id: u32) -> Result<crate::rir::Func, String> {
    let frame = BuildFrame::enter();
    let r = build_rir_inner_body(up, id);
    let mine = DEFERRED_ABORT.with(|a| a.get());
    let r = if mine { r.and(Err(DEFERRED_RETRY.to_string())) } else { r };
    let r = match r {
        Err(e) => {
            let missing = crate::program::take_missing();
            if missing.is_empty() || e.starts_with(DEFERRED_RETRY) { Err(e) } else {
                // keyed by the outermost build of this thread: its compile loads them
                let key = BUILD_STACK.try_with(|s| s.borrow().first().copied()).ok().flatten().unwrap_or(id);
                let mut g = DEFERRED_MISSING.lock().unwrap_or_else(|e| e.into_inner());
                for m in missing { let m = (key, m); if !g.contains(&m) { crate::zone_alloc::kept_with(|| g.push(m.clone())); } }
                Err(format!("{DEFERRED_RETRY}: {e}"))
            }
        }
        ok => ok,
    };
    drop(frame);
    r
}

/// Post-boot loader: loads the skipped modules failed builds needed (unit lock held, outer
/// compile: no front-end reader and no program reference on this thread's stack), collects
/// their items, and lets every failed, uninstalled build run again. False: nothing loaded.
pub unsafe fn load_deferred(up: *mut Unit, id: u32) -> bool {
    // only an outermost request loads (it takes the front's write lock)
    owner::busy_if_nested();
    // the misses of this slot's own failed build; when it has none, the failure it reports
    // came from a build of it nested in another slot's build on a speculative or ahead worker
    // (misses are keyed by that thread's outermost build): every pending miss is loaded then
    // -- each was demanded by a failed build, so loading it is never wasted
    let missing: Vec<(u32, String)> = {
        let mut g = DEFERRED_MISSING.lock().unwrap_or_else(|e| e.into_inner());
        let mine: Vec<(u32, String)> = g.iter().filter(|(k, _)| *k == id).map(|(_, m)| m.clone()).collect();
        if mine.is_empty() {
            std::mem::take(&mut *g).into_iter().map(|(_, m)| m).collect()
        } else {
            g.retain(|(k, _)| *k != id);
            mine
        }
    };
    let u = &mut *up;
    // only the skipped files whose outline can bind the missed name
    let mut mods: Vec<u32> = Vec::new();
    let trace = std::env::var_os("MAKEPAD_RUN_DEFERRED_TRACE").is_some();
    for (m, name) in &missing {
        let skipped = u.prog.skipped_files.contains_key(m);
        let declares = skipped && u.prog.skipped_declares(*m, name);
        if trace {
            eprintln!("makepad-run deferred: miss `{}` in {} ({})", name, u.prog.def_path(u.prog.mods[*m as usize].def),
                if !skipped { "not skipped" } else if declares { "load" } else { "outline does not bind it" });
        }
        if !mods.contains(m) && declares { mods.push(*m); }
    }
    if mods.is_empty() {
        return false;
    }
    load_modules(up, &mods, "demand");
    true
}

/// Loads skipped modules into the running program (unit lock held at depth 1).
unsafe fn load_modules(up: *mut Unit, mods: &[u32], why: &str) {
    let u = &mut *up;
    let mut mods: Vec<u32> = mods.iter().copied().filter(|m| u.prog.skipped_files.contains_key(m)).collect();
    if mods.is_empty() { return; }
    let t = wall_ns();
    let _fw = front_write();
    let d0 = u.prog.defs.len();
    // Loading is append-only: with no new definitions, imports can only add bindings,
    // globs or anonymous traits, or resolve a pending import. An empty/cfg-disabled file
    // with none of those changes needs no re-collection or invalidation of waiting bodies.
    let mut bindings_changed = false;
    let mut paths = Vec::new();
    let mut loaded = 0;
    let first_file = u.prog.files.len() as u32;
    let defs = loop {
        let f0 = u.prog.files.len() as u32;
        for m in &mods {
            if let Some((path, ..)) = u.prog.skipped_files.get(m) { paths.push(path.clone()); }
            match u.prog.load_deferred_module_changes(*m) {
                Ok((_, changed)) => bindings_changed |= changed,
                Err(e) => {
                    bindings_changed = true; // collection may have published part of the file
                    eprintln!("makepad: loading a deferred module failed: {}", e);
                }
            }
        }
        u.prog.link_deferred_modules(f0, d0);
        loaded += mods.len();
        u.prog.reclaim_caches();
        let defs: Vec<DefId> = (d0 as u32..u.prog.defs.len() as u32).map(DefId).collect();
        if defs.is_empty() && !bindings_changed { break defs; }
        crate::program::collect_misses_begin();
        // the transaction's files are seen by their crates and those crates' dependents only
        // (lead decision 52 R1, as a declaration edit's reload): every other crate's
        // declarations and query results stay
        let visible = u.prog.crates_seeing_files(first_file..u.prog.files.len() as u32);
        let errors = u.tcx.collect_loaded(&u.prog, &defs, &visible);
        // a loaded item's signature, field or impl header names a type in another skipped
        // file: load that too and collect the transaction's items again
        mods = u.prog.misses_to_load(&crate::program::collect_misses_take());
        if !mods.is_empty() {
            let impls: Vec<DefId> = defs.iter().copied().filter(|d| u.prog.def(*d).kind == DefKind::Impl).collect();
            u.tcx.uncollect_impls(&impls);
            continue;
        }
        // collect_added also drains diagnostics of background checks: report the loaded files'
        for e in errors {
            if paths.iter().any(|p| e.starts_with(p.as_str())) { eprintln!("makepad: {}", e); }
        }
        break defs;
    };
    let resigned = u.tcx.forget_error_signatures();
    // builds that failed for want of these names (here or on a speculative worker) run again
    for f in u.fns.iter_mut() {
        if !f.compiled { f.build_err = None; }
    }
    DEFERRED_LOADS.fetch_add(loaded as u64, Ordering::Relaxed);
    if std::env::var_os("MAKEPAD_RUN_DEFERRED_TRACE").is_some() {
        eprintln!("makepad-run deferred: {} loaded {} module(s) ({} defs, {} errored signatures reset) in {:.1} ms on {}{}: {}", why, loaded, defs.len(), resigned,
            (wall_ns() - t) as f64 / 1e6, if MAIN_TID.load(Ordering::Relaxed) == me() { "main" } else { "worker" },
            if FIRST_PRESENT.load(Ordering::Acquire) { " after present" } else { " before present" }, paths.join(", "));
    }
}

pub fn start_completion(up: *mut Unit, runtime: &crate::options::RuntimeOptions) {
    if std::env::var("MAKEPAD_RUN_LAZY_COMPLETE").is_ok_and(|v| v == "0") { return; }
    if unsafe { (*up).prog.skipped_files.is_empty() } { return; }
    let upn = up as usize;
    let runtime = crate::zone_alloc::kept_clone(runtime);
    { let _k = crate::zone_alloc::kept();
    let _ = std::thread::Builder::new().name("run-complete".into()).spawn(move || { let _z = crate::zone_alloc::compiler_thread();
        let up = upn as *mut Unit;
        while !FIRST_PRESENT.load(Ordering::Acquire) {
            if crate::preempt::is_stopped() { return; }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        lower_worker_priority(&runtime);
        set_worker();
        {
            let _r = FRONT_RW.read().unwrap_or_else(|e| e.into_inner());
            unsafe { (*up).prog.warm_skipped_indexes() };
        }
        loop {
            // the process is ending: the exit records the program with what is still skipped
            if owner::exit_tid() != 0 { break; }
            while RELOADING.load(Ordering::Acquire) > 0 { std::thread::sleep(std::time::Duration::from_millis(5)); }
            if crate::preempt::is_stopped() { return; }
            // Like O3 admission, a file transaction waits for foreground demand
            // to become quiet. Its parse/derive work cannot release the unit
            // midway; do not start one between a quit handler's lazy calls.
            if !tier1_workers_ready() {
                std::thread::sleep(TIER1_POLL);
                continue;
            }
            // the next skipped file, and its lowering off the unit lock
            let next = {
                let _r = FRONT_RW.read().unwrap_or_else(|e| e.into_inner());
                let u = unsafe { &*up };
                u.prog.skipped_files.keys().min().copied()
            };
            let Some(m) = next else { break };
            // a hot-reload transaction goes first: a load now would make it wait for the unit
            // lock and refuse its off-lock parse (the collection epoch moved). Checked under
            // the lock: a transaction marks itself before it first takes the lock.
            // Completion is speculative: let demanded code (including the quit
            // handler and destructors) run between files (a background unit call).
            let reloading = owner::call(owner::Class::Background, || {
                let reloading = RELOADING.load(Ordering::Acquire) > 0;
                unsafe {
                    if !reloading && !crate::preempt::is_stopped() && tier1_workers_ready() && !owner::nested() && (*up).prog.skipped_files.contains_key(&m) { load_modules(up, &[m], "completion"); }
                }
                reloading
            });
            if reloading { continue; }
            std::thread::yield_now();
        }
    });
    }
}

/// `makepad check` checks every body: the skipped files are loaded first (unit lock).
pub unsafe fn load_all_skipped(up: *mut Unit) {
    owner::call(owner::Class::App, || {
        // a loaded file's own skipped `mod` children are skipped files in turn
        loop {
            let mods: Vec<u32> = (*up).prog.skipped_files.keys().copied().collect();
            if mods.is_empty() { break; }
            load_modules(up, &mods, "check");
        }
    });
}

/// `makepad check`: the skipped files of the checked crates only (their bodies are what the check
/// reads); other crates' files stay skipped unless a check needs them (check.rs loads them all
/// and checks again then).
pub unsafe fn load_skipped_of(up: *mut Unit, crates: &[u32]) {
    owner::call(owner::Class::App, || loop {
        let prog = &(*up).prog;
        let mods: Vec<u32> = prog.skipped_files.keys().copied().filter(|m| crates.contains(&prog.mods[*m as usize].krate)).collect();
        if mods.is_empty() { break; }
        load_modules(up, &mods, "check");
    });
}

/// Modules the post-boot loader loaded (run.rs reports it).
pub static DEFERRED_LOADS: AtomicU64 = AtomicU64::new(0);
/// Hot-reload work in progress (a save being read, a transaction): the completion loader waits.
pub static RELOADING: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
pub struct Reloading;
impl Reloading {
    pub fn begin() -> Self { RELOADING.fetch_add(1, Ordering::Release); Reloading }
}
impl Drop for Reloading {
    fn drop(&mut self) { RELOADING.fetch_sub(1, Ordering::Release); }
}

unsafe fn build_rir_inner_body(up: *mut Unit, id: u32) -> Result<crate::rir::Func, String> {
    let session = unsafe { (*std::ptr::addr_of!((*up).prog.session)).clone() };
    let options = &session.options;
    let f = build_rir_lowered(up, id)?;
    let u = &*up;
    let name = &u.fns[id as usize].name;
    if !name.starts_with("core::arch::") {
        return Ok(f);
    }
    // core::arch intrinsics: vector instructions in place of std's scalar bodies
    let mut consts = Vec::new();
    if let FnKey::Inst(_, args) = &u.fns[id as usize].key {
        for t in args {
            if let TyKind::Const(v) = u.tcx.tys.kind(*t) {
                // rustc_middle/src/ty/consts/valtree.rs:114-121 and int.rs:384:
                // typed scalar extraction is
                // independent of the const parameter's integer type (e.g. i32 IMM8).
                let value = v.try_to_value(&u.tcx.tys).ok_or("intrinsic const argument did not evaluate")?;
                let scalar = value.valtree.try_to_leaf().ok_or("intrinsic const argument is not scalar")?;
                let (size, signed) = match u.tcx.tys.kind(value.ty) {
                    TyKind::Int(p) => crate::types::Types::int_info(*p),
                    TyKind::Bool => (1, false),
                    TyKind::Char => (4, false),
                    _ => return Err("intrinsic const argument has a non-scalar type".into()),
                };
                let bits = scalar.to_bits(size as u8);
                let shift = 128 - size * 8;
                let value = if signed { ((bits << shift) as i128) >> shift } else { bits as i128 };
                consts.push(value as i64);
            }
        }
    }
    Ok(crate::simd::lower_intrinsic(options, name, &consts, f))
}

/// The bridge body of `d` when it is a provided crate's bound fn; Ok(None) when `d` compiles
/// from source; Err when its shim is missing or its signature differs from what rustc built (rebuild makepad).
/// `<T as Hash>::hash::<H>`, `<T as Debug>::fmt`, `<T as Display>::fmt` of a provided crate's
/// opaque type T (rustc's layout; its fields are rustc's to read, also by its own impls of
/// these traits, derived or written): forwarded to std's `core::hash::__relax_opaque_hash::<T,
/// H>` / `core::fmt::__relax_opaque_debug::<T>` / `__relax_opaque_display::<T>`, whose
/// `__relax_opaque_hash_into::<T>` / `__relax_opaque_fmt_into::<T>` are bridged here to the
/// ops `#[relax_api(handle, hash, debug, display)]` generates for T (rustc runs the impl; bytes / text come back by callback).
unsafe fn provided_fmt_hash(u: &mut Unit, d: DefId, args: &[TyId], id: u32) -> Result<Option<crate::rir::Func>, String> {
    let opaque = |u: &Unit, t: TyId| match u.tcx.tys.kind(t) {
        TyKind::Adt(ad, aa, _) if aa.is_empty() && u.provided_types.contains_key(ad) => Some(*ad),
        _ => None,
    };
    let def = u.prog.def(d);
    // the bridged helpers
    let hash_into = u.core_path(&["hash", "__relax_opaque_hash_into"]);
    let fmt_into = u.core_path(&["fmt", "__relax_opaque_fmt_into"]);
    if Some(d) == hash_into || Some(d) == fmt_into {
        let Some(ad) = args.first().and_then(|t| opaque(u, *t)) else { return Ok(None) };
        let tr = if Some(d) == hash_into { "Hash>::hash_into" } else { "Fmt>::fmt_into" };
        let key = crate::provided::norm(&format!("<{} as {}", u.prog.def_path(ad), tr));
        let krate = u.prog.def(ad).krate;
        let pc = &u.provided.iter().find(|(k, _)| *k == krate).unwrap().1;
        let Some(shim) = pc.shims.get(key.as_str()).map(|s| s.f) else {
            return Err(format!("`{}` is not provided: mark the type #[relax_api(handle, ..)] with that trait", key));
        };
        let sig = u.tcx.fn_sig(&u.prog, d).cloned().ok_or("no signature")?;
        let ps: Vec<TyId> = sig.params.iter().map(|p| u.tcx.tys.subst_erasing_regions(*p, args)).collect();
        let ret = u.tcx.tys.unit;
        let shim = u.syms.intern(crate::syms::SymKind::Provided(format!("shim:{}", key)), shim as usize as u64);
        let last = u.syms.intern(crate::syms::SymKind::Provided(format!("last_panic:{}", pc.name)), pc.last_panic as usize as u64);
        return Ok(Some(crate::lower::bridge_func_sig(u, &ps, ret, format!("{} (provided)", key), shim, last)));
    }
    // the trait impls' methods of an opaque type
    if def.kind != DefKind::AssocFn || u.prog.def(def.parent).kind != DefKind::Impl {
        return Ok(None);
    }
    let Some(&tr) = u.tcx.impl_trait.get(&def.parent) else { return Ok(None) };
    let Some(&st) = u.tcx.impl_self.get(&def.parent) else { return Ok(None) };
    let Some(_ad) = opaque(u, st) else { return Ok(None) };
    let mname = u.prog.name(d).to_string();
    let is_eq = Some(tr) == u.tcx.lang_items.get(LangItem::PartialEq) && mname == "eq"
        && u.tcx.impl_trait_args.get(&def.parent).is_some_and(|a| a.as_slice() == [st]);
    let is_clone = Some(tr) == u.tcx.lang_items.get(LangItem::Clone) && mname == "clone";
    if is_eq || is_clone {
        let ad = _ad;
        let krate = u.prog.def(ad).krate;
        let key = crate::provided::opaque_method_key(&u.prog, ad, &mname);
        let pc = &u.provided.iter().find(|(k, _)| *k == krate).unwrap().1;
        let Some(shim) = pc.shims.get(key.as_str()).map(|s| s.f) else {
            return Err(format!("`{}` is not provided: mark the type #[relax_api(handle, ..)] with that trait", key));
        };
        let r = u.tcx.tys.intern(TyKind::Ref(false, st, crate::types::RegionId::ERASED));
        let (ps, ret) = if is_eq { (vec![r, r], u.tcx.tys.bool_) } else { (vec![r], st) };
        let shim = u.syms.intern(crate::syms::SymKind::Provided(format!("shim:{}", key)), shim as usize as u64);
        let last = u.syms.intern(crate::syms::SymKind::Provided(format!("last_panic:{}", pc.name)), pc.last_panic as usize as u64);
        return Ok(Some(crate::lower::bridge_func_sig(u, &ps, ret, format!("{} (provided)", key), shim, last)));
    }
    let (helper, hargs) = if Some(tr) == u.core_path(&["hash", "Hash"]) && mname == "hash" {
        let Some(h) = args.last() else { return Ok(None) };
        (u.core_path(&["hash", "__relax_opaque_hash"]), vec![st, *h])
    } else if Some(tr) == u.core_path(&["fmt", "Debug"]) && mname == "fmt" {
        (u.core_path(&["fmt", "__relax_opaque_debug"]), vec![st])
    } else if Some(tr) == u.core_path(&["fmt", "Display"]) && mname == "fmt" {
        (u.core_path(&["fmt", "__relax_opaque_display"]), vec![st])
    } else {
        return Ok(None);
    };
    let Some(helper) = helper else { return Err("std lacks the provided-type helpers (core::hash / core::fmt __relax_opaque_*)".to_string()) };
    let sig = u.tcx.fn_sig(&u.prog, d).cloned().ok_or("no signature")?;
    let ps: Vec<TyId> = sig.params.iter().map(|p| u.tcx.tys.subst_erasing_regions(*p, args)).collect();
    let ret = u.tcx.tys.subst_erasing_regions(sig.ret, args);
    let callee = u.fn_id(FnKey::Inst(helper, hargs));
    let name = u.fns[id as usize].name.to_string();
    Ok(Some(crate::lower::forward_func(u, &ps, ret, name, callee)))
}

fn provided_bridge(u: &Unit, d: DefId) -> Result<Option<crate::rir::Func>, String> {
    let krate = u.prog.def(d).krate;
    let Some((_, pc)) = u.provided.iter().find(|(k, _)| *k == krate) else {
        return Ok(None);
    };
    if crate::provided::unbound_reason(&u.prog, &u.tcx, krate, d, &|a| u.provided_types.contains_key(&a)).is_some() {
        return Ok(None);
    }
    let key = crate::provided::key(&u.prog, &u.tcx, d);
    match pc.shims.get(key.as_str()) {
        Some(sh) => {
            let ours = crate::provided::sig(&u.prog, &u.tcx, d);
            if crate::provided::norm(sh.sig) != ours {
                return Err(format!("`{}`: rustc built it as `{}`, this source reads `{}`: {}", key, sh.sig, ours, crate::srchash::rebuild_hint()));
            }
            // every parameter and the result laid out as rustc does
            if let Some(sg) = u.tcx.fn_sig(&u.prog, d) {
                let cx = crate::layout::LayoutCx::new(&u.prog);
                let tys: Vec<TyId> = sg.params.iter().copied().chain(std::iter::once(sg.ret)).collect();
                if tys.len() != sh.slots.len() {
                    return Err(format!("`{}`: rustc records {} slots, makepad run reads {}: {}", key, sh.slots.len(), tys.len(), crate::srchash::rebuild_hint()));
                }
                for (t, slot) in tys.iter().zip(sh.slots) {
                    let empty = matches!(u.tcx.tys.kind(*t), TyKind::Never) || matches!(u.tcx.tys.kind(*t), TyKind::Tuple(v) if v.is_empty());
                    if empty {
                        continue;
                    }
                    let l = u.lay.of(&cx, &u.tcx, *t);
                    if l.size as u64 != slot[0] || l.align as u64 != slot[1] {
                        let ts = crate::typeck::ty_to_string(false, &u.prog, &u.tcx, *t);
                        return Err(format!("`{}` passes `{}`, which rustc lays out in {} bytes (align {}) and makepad run in {} (align {})", key, ts, slot[0], slot[1], l.size, l.align));
                    }
                    if slot[2] != 0 {
                        if let TyKind::Ref(_, x, _) = u.tcx.tys.kind(*t).clone() {
                            let l = u.lay.of(&cx, &u.tcx, x);
                            if l.size as u64 != slot[2] || l.align as u64 != slot[3] {
                                let ts = crate::typeck::ty_to_string(false, &u.prog, &u.tcx, x);
                                return Err(format!("`{}` passes a reference to `{}`, which rustc lays out in {} bytes (align {}) and makepad run in {} (align {})", key, ts, slot[2], slot[3], l.size, l.align));
                            }
                        }
                    }
                }
            }
            let name = format!("{} (provided)", key);
            let shim = u.syms.intern(crate::syms::SymKind::Provided(format!("shim:{}", key)), sh.f as usize as u64);
            let last = u.syms.intern(crate::syms::SymKind::Provided(format!("last_panic:{}", pc.name)), pc.last_panic as usize as u64);
            Ok(crate::lower::bridge_func(u, d, name, shim, last))
        }
        // not marked #[relax_api]: compiled from source
        None => Ok(None),
    }
}

unsafe fn build_rir_lowered(up: *mut Unit, id: u32) -> Result<crate::rir::Func, String> {
    let _why = why_function(&*up, id, "lower-fn");
    let _cost_lower = crate::fncost::phase(3);
    let _mem = crate::memstat::tag(crate::memstat::Tag::Lower);
    if let Some(f) = (&*up).coro_rir.get(&id) {
        return Ok(f.clone());
    }
    let _warm = crate::warm::Scope::slot(id, "lowering");
    let key = (&*up).fns[id as usize].key.clone();
    let fname = {
        let u = &*up;
        let mut n = u.fns[id as usize].name.to_string();
        if let FnKey::Inst(_, args) = &key {
            if !args.is_empty() && trace_on() {
                let a: Vec<String> = args.iter().map(|t| crate::typeck::ty_to_string(false, &u.prog, &u.tcx, *t)).collect();
                n = format!("{}<{}>", n, a.join(", "));
            }
        }
        n
    };
    trace("typeck", &fname);
    // a startup image slot reached before its identity was decoded: decode it now
    let key = if matches!(key, FnKey::Glue(GLUE_UNNAMED, _)) {
        crate::cache::name_unnamed_unit(up, id);
        (&*up).fns[id as usize].key.clone()
    } else {
        key
    };
    if matches!(key, FnKey::Glue(GLUE_UNNAMED, _)) {
        return Err(format!("slot {} of the startup image has no identity (its key did not decode)", id));
    }
    if let FnKey::Shim(ref shim) = key {
        let t0 = cpu_ns();
        let result = crate::lower::shim_func(&*up, shim, id);
        (&mut *up).stats.lower_ns += cpu_ns() - t0;
        return result;
    }
    if let FnKey::Glue(kind, t) = key {
        if kind != GLUE_CLONE {
            let u = &mut *up;
            let t0 = cpu_ns();
            let f = crate::lower::glue_func(u, kind, t);
            u.stats.lower_ns += cpu_ns() - t0;
            return Ok(f);
        }
    }
    // a provided crate's bound fn (provided.rs): a bridge to its rustc-built code
    if let FnKey::Inst(d, args) = &key {
        if args.is_empty() {
            if let Some(f) = provided_bridge(&*up, *d)? {
                return Ok(f);
            }
        }
    }
    // Hash / Debug / Display of a provided crate's opaque type: rustc's impl (std helpers +
    // the loader's shims, provided_fmt_hash)
    if let FnKey::Inst(d, args) = &key {
        let u = &mut *up;
        if !u.provided_types.is_empty() {
            if let Some(f) = provided_fmt_hash(u, *d, args, id)? {
                return Ok(f);
            }
        }
    }
    // Builtin Clone shims. ADT derives have ordinary impl bodies.
    if let FnKey::Glue(GLUE_CLONE, ty) = &key {
        let u = &mut *up;
        let d = u.tcx.lang_items.get(LangItem::CloneFn).expect("clone lang item");
        let args = vec![*ty];
        {
            let t0 = cpu_ns();
            let body = Body::empty(d, args.clone(), u.prog.def(d).file, u.tcx.tys.unit);
            let name = u.fns[id as usize].name.to_string();
            let (func, errors) = {
                let mut lcx = Lcx::new(u, &body, name, id);
                lcx.lower_clone_shim(args[0]);
                lcx.finish();
                (lcx.f, lcx.errors)
            };
            (&mut *up).stats.lower_ns += cpu_ns() - t0;
            if !errors.is_empty() { return Err(errors[0].clone()); }
            if let Err(e) = func.verify() { return Err(format!("RIR verify: {}\n{}", e, func.dump())); }
            return Ok(func);
        }
    }
    // Bridge, virtual and derived bodies above always keep their usual selection. Ahead
    // helpers only supply the source-body lowering below.
    if let Some(f) = take_lowered_ahead(&mut *up, id) {
        return Ok(f);
    }
    let t0 = cpu_ns();
    let n0 = nested_build_ns();
    let body = match &key {
        FnKey::Inst(d, args) => {
            // consts that lengths wait for are evaluated first (CTFE by running them), and a body
            // that requested more is checked again once they are
            // checked ahead (off the unit lock) when it could be; else here
            let again = (&*up).front.built.contains(&(*d, args.clone()));
            let body = match take_ahead(&(&*up).prog, &(&*up).front, &(&*up).tcx, *d, args) {
                Ok(b) => { ahead_check_diag(&*up, id, "hit", 0); b }
                Err(reason) => {
                    let u = &mut *up;
                    if let Some(b) = ctfe_checked_body(u, *d, args) {
                        ahead_check_diag(u, id, "ctfe", 0);
                        b
                    } else {
                        let _o = crate::clock::origin(if again { "build-again" } else { "build" });
                        let start = cpu_ns();
                        let b = crate::typeck::codegen_body(&u.prog, &u.tcx, *d, args);
                        ahead_check_diag(u, id, reason, cpu_ns() - start);
                        b
                    }
                }
            };
            body
        }
        FnKey::Closure(cfile, _, parent) => {
            // a parent installed from the disk cache was never type-checked here: check it now
            // (a closure made by an older version of a hot-reloaded file is lowered from that
            // version's body: its expression ids mean that version's text)
            if !(&*up).has_body(*parent, *cfile) {
                ensure_parent_body(up, *parent, *cfile, 0)?;
            }
            let u = &mut *up;
            // a cache entry linked this run named this expression as a closure with another
            // identity (made from another parse of the file): refuse it rather than build
            // whatever the id names here
            if let FnKey::Closure(_, ce, _) = &key {
                if !crate::cache::closure_identity_ok(u, *cfile, *ce) {
                    return Err(format!("{}: a cached caller names expression {} of {} as another closure (stale code cache entry; `makepad clean` repairs it)",
                        u.fns[id as usize].name, ce & !crate::tcx::ASYNC_FN_BODY, u.prog.file_paths.get(*cfile as usize).map_or("?", |p| p.as_str())));
                }
            }
            match u.body_of(*parent, *cfile) {
                Some(b) => b.clone(),
                None => return Err("closure parent body missing".to_string()),
            }
        }
        FnKey::Shim(_) | FnKey::Glue(..) => return Err("glue".to_string()),
        FnKey::Sandbox(_) => return Err("sandbox twins are built from their base slot's RIR".to_string()),
    };
    if !body.errors.is_empty() {
        return Err(body.errors[0].clone());
    }
    let t1 = cpu_ns();
    let n1 = nested_build_ns();
    {
        static SLOW: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
        let lim = *SLOW.get_or_init(|| std::env::var("MAKEPAD_RUN_SLOW_TYPECK_US").ok().and_then(|v| v.parse().ok()).unwrap_or(0));
        if lim > 0 && (t1 - t0 - (n1 - n0)) / 1000 >= lim {
            eprintln!("slow typeck {} us: {}", (t1 - t0 - (n1 - n0)) / 1000, (&*up).fns[id as usize].name);
        }
    }
    // This is a codegen-owned body, after type checking. Borrowck/typeck results
    // retain their regions; lowering, constants and the closure codegen cache do not.
    // evaluate constants used by the body before lowering (may run JIT code)
    let mut const_uses = Vec::new();
    for (k, r) in &body.res {
        if let Res::Def(d) = r {
            let kind = (&*up).prog.def(*d).kind;
            if matches!(kind, DefKind::Const | DefKind::AssocConst | DefKind::Static) {
                let args = body.res_args.get(k).map_or_else(Vec::new, |&l| (&*up).tcx.tys.list(l).to_vec());
                const_uses.push((*d, args, kind));
            }
        }
    }
    for (k, r) in &body.pat_res {
        if let Res::Def(d) = r {
            let kind = (&*up).prog.def(*d).kind;
            if matches!(kind, DefKind::Const | DefKind::AssocConst) {
                let args = body.res_args.get(&(k | crate::typeck::PAT_KEY)).map_or_else(Vec::new, |&l| (&*up).tcx.tys.list(l).to_vec());
                const_uses.push((*d, args, kind));
            }
        }
    }
    for r in body.fmt_named.values() {
        if let Res::Def(d) = r {
            let kind = (&*up).prog.def(*d).kind;
            if matches!(kind, DefKind::Const | DefKind::AssocConst) {
                const_uses.push((*d, Vec::new(), kind));
            }
        }
    }
    for (d, args, kind) in const_uses {
        // a const used inside its own initializer is an error rustc already reports
        if let FnKey::Inst(me, _) = &key {
            if *me == d {
                continue;
            }
        }
        if kind == DefKind::Static {
            (&mut *up).static_addr(d);
        } else {
            (&mut *up).const_addr(d, args);
        }
    }
    let u = &mut *up;
    let (func, closures) = lower_checked_body(u, id, &key, &body)?;
    let u = &mut *up;
    u.stats.typeck_ns += (t1 - t0).saturating_sub(n1 - n0);
    u.stats.lower_ns += (cpu_ns() - t1).saturating_sub(nested_build_ns() - n1);
    // closures are lowered from their parent's body (looked up by the parent slot)
    if closures {
        u.put_body(id, body);
    }
    // a coroutine's poll function: its state layout was recorded from this RIR (every later
    // build of the slot takes it again)
    if let FnKey::Closure(file, ce, _) = &key {
        if u.tcx.coroutines.contains(&(*file, *ce)) {
            u.coro_rir.insert(id, func.clone());
        }
    }
    Ok(func)
}

/// The same unoptimised RIR on the locked path and the ahead helpers. Publishing a helper's
/// result does not optimise it: prepare_rir still applies the caller's normal inline policy.
fn lower_checked_body(u: &Unit, id: u32, key: &FnKey, body: &Body) -> Result<(crate::rir::Func, bool), String> {
    let name = u.fns[id as usize].name.to_string();
    trace("lower", &name);
    let mut lcx = Lcx::new(u, body, name, id);
    match key {
        FnKey::Inst(d, _) => lcx.lower_fn_body(*d),
        FnKey::Closure(_, ce, _) => lcx.lower_closure_body(*ce),
        FnKey::Shim(_) | FnKey::Glue(..) | FnKey::Sandbox(_) => {}
    }
    lcx.finish();
    // a body that cannot unwind (C-family ABIs without `-unwind`): a panic aborts
    // at its edge (rustc AbortUnwindingCalls)
    if let FnKey::Inst(d, _) = key {
        let nounwind_abi = u.tcx.fn_sig(&u.prog, *d).is_some_and(|s| matches!(s.header.abi,
            relax_types::types::Abi::C | relax_types::types::Abi::System | relax_types::types::Abi::SysV64 | relax_types::types::Abi::Win64));
        if nounwind_abi && lcx.f.cabi.is_some() && u.prog.session.options.unwind && lcx.errors.is_empty() {
            let t = u.syms.intern(crate::syms::SymKind::Rt("cannot_unwind"), crate::lower::cannot_unwind_addr());
            crate::lower::abort_unwinding_calls(&mut lcx.f, t);
        }
    }
    if let Some(e) = lcx.errors.first() {
        return Err(e.clone());
    }
    if let Err(e) = lcx.f.verify() {
        return Err(format!("RIR verify: {}\n{}", e, lcx.f.dump()));
    }
    Ok((lcx.f, !lcx.new_closures.is_empty()))
}

/// Finds `#[test]` functions of a crate.
pub fn find_tests(prog: &Program, krate: u32) -> Vec<DefId> {
    let mut out = Vec::new();
    for (i, d) in prog.defs.iter().enumerate() {
        if d.krate != krate || d.kind != DefKind::Fn {
            continue;
        }
        let it = prog.files[d.file as usize].ast.item(d.item);
        let mut is_test = false;
        let mut ignored = false;
        for a in prog.files[d.file as usize].ast.list(it.attrs) {
            if a.toks.hi > a.toks.lo {
                let n = prog.files[d.file as usize].tok_text(a.toks.lo);
                if n == "test" && a.toks.hi == a.toks.lo + 1 {
                    is_test = true;
                }
                if n == "ignore" {
                    ignored = true;
                }
            }
        }
        if is_test && !ignored {
            out.push(DefId(i as u32));
        }
    }
    out
}

// ------------------------------------------------------------ O3 (tier-up)
//
// Mechanism (runtime lane); the policy -- when a function is hot and how its O3 build
// is optimised -- is `Tier1Cfg` and `tier1_optimize` (backend lane).
//
// A function compiled lazily at O1 gets a call counter: its table slot points at a
// count stub that decrements the slot's counter and jumps to the O1 code. When the
// counter reaches 0 the stub enters `rt_hot`, which points the slot straight at the O1
// code (no more counting) and queues the slot. A background thread builds the O3 code
// (compile_tier1) and stores it into the slot atomically: new calls run O3, frames
// already in O1 finish there (the old code stays mapped; `retired` keeps it findable
// for backtraces). The program never waits for O3: the O3 thread takes the unit lock
// only while nobody waits for it, gives it up between steps, and optimises + generates code
// from a snapshot without it.

/// Tier-up policy (backend lane). `None` in `Unit::tier1`: O1 only.
pub struct Tier1Cfg {
    /// calls through the slot before a function is queued for O3
    pub calls: u64,
    /// optimisation of the O3 build (inlining over every callee that has RIR by then)
    pub opt: crate::opt::OptCfg,
    /// callees that have no RIR yet are built for inlining up to this many source tokens
    pub src_toks: u32,
}

impl Tier1Cfg {
    /// On by default; MAKEPAD_RUN_O3=0 disables it. MAKEPAD_RUN_O3_CALLS=n sets the hotness threshold.
    pub fn from_options(options: &crate::options::Options) -> Option<Tier1Cfg> {
        // on by default (MAKEPAD_RUN_O3=0: O1 only, which then also inlines Relax core's small
        // functions itself). Hot = calls through the slot (count stub) or CPU samples (loops
        // in functions entered rarely: sampler). Counter (hidden): first present 4.05-4.45 ->
        // 3.74-3.78 s, opt CPU 1.93 -> 1.6 s; bench_rt geomean 1.99 -> 2.05.
        if !options.tier1 {
            return None;
        }
        let calls = options.tier1_calls;
        Some(Tier1Cfg { calls, opt: crate::opt::OptCfg::from_options(options), src_toks: TIER1_SRC_TOKS })
    }
}

/// The O3 optimisation of a function (backend lane): `func` is a copy of its O1
/// RIR, `get` gives the callees' RIR (snapshot). Returns the inlined callees.
/// MAKEPAD_RUN_O3_QUICK_RA=1: O3 allocates registers as O1 does (no thorough allocation, block
/// frequencies, loop splitting) -- the on/off switch for measuring that allocation's compile cost.
fn t1_quick_ra(options: &crate::options::Options) -> bool {
    options.t1_quick_ra
}

fn tier1_optimize(options: &crate::options::Options, func: &mut crate::rir::Func, id: u32, name: &str, get: &dyn Fn(u32) -> Option<(*const crate::rir::Func, u8)>, single: &dyn Fn(u32, u32) -> bool, ro: Option<crate::opt::RoReader>, cfg: &crate::opt::OptCfg, nounwind: &dyn Fn(u32) -> bool, frequencies: &mut dyn FnMut(usize, &crate::rir::Func) -> Option<Vec<f32>>) -> Vec<(u32, usize)> {
    let _cost_opt = crate::fncost::phase(4);
    crate::fncost::size(true, func);
    func.baseline = false;
    // sroa may keep a slot in registers across a call that provably keeps no pointer to it
    // (the callee's RIR in the snapshot; its own calls one level down); removed below
    struct NocapGuard;
    impl Drop for NocapGuard {
        fn drop(&mut self) {
            crate::opt::set_sroa_nocapture(None);
        }
    }
    let _guard = NocapGuard;
    {
        // SAFETY: the oracle borrows `get` only while this function runs: the guard removes it
        // before `get` goes out of scope, also when the optimiser panics
        let get_static: &'static dyn Fn(u32) -> Option<(*const crate::rir::Func, u8)> = unsafe { std::mem::transmute(get) };
        // callees' params, depth-limited (a Vec's push -> reserve -> grow -> realloc chain);
        // false while a query is in progress (recursion), memoised per (callee, param)
        fn nocap(get: &dyn Fn(u32) -> Option<(*const crate::rir::Func, u8)>, memo: &std::cell::RefCell<HashMap<(u32, usize), Option<bool>>>, cid: u32, k: usize, depth: u32) -> bool {
            if let Some(r) = memo.borrow().get(&(cid, k)) {
                return r.unwrap_or(false);
            }
            if depth == 0 {
                return false;
            }
            let Some((fp, _)) = get(cid) else { return false };
            memo.borrow_mut().insert((cid, k), None);
            let f2 = unsafe { &*fp };
            let sub = |c2: &crate::rir::Callee, k2: usize| -> bool {
                match c2 {
                    crate::rir::Callee::Fn(i2) => nocap(get, memo, *i2, k2, depth - 1),
                    _ => false,
                }
            };
            let r = crate::opt::param_nocapture(f2, k, &sub);
            memo.borrow_mut().insert((cid, k), Some(r));
            r
        }
        let memo: std::cell::RefCell<HashMap<(u32, usize), Option<bool>>> = std::cell::RefCell::new(HashMap::new());
        crate::opt::set_sroa_nocapture(Some(Box::new(move |c: &crate::rir::Callee, k: usize| -> bool {
            let crate::rir::Callee::Fn(cid) = c else { return false };
            nocap(get_static, &memo, *cid, k, 4)
        })));
    }
    static INLINE_STATS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let mut inline_stats = (*INLINE_STATS.get_or_init(|| std::env::var("MAKEPAD_RUN_INLINE_STATS").map_or(false, |v| v == "1"))).then(crate::opt::InlineStats::default);
    let mut limits = crate::opt::InlineLimits::tier1_for(func);
    let mut inlined: Vec<(u32, usize)> = Vec::new();
    if cfg.inline {
        let ti = crate::opt::pass_clock();
        let freqs = frequencies(0, func);
        for (c, rule) in crate::opt::inline_calls_ordered(options, func, id, get, single, nounwind, &limits, freqs.as_deref(), inline_stats.as_mut()) {
            // a single-caller body moved into this fn: the second round's ceiling includes it
            if rule == crate::opt::RULE_SINGLE {
                if let (Some(g), Some((p, _))) = (limits.growth_limit.as_mut(), get(c)) {
                    *g = g.saturating_add(crate::opt::inst_count(unsafe { &*p }));
                }
            }
            if !inlined.iter().any(|x| x.0 == c) {
                inlined.push((c, rule));
            }
        }
        crate::opt::pass_time(5, ti);
    }
    opt_pipeline(options, func, ro, cfg, layout_for(options, name), true, nounwind);
    // optimisation exposes calls inlining could not see (a call through a known fn address
    // became a direct call: `.map(GlyphId)`): one more inlining round for them
    if cfg.inline {
        let ti = crate::opt::pass_clock();
        let freqs = frequencies(1, func);
        let more = crate::opt::inline_calls_ordered(options, func, id, get, single, nounwind, &limits, freqs.as_deref(), inline_stats.as_mut());
        crate::opt::pass_time(5, ti);
        if !more.is_empty() {
            for (c, rule) in more {
                if !inlined.iter().any(|x| x.0 == c) {
                    inlined.push((c, rule));
                }
            }
            crate::opt::simplify(options, func, ro, cfg.imm, cfg.cse);
            // simplify merges the blocks a copy and its reads were split over: forward again
            for _ in 0..2 {
                if !(cfg.sroa && (crate::opt::forward_const_slots(func, ro) | crate::opt::forward_slot_copies(func) | crate::opt::forward_slot_dests(func) | crate::opt::forward_slot_dests_x(options, func, BIG_SLOT) | crate::opt::forward_slot_loads(func))) {
                    break;
                }
                crate::opt::simplify(options, func, ro, cfg.imm, false);
            }
        }
    }
    // post-inline cleanup (codegen2's copy-elimination finding + LLVM SROA's whole-slot slicing):
    // small aggregate temporaries the inlined bodies copy through (Option payloads, sret results)
    // are forwarded from 32 bytes up, then groups copied in and out are tiled into registers;
    // later wider loads of narrower stores otherwise fail store forwarding (layout_widgets)
    if cfg.sroa && !crate::opt::opt_off(options, "-postinline") {
        for _ in 0..3 {
            // forward_slot_dests_x(options, func, 32) is off here: it merged a by-value parameter
            // copy into the result buffer while that copy was still read after the first payload
            // write (O3 Result<&T,E>::cloned; tweaker build.rs crashed, coord.md 2026-10-08
            // 08:10). backend2 re-enables it with a fixed interference check.
            let ch = crate::opt::forward_slot_dests(func);
            let ch = ch | crate::opt::sroa_post_inline(func);
            if !ch {
                break;
            }
            crate::opt::forward_slot_copies(func);
            crate::opt::forward_slot_loads(func);
            crate::opt::simplify(options, func, ro, cfg.imm, false);
        }
    }
    // array iterators (a slot read at runtime offsets): their scalar fields in registers, so
    // the iteration counter can drive full unrolling below
    if cfg.fold && cfg.sroa && crate::opt::promote_dyn_read_slots(func) {
        // the cells' entry zeros that a write always precedes are dead by liveness only
        crate::opt::live_dce(func);
        crate::opt::simplify(options, func, ro, cfg.imm, cfg.cse);
    }
    // loops with a small constant trip count: fully unrolled (each copy's counter a constant);
    // a second round reaches the loop around an unrolled inner one
    for _ in 0..2 {
        if !(cfg.fold && relax_codegen::loops::unroll_const_loops(options, func)) {
            break;
        }
        crate::opt::simplify(options, func, ro, cfg.imm, cfg.cse);
        // the original loops the copies replaced: unreachable now, emptied for the passes below
        relax_codegen::loops::clear_unreachable(func);
        // unrolled reads at constant offsets: the slots they read can now go to registers (a
        // write-through counter's stores nobody reads go first, then the copy chains)
        if cfg.sroa && crate::opt::drop_unread_slot_stores(func) {
            crate::opt::forward_slot_copies(func);
        }
        if cfg.sroa && crate::opt::sroa(func) {
            crate::opt::live_dce(func);
            crate::opt::simplify(options, func, ro, cfg.imm, cfg.cse);
        }
    }
    // iterator steps: the element pointer's accesses read the advanced cursor, one register
    if cfg.fold {
        crate::opt::fold_post_increments(func);
    }
    // Rust min/max with an operand that is never NaN: one minss/maxss (bounding boxes)
    if cfg.fold {
        crate::opt::min_max_non_nan(func);
    }
    // compares value ranges decide (bounds checks of masked / branch-limited indices)
    if cfg.fold && relax_codegen::ranges::fold_ranges(options, func) {
        crate::opt::simplify(options, func, ro, cfg.imm, cfg.cse);
    }
    // Run after both inlining rounds and before small slots share storage:
    // aggregate ABI halves and partial byte buffers still name distinct slots.
    if cfg.fold && cfg.sroa && relax_codegen::opt::promote_vector_slots(func) {
        verify_pass(func, "promote_vector_slots");
        crate::opt::live_dce(func);
        crate::opt::simplify(options, func, ro, cfg.imm, cfg.cse);
        if crate::opt::sroa(func) {
            crate::opt::live_dce(func);
            crate::opt::simplify(options, func, ro, cfg.imm, cfg.cse);
        }
        verify_pass(func, "vector slot cleanup");
    }
    crate::opt::compact_slots(func);
    if cfg.layout && layout_for(options, name) {
        crate::opt::rotate_loops(func);
    }
    // hot code: small counted single-block loops unrolled (rotation made them single blocks;
    // address modes then fold the copies' i + j into displacements)
    // integer sum reductions over arrays: vector loops in front of the scalar ones (before
    // unrolling, which then takes the scalar remainder loops as before)
    relax_codegen::loops::vectorize_reductions(options, func);
    relax_codegen::loops::unroll_loops(options, func);
    // byte stores of one value's bytes -> one wide store (before address modes)
    relax_codegen::loops::merge_stores(options, func);
    if cfg.fold {
        crate::opt::addr_modes(func);
        // results computed early for one later use move next to it (O3)
        crate::opt::sink_single_uses(func);
    }
    if let Some(stats) = inline_stats { stats.report(name); }
    inlined
}

// ---- loop hotness by sampling (tier-up of functions entered rarely that loop a lot)
//
// A sampler thread reads the pc of every thread running JIT code (registered by call_jit)
// every SAMPLE_MS -- without signals, so no system call of the program is ever interrupted
// (EINTR): on macOS by suspending the thread for a Mach thread_get_state, on Linux from a
// perf_event task-clock sampling ring the kernel fills. pcs inside the JIT code region are
// mapped to slots under a non-waiting unit lock through an address-ordered code index and a
// slot is queued for O3 after SAMPLES_HOT samples (~SAMPLES_HOT * SAMPLE_MS of its CPU),
// like a count stub reaching zero. Without OSR the next entry runs O3.

pub(crate) const SAMPLE_MS: u64 = 2;

/// Diagnostic sampling cadence in milliseconds. A comma-separated sequence
/// (for example 8,20,11,17) also exercises irregular/coalesced sampling.
pub(crate) fn sample_periods() -> &'static [u64] {
    static PERIODS: std::sync::OnceLock<Vec<u64>> = std::sync::OnceLock::new();
    PERIODS.get_or_init(|| {
        let _k = crate::zone_alloc::kept();
        std::env::var("MAKEPAD_RUN_SAMPLE_MS").ok().and_then(|s| {
            s.split(',').map(|v| v.trim().parse::<u64>().ok()
                .filter(|v| (1..=1000).contains(v))).collect::<Option<Vec<_>>>()
        }).filter(|v| !v.is_empty()).unwrap_or_else(|| vec![SAMPLE_MS])
    })
}
const SAMPLES_HOT: u32 = 5;
/// callers credited with a sample too (a loop calling a hot callee is hot itself)
#[cfg(not(target_os = "android"))]
pub(crate) const SAMPLE_CALLERS: usize = 2;
static EXEC_LO: AtomicU64 = AtomicU64::new(0);
static EXEC_HI: AtomicU64 = AtomicU64::new(0);
/// threads running JIT code: (thread id, call_jit nesting depth, OS sampling handle)
static SAMPLED: relax_session::sync::Mutex<Vec<(u64, u32, os::sample::Handle)>> = relax_session::sync::Mutex::new(Vec::new());
static SAMPLER: std::sync::Once = std::sync::Once::new();

fn sampler_enter(exec_base: u64, exec_size: u64) {
    EXEC_LO.store(exec_base, Ordering::Relaxed);
    EXEC_HI.store(exec_base + exec_size, Ordering::Relaxed);
    let me = os::thread::current();
    {
        let mut g = SAMPLED.lock().unwrap();
        match g.iter_mut().find(|x| x.0 == me) {
            Some(x) => x.1 += 1,
            None => {
                if let Some(h) = os::sample::open_this_thread(me) {
                    g.push((me, 1, h));
                }
            }
        }
    }
    SAMPLER.call_once(|| {
        { let _k = crate::zone_alloc::kept();
        let _ = std::thread::Builder::new().name("run-sampler".to_string()).spawn(sampler_thread);
        }
    });
}

fn sampler_leave() {
    let me = os::thread::current();
    let mut g = SAMPLED.lock().unwrap();
    if let Some(k) = g.iter().position(|x| x.0 == me) {
        g[k].1 -= 1;
        if g[k].1 == 0 {
            let (_, _, h) = g.swap_remove(k);
            os::sample::close(h);
        }
    }
}

fn sample_trace() -> bool {
    static T: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *T.get_or_init(|| std::env::var("MAKEPAD_RUN_SAMPLE_TRACE").is_ok())
}

/// MAKEPAD_RUN_DUMP=<suffix>[,<suffix>..]: machine code (.bin / O3 .t1.bin) and RIR (.rir) of the fns whose
/// name ends with one of the suffixes go to <MAKEPAD_RUN_DUMP_DIR, default /tmp>/relax_dump_<suffix, :: -> _>.
/// (tools: bench_rt/asmdiff.py)
fn dump_base(name: &str) -> Option<String> {
    static DUMP: std::sync::OnceLock<Option<(Vec<String>, String)>> = std::sync::OnceLock::new();
    let (wants, dir) = DUMP.get_or_init(|| crate::zone_alloc::kept_with(|| std::env::var("MAKEPAD_RUN_DUMP").ok().map(|w| (w.split(',').filter(|x| !x.is_empty()).map(|x| x.to_string()).collect(), std::env::var("MAKEPAD_RUN_DUMP_DIR").unwrap_or_else(|_| "/tmp".to_string()))))).as_ref()?;
    let w = wants.iter().find(|w| name.ends_with(w.as_str()))?;
    Some(format!("{}/relax_dump_{}", dir, w.replace("::", "_")))
}

/// MAKEPAD_RUN_DUMP of an O1 / baseline build: its code (.bin), the RIR it was emitted from (.rir) and
/// its frame size and slots (.frame: `frame <bytes>`, then `slot <n> <size> <align>` per slot)
fn dump_o1(func: &crate::rir::Func, c: &x64::Compiled) {
    if let Some(base) = dump_base(&func.name) {
        let _ = std::fs::write(format!("{}.bin", base), &c.code[..c.text_len as usize]);
        let _ = std::fs::write(format!("{}.rir", base), func.dump());
        let (at, end) = crate::opt::slot_layout(func, 0, true);
        let mut m = format!("frame {} slot-area {} of {}\n", c.frame_size, end, slot_bytes(func));
        for (k, sl) in func.slots.iter().enumerate() {
            m.push_str(&format!("slot {} {} {} at {}{}\n", k, sl.size, sl.align, at[k], if sl.arg { " arg" } else { "" }));
        }
        let _ = std::fs::write(format!("{}.frame", base), m);
    }
}

/// MAKEPAD_RUN_PROFILE=<path>: the sampler also counts each JIT-code sample's own function (no callers) and
/// rewrites <path> every 0.5 s (and when the unit is dropped): `samples<TAB>tier<TAB>code bytes<TAB>name`, hottest first.
/// A profile of steady-state hot code for the asm-diff ranking (bench_rt/rank.py), not a timing.
fn profile_path() -> Option<&'static str> {
    static P: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    P.get_or_init(|| crate::zone_alloc::kept_with(|| std::env::var("MAKEPAD_RUN_PROFILE").ok().filter(|s| !s.is_empty()))).as_deref()
}

/// MAKEPAD_RUN_PROFILE_FROM_MS=<ms>: count samples only after that long since the first sample (steady state of
/// an app: startup's O1 code stays out of the profile)
fn profile_started() -> bool {
    static T: std::sync::OnceLock<(std::time::Instant, u64)> = std::sync::OnceLock::new();
    let (t0, from) = T.get_or_init(|| (std::time::Instant::now(), std::env::var("MAKEPAD_RUN_PROFILE_FROM_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0)));
    t0.elapsed().as_millis() as u64 >= *from
}

/// Samples belong to the physical function, not a global source-position pool:
/// inlined copies and generic instances must not train one another. Freeze after
/// actual observations (timer-coalescing weights are not independent samples).
const FREQUENCY_SAMPLES: u32 = 64;
#[derive(Default)]
struct FrequencyProfile {
    // Time exposure in default-period units; the stopping rule below counts
    // landed observations separately, so a stretched sleep cannot invent data.
    positions: HashMap<u64, u32>,
    samples: u32,
    queued: bool,
}

fn inline_structural_freqs(f: &crate::rir::Func) -> Vec<f32> {
    let nb = f.blocks.len();
    if nb == 0 { return Vec::new(); }
    let reach = crate::regalloc::reachable(f);
    let depth = crate::regalloc::loop_depths(f, &reach);
    // Propagate path probability over the forward CFG. Reverse postorder
    // cuts feedback edges; loop depth supplies their repeat weight instead.
    let mut seen = vec![false; nb];
    let mut order = Vec::new();
    let mut stack = vec![(0usize, false)];
    while let Some((b, exit)) = stack.pop() {
        if exit { order.push(b); continue; }
        if seen[b] { continue; }
        seen[b] = true;
        stack.push((b, true));
        for to in f.blocks[b].term.succs() {
            if !seen[to as usize] { stack.push((to as usize, false)); }
        }
    }
    order.reverse();
    let mut rank = vec![usize::MAX; nb];
    for (i, &b) in order.iter().enumerate() { rank[b] = i; }
    let mut mass = vec![0.0f32; nb];
    mass[0] = 1.0;
    for &b in &order {
        let forward = |to: &u32| rank[*to as usize] > rank[b]
            && !matches!(f.blocks[*to as usize].term, crate::rir::Term::Unreachable | crate::rir::Term::Resume);
        let n = f.blocks[b].term.succs().filter(forward).count();
        if n > 0 {
            let share = mass[b] / n as f32;
            for to in f.blocks[b].term.succs().filter(forward) { mass[to as usize] += share; }
        }
    }
    let raw: Vec<_> = depth.iter().enumerate().map(|(b, d)| if *d == 0 { 0.0 } else { mass[b] * 8f32.powi((*d).min(4) as i32) }).collect();
    let reference = raw.iter().copied().fold(0.0f32, f32::max).max(0.01);
    raw.iter().map(|v| (v / reference).max(0.01)).collect()
}

/// Allocation weights use the frozen evidence directly. A missing PC observation
/// is unknown, never proof that a block is unexecuted: retain a positive floor
/// so the allocator can still split its loops. Inlining uses the CFG estimate.
fn sampled_freqs(f: &crate::rir::Func, h: &HashMap<u64, u32>) -> Vec<f32> {
    let counts: Vec<u32> = f.blocks.iter().map(|b| {
        b.pos.iter().chain(std::iter::once(&b.term_pos))
            .map(|p| h.get(p).copied().unwrap_or(0)).max().unwrap_or(0)
    }).collect();
    // The allocator expects 0..=1 relative to the hottest block. Entry PCs are
    // too short to give a reliable denominator; never divide by a noisy entry.
    let reference = counts.iter().copied().max().unwrap_or(1).max(1) as f32;
    counts.iter().map(|n| {
        if *n == 0 { 0.01 } else {
            (*n as f32 / reference).max(0.01)
        }
    }).collect()
}

/// The source position of the instruction a JIT pc is in (current or retired code of its slot).
fn pc_source_pos(u: &Unit, pc: u64) -> Option<u64> {
    let (start, _, id) = u.code.read().unwrap_or_else(|e| e.into_inner()).entry_at(pc)?;
    let f = &u.fns[id as usize];
    let map = if pc >= f.code && pc < f.code + f.code_len as u64 {
        &f.pc_map
    } else {
        &f.history().retired.iter().find(|r| pc >= r.0 && pc < r.0 + r.1 as u64)?.2
    };
    let off = (pc - start) as u32;
    map.lookup(off)
}

/// MAKEPAD_RUN_FIELD_PROF=<path> (with MAKEPAD_RUN_PROFILE, steady from MAKEPAD_RUN_PROFILE_FROM_MS): every struct
/// field reached through memory in lowered code is a site; profile_write also writes <path>:
/// `samples<TAB>fn<TAB>file:line<TAB>type<TAB>field index<TAB>field<TAB>offset<TAB>size<TAB>struct
/// size<TAB>struct align<TAB>fixed layout (repr C/packed/int)`, samples = the steady samples on the
/// site's source line. A measurement of how scattered hot fields are (no timing, no counters).
pub fn field_prof_on() -> bool {
    static E: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    E.get_or_init(|| crate::zone_alloc::kept_with(|| std::env::var("MAKEPAD_RUN_FIELD_PROF").ok().filter(|s| !s.is_empty()))).is_some()
}

#[derive(Clone)]
pub struct FieldSite {
    pub func: String,
    pub file: u32,
    pub line_lo: u32,
    pub line_hi: u32,
    pub at: String,
    pub ty: String,
    pub field: u32,
    pub fname: String,
    pub off: u32,
    pub size: u32,
    pub ssize: u32,
    pub salign: u32,
    pub fixed: bool,
}

/// field sites by (fn, type, field, line): one entry per distinct access per line
static FIELD_SITES: relax_session::sync::Mutex<Option<std::collections::HashMap<(String, String, u32, u32, u32), FieldSite>>> = relax_session::sync::Mutex::new(None);
/// steady samples per source position (file << 32 | byte), for MAKEPAD_RUN_FIELD_PROF
static FIELD_HIST: relax_session::sync::Mutex<Option<std::collections::HashMap<u64, u64>>> = relax_session::sync::Mutex::new(None);

pub fn field_note(s: FieldSite) {
    let _k = crate::zone_alloc::kept();
    let mut g = FIELD_SITES.lock().unwrap();
    let m = g.get_or_insert_with(std::collections::HashMap::new);
    m.entry((s.func.clone(), s.ty.clone(), s.field, s.file, s.line_lo)).or_insert_with(|| s.clone());
}

fn field_prof_write() {
    let Some(path) = std::env::var("MAKEPAD_RUN_FIELD_PROF").ok().filter(|s| !s.is_empty()) else { return };
    // samples per (file, byte) sorted, so a line's samples are one range sum
    let mut by_file: std::collections::HashMap<u32, Vec<(u32, u64)>> = std::collections::HashMap::new();
    if let Some(h) = FIELD_HIST.lock().unwrap().as_ref() {
        for (&pos, &n) in h {
            by_file.entry((pos >> 32) as u32).or_default().push((pos as u32, n));
        }
    }
    for v in by_file.values_mut() {
        v.sort();
        let mut acc = 0u64;
        for x in v.iter_mut() {
            acc += x.1;
            x.1 = acc;
        }
    }
    let line_samples = |file: u32, lo: u32, hi: u32| -> u64 {
        let Some(v) = by_file.get(&file) else { return 0 };
        let a = v.partition_point(|x| x.0 < lo);
        let b = v.partition_point(|x| x.0 <= hi);
        if b == 0 || b <= a {
            return 0;
        }
        v[b - 1].1 - if a > 0 { v[a - 1].1 } else { 0 }
    };
    let g = FIELD_SITES.lock().unwrap();
    let Some(m) = g.as_ref() else { return };
    // a source line shared by several instances (a generic fn's monomorphs, or one inlined
    // into many callers) gets its samples once, split evenly over the types accessed there
    let mut per_line: std::collections::HashMap<(u32, u32), std::collections::HashSet<&str>> = std::collections::HashMap::new();
    for s in m.values() {
        per_line.entry((s.file, s.line_lo)).or_default().insert(s.ty.as_str());
    }
    let mut out = String::new();
    for s in m.values() {
        let k = per_line.get(&(s.file, s.line_lo)).map_or(1, |x| x.len().max(1)) as u64;
        let n = line_samples(s.file, s.line_lo, s.line_hi) / k;
        if n == 0 {
            continue;
        }
        out.push_str(&format!("{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n", n, s.func, s.at, s.ty, s.field, s.fname, s.off, s.size, s.ssize, s.salign, s.fixed as u8));
    }
    drop(g);
    let tmp = format!("{}.tmp", path);
    if std::fs::write(&tmp, out).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

/// MAKEPAD_RUN_PROFILE counts per fn name (a process can run several units: build scripts, proc-macro
/// hosts, the program): (samples, O3?, code bytes)
static PROFILE: relax_session::sync::Mutex<Option<std::collections::HashMap<String, (u64, bool, u32)>>> = relax_session::sync::Mutex::new(None);
/// (fn name, code start, code offset) -> samples, for the fns MAKEPAD_RUN_DUMP names (written next to their dumps as .prof)
static PC_PROFILE: relax_session::sync::Mutex<Option<std::collections::HashMap<(String, u64, u32), u64>>> = relax_session::sync::Mutex::new(None);
/// current code start per dumped fn name (the .prof keeps the samples of that code only)
static PC_CODE: relax_session::sync::Mutex<Option<std::collections::HashMap<String, (u64, bool)>>> = relax_session::sync::Mutex::new(None);

/// Writes the MAKEPAD_RUN_PROFILE table and the dumped fns' per-instruction samples.
fn profile_write() {
    let Some(p) = profile_path() else { return };
    if field_prof_on() {
        field_prof_write();
    }
    let g = PROFILE.lock().unwrap();
    let Some(m) = g.as_ref() else { return };
    let mut rows: Vec<(&String, &(u64, bool, u32))> = m.iter().collect();
    rows.sort_by(|a, b| b.1 .0.cmp(&a.1 .0).then(a.0.cmp(b.0)));
    let mut out = String::new();
    for (name, (n, t1, len)) in rows {
        out.push_str(&format!("{}\t{}\t{}\t{}\n", n, if *t1 { 1 } else { 0 }, len, name));
    }
    drop(g);
    let tmp = format!("{}.tmp", p);
    if std::fs::write(&tmp, out).is_ok() {
        let _ = std::fs::rename(&tmp, p);
    }
    // per-instruction samples of dumped fns: `<offset hex> <samples>` (offsets into the current code,
    // i.e. the .t1.bin once O3 installed)
    let codes = PC_CODE.lock().unwrap().clone().unwrap_or_default();
    if let Some(pm) = PC_PROFILE.lock().unwrap().as_ref() {
        let mut by: std::collections::HashMap<&String, Vec<(u32, u64)>> = std::collections::HashMap::new();
        for ((name, code, off), n) in pm {
            if codes.get(name).map(|c| c.0) == Some(*code) {
                by.entry(name).or_default().push((*off, *n));
            }
        }
        for (name, mut v) in by {
            if let Some(base) = dump_base(name) {
                v.sort();
                let s: String = v.iter().map(|(o, n)| format!("{:x} {}\n", o, n)).collect();
                let t1 = codes.get(name).map_or(false, |c| c.1);
                let _ = std::fs::write(format!("{}{}.prof", base, if t1 { ".t1" } else { "" }), s);
            }
        }
    }
}

fn sampler_thread() {
    // macOS coalesces the timers of background-QoS threads (a process started from a
    // daemon or a script): a 2 ms sleep then lasts tens of ms and loops never get enough
    // samples to tier up. The sampler asks for interactive QoS (no coalescing).
    os::thread::interactive();
    let mut wait = os::sample::Wait::new();
    let mut pcs: Vec<os::sample::Sample> = Vec::with_capacity(4096);
    let mut last_half = wall_ns();
    let mut hot: Vec<u32> = Vec::new();
    let mut selfs: Vec<os::sample::Sample> = Vec::with_capacity(1024);
    let mut last = std::time::Instant::now();
    let mut last_write = std::time::Instant::now();
    let periods = sample_periods();
    let mut period_index = 0;
    loop {
        let period = periods[period_index];
        period_index = (period_index + 1) % periods.len();
        // Linux selects irregular observations from its perf stream. Drain
        // often enough to avoid overflowing the ring even for long intervals.
        let sleep_ms = if cfg!(target_os = "linux") && periods.len() > 1 { period.min(32) } else { period };
        wait.sleep(sleep_ms);
        if TIER_STOP.load(Ordering::SeqCst) { break; }
        // Direct-PC samplers use the time since the previous wake. Linux's ring
        // supplies each observation's own period instead. A sleep the OS stretched
        // (timer coalescing of low-QoS / nice'd processes: 2 ms became ~18 ms on the bench
        // mini) weighs more, so hotness tracks CPU time, not the number of wake-ups
        let now = std::time::Instant::now();
        let weight = ((now - last).as_secs_f64() * 1000.0 / SAMPLE_MS as f64).round().clamp(1.0, 16.0) as u32;
        last = now;
        let _job = compile_job();
        let (lo, hi) = (EXEC_LO.load(Ordering::Relaxed), EXEC_HI.load(Ordering::Relaxed));
        {
            let g = SAMPLED.lock().unwrap();
            for (_, _, h) in g.iter() {
                os::sample::read(h, &mut pcs, &mut selfs, lo, hi, weight);
            }
        }
        if sample_trace() {
            static T0: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
            let t = T0.get_or_init(std::time::Instant::now).elapsed().as_secs_f64() * 1000.0;
            let threads = SAMPLED.lock().map(|g| g.len()).unwrap_or(0);
            eprintln!("sampler: t={:.1} ms, {} threads, {} pcs", t, threads, pcs.len());
        }
        // Train only after the first structural O3 installation. Sampling
        // retired code would mix the old and new code shapes during long calls.
        if !selfs.is_empty() && !TIER_STOP.load(Ordering::SeqCst) {
            owner::call_within(owner::Class::Background, 0, || {
                let up = unsafe { UNIT };
                if !up.is_null() {
                    let u = unsafe { &mut *up };
                    if !u.prog.session.options.no_block_freq && !t1prof_replay(&u.prog.session.options) {
                        for &os::sample::Sample { pc, weight } in &selfs {
                            let Some(id) = u.find_fn(pc) else { continue; };
                            let f = &u.fns[id as usize];
                            if pc < f.code || pc >= f.code + f.code_len as u64 { continue; }
                            let Some(pos) = pc_source_pos(u, pc) else { continue; };
                            if pos == 0 { continue; }
                            let Some(h) = u.frequency_profiles.get_mut(&id) else { continue; };
                            if h.samples >= FREQUENCY_SAMPLES { continue; }
                            crate::zone_alloc::kept_with(|| *h.positions.entry(pos).or_default() += weight);
                            h.samples += 1;
                            if h.samples == FREQUENCY_SAMPLES && !h.queued {
                                if tier1_trace() {
                                    eprintln!("O3-frequency ready @{:.3} ms samples={} exposure={} {}",
                                        crate::clock::ms_since_start(), h.samples,
                                        h.positions.values().map(|n| *n as u64).sum::<u64>(), f.name);
                                }
                                h.queued = true;
                                u.stats.tier1_retier += 1;
                                churn_queued(id);
                                tier_push(id, Q_RETIER, 0);
                            }
                        }
                    }
                }
            });
        }
        if profile_path().is_some() && !selfs.is_empty() && profile_started() && !TIER_STOP.load(Ordering::SeqCst) {
            owner::call_within(owner::Class::Background, 0, || {
                let _k = crate::zone_alloc::kept();
                let up = unsafe { UNIT };
                if !up.is_null() {
                    let u = unsafe { &*up };
                    if field_prof_on() {
                        let mut fh = FIELD_HIST.lock().unwrap();
                        let fh = fh.get_or_insert_with(std::collections::HashMap::new);
                        for &os::sample::Sample { pc, weight } in &selfs {
                            if let Some(pos) = pc_source_pos(u, pc) {
                                *fh.entry(pos).or_insert(0) += weight as u64;
                            }
                        }
                    }
                    {
                        let mut g = PROFILE.lock().unwrap();
                        let m = g.get_or_insert_with(std::collections::HashMap::new);
                        let mut pp = PC_PROFILE.lock().unwrap();
                        let pm = pp.get_or_insert_with(std::collections::HashMap::new);
                        let mut pc_code = PC_CODE.lock().unwrap();
                        let pcm = pc_code.get_or_insert_with(std::collections::HashMap::new);
                        for &os::sample::Sample { pc, weight } in &selfs {
                            if let Some(id) = u.find_fn(pc) {
                                let f = &u.fns[id as usize];
                                let t1 = !f.history().retired.is_empty();
                                let e = m.entry(f.name.to_string()).or_insert((0, t1, f.code_len));
                                e.0 += weight as u64;
                                e.1 = t1;
                                e.2 = f.code_len;
                                // per-instruction counts of the dumped fns' current code
                                if pc >= f.code && pc < f.code + f.code_len as u64 && dump_base(&f.name).is_some() {
                                    *pm.entry((f.name.to_string(), f.code, (pc - f.code) as u32)).or_insert(0) += weight as u64;
                                    pcm.insert(f.name.to_string(), (f.code, t1));
                                }
                            }
                        }
                    }
                    if last_write.elapsed().as_millis() >= 500 {
                        last_write = std::time::Instant::now();
                        profile_write();
                    }
                }
            });
        }
        selfs.clear();
        // Decay even with no samples or a busy unit lock. Otherwise yesterday's hot code
        // retains its priority during an idle spell. Catch up after delayed sampler wakes.
        let now = wall_ns();
        let halves = now.saturating_sub(last_half) / HEAT_HALF_NS;
        if halves > 0 {
            last_half += halves * HEAT_HALF_NS;
            if let Some(counts) = HEAT.lock().unwrap().as_mut() {
                counts.retain(|_, c| {
                    *c = c.checked_shr(halves.min(32) as u32).unwrap_or(0);
                    *c > 0
                });
            }
        }
        if pcs.is_empty() || TIER_STOP.load(Ordering::SeqCst) {
            pcs.clear();
            continue;
        }
        owner::call_within(owner::Class::Background, 0, || {
            let up = unsafe { UNIT };
            if up.is_null() {
                return;
            }
            let u = unsafe { &*up };
            let mut heat = crate::zone_alloc::kept_with(|| HEAT.lock().unwrap());
            let counts = heat.get_or_insert_with(std::collections::HashMap::new);
            for &os::sample::Sample { pc, weight } in &pcs {
                let Some(id) = u.find_fn(pc) else { continue };
                unsafe { base_sampled(up, id) };
                let (before, count) = crate::zone_alloc::kept_with(|| {
                    let c = counts.entry(id).or_insert(0);
                    let before = *c;
                    *c += weight;
                    (before, *c)
                });
                if before < SAMPLES_HOT && count >= SAMPLES_HOT {
                    if sample_trace() {
                        eprintln!("sampler: {} hot", u.fns[id as usize].name);
                    }
                    // (the sampler's own vector grows here, on the owner: kept, it outlives the compiler arena)
                    crate::zone_alloc::kept_with(|| hot.push(id));
                }
            }
            // HEAT before TIER_QUEUE nowhere (tier_pop holds the queue, then reads HEAT)
            drop(heat);
            for &id in &hot {
                unsafe { queue_hot(u, id) };
            }
        });
        hot.clear();
        pcs.clear();
    }
}

/// The unit is going away (end of `makepad test` / run): the background O3 and sampler
/// threads must not touch it again. They take the unit lock for every access to it, so
/// holding the lock from here on (never released: the process is ending) stops them.
impl Drop for Unit {
    fn drop(&mut self) {
        crate::domain::forget_tls_keys(self.syms.dom0());
        let options = &self.prog.session.clone().options;
        if !self.retired {
            crate::borrowck::lazy::stop();
        }
        // checks ahead in flight finish against this unit's program first
        {
            let _w = front_write();
            self.front.alive.store(false, Ordering::SeqCst);
        }
        self.front.wake.notify_all();
        // Only the unit the process-wide O3 machinery (tier threads: TIER_UNIT; sampler:
        // UNIT) can reach is stopped, and its lock is kept: a build that loaded the pointer
        // must never resume on a freed unit. Any other unit (never ran code, never tiered)
        // leaves the lock alone -- taking it here left it held by a thread that was gone, and
        // the next unit dropped on another thread spun in lock_unit forever.
        let me = self as *mut Unit;
        if self.tier1.is_some() && (TIER_UNIT.load(Ordering::SeqCst) == me || unsafe { UNIT } == me) {
            TIER_STOP.store(true, Ordering::SeqCst);
            if profile_path().is_some() {
                profile_write();
            }
            if t1prof_on(options) {
                t1prof_finish();
            }
            TIER_UNIT.store(std::ptr::null_mut(), Ordering::SeqCst);
            if !owner::is_owner() {
                // A popped O3 job registers in TIER_ACTIVE before it loads TIER_UNIT, so
                // once TIER_UNIT is null and TIER_ACTIVE is empty no build can still reach this
                // unit. The sampler uses UNIT only under the lock: clear it under the lock.
                // Keeping the lock instead (the old way) left it held by a thread that may be
                // gone, and the next unit retired on another thread spun forever
                // (serve::lazy_tests in the parallel lib suite).
                while !TIER_ACTIVE.lock().unwrap_or_else(|e| e.into_inner()).is_empty() {
                    std::thread::yield_now();
                }
                owner::call(owner::Class::App, || {
                    if unsafe { UNIT } == me {
                        unsafe { UNIT = std::ptr::null_mut() };
                    }
                });
            }
        }
        // a retired unit's code and data go with it (nothing calls into it any more); the
        // program's own unit lives to process exit
        if !std::thread::panicking() { self.flush_query_errors(true); }
        if self.retired {
            let rest = UNIT_DATA_SIZE + UNIT_RW_SIZE + (crate::syms::SYM_CAP * 8) as u64;
            os::mem::release(self.exec_base, self.exec_size as usize, rest as usize);
            // the slot segments: thunks and stubs first, then the table words and tier records
            for seg in 0..crate::rir::SLOT_SEGS {
                if self.slots.has_seg(seg) {
                    let count = crate::rir::SlotTable::seg_slots(seg);
                    os::mem::release(self.slots.seg_base(0, seg), count * (THUNK_SIZE + STUB_SIZE), count * (8 + TIER_REC));
                }
            }
        }
    }
}

/// An auxiliary unit (the proc-macro world) is about to be dropped mid-run: compile jobs are
/// parked, the speculative queue forgets it (its slot ids mean nothing to another unit) and the
/// runtime's unit pointer no longer names it. Its Drop then leaves the process-wide machinery
/// alone. Called with no unit lock held.
pub unsafe fn retire_unit(up: *mut Unit) {
    pause_compile_jobs(false);
    owner::call(owner::Class::App, || {
        (&mut *up).retired = true;
        if SPEC_UNIT.load(Ordering::Acquire) == up {
            SPEC_UNIT.store(std::ptr::null_mut(), Ordering::Release);
            SPEC_Q.lock().unwrap_or_else(|e| e.into_inner()).clear();
        }
        if UNIT == up {
            UNIT = std::ptr::null_mut();
        }
    });
    resume_compile_jobs(false);
}

/// MAKEPAD_RUN_O3_TRACE: why and when the build in flight for a slot was queued
static TRACE_WHY: relax_session::sync::Mutex<std::collections::BTreeMap<u32, (u8, u64)>> = relax_session::sync::Mutex::new(std::collections::BTreeMap::new());
/// MAKEPAD_RUN_O3_TRACE: builds per slot so far
static TRACE_BUILDS: relax_session::sync::Mutex<std::collections::BTreeMap<u32, u32>> = relax_session::sync::Mutex::new(std::collections::BTreeMap::new());

fn trace_build(u: &Unit, id: u32, what: &str, build_ns: u64) {
    let (why, at) = TRACE_WHY.lock().unwrap().remove(&id).unwrap_or((255, 0));
    let n = {
        let _k = crate::zone_alloc::kept();
        let mut b = TRACE_BUILDS.lock().unwrap();
        let n = b.entry(id).or_insert(0);
        *n += 1;
        *n
    };
    let now = wall_ns();
    eprintln!(
        "O3b @{:.3} #{} {} {} build#{} why={} waited={:.0}ms build={:.1}ms q={}",
        now as f64 / 1e9,
        id,
        u.fns[id as usize].name,
        what,
        n,
        Q_WHY.get(why as usize).copied().unwrap_or("?"),
        now.saturating_sub(at) as f64 / 1e6,
        build_ns as f64 / 1e6,
        TIER_QUEUE.lock().unwrap().len()
    );
}

/// O3 is on for this process (O1 then runs the cheaper pipeline)
pub(crate) static TIER1_ENABLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

static TIER_STOP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Queues `#[relax(O3)]` slot `id` for its O3 build ahead of hotness (if it is still counting).
unsafe fn queue_marked(u: &Unit, id: u32) {
    let rec = tier_rec(&u.slots, id);
    let code = (*rec.add(1)).load(Ordering::Relaxed);
    let stub = (*rec.add(2)).load(Ordering::Relaxed);
    if (*rec.add(3)).compare_exchange(T_COUNTING, T_QUEUED, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
        let s = u.slots.table(id) as *const AtomicU64;
        let _ = (*s).compare_exchange(stub, code, Ordering::SeqCst, Ordering::SeqCst);
        TIER_QUEUED.fetch_add(1, Ordering::Relaxed);
        tier_push(id, Q_MARKED, 0);
        TIER_THREAD.call_once(|| spawn_tier1_threads(&u.prog.session.runtime, &u.prog.session.options));
    }
}

/// Queues slot `id` for O3 as the count stub's rt_hot does (if it is still counting).
unsafe fn queue_hot(u: &Unit, id: u32) {
    let rec = tier_rec(&u.slots, id);
    let code = (*rec.add(1)).load(Ordering::Relaxed);
    let stub = (*rec.add(2)).load(Ordering::Relaxed);
    if (*rec.add(3)).compare_exchange(T_COUNTING, T_QUEUED, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
        let s = u.slots.table(id) as *const AtomicU64;
        let _ = (*s).compare_exchange(stub, code, Ordering::SeqCst, Ordering::SeqCst);
        TIER_QUEUED.fetch_add(1, Ordering::Relaxed);
        churn_queued(id);
        tier_push(id, Q_SAMPLES, call_rate(rec, u.tier1.as_ref().map_or(0, |t| t.calls), wall_ns()));
        TIER_THREAD.call_once(|| spawn_tier1_threads(&u.prog.session.runtime, &u.prog.session.options));
    }
}

/// Per-slot record at `SlotTable::tier(slot)`: [counter, O1 code, count stub, state,
/// counting window start (wall_ns), O1 code bytes] (read by JIT code and rt_hot without
/// the lock).
const TIER_REC: usize = 48;
/// O1 code at most this big tiers up on its call rate alone (no sampler word: samples
/// rarely land in a 12-byte `as_ptr`, and a O1 kernel calling it then stayed 6x rustc --
/// bench_rt rc_refcell); its build is cheap either way
const SMALL_CODE: u64 = 256;
/// A count stub reaching zero queues its slot only if the TIER1_CALLS calls came within this
/// window (a rate, not a total): otherwise the count starts again. Counts never decayed: in a
/// UI everything called once per frame reached 2000 within a minute and O3 never went
/// quiet (Stage playing P(DOOM): 3 O3 threads busy at 90 s).
const CALL_WINDOW_NS: u64 = 1_000_000_000;
/// TIER1_CALLS calls within this: hot without the sampler's word
const CALL_FAST_NS: u64 = 10_000_000;
const T_COUNTING: u64 = 1;
const T_QUEUED: u64 = 2;
// The sampler cannot queue a recorded instance before its first call.
const T_REPLAY: u64 = 3;

fn tier_rec(slots: &crate::rir::SlotTable, id: u32) -> *const AtomicU64 {
    slots.tier(id) as *const AtomicU64
}

static TIER_UNIT: std::sync::atomic::AtomicPtr<Unit> = std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());
// Protected in queue -> active order: a popped job stays pinned through snapshot, codegen
// and install (including unwinding). T_QUEUED also means already tiered, so it cannot be
// used as the settle pin. The unit lock alone does not cover off-lock O3 work.
static TIER_ACTIVE: relax_session::sync::Mutex<Vec<u32>> = relax_session::sync::Mutex::new(Vec::new());

struct TierActive(u32);
impl Drop for TierActive {
    fn drop(&mut self) {
        let mut active = TIER_ACTIVE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(i) = active.iter().position(|id| *id == self.0) { active.swap_remove(i); }
        drop(active);
        TIER1_FINISHED.fetch_add(1, Ordering::Relaxed);
    }
}

/// queued O3 builds: (slot, why: Q_*, queued at wall_ns, calls/sec at enqueue)
static TIER_QUEUE: relax_session::sync::Mutex<Vec<(u32, u8, u64, u64)>> = relax_session::sync::Mutex::new(Vec::new());
const Q_CALLS: u8 = 0;
const Q_SAMPLES: u8 = 1;
const Q_RETIER: u8 = 2;
const Q_MARKED: u8 = 3;
const Q_WHY: [&str; 4] = ["calls", "samples", "retier", "marked"];
/// the sampler's recent samples per slot (halved every HEAT_HALF_NS): which queued build
/// goes first
static HEAT: relax_session::sync::Mutex<Option<std::collections::HashMap<u32, u32>>> = relax_session::sync::Mutex::new(None);
const HEAT_HALF_NS: u64 = 1_000_000_000;

fn tier1_trace() -> bool {
    static T: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *T.get_or_init(|| std::env::var_os("MAKEPAD_RUN_O3_TRACE").is_some())
}

/// Snapshot the rate before bypassing the count stub: queued slots no longer count calls.
/// An exhausted counter can wrap when several threads enter the stub together.
unsafe fn call_rate(rec: *const AtomicU64, budget: u64, now: u64) -> u64 {
    let left = (*rec).load(Ordering::Relaxed);
    let calls = if left > budget { budget } else { budget - left };
    let elapsed = now.saturating_sub((*rec.add(4)).load(Ordering::Relaxed)).max(1);
    ((calls as u128 * 1_000_000_000 / elapsed as u128).min(u64::MAX as u128)) as u64
}

fn tier_push(id: u32, why: u8, rate: u64) {
    let mut q = TIER_QUEUE.lock().unwrap();
    q.push((id, why, wall_ns(), rate));
    if tier1_trace() {
        // no unit lock here (rt_hot): the slot, not its name
        eprintln!("O3q @{:.3} queued #{} ({}) q={}", wall_ns() as f64 / 1e9, id, Q_WHY[why as usize], q.len());
    }
    drop(q);
    TIER_WAKE.notify_all();
}

/// Current sampled CPU first (including hits inside loops), call rate next, then oldest.
/// Scan once under the heat lock: O(queue), no stale heap keys when samples change/halve,
/// and only ~1,000 cheap lookups for a build that takes milliseconds.
fn tier_pop(q: &mut Vec<(u32, u8, u64, u64)>) -> Option<(u32, u8, u64, u64)> {
    let heat = HEAT.lock().unwrap();
    let best = q.iter().enumerate().max_by_key(|(k, e)| {
        (heat.as_ref().and_then(|h| h.get(&e.0)).copied().unwrap_or(0), e.3, std::cmp::Reverse(e.2), std::cmp::Reverse(*k))
    }).map(|(k, _)| k)?;
    Some(q.remove(best))
}
static TIER_WAKE: relax_session::sync::Condvar = relax_session::sync::Condvar::new();
static TIER_THREAD: std::sync::Once = std::sync::Once::new();

/// Three workers by default. MAKEPAD_RUN_O3_ADAPTIVE=1 opts into additional low-priority
/// workers admitted only after startup and while >32 builds remain. Queue length is a
/// cheap demand proxy, not an idle-core measurement: leave at least one core to the program, cap at half the machine, and let
/// the OS prefer foreground work. Explicit overrides keep a fixed pool.
fn spawn_tier1_threads(runtime: &crate::options::RuntimeOptions, options: &crate::options::Options) {
    let cores = std::thread::available_parallelism().map_or(2, |n| n.get());
    let cap = cores.saturating_sub(1).max(1);
    let forced = runtime.tier1_threads;
    let (normal, max) = if options.tier1_profile.is_some() {
        (1, 1)
    } else if let Some(n) = forced {
        let n = n.clamp(1, cap);
        (n, n)
    } else if runtime.tier1_adaptive {
        (3.min(cap), 3.max(cores / 2).min(cap))
    } else {
        (3.min(cap), 3.min(cap))
    };
    for k in 0..max {
        let runtime = crate::zone_alloc::kept_clone(runtime);
        { let _k = crate::zone_alloc::kept();
        let _ = std::thread::Builder::new().name(format!("run-o3-{}", k)).spawn(move || { let _z = crate::zone_alloc::compiler_thread(); tier1_thread(&runtime, k, normal) });
        }
    }
}

/// Opt-in only: no extra clocks, pass timing, allocations or statistics locks when off.
fn tier1_stats_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("MAKEPAD_RUN_O3_STATS").as_deref() == Ok("1"))
}

#[derive(Default)]
struct Tier1Stats {
    builds: u64,
    cpu: u64,
    phases: [u64; 3], // snapshot (including RIR rebuild), backend, install
    passes: [u64; 16], // backend only; snapshot compiles are counted in snapshot
    waits: Vec<u64>,
    installs: Vec<u64>, // counts in 1 s buckets since process wall-clock origin
    active: usize,
    max_active: usize,
}
static TIER1_STATS: relax_session::sync::Mutex<Option<Tier1Stats>> = relax_session::sync::Mutex::new(None);

struct Tier1Active;
impl Tier1Active {
    fn new(queued_at: u64) -> Self {
        let waited = wall_ns().saturating_sub(queued_at);
        let mut g = TIER1_STATS.lock().unwrap();
        let s = g.get_or_insert_with(Tier1Stats::default);
        crate::zone_alloc::kept_with(|| s.waits.push(waited));
        s.active += 1;
        s.max_active = s.max_active.max(s.active);
        Self
    }
}
impl Drop for Tier1Active {
    fn drop(&mut self) {
        if let Some(s) = TIER1_STATS.lock().unwrap().as_mut() { s.active -= 1; }
    }
}

/// RAII accounts for early returns and unwinding too. Only completed attempts contribute
/// CPU totals: a process exiting with builds in flight reports their count separately.
struct Tier1Measure {
    start: u64,
    last: u64,
    phase: usize,
    phases: [u64; 3],
    passes: [u64; 16],
    pass_start: [u64; 16],
    installed: bool,
}
impl Tier1Measure {
    fn new(start: u64) -> Self {
        Self { start, last: start, phase: 0, phases: [0; 3], passes: [0; 16], pass_start: [0; 16], installed: false }
    }
    fn phase(&mut self, phase: usize, now: u64) {
        self.phases[self.phase] += now.saturating_sub(self.last);
        if self.phase == 1 {
            let end = crate::opt::pass_local();
            for i in 0..16 { self.passes[i] += end[i] - self.pass_start[i]; }
        }
        self.phase = phase;
        self.last = now;
        if phase == 1 {
            crate::opt::pass_timing_on();
            self.pass_start = crate::opt::pass_local();
        }
    }
}
impl Drop for Tier1Measure {
    fn drop(&mut self) {
        let now = cpu_ns();
        self.phase(2, now);
        let mut g = TIER1_STATS.lock().unwrap();
        let s = g.get_or_insert_with(Tier1Stats::default);
        s.builds += 1;
        s.cpu += now.saturating_sub(self.start);
        for i in 0..3 { s.phases[i] += self.phases[i]; }
        for i in 0..16 { s.passes[i] += self.passes[i]; }
        if self.installed {
            let bucket = (wall_ns() / 1_000_000_000) as usize;
            crate::zone_alloc::kept_with(|| s.installs.resize(s.installs.len().max(bucket + 1), 0));
            s.installs[bucket] += 1;
        }
    }
}

pub(crate) fn tier1_stats_report() {
    if !tier1_stats_on() { return; }
    let g = TIER1_STATS.lock().unwrap();
    let empty = Tier1Stats::default();
    let s = g.as_ref().unwrap_or(&empty);
    let ms = |ns: u64| ns as f64 / 1e6;
    eprintln!("makepad-run o3-stats: builds={} cpu_ms={:.3} max_threads={} in_flight={} (completed attempts; Windows CPU is cycle-estimated)", s.builds, ms(s.cpu), s.max_active, s.active);
    eprintln!("makepad-run o3-stats: phases_cpu_ms snapshot={:.3} backend={:.3} install={:.3}", ms(s.phases[0]), ms(s.phases[1]), ms(s.phases[2]));
    let passes: Vec<String> = crate::opt::PASS_NAMES.iter().enumerate().map(|(i, n)| format!("{}={:.3}", n, ms(s.passes[i]))).collect();
    eprintln!("makepad-run o3-stats: backend_pass_ms {} (pass slots can overlap; MAKEPAD_RUN_PASS_TIMES=wall selects wall time)", passes.join(" "));
    let mut waits = s.waits.clone();
    waits.sort_unstable();
    let pct = |p: usize| ms(waits.get((waits.len() * p).div_ceil(100).saturating_sub(1)).copied().unwrap_or(0));
    eprintln!("makepad-run o3-stats: queue_wait_ms n={} p50={:.3} p90={:.3} p95={:.3} p99={:.3} max={:.3}", waits.len(), pct(50), pct(90), pct(95), pct(99), pct(100));
    let buckets: Vec<String> = s.installs.iter().enumerate().map(|(i, n)| format!("{}s:{}", i, n)).collect();
    eprintln!("makepad-run o3-stats: installs_per_second {} (since process clock start; last bucket partial)", buckets.join(" "));
}

/// Wall time (wall_ns) of the latest lazy or speculative compile's start or end. Used by
/// missing-RIR reconstruction and the background borrow checker, which need the front end
/// to be quiet. Worker admission separately tracks foreground demand below.
static COMPILE_LAST: AtomicU64 = AtomicU64::new(0);
static STARTUP_COMPILES: AtomicU64 = AtomicU64::new(0);

// A long compile must not look quiet merely because its start was >500 ms ago.
// Include type checking before the unit lock, and release this count on panic too.
// Only one O3 worker competes with foreground startup compiles (mini, Amp: first
// present +23% with three). Admission follows demand, not speculative work.
// A speculative tail can keep COMPILE_LAST fresh long after the hot loop starts. Keep the
// stricter all-compiler quiet rule below for expensive missing-RIR reconstruction only.
static DEMAND_COMPILES: AtomicU64 = AtomicU64::new(0);
static DEMAND_LAST: AtomicU64 = AtomicU64::new(0);

struct StartupCompile { foreground: bool }
impl StartupCompile {
    fn new() -> Self {
        STARTUP_COMPILES.fetch_add(1, Ordering::SeqCst);
        Self { foreground: false }
    }
    fn foreground() -> Self {
        DEMAND_COMPILES.fetch_add(1, Ordering::SeqCst);
        STARTUP_COMPILES.fetch_add(1, Ordering::SeqCst);
        Self { foreground: true }
    }
}
impl Drop for StartupCompile {
    fn drop(&mut self) {
        let now = wall_ns();
        COMPILE_LAST.store(now, Ordering::SeqCst);
        STARTUP_COMPILES.fetch_sub(1, Ordering::SeqCst);
        if self.foreground {
            DEMAND_LAST.store(now, Ordering::SeqCst);
            DEMAND_COMPILES.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

// One admission polling interval without demand compiles demonstrates program progress.
// Recheck for every job: renewed foreground compilation immediately throttles new builds
// back to worker zero. In-flight builds always finish and install, even during startup.
const TIER1_POLL: std::time::Duration = std::time::Duration::from_millis(50);
fn tier1_workers_ready() -> bool {
    DEMAND_COMPILES.load(Ordering::SeqCst) == 0
        && wall_ns().saturating_sub(DEMAND_LAST.load(Ordering::SeqCst)) >= TIER1_POLL.as_nanos() as u64
}

fn tier1_quiet() -> bool {
    STARTUP_COMPILES.load(Ordering::SeqCst) == 0
        && wall_ns().saturating_sub(COMPILE_LAST.load(Ordering::SeqCst)) >= TIER1_QUIET_NS
}
/// No lazy / speculative compile for this long: missing RIR may be rebuilt under the lock.
const TIER1_QUIET_NS: u64 = 500_000_000;
/// slots queued hot (rt_hot runs without the lock; Stats reads it)
pub static TIER_QUEUED: AtomicU64 = AtomicU64::new(0);

impl Unit {
    /// A patch/recycle replaced a twin's code: count against its new address, even
    /// if the old version was already queued or optimised (unit lock held).
    pub(crate) fn rearm_sandbox_counter(&mut self, id: u32, code: u64) {
        debug_assert!(matches!(self.fns[id as usize].key, FnKey::Sandbox(_)));
        if self.tier1.is_some() {
            unsafe { (*tier_rec(&self.slots, id).add(3)).store(0, Ordering::Release); }
            self.arm_counter(id, code);
        }
    }

    /// After a O1 compile: count calls through the slot (unit lock held).
    fn arm_counter(&mut self, id: u32, code: u64) {
        let counted = match &self.fns[id as usize].key {
            FnKey::Inst(d, _) => self.prog.def(*d).kind != DefKind::ForeignFn,
            FnKey::Closure(..) => true,
            FnKey::Sandbox(base) => *base != u32::MAX,
            FnKey::Shim(_) | FnKey::Glue(..) => false,
        };
        self.arm_counter_as(id, code, counted);
    }

    /// arm_counter for slots `ids` (id, code) installed together (the startup image's): their
    /// count stubs made as one block of code.
    fn arm_counters(&mut self, ids: &[(u32, u64)]) {
        if !self.frequency_profiles.is_empty() {
            for &(id, _) in ids {
                self.frequency_profiles.remove(&id);
            }
        }
        let Some(calls) = self.tier1.as_ref().map(|t| t.calls) else { return };
        if ids.is_empty() {
            return;
        }
        // (profile replay arms slot by slot: the image is prelinked with names then)
        if t1prof_replay(&self.prog.session.options) {
            for &(id, code) in ids {
                self.arm_counter_as(id, code, true);
            }
            return;
        }
        let now = wall_ns();
        let mut buf = Vec::with_capacity(ids.len() * 48);
        let mut stubs = Vec::with_capacity(ids.len());
        unsafe {
            for &(id, code) in ids {
                let rec = tier_rec(&self.slots, id);
                if (*rec.add(3)).load(Ordering::Relaxed) != 0 {
                    continue;
                }
                (*rec).store(calls, Ordering::Relaxed);
                (*rec.add(4)).store(now, Ordering::Relaxed);
                (*rec.add(5)).store(self.fns[id as usize].code_len as u64, Ordering::Relaxed);
                (*rec.add(1)).store(code, Ordering::Relaxed);
                buf.resize((buf.len() + 15) & !15, 0xcc);
                stubs.push((id, buf.len() as u64));
                buf.extend_from_slice(&x64::count_stub(rec as u64, id, code, self.hot_entry));
            }
            if stubs.is_empty() {
                return;
            }
            let at = self.emit_code(&buf);
            for (id, off) in stubs {
                let rec = tier_rec(&self.slots, id);
                (*rec.add(2)).store(at + off, Ordering::Relaxed);
                (*rec.add(3)).store(T_COUNTING, Ordering::Release);
                let slot = self.slots.table(id) as *const AtomicU64;
                (*slot).store(at + off, Ordering::SeqCst);
            }
        }
        let me = self as *mut Unit;
        if let Err(other) = TIER_UNIT.compare_exchange(std::ptr::null_mut(), me, Ordering::AcqRel, Ordering::Acquire) {
            if other != me {
                panic!("makepad: a second unit with O3 in this process (tier-up state is per process): give every other unit `tier1 = None`");
            }
        }
    }

    /// arm_counter for a slot whose kind the caller knows (`counted`: an instance that is not
    /// a foreign fn, a closure or a sandbox twin).
    fn arm_counter_as(&mut self, id: u32, code: u64, counted: bool) {
        self.frequency_profiles.remove(&id);
        let mut calls = match &self.tier1 {
            Some(t) => t.calls,
            None => return,
        };
        if !counted {
            return;
        }
        let recorded = t1prof_replay(&self.prog.session.options) && unsafe {
            T1PROF.lock().unwrap().as_ref().unwrap().map.contains_key(&t1_key(self, id))
        };
        if recorded { calls = 1; }
        let rec = tier_rec(&self.slots, id);
        unsafe {
            if (*rec.add(3)).load(Ordering::Relaxed) != 0 {
                return;
            }
            (*rec).store(calls, Ordering::Relaxed);
            (*rec.add(4)).store(wall_ns(), Ordering::Relaxed);
            (*rec.add(5)).store(self.fns[id as usize].code_len as u64, Ordering::Relaxed);
            (*rec.add(1)).store(code, Ordering::Relaxed);
            // Once the threshold-one stub has fired, another caller must also enter
            // rt_hot and wait for the build, rather than underflowing into O1.
            let fallback = if recorded { self.emit_code(&x64::stub(id, self.hot_entry)) } else { code };
            let stub = x64::count_stub(rec as u64, id, fallback, self.hot_entry);
            let at = self.emit_code(&stub);
            (*rec.add(2)).store(at, Ordering::Relaxed);
            (*rec.add(3)).store(if recorded { T_REPLAY } else { T_COUNTING }, Ordering::Release);
            let slot = self.slots.table(id) as *const AtomicU64;
            (*slot).store(at, Ordering::SeqCst);
        }
        // tier-up state is process-wide (TIER_UNIT, the hot queue, the sampler: slot ids with
        // no unit attached): a second tiering unit would get its hot slots built against the
        // first one's table (code installed into the wrong slots). One per process, loudly.
        let me = self as *mut Unit;
        if let Err(other) = TIER_UNIT.compare_exchange(std::ptr::null_mut(), me, Ordering::AcqRel, Ordering::Acquire) {
            if other != me {
                panic!("makepad: a second unit with O3 in this process (tier-up state is per process): give every other unit `tier1 = None`");
            }
        }
    }
}

impl Unit {
    /// Queues O3 slot `id` to be built again (a callee it kept as a never-run call has
    /// run since); at most RETIERS times per slot.
    fn retier(&mut self, id: u32) {
        if t1prof_replay(&self.prog.session.options) { return; }
        const RETIERS: u8 = 4;
        let _k = crate::zone_alloc::kept();
        let n = self.retiers.entry(id).or_insert(0);
        if *n >= RETIERS {
            return;
        }
        *n += 1;
        self.stats.tier1_retier += 1;
        churn_queued(id);
        tier_push(id, Q_RETIER, 0);
    }
}

rt_fn! {
/// The count stub's hot path (no unit lock): stop counting, queue the slot, continue in
/// O1.
fn rt_hot(slot: u64) -> u64 {
    let _z = crate::zone_alloc::compiler();
    let _keep = os::proc::KeepLastError::new();
    let up = TIER_UNIT.load(Ordering::Acquire);
    unsafe {
        let u = &*up;
        u.exec_event(slot as u32, "call");
        let rec = tier_rec(&u.slots, slot as u32);
        let code = (*rec.add(1)).load(Ordering::Relaxed);
        let stub = (*rec.add(2)).load(Ordering::Relaxed);
        let s = u.slots.table(slot as u32) as *const AtomicU64;
        if (*rec.add(3)).load(Ordering::Acquire) == T_REPLAY {
            if startup_holds_tier1() {
                if (*rec.add(3)).compare_exchange(T_REPLAY, T_QUEUED, Ordering::AcqRel, Ordering::Acquire).is_ok() {
                    TIER_QUEUED.fetch_add(1, Ordering::Relaxed);
                    let _ = (*s).compare_exchange(stub, code, Ordering::SeqCst, Ordering::SeqCst);
                    tier_push(slot as u32, Q_CALLS, 0);
                    TIER_THREAD.call_once(|| spawn_tier1_threads(&u.prog.session.runtime, &u.prog.session.options));
                }
                return code;
            }
            let _job = compile_job();
            ensure_sufficient_stack(|| {
                // One unit call for the whole build, its back end included. Concurrent
                // callers wait for it.
                owner::call(owner::program_class(), || {
                    if (*rec.add(3)).load(Ordering::Acquire) == T_REPLAY {
                        TIER_QUEUED.fetch_add(1, Ordering::Relaxed);
                        let ok = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| compile_tier1(up, slot as u32)));
                        if let Err(_) = ok {
                            // (a cancelled nested build too: the O1 code stays; never unwound
                            // through the JIT frames that called rt_hot)
                            t1prof_divergence("first-call-build", &t1_key(up, slot as u32));
                            (*s).store(code, Ordering::SeqCst);
                        } else if !matches!(ok, Ok(true)) {
                            t1prof_divergence("first-call-build", &t1_key(up, slot as u32));
                            (*s).store(code, Ordering::SeqCst);
                        }
                        (*rec.add(3)).store(T_QUEUED, Ordering::Release);
                        t1prof_finish_quiet();
                    }
                });
            });
            return (*s).load(Ordering::SeqCst);
        }
        let now = wall_ns();
        // the sampler has not seen it lately: many calls, little CPU (a cheap function a
        // frame calls a few dozen times) -- a O3 build would cost more than it saves
        // (no samples at all: no sampler on this platform -- the count alone decides)
        let unseen = || HEAT.lock().unwrap().as_ref().map_or(false, |h| h.get(&(slot as u32)).copied().unwrap_or(0) == 0);
        let took = now.saturating_sub((*rec.add(4)).load(Ordering::Relaxed));
        // ... unless the calls came very fast (a tight loop: hot before the sampler's first
        // ticks see it -- bench_rt rc_refcell's cold run stayed at O1, 6x rustc)
        let small = (*rec.add(5)).load(Ordering::Relaxed) <= SMALL_CODE;
        if (*rec.add(3)).load(Ordering::Relaxed) == T_COUNTING && (took > CALL_WINDOW_NS || (took > CALL_FAST_NS && !small && unseen())) {
            // too slow a rate, or no CPU to show: count again from now (the stub stays)
            if let Some(t) = &u.tier1 {
                (*rec).store(t.calls, Ordering::Relaxed);
            }
            (*rec.add(4)).store(now, Ordering::Relaxed);
            return code;
        }
        // a patch may have replaced the stub in the meantime: then that code runs
        let _ = (*s).compare_exchange(stub, code, Ordering::SeqCst, Ordering::SeqCst);
        if (*rec.add(3)).compare_exchange(T_COUNTING, T_QUEUED, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
            TIER_QUEUED.fetch_add(1, Ordering::Relaxed);
            churn_queued(slot as u32);
            tier_push(slot as u32, Q_CALLS, call_rate(rec, u.tier1.as_ref().map_or(0, |t| t.calls), now));
            TIER_THREAD.call_once(|| spawn_tier1_threads(&u.prog.session.runtime, &u.prog.session.options));
        }
        (*s).load(Ordering::SeqCst)
    }
}
}

/// Runs this compile worker (speculation, O3: never the critical path, which the program's
/// own threads compile) below the program's threads, so a launched app's background compiles
/// do not take the CPU from a UI -- its own, or a launcher's (the runner's frames stuttered to
/// 208 ms under a child's workers). macOS: the default QoS class at relative priority -15
/// (still on performance cores; QOS_CLASS_UTILITY made startup 9-14% slower on the mini);
/// Linux: nice +5 per thread; Windows: THREAD_PRIORITY_BELOW_NORMAL.
/// MAKEPAD_RUN_WORKER_QOS=0 keeps the default, =utility uses QOS_CLASS_UTILITY (A/B).
/// Background-thread priority for this thread (the cache's pruning).
pub fn lower_this_thread_priority(runtime: &crate::options::RuntimeOptions) {
    lower_worker_priority(runtime);
}

fn lower_worker_priority(runtime: &crate::options::RuntimeOptions) {
    let mode = runtime.worker_qos as u8;
    if mode == 0 {
        return;
    }
    os::thread::lower_priority(mode == 2);
}

fn tier1_thread(runtime: &crate::options::RuntimeOptions, k: usize, normal: usize) {
    let _mem = crate::memstat::tag(crate::memstat::Tag::Tier1);
    ensure_altstack();
    lower_worker_priority(runtime);
    loop {
        let (id, why, queued_at, _) = {
            let mut q = TIER_QUEUE.lock().unwrap();
            loop {
                if TIER_STOP.load(Ordering::SeqCst) { return; }
                // Recheck admission on every wake and pop, including a wake from an empty
                // queue during startup. Already-running builds finish when demand drops.
                let ready = tier1_workers_ready();
                if !startup_holds_tier1() && (k == 0 || ready) && (k < normal || q.len() > 32) {
                    if let Some(e) = tier_pop(&mut q) {
                        TIER_ACTIVE.lock().unwrap_or_else(|e| e.into_inner()).push(e.0);
                        break e;
                    }
                }
                q = TIER_WAKE.wait_timeout(q, TIER1_POLL).unwrap().0;
            }
        };
        let _active = TierActive(id);
        if tier1_trace() {
            crate::zone_alloc::kept_with(|| TRACE_WHY.lock().unwrap().insert(id, (why, queued_at)));
            eprintln!("O3p @{:.6} #{} worker={} phase=popped ready={} startup={} last_compile={:.6} demand={} last_demand={:.6}", wall_ns() as f64 / 1e9, id, k, tier1_workers_ready(), STARTUP_COMPILES.load(Ordering::SeqCst), COMPILE_LAST.load(Ordering::SeqCst) as f64 / 1e9, DEMAND_COMPILES.load(Ordering::SeqCst), DEMAND_LAST.load(Ordering::SeqCst) as f64 / 1e9);
        }
        let up = TIER_UNIT.load(Ordering::Acquire);
        if up.is_null() || TIER_STOP.load(Ordering::SeqCst) {
            continue;
        }
        let _job = compile_job();
        if why == Q_CALLS || why == Q_SAMPLES {
            owner::call(owner::Class::Background, || unsafe { o2_hot(up, id) });
        }
        let _active = tier1_stats_on().then(|| Tier1Active::new(queued_at));
        // a panic escaping the build (snapshot, install: outside its own catch_unwind) ended
        // this thread holding the unit lock: every lazy compile then waited for a thread that
        // was gone (`makepad check makepad-example-uizoo` hung ~1 run in 20-50; the panic was
        // silent: build_rir's hook)
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe { compile_tier1(up, id) }));
        if let Err(p) = r {
            let msg = p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_else(|| "panic".to_string());
            let n = TIER1_PANICS.fetch_add(1, Ordering::Relaxed);
            if n < 3 {
                eprintln!("makepad: O3 build of slot {} panicked: {} -- O3 dropped for it", id, msg);
            }
        }
    }
}

/// Builds slot `id` at O3 and installs it (O3 thread). False: dropped (no RIR,
/// failed, or the slot changed meanwhile).
/// MAKEPAD_RUN_O3_PROFILE=<file>: missing file records the last decisions per instance;
/// an existing file replays those decisions on the instance's first call. Unrecorded
/// instances still tier on live hotness, and are listed separately in the summary.
/// Decisions include the snapshot, never-run and single-reference sets, both inlining
/// rounds' frequencies and the backend frequencies.
struct T1Entry {
    cold: Vec<String>,
    /// callees whose RIR the build had (inline candidates)
    snap: Vec<String>,
    /// Single-reference callees at snapshot time (lazy builds can change live counts).
    single: Vec<String>,
    freq: Option<Vec<f32>>,
    // Frequencies before each inlining round; backend frequencies describe a later RIR.
    inline_freq: Option<[Option<Vec<f32>>; 2]>,
    input: u64,
    code: u64,
}
struct T1Profile {
    replay: bool,
    path: String,
    map: std::collections::HashMap<String, T1Entry>,
    installed: usize,
    live: std::collections::HashSet<String>,
    differ: Vec<String>,
    /// replay: keys installed already (later hot triggers do not rebuild them)
    done: std::collections::HashSet<String>,
}
static T1PROF: relax_session::sync::Mutex<Option<T1Profile>> = relax_session::sync::Mutex::new(None);

fn t1prof_on(options: &crate::options::Options) -> bool {
    static E: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *E.get_or_init(|| {
        let _k = crate::zone_alloc::kept();
        let Some(p) = options.tier1_profile.as_ref().and_then(|p| p.to_str()) else { return false };
        let p = p.to_string();
        let mut map = std::collections::HashMap::new();
        let replay = match options.tier1_profile_data.as_deref() {
            Some(t) if !t.is_empty() => {
                for l in t.lines() {
                    let x: Vec<&str> = l.split('\t').collect();
                    if x.len() != 6 && x.len() != 8 && x.len() != 9 {
                        continue;
                    }
                    let list = |t: &str| -> Vec<String> { if t.is_empty() { Vec::new() } else { t.split('\u{1f}').map(|c| c.to_string()).collect() } };
                    let parse_freq = |v: &str| if v == "-" { None } else { Some(v.split(',').filter_map(|v| u32::from_str_radix(v, 16).ok()).map(f32::from_bits).collect()) };
                    let freq = parse_freq(x[3]);
                    let inline_freq = (x.len() >= 8).then(|| [parse_freq(x[6]), parse_freq(x[7])]);
                    // Append single-reference keys after both rounds; older profiles
                    // have no single-reference facts to replay.
                    let single = x.get(8).map_or_else(Vec::new, |s| list(s));
                    map.insert(x[0].to_string(), T1Entry { cold: list(x[1]), snap: list(x[2]), single, freq, inline_freq, input: u64::from_str_radix(x[4], 16).unwrap_or(0), code: u64::from_str_radix(x[5], 16).unwrap_or(0) });
                }
                true
            }
            _ => false,
        };
        eprintln!("makepad: O3 profile {} ({}, {} entries)", p, if replay { "replay" } else { "record" }, map.len());
        *T1PROF.lock().unwrap() = Some(T1Profile { replay, path: p, map, installed: 0, live: std::collections::HashSet::new(), differ: Vec::new(), done: std::collections::HashSet::new() });
        true
    })
}

fn t1_hash<T: std::hash::Hash + ?Sized>(x: &T) -> u64 {
    use std::hash::Hasher;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    x.hash(&mut h);
    h.finish()
}

/// Instance identity independent of slot allocation and RIR availability. Pretty
/// type names alone collide (e.g. two modules' Entry types), so include stable hashes.
unsafe fn t1_key(up: *const Unit, id: u32) -> String {
    let u = &*up;
    let ty = |t| format!("{}#{:032x}", crate::typeck::ty_to_string(false, &u.prog, &u.tcx, t), crate::stable::type_hash(&u.prog, &u.tcx, t));
    let inst = match &u.fns[id as usize].key {
        FnKey::Inst(d, a) => {
            let mut s = format!("{}@{:?}", u.prog.def_path(*d), d);
            for t in a.iter() { s.push(','); s.push_str(&ty(*t)); }
            s
        }
        FnKey::Closure(file, e, parent) => format!("closure:{}:{}:{}", file, e, t1_key(up, *parent)),
        FnKey::Shim(shim) => crate::lower::shim_name(&u.prog, &u.tcx, shim),
        FnKey::Glue(g, t) => format!("glue{}:{}", g, ty(*t)),
        FnKey::Sandbox(b) if (*b as usize) < u.fns.len() => format!("sandbox:{}", t1_key(up, *b)),
        FnKey::Sandbox(b) => format!("sandbox:{}", b),
    };
    inst.replace('\t', " ")
}

/// Hash of a RIR dump without fn slot numbers, symbol ids and host addresses (they differ
/// between runs).
fn t1_norm_hash(rir: &crate::rir::Func) -> u64 {
    let d = rir.dump();
    // The key already identifies the instance. Closure display names include the
    // parent's process-local slot ("{closure#n}@slot"), which is not a code input.
    let d = d.find(" params ").map_or(d.as_str(), |i| &d[i..]);
    let b = d.as_bytes();
    let mut norm: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        // FnAddr(VReg(v), slot): the slot
        if b[i..].starts_with(b"FnAddr(VReg(") {
            while i < b.len() && b[i] != b')' {
                norm.push(b[i]);
                i += 1;
            }
            norm.extend_from_slice(b"), ");
            i += 3;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            continue;
        }
        let skip_after = [&b"Fn("[..], &b"id: "[..], &b"CHost("[..], &b"Addr(VReg("[..]];
        let mut hit = false;
        for t in skip_after {
            if b[i..].starts_with(t) {
                norm.extend_from_slice(t);
                i += t.len();
                while i < b.len() && b[i].is_ascii_digit() {
                    i += 1;
                }
                hit = true;
                break;
            }
        }
        if !hit {
            norm.push(b[i]);
            i += 1;
        }
    }
    t1_hash(&norm)
}

fn t1prof_replay(options: &crate::options::Options) -> bool {
    t1prof_on(options) && T1PROF.lock().unwrap().as_ref().is_some_and(|p| p.replay)
}

fn t1prof_divergence(kind: &str, key: &str) {
    if let Some(p) = T1PROF.lock().unwrap().as_mut() {
        crate::zone_alloc::kept_with(|| p.differ.push(format!("{} {}", kind, key)));
    }
}

fn t1prof_summary(p: &T1Profile) -> String {
    let mut missing: Vec<_> = p.map.keys().filter(|k| !p.done.contains(*k)).collect();
    missing.sort();
    let mut live: Vec<_> = p.live.iter().collect();
    live.sort();
    let mut kinds = std::collections::BTreeMap::new();
    for d in &p.differ { *kinds.entry(d.split(' ').next().unwrap_or("unknown")).or_insert(0usize) += 1; }
    let mut out = format!("installed {} / recorded {} / unrecorded-live {} / divergences {}\n",
        p.installed, p.map.len(), p.live.len(), p.differ.len());
    for (kind, count) in kinds { out.push_str(&format!("{} {}\n", kind, count)); }
    for k in missing { out.push_str(&format!("MISSING {}\n", k)); }
    for k in live { out.push_str(&format!("UNRECORDED-LIVE {}\n", k)); }
    for d in &p.differ { out.push_str(&format!("{}\n", d)); }
    out
}

/// Keep the summary complete even when the process exits without dropping the Unit.
fn t1prof_finish_quiet() {
    let g = T1PROF.lock().unwrap();
    let Some(p) = g.as_ref() else { return };
    if p.replay { let _ = std::fs::write(format!("{}.replay", p.path), t1prof_summary(p)); }
}

fn t1prof_finish() {
    let g = T1PROF.lock().unwrap();
    let Some(p) = g.as_ref() else { return };
    if p.replay {
        let out = t1prof_summary(p);
        let _ = std::fs::write(format!("{}.replay", p.path), &out);
        eprintln!("makepad: O3 profile replay: {}", out.lines().next().unwrap_or(""));
        return;
    }
    let mut keys: Vec<&String> = p.map.keys().collect();
    keys.sort();
    let mut out = String::new();
    for k in keys {
        let e = &p.map[k];
        let fmt_freq = |f: &Option<Vec<f32>>| match f {
            Some(f) => f.iter().map(|x| format!("{:x}", x.to_bits())).collect::<Vec<_>>().join(","),
            None => "-".to_string(),
        };
        let inline = e.inline_freq.as_ref().expect("recorded inlining frequencies");
        out.push_str(&format!("{}\t{}\t{}\t{}\t{:016x}\t{:016x}\t{}\t{}\t{}\n", k, e.cold.join("\u{1f}"), e.snap.join("\u{1f}"), fmt_freq(&e.freq), e.input, e.code, fmt_freq(&inline[0]), fmt_freq(&inline[1]), e.single.join("\u{1f}")));
    }
    let _ = std::fs::write(&p.path, out);
    if !p.differ.is_empty() {
        eprintln!("makepad: O3 profile record: {} fns rebuilt with equal inputs gave different code: {}", p.differ.len(), p.differ.iter().take(8).cloned().collect::<Vec<_>>().join(", "));
    }
}

/// MAKEPAD_RUN_O3_CHURN=1: every 5 s, O3 work on stderr -- builds and their CPU in the period,
/// installed / dropped / retier requests, the queue length and its oldest entry's age, and the
/// fns built most often so far (builds, CPU, retiers). For long-running apps where the O3
/// threads never settle.
fn churn_on() -> bool {
    static E: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *E.get_or_init(|| std::env::var_os("MAKEPAD_RUN_O3_CHURN").is_some())
}
struct Churn {
    /// slot -> (builds, cpu ns, installed)
    per: std::collections::HashMap<u32, (u32, u64, u32)>,
    period_builds: u32,
    period_ns: u64,
    period_installed: u32,
    period_dropped: u32,
    last: Option<std::time::Instant>,
    start: Option<std::time::Instant>,
}
static CHURN: std::sync::LazyLock<relax_session::sync::Mutex<Churn>> = std::sync::LazyLock::new(|| relax_session::sync::Mutex::new(Churn { per: std::collections::HashMap::new(), period_builds: 0, period_ns: 0, period_installed: 0, period_dropped: 0, last: None, start: None }));
/// slot -> wall time it was queued (oldest-entry age; MAKEPAD_RUN_O3_CHURN only)
static CHURN_QUEUED_AT: relax_session::sync::Mutex<Option<std::collections::HashMap<u32, std::time::Instant>>> = relax_session::sync::Mutex::new(None);

fn churn_queued(id: u32) {
    if churn_on() {
        let _k = crate::zone_alloc::kept();
        let mut g = CHURN_QUEUED_AT.lock().unwrap();
        g.get_or_insert_with(std::collections::HashMap::new).entry(id).or_insert_with(std::time::Instant::now);
    }
}

unsafe fn churn_note(up: *mut Unit, id: u32, ns: u64, installed: bool) {
    if let Some(m) = CHURN_QUEUED_AT.lock().unwrap().as_mut() {
        m.remove(&id);
    }
    let mut c = CHURN.lock().unwrap();
    let now = std::time::Instant::now();
    let start = *c.start.get_or_insert(now);
    let e = crate::zone_alloc::kept_with(|| c.per.entry(id).or_insert((0, 0, 0)));
    e.0 += 1;
    e.1 += ns;
    if installed {
        e.2 += 1;
    }
    c.period_builds += 1;
    c.period_ns += ns;
    if installed {
        c.period_installed += 1;
    } else {
        c.period_dropped += 1;
    }
    let last = *c.last.get_or_insert(now);
    if now.duration_since(last).as_secs_f64() < 5.0 {
        return;
    }
    c.last = Some(now);
    let u = &*up;
    let qlen = TIER_QUEUE.lock().map(|q| q.len()).unwrap_or(0);
    let oldest = CHURN_QUEUED_AT.lock().unwrap().as_ref().and_then(|m| m.iter().min_by_key(|x| *x.1).map(|(i, t)| (*i, now.duration_since(*t).as_secs_f64())));
    let mut top: Vec<(&u32, &(u32, u64, u32))> = c.per.iter().collect();
    top.sort_by(|a, b| b.1 .1.cmp(&a.1 .1));
    let total_builds: u32 = c.per.values().map(|x| x.0).sum();
    eprintln!(
        "O3-churn t={:.0}s: period {} builds ({} installed, {} dropped) {:.0} ms cpu; total {} builds of {} fns, {} retier requests; queue {} (oldest {})",
        now.duration_since(start).as_secs_f64(),
        c.period_builds,
        c.period_installed,
        c.period_dropped,
        c.period_ns as f64 / 1e6,
        total_builds,
        c.per.len(),
        u.stats.tier1_retier,
        qlen,
        oldest.map_or("-".to_string(), |(i, a)| format!("{} queued {:.1}s ago", u.fns[i as usize].name, a))
    );
    for (i, (b, ns, inst)) in top.iter().take(8) {
        eprintln!("  {:>4} builds {:>4} installed {:>7.1} ms  retiers {}  {}", b, inst, *ns as f64 / 1e6, u.retiers.get(*i).copied().unwrap_or(0), u.fns[**i as usize].name);
    }
    c.period_builds = 0;
    c.period_ns = 0;
    c.period_installed = 0;
    c.period_dropped = 0;
}

fn trace_tier1_phase(id: u32, phase: &str) {
    if tier1_trace() {
        eprintln!("O3p @{:.6} #{} phase={}", wall_ns() as f64 / 1e9, id, phase);
    }
}

pub unsafe fn compile_tier1(up: *mut Unit, id: u32) -> bool {
    let _census_context = (&*up).exec_context(crate::exec_census::active() && crate::exec_profile::is_call(up as usize, id));

    let _cost = crate::fncost::Record::new(id, 1, "O3");
    let session = unsafe { (*std::ptr::addr_of!((*up).prog.session)).clone() };
    let options = &session.options;
    let runtime = &session.runtime;
    // Replay has one active O3 build, including live fallback builds: the whole build is one
    // unit call (its back end too), so it cannot race a caller's first-call build.
    if t1prof_replay(options) && !owner::is_owner() {
        return owner::call(owner::Class::Background, || compile_tier1(up, id));
    }
    trace_tier1_phase(id, "start");
    // Also pin direct callers (cache verification / embedders), not only queue workers.
    TIER_ACTIVE.lock().unwrap_or_else(|e| e.into_inner()).push(id);
    let _active = TierActive(id);
    let t0 = cpu_ns();
    let mut measure = tier1_stats_on().then(|| Tier1Measure::new(t0));
    // code installed from the disk cache has no RIR: build it here (the front end and lowering
    // run on this thread), else a cached function would stay at O1 for good. Not while the
    // program still compiles (or links) its way to a frame: that front end would hold the unit
    // the main thread is waiting for.
    let needs_rir = |up: *mut Unit| {
        let f = &(&*up).fns[id as usize];
        f.compiled && f.rir.is_none() && f.build_err.is_none() && !f.building
    };
    let (needs, key, front) = owner::call(owner::Class::Background, || (needs_rir(up), (&*up).fns[id as usize].key.clone(), (&*up).front.clone()));
    if needs && !t1prof_replay(options) && !owner::is_owner() {
        while !tier1_quiet() {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        // its type check runs first, off the unit (the build takes the checked body): one
        // solver search there took seconds, and every thread needing a lazy compile -- the UI
        // thread handling a close (Files closed while indexing: 13-14 s) -- waited for it.
        // A front-end writer (live patch) preempts it; the build then checks the body itself.
        if let FnKey::Inst(d, args) = key {
            let _preemptible = crate::preempt::Preemptible::on();
            typeck_ahead(&front, d, args, 0, false, false);
        }
    }
    drop(front);
    let gave_way = std::cell::Cell::new(false);
    let snapped = owner::call(owner::Class::Background, || {
        trace_tier1_phase(id, "snapshot");
        cost_name(up, id);
        compile_watch(Some((&format!("tier1 locked: {}", (&*up).fns[id as usize].name), 0)));
        if needs_rir(up) {
            let t = wall_ns();
            let _ = build_rir(up, id);
            TIER1_RIR_REBUILT[0].fetch_add(1, Ordering::Relaxed);
            TIER1_RIR_REBUILT[1].fetch_add(wall_ns() - t, Ordering::Relaxed);
        }
        if (&*up).fns[id as usize].rir.as_ref().is_some_and(|r| r.baseline) {
            let _ = build_rir_mode(up, id, RirMode::Normal);
        }
        // A caller's version alone cannot protect an in-flight build: a patch may change
        // a callee before this build has registered its inlined_into edge. Capture the
        // epoch before any snapshot yields, and reject the entire old snapshot on install.
        let epoch = PATCH_EPOCH.load(Ordering::SeqCst);
        let (rir, version) = {
            let u = &*up;
            let f = &u.fns[id as usize];
            match (&f.rir, f.compiled) {
                (Some(r), c) if c || o3_first() => (r.clone(), f.version),
                _ => {
                    let why = if !f.compiled {
                        "not compiled"
                    } else if f.build_err.is_some() {
                        "no RIR: build failed"
                    } else {
                        "no RIR"
                    };
                    drop_why(up, why);
                    (&mut *up).stats.tier1_dropped += 1;
                    if tier1_trace() {
                        trace_build(&*up, id, if f.rir.is_none() { "dropped(no rir)" } else { "dropped(not compiled)" }, 0);
                    }
                    return None;
                }
            }
        };
        // deterministic O3 (MAKEPAD_RUN_O3_PROFILE): recorded fns replay once;
        // unrecorded fns still tier on live hotness.
        let t1key: Option<String> = if t1prof_on(options) { Some(t1_key(up, id)) } else { None };
        if let Some(k) = &t1key {
            let g = T1PROF.lock().unwrap();
            let p = g.as_ref().unwrap();
            if p.replay && p.done.contains(k) {
                drop(g);
                drop_why(up, "replay: done");
                (&mut *up).stats.tier1_dropped += 1;
                return None;
            }

        }
        let sandboxed = matches!((&*up).fns[id as usize].key, FnKey::Sandbox(_));
        let mut callees: Vec<u32> = Vec::new();
        for b in &rir.blocks {
            for i in &b.insts {
                if let crate::rir::Inst::Call(crate::rir::Callee::Fn(c), _, _) | crate::rir::Inst::FnAddr(_, c) = i {
                    // The transform already confined this body. Inlining a host API here
                    // would introduce unconfined memory operations after that boundary.
                    if !sandboxed && *c != id && !callees.contains(c) {
                        callees.push(*c);
                    }
                }
            }
        }
        // callees the O3 policy wants inlinable get their RIR built, one per lock hold;
        // then their callees (outside cold blocks), 16 levels (a binary search closure sits ~10 deep), built the same way: an inlined
        // body still calls what O1 did not inline into it (quicksort -> stable_partition ->
        // partition_one; HashMap::get -> probe_find -> Group::next)
        let (src_toks, inline_all) = {
            let t = (&*up).tier1.as_ref().unwrap();
            (t.src_toks, t.opt.inline_all)
        };
        // deterministic O3, replay: the recorded build's never-run, snapshot and single sets
        let t1_rec: Option<(Vec<String>, Vec<String>, Vec<String>)> = t1key.as_ref().and_then(|k| {
            let g = T1PROF.lock().unwrap();
            let p = g.as_ref().unwrap();
            if p.replay { p.map.get(k).map(|e| (e.cold.clone(), e.snap.clone(), e.single.clone())) } else { None }
    });
    let mut input_pins = Vec::new();
    let mut ensure_rir = |c: u32, force: bool, callers: &[u32]| {
        // a program thread waits for the owner: the snapshot gives way (built RIR is kept;
        // the slot is queued again)
        if gave_way.get() || !t1prof_replay(options) && !o3_first() && owner::serving().0 == owner::Class::Background && owner::urgent() {
            gave_way.set(true);
            return;
        }
        // Each prepared callee stays readable until the final Arc snapshots exist.
        TIER_ACTIVE.lock().unwrap_or_else(|e| e.into_inner()).push(c);
        input_pins.push(TierActive(c));
        let u = &mut *up;
        u.sync_entry_ref(c);
        if u.fns[c as usize].rir.as_ref().is_some_and(|r| r.baseline) {
            let _ = build_rir_mode(up, c, RirMode::Normal);
        }
        let u = &mut *up;
        let f = &u.fns[c as usize];
        if f.rir.is_some() || f.building || f.build_err.is_some() {
            return;
        }
        // a closure whose enclosing fn came from the disk cache: its body is checked here (a
        // cold run had it from the parent's own compile)
        if let FnKey::Closure(cfile, _, parent) = f.key {
            if !u.has_body(parent, cfile) {
                let _ = ensure_parent_body(up, parent, cfile, 0);
            }
        }
        let u = &mut *up;
        let f = &u.fns[c as usize];
        let attr = match &f.key {
            FnKey::Inst(d, _) if u.prog.def(*d).kind != DefKind::ForeignFn => inline_attrs(&u.prog, *d),
            FnKey::Shim(_) | FnKey::Glue(..) => 0,
            FnKey::Closure(cf, _, parent) if u.has_body(*parent, *cf) => 0,
            _ => return,
        };
        // a callee that ran with code from the disk cache has no RIR: in a cold run its own
        // compile left one (inlinable whatever its size), so build it here as well
        let cached_code = f.compiled;
        let single = crate::opt::single_on(options) && u.single_ref(c)
            && (u.fns[id as usize].rir_refs.contains(&c)
                || callers.iter().any(|p| u.fns[*p as usize].rir.is_some() && u.fns[*p as usize].rir_refs.contains(&c)));
        let source_size = src_size(u, c);
        if force || cached_code || source_size <= src_toks || attr & crate::opt::ATTR_ALWAYS != 0
            || (single && source_size <= TIER1_SINGLE_SRC_TOKS) {
            compile_watch(Some((&format!("O3 snapshot: build_rir {}", u.fns[c as usize].name), 0)));
            let _ = build_rir(up, c);
            compile_watch(None);
        }
    };
    let mut frontier = callees.clone();
    const LEVELS: usize = 16;
    for level in 0..LEVELS {
        if frontier.is_empty() {
            break;
        }
        for &c in &frontier {
            let force = t1_rec.as_ref().is_some_and(|(_, snap, _)| snap.contains(&t1_key(up, c)));
            ensure_rir(c, force, &callees);
        }
        if level == LEVELS - 1 {
            break;
        }
        let u = &*up;
        let mut next = Vec::new();
        for &c in &frontier {
            let cf = &u.fns[c as usize];
            if let Some(r) = &cf.rir {
                // Cost-model admissions also expose calls in never-run bodies.
                let never_run = match &t1_rec {
                    Some((cold, _, _)) => cold.contains(&t1_key(up, c)),
                    None => !cf.compiled && cf.history().inlined_into.is_empty(),
                };
                let cost_admits = !crate::opt::opt_off(options, "-costmodel")
                    && callee_attrs(&u.prog, &u.tcx, &u.fns, u.core_crate, c, inline_all, true)
                        .map_or(false, |attrs| crate::opt::rustc_inline_cost(r, c) <= crate::opt::rustc_inline_threshold(r, attrs));
                if never_run && crate::opt::inst_count(r) > crate::opt::INLINE_FREE_INSTS && !cost_admits {
                    continue;
                }
                for b in &r.blocks {
                    if matches!(b.term, crate::rir::Term::Unreachable | crate::rir::Term::Resume) {
                        continue;
                    }
                    for i in &b.insts {
                        // calls, and functions whose address is taken (a call through it becomes
                        // a direct call in O3: `.map(GlyphId)`)
                        let x = match i {
                            crate::rir::Inst::Call(crate::rir::Callee::Fn(x), _, _) | crate::rir::Inst::FnAddr(_, x) => x,
                            _ => continue,
                        };
                        {
                            if *x != id && !callees.contains(x) && callees.len() < 1024 {
                                callees.push(*x);
                                next.push(*x);
                            }
                        }
                    }
                }
            }
        }
        frontier = next;
    }
    // Recorded descendants can be absent from the live traversal (different
    // O1 inlining). Resolve all known keys, repeating when lowering adds slots.
    if let Some((_, recorded, _)) = &t1_rec {
        let mut seen = 0;
        while seen < (&*up).fns.len() {
            let end = (&*up).fns.len();
            for c in seen..end {
                if recorded.contains(&t1_key(up, c as u32)) {
                    ensure_rir(c as u32, true, &callees);
                    if !callees.contains(&(c as u32)) { callees.push(c as u32); }
                }
            }
            seen = end;
        }
    }
    if gave_way.get() {
        return None;
    }
    for &c in &callees { (&mut *up).sync_entry_ref(c); }
    let u = &*up;
    if std::env::var_os("MAKEPAD_RUN_O3_TRACE").is_some() {
        eprintln!("O3 snapshot of {}: {} callees", u.fns[id as usize].name, callees.len());
    }
    let mut snap: Vec<(u32, std::sync::Arc<crate::rir::Func>, u8)> = Vec::new();
    let mut t1_cold_keys: Vec<String> = Vec::new();
    let mut t1_snap_keys: Vec<String> = Vec::new();
    for &c in &callees {
        let f = &u.fns[c as usize];
        let attrs = match callee_attrs(&u.prog, &u.tcx, &u.fns, u.core_crate, c, inline_all, true) {
            Some(a) => a,
            None => continue,
        };
        let live_cold = !f.compiled && f.history().inlined_into.is_empty();
        if let Some(r) = &f.rir {
            let cold = match &t1key {
                Some(k) => {
                    let ck = t1_key(up, c);
                    // replay: exactly the recorded snapshot
                    if let Some((_, rs, _)) = &t1_rec {
                        if !rs.contains(&ck) {
                            continue;
                        }
                    }
                    t1_snap_keys.push(ck.clone());
                    let g = T1PROF.lock().unwrap();
                    let p = g.as_ref().unwrap();
                    let cold = if p.replay { p.map.get(k).map_or(live_cold, |e| e.cold.contains(&ck)) } else { live_cold };
                    drop(g);
                    if cold {
                        t1_cold_keys.push(ck);
                    }
                    cold
                }
                None => live_cold,
            };
            let attrs = if cold { attrs | crate::opt::ATTR_COLD } else { attrs };
            snap.push((c, r.clone(), attrs));
        }
    }
    t1_cold_keys.sort();
    drop(input_pins);
    t1_cold_keys.dedup();
    t1_snap_keys.sort();
    t1_snap_keys.dedup();
    if let Some((_, recorded, _)) = &t1_rec {
        for k in recorded {
            if !t1_snap_keys.contains(k) { t1prof_divergence("snapshot-missing", k); }
        }
    }
    // Freeze both the single-reference fact and its O1 owner before unlocking.
    // An exposed call is eligible only while inlining that owner's body, never just
    // because an unrelated snapshot body happened to reference the same slot.
    let mut owners = HashMap::new();
    for (owner, body) in std::iter::once((id, &*rir)).chain(snap.iter().map(|x| (x.0, &*x.1))) {
        for i in body.blocks.iter().flat_map(|b| &b.insts) {
            if let crate::rir::Inst::Call(crate::rir::Callee::Fn(c), _, _) | crate::rir::Inst::FnAddr(_, c) = i {
                let entry = owners.entry(*c).or_insert((owner, 0usize));
                entry.1 += 1;
            }
        }
    }
    let mut singles = HashMap::new();
    let mut t1_single_keys = Vec::new();
    for &(c, _, _) in &snap {
        let key = t1_key(up, c);
        let is_single = match &t1_rec {
            Some((_, _, recorded)) => recorded.contains(&key),
            None => u.single_ref(c) && !u.recursive_fn(c),
        };
        if !is_single { continue; }
        if let Some(&(owner, 1)) = owners.get(&c) {
            if owner != c {
                singles.insert(c, owner);
                t1_single_keys.push(key);
            }
        }
    }
    t1_single_keys.sort();
    // recorded frequencies (replay)
    let t1_freq: Option<Option<Vec<f32>>> = t1key.as_ref().and_then(|k| {
        let g = T1PROF.lock().unwrap();
        let p = g.as_ref().unwrap();
        if p.replay { p.map.get(k).map(|e| e.freq.clone()) } else { None }
    });
    let t1_inline_freq = t1key.as_ref().and_then(|k| {
        let g = T1PROF.lock().unwrap();
        let p = g.as_ref().unwrap();
        if p.replay { p.map.get(k).map(|e| e.inline_freq.clone()) } else { None }
    });
    let ro = u.ro_reader();
    let env = x64::Env { slots: u.env.slots, tls_key: u.env.tls_key, tls_slow: u.env.tls_slow, resume: u.env.resume, opt_regs: !t1_quick_ra(options), syms: u.env.syms, sym_class: u.env.sym_class };
    let cfg = u.tier1.as_ref().unwrap().opt;
    let name = u.fns[id as usize].name.to_string();
    // One immutable sample input for register allocation. Both inlining rounds
    // use structural frequencies from their current CFG, independent of when
    // the sampler reaches this evidence threshold.
    let sampled_positions = if options.no_block_freq || t1prof_replay(options) { None } else {
        u.frequency_profiles.get(&id).filter(|h| h.samples >= FREQUENCY_SAMPLES)
            .map(|h| h.positions.clone())
    };
    if tier1_trace() {
        eprintln!("O3-frequency input @{:.3} ms allocation={} samples={} {}",
            crate::clock::ms_since_start(), if sampled_positions.is_some() { "sampled" } else { "structural" },
            u.frequency_profiles.get(&id).map_or(0, |h| h.samples), name);
    }
    let t1 = cpu_ns();
    Some((rir, version, epoch, t1key, sandboxed, snap, singles, t1_cold_keys, t1_snap_keys, t1_single_keys, t1_freq, t1_inline_freq, ro, env, cfg, name, sampled_positions, t1))
    });
    let Some((rir, version, epoch, t1key, sandboxed, snap, singles, t1_cold_keys, t1_snap_keys, t1_single_keys, t1_freq, t1_inline_freq, ro, env, cfg, name, sampled_positions, t1)) = snapped else {
        if gave_way.get() {
            tier_push(id, Q_RETIER, 0);
        }
        return false;
    };
    if let Some(m) = measure.as_mut() { m.phase(1, t1); }
    trace_tier1_phase(id, "backend");
    // optimise and generate code without the lock
    let built = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<_, String> {
        let _scratch = crate::zone_alloc::scratch(runtime);
        let mut func = (*rir).clone();
        let get = |c: u32| snap.iter().find(|x| x.0 == c).map(|x| (&*x.1 as *const crate::rir::Func, x.2));
        let single = |owner: u32, c: u32| singles.get(&c) == Some(&owner);
        let trace = tier1_trace();
        if trace {
            crate::opt::pass_timing_on();
        }
        let (p0, n0) = if trace { (crate::opt::pass_local(), crate::opt::inst_count(&func)) } else { ([0; 16], 0) };
        let mut inline_freq = [None, None];
        let mut frequencies = |round: usize, f: &crate::rir::Func| {
            let freq = match &t1_inline_freq {
                Some(Some(fr)) => match &fr[round] {
                    Some(v) if v.len() != f.blocks.len() => {
                        t1prof_divergence("inline-frequency-shape", t1key.as_deref().unwrap());
                        None
                    }
                    v => v.clone(),
                },
                Some(None) => {
                    t1prof_divergence("inline-frequency-missing", t1key.as_deref().unwrap());
                    None
                }
                None if options.no_block_freq => None,
                None => Some(inline_structural_freqs(f)),
            };
            inline_freq[round] = freq.clone();
            freq
        };
        let queries = crate::nounwind::QueryCx::new(&snap);
        let (optimized, inline_rules) = queries.optimize(id, func, &mut |func, nounwind| {
            Ok(tier1_optimize(options, func, id, &name, &get, &single, ro, &cfg, nounwind, &mut frequencies))
        })?;
        func = optimized;
        let mut inlined = Vec::with_capacity(inline_rules.len());
        for (c, _) in inline_rules { inlined.push(c); }
        let nounwind_used = func.nounwind_inputs.clone();
        if let Some(w) = debug_env().0.as_deref() {
            if dump_rir_match(&name, w) {
                eprintln!("O3 RIR:\n{}", func.dump());
            }
        }
        let tv = crate::opt::pass_clock();
        let verified = func.verify();
        crate::opt::pass_time(13, tv);
        verified.map_err(|e| format!("RIR verify after O3: {}", e))?;
        let freq = match &t1_freq {
            Some(Some(fr)) if fr.len() == func.blocks.len() => Some(fr.clone()),
            Some(None) => None,
            Some(Some(_)) => {
                if let Some(k) = &t1key { t1prof_divergence("frequency-shape", k); }
                None
            }
            _ => sampled_positions.as_ref().map(|h| sampled_freqs(&func, h))
                .or_else(|| (!options.no_block_freq).then(|| inline_structural_freqs(&func))),
        };
        let mps = crate::memstat::peak_start();
        if dump_base(&name).is_some() {
            // tools/run/tools/asmcmp: code offset -> RIR block/instruction of the dumped fn
            x64::set_rir_map(true);
        }
        let c = crate::memstat::tagged(crate::memstat::Tag::Codegen, || x64::compile_freq(options, &func, &env, freq.as_deref()));
        crate::memstat::note_big("codegen O3", &func.name, mps, || cg_shape(&func));
        let c = c?;
        let mut profile_entry = None;
        if let Some(k) = &t1key {
            let bits = |f: &Option<Vec<f32>>| f.as_ref().map(|f| f.iter().map(|x| x.to_bits()).collect::<Vec<u32>>());
            let input = t1_hash(&(t1_cold_keys.clone(), t1_snap_keys.clone(), t1_single_keys.clone(), bits(&freq), inline_freq.iter().map(bits).collect::<Vec<_>>()));
            // the optimised RIR (normalised) and the code size: code bytes hold slot numbers that
            // differ between runs
            let code = t1_norm_hash(&func) ^ (c.text_len as u64).rotate_left(48);
            let _k = crate::zone_alloc::kept();
            let mut g = T1PROF.lock().unwrap();
            let p = g.as_mut().unwrap();
            match p.map.get(k) {
                Some(e) if p.replay && e.input != input => p.differ.push(format!("input-differ {} (cold {}/{}, snapshot {}/{})", k, t1_cold_keys.len(), e.cold.len(), t1_snap_keys.len(), e.snap.len())),
                Some(e) if e.input == input && e.code != code => p.differ.push(format!("output-differ {}", k)),
                _ => {}
            }
            if !p.replay {
                profile_entry = Some(T1Entry { cold: t1_cold_keys.clone(), snap: t1_snap_keys.clone(), single: t1_single_keys.clone(), freq: freq.clone(), inline_freq: Some(inline_freq.clone()), input, code });
            }
        }
        note_frame(id, c.frame_size, slot_bytes(&func), true);
        if trace && cpu_ns() - t1 > 1_000_000_000 {
            // a slow build: where its time went (passes, sizes)
            let p1 = crate::opt::pass_local();
            let mut v: Vec<(u64, &str)> = (0..16).map(|i| (p1[i] - p0[i], crate::opt::PASS_NAMES[i])).collect();
            v.sort_by(|a, b| b.0.cmp(&a.0));
            let split: Vec<String> = v.iter().take(6).map(|(ns, n)| format!("{} {:.0}ms", n, *ns as f64 / 1e6)).collect();
            eprintln!(
                "O3 slow build {}: {:.0} ms, {} insts / {} blocks in, {} insts / {} blocks out, {} inlined, {} snapshot callees, code {} bytes; passes: {}",
                name,
                (cpu_ns() - t1) as f64 / 1e6,
                n0,
                rir.blocks.len(),
                crate::opt::inst_count(&func),
                func.blocks.len(),
                inlined.len(),
                snap.len(),
                c.code.len(),
                split.join(", ")
            );
            let r = crate::regalloc::ra_stats();
            eprintln!(
                "  regalloc: first {:.0} ms, cold-call split {:.0} ms, loop split {:.0} ms; vregs {} blocks {} -> vregs {} blocks {}; {} spilled; frame {} bytes",
                r[0] as f64 / 1e6, r[1] as f64 / 1e6, r[2] as f64 / 1e6, r[3], r[4], r[5], r[6], r[7], c.frame_size
            );
        }
        if let Some(base) = dump_base(&name) {
            let _ = std::fs::write(format!("{}.t1.bin", base), &c.code[..c.text_len as usize]);
            let _ = std::fs::write(format!("{}.t1.frame", base), format!("frame {} slot-area {} of {}\n", c.frame_size, crate::opt::slot_layout(&func, 0, true).1, slot_bytes(&func)));
            let _ = std::fs::write(format!("{}.t1.rir", base), func.dump());
            // offset<TAB>block<TAB>instruction (- = terminator)<TAB>source position, per emitted RIR
            // instruction (tools/run/tools/asmcmp); the RIR is the emitted function (after back-end splits)
            let mut m = String::new();
            let mut pi = 0usize;
            for &(off, b, i) in &c.rir_map {
                while pi + 1 < c.pc_map.len() && c.pc_map[pi + 1].0 <= off {
                    pi += 1;
                }
                let pos = c.pc_map.get(pi).map_or(0, |x| x.1);
                // (a debug dump: the program's files, read off the unit)
                let src = unsafe { (&*up).pos_str((pos >> 32) as u32, pos as u32) };
                let ins = if i == u32::MAX { "-".to_string() } else { i.to_string() };
                m.push_str(&format!("{:x}\t{}\t{}\t{}\n", off, b, ins, src));
            }
            let _ = std::fs::write(format!("{}.t1.map", base), m);
            let _ = std::fs::write(format!("{}.t1.emit.rir", base), &c.rir_emitted);
        }
        // calls left to callees that had never run; whether the code calls itself (the disk
        // cache's frame limit for recursive code)
        let mut cold_left: Vec<u32> = Vec::new();
        let self_call = func.blocks.iter().any(|b| b.insts.iter().any(|i| matches!(i, crate::rir::Inst::Call(crate::rir::Callee::Fn(c), _, _) if *c == id)));
        for b in &func.blocks {
            if matches!(b.term, crate::rir::Term::Unreachable | crate::rir::Term::Resume) {
                continue;
            }
            for i in &b.insts {
                if let crate::rir::Inst::Call(crate::rir::Callee::Fn(c), _, _) = i {
                    if get(*c).map_or(false, |x| x.1 & crate::opt::ATTR_COLD != 0) && !cold_left.contains(c) {
                        cold_left.push(*c);
                    }
                }
            }
        }
        // the calls O3 left (MAKEPAD_RUN_O3_TRACE): (callee slot, in a cold block)
        let mut left: Vec<(u32, bool, u8)> = Vec::new();
        if std::env::var_os("MAKEPAD_RUN_O3_TRACE").is_some() {
            for b in &func.blocks {
                let cold = matches!(b.term, crate::rir::Term::Unreachable | crate::rir::Term::Resume);
                for i in &b.insts {
                    if let crate::rir::Inst::Call(crate::rir::Callee::Fn(c), _, _) = i {
                        left.push((*c, cold, get(*c).map_or(255, |x| x.1)));
                    }
                }
            }
        }
        Ok((c.code, c.pc_map, c.relocs, (inlined, left, cold_left, c.frame_size, self_call, nounwind_used, profile_entry)))
    }));
    let t2 = cpu_ns();
    if let Some(m) = measure.as_mut() { m.phase(2, t2); }
    trace_tier1_phase(id, "built");
    let ok = owner::call(owner::Class::Finish, || {
        trace_tier1_phase(id, "install");
        let _cost_install = crate::fncost::phase(6);
        let u = &mut *up;
        u.stats.tier1_locked_ns += (t1 - t0) + (cpu_ns() - t2);
        u.stats.tier1_free_ns += t2 - t1;
        let ok = match built {
            Ok(Ok((code, map, relocs, (inlined, left_calls, cold_left, frame_size, self_call, nounwind_used, profile_entry)))) if PATCH_EPOCH.load(Ordering::SeqCst) == epoch && !PATCH_OPEN.load(Ordering::SeqCst) && u.fns[id as usize].version == version && !crate::inspect::instrument::active(id) && {
                let n = if relax_codegen::interp::blob::is_blob(&code) { code.len() } else { crate::rir::split_unwind_info(&code).0 };
                u.link_check(&code[..n], &relocs)
            } => {
                // Sign O3 machine bytes and symbolic relocations while the unit is locked.
                // The ordinary signature path only covers foreground/O1 compilation.
                if std::env::var_os("MAKEPAD_RUN_RELOC_SIG").is_some() {
                    crate::syms::reloc_sig(u, || format!("O3:{}", sig_name(u, id)), &code, &relocs, &map);
                }
                // Installed O3 code (marked or hot, including retier) is kept as the slot's ordinary
                // entry: the next run starts with it -- not a huge frame (inlining into a recursive
                // fn: makepad_script_compute::lower::expr reached 68 KB per level and overflowed a
                // kernel worker's stack when installed at start). Recursive: it calls itself,
                // directly or through code it inlined. The entry is the code WITH its unwind
                // trailer, as O1's are: a cached install splits it off in Unit::install; without
                // it a panic through the frame skipped its cleanups.
                let recursive = self_call || inlined.contains(&id);
                let frame_max = if recursive { TIER1_CACHE_FRAME_MAX } else { TIER1_CACHE_FRAME_MAX_FLAT };
                if frame_size <= frame_max {
                    if let Some(mut c) = u.cache.take() {
                        c.record_o3(u, id, &code, &relocs, &map, &inlined, &nounwind_used);
                        u.cache = Some(c);
                    }
                }
                u.fns[id as usize].o3.store(true, Ordering::Relaxed);
                if o3_first() {
                    // #[relax(O3, always)]: its first code (no O1/O2 ever), installed as a lazy
                    // compile's is, its O3 state already final (no call counter)
                    let rec = tier_rec(&u.slots, id);
                    if u.tier1.is_some() {
                        (*rec.add(3)).store(T_QUEUED, Ordering::Release);
                    }
                    for &c in &inlined {
                        u.exec_event(c, "inline");
                        if c != id && !u.fns[c as usize].history().inlined_into.contains(&id) {
                            crate::zone_alloc::kept_with(|| u.fns[c as usize].history.get_or_insert_with(Default::default).inlined_into.push(id));
                        }
                    }
                    u.stats.tier1_installed += 1;
                    TIER1_INSTALLS.fetch_add(1, Ordering::Relaxed);
                    let a = install_compiled(up, id, &code, map, &relocs, false);
                    O3_FIRST_AT.with(|c| c.set(a));
                    true
                } else {
                // an interpreted function's O3 build is its relocatable form (interp/blob.rs)
                let blob = relax_codegen::interp::blob::is_blob(&code);
                let (n, info) = if blob { (code.len(), None) } else { crate::rir::split_unwind_info(&code) };
                let code = code[..n].to_vec();
                let u = &mut *up;
                let at = if blob {
                    u.emit_or_redirect(&code, &relocs, "O3").expect("O3 interpreted code installs")
                } else {
                    match if sandboxed { crate::sandbox::emit_twin(u, &code, &relocs) } else { None } {
                        Some(at) => at,
                        None => u.emit_linked(&code, &relocs).expect("O3 code links (checked)"),
                    }
                };
                if sandboxed {
                    crate::sandbox::note_dead(u.fns[id as usize].code_len as u64);
                }
                // Twin spaces can precede the last host code in the address index.
                if !blob {
                    u.index_code(at, code.len() as u32, id, info);
                }
                if perf_map_on() {
                    perf_map(at, code.len(), &format!("{} [O3]", u.fns[id as usize].name));
                }
                let f = &mut u.fns[id as usize];
                let old_map = std::mem::take(&mut f.pc_map);
                crate::zone_alloc::kept_with(|| f.history.get_or_insert_with(Default::default).retired.push((f.code, f.code_len, old_map)));
                f.code = at;
                f.code_len = code.len() as u32;
                f.pc_map = PcMap::new(&map);
                f.version += 1;
                let slot = u.slots.table(id) as *const AtomicU64;
                (*slot).store(at, Ordering::SeqCst);
                // a patch of an inlined callee recompiles this slot (live.rs)
                let n_inl = inlined.len();
                for c in inlined {
                    u.exec_event(c, "inline");
                    if !u.fns[c as usize].history().inlined_into.contains(&id) {
                        crate::zone_alloc::kept_with(|| u.fns[c as usize].history.get_or_insert_with(Default::default).inlined_into.push(id));
                    }
                }
                if !options.no_block_freq && !t1prof_replay(options) {
                    crate::zone_alloc::kept_with(|| { u.frequency_profiles.entry(id).or_default(); });
                    if sampled_positions.is_some() && tier1_trace() {
                        eprintln!("O3-frequency installed @{:.3} ms samples={} {}", crate::clock::ms_since_start(), FREQUENCY_SAMPLES, name);
                    }
                }
                // Publish only installed builds. A rejected/stale build must not
                // replace the profile of the code the program actually executes.
                if let (Some(k), Some(entry)) = (&t1key, profile_entry) {
                    crate::zone_alloc::kept_with(|| T1PROF.lock().unwrap().as_mut().unwrap().map.insert(k.clone(), entry));
                    t1prof_finish();
                }
                // never-run callees kept as calls: rebuilt when one first runs (or already has)
                let mut again = false;
                for c in cold_left {
                    if u.fns[c as usize].compiled {
                        again = true;
                    } else {
                        crate::zone_alloc::kept_with(|| {
                            let v = u.cold_users.entry(c).or_default();
                            if !v.contains(&id) {
                                v.push(id);
                            }
                        });
                    }
                }
                if again && !(t1prof_on(options) && T1PROF.lock().unwrap().as_ref().map_or(false, |p| p.replay)) {
                    u.retier(id);
                }
                u.stats.code_bytes += code.len() as u64;
                u.stats.tier1_installed += 1;
                TIER1_INSTALLS.fetch_add(1, Ordering::Relaxed);
                // deterministic O3, replay: installed (a build whose install was refused is retried
                // by the next trigger)
                if let Some(k) = &t1key {
                    let _k = crate::zone_alloc::kept();
                    let mut g = T1PROF.lock().unwrap();
                    if let Some(p) = g.as_mut() {
                        if p.replay {
                            if p.map.contains_key(k) { p.installed += 1; } else { p.live.insert(k.clone()); }
                            p.done.insert(k.clone());
                        }
                    }
                    drop(g);
                    t1prof_finish_quiet();
                }
                if tier1_trace() {
                    trace_build(u, id, "installed", t2 - t1);
                }
                if std::env::var("MAKEPAD_RUN_O3_TRACE").is_ok() {
                    eprintln!("tier1: {} installed ({} bytes, {} callees inlined, {:.1} ms build, {:.1} ms snapshot)", u.fns[id as usize].name, code.len(), n_inl, (t2 - t1) as f64 / 1e6, (t1 - t0) as f64 / 1e6);
                    let mut left = Vec::new();
                    for (c, cold, snap) in &left_calls {
                        let n = u.fns[*c as usize].rir.as_ref().map(|r| crate::opt::inst_count(r)).unwrap_or(0);
                        let s = if *snap == 255 { format!(" nosnap{}", if u.fns[*c as usize].rir.is_none() { "(norir)" } else { "" }) } else { format!(" a{}", snap) };
                        left.push(format!("{}[{}]{}{}", u.fns[*c as usize].name, n, s, if *cold { " (cold)" } else { "" }));
                    }
                    eprintln!("  calls left: {}", left.join(", "));
                }
                true
                }
            }
            _ => {
                let why = match &built {
                    Err(_) => "build panicked",
                    Ok(Err(_)) => "codegen error",
                    Ok(Ok(_)) if PATCH_EPOCH.load(Ordering::SeqCst) != epoch || PATCH_OPEN.load(Ordering::SeqCst) => "patch changed meanwhile",
                    Ok(Ok(_)) if u.fns[id as usize].version != version => "slot changed meanwhile",
                    Ok(Ok(_)) => "link check failed",
                };
                drop_why(up, why);
                let u = &mut *up;
                u.stats.tier1_dropped += 1;
                // The slot got other code while this build ran -- nearly always its O2 build from
                // the relax-O2 worker, queued together with this one (tier1_thread: o2_hot) and
                // finished first. The slot is T_QUEUED (it no longer counts calls), so a dropped
                // build left hot code at O2 for the rest of the run (sandbox leap: 917 of 3,388 O3
                // builds dropped this way, 17% of the main thread's JIT samples in O2 code at
                // steady state). Build it again from the new version (retier: RETIERS times at most
                // per slot, so a slot a patch keeps changing is not chased).
                if why == "slot changed meanwhile" && !u.fns[id as usize].o3.load(Ordering::Relaxed) {
                    u.retier(id);
                }
                // Replay never retries according to install order; expose a refused install.
                if let Some(k) = &t1key {
                    if t1prof_replay(options) { t1prof_divergence(why, k); }
                }
                if tier1_trace() {
                    let what = match &built {
                        Ok(Ok(_)) if PATCH_EPOCH.load(Ordering::SeqCst) != epoch || PATCH_OPEN.load(Ordering::SeqCst) => "dropped(patch)".to_string(),
                        Ok(Ok(_)) if u.fns[id as usize].version != version => "dropped(version)".to_string(),
                        Ok(Ok(_)) => "dropped(link)".to_string(),
                        Ok(Err(e)) => format!("dropped(err: {})", e.chars().take(120).collect::<String>()),
                        Err(_) => "dropped(panic)".to_string(),
                    };
                    trace_build(u, id, &what, t2 - t1);
                }
                false
            }
        };
        if churn_on() {
            churn_note(up, id, cpu_ns().saturating_sub(t0), ok);
        }
        ok
    });
    if let Some(m) = measure.as_mut() { m.installed = ok; }
    drop(measure);
    ok
}


// ------------------------------------------------------------ pipelined lazy compile
//
// A slot's compile has a front end (type check, lower, the inline candidates' RIR) that needs
// the unit lock, and a back end (inlining, optimisation, code generation) that works on a
// snapshot (`Prep`) and needs no lock. compile_split runs the back end unlocked, so other
// threads' front ends overlap it.
//
// Speculation: when a slot gets code, its direct callees that remain calls are queued
// (depth 1; their callees depth 2, up to MAKEPAD_RUN_SPEC_DEPTH). Worker threads build them with
// compile_split, taking the lock only while nobody waits for it (lock_bg), and keep the
// code in `FnEntry::spec`. The first call installs it (compile_slot_locked). A call that
// meets a build still in flight waits for it instead of building twice. Code built ahead
// and never called is the waste (MAKEPAD_RUN_COMPILE_REPORT column `spec`: 1 unused, 2 used).

pub enum Split {
    Installed(u64),
    Spec,
    Failed(String),
    /// a live patch landed while the back end ran: build again
    Restart,
}

/// Startup admission: 0 = normal, 1 = manifest slots only, 2 = speculation released,
/// 3 = O3 released. The first-present callback only wakes the coordinator;
/// it never takes the unit lock or starts compiler work on the UI thread.
static STARTUP_PHASE: AtomicU64 = AtomicU64::new(0);
// Retain roots (not bodies) so code built during startup can seed its ordinary
// speculation after the signal even when the app makes no more lazy calls.
static STARTUP_ROOTS: relax_session::sync::Mutex<Vec<(usize, u32, u8)>> = relax_session::sync::Mutex::new(Vec::new());
static STARTUP_CACHED: relax_session::sync::Mutex<Vec<(usize, Vec<u32>, u8)>> =
    relax_session::sync::Mutex::new(Vec::new());
pub static FIRST_PRESENT_NS: AtomicU64 = AtomicU64::new(0);
pub(crate) static FIRST_PRESENT: AtomicBool = AtomicBool::new(false);
// Live edits have their own admission gate: disabling speculative startup scheduling
// must not allow the watcher to mutate a program while it is booting.
static LIVE_STARTUP_DONE: AtomicBool = AtomicBool::new(false);
static LIVE_NEEDS_PRESENT: AtomicBool = AtomicBool::new(false);

pub(crate) fn live_needs_present(needs_present: bool) {
    LIVE_NEEDS_PRESENT.store(needs_present, Ordering::Release);
}

/// A program that links makepad_platform takes live edits only after its first present; the
/// quiet release of a headless start (startup_main) admits them only for other programs.
pub(crate) fn live_startup_done() -> bool {
    FIRST_PRESENT.load(Ordering::Acquire)
        || LIVE_STARTUP_DONE.load(Ordering::Acquire) && !LIVE_NEEDS_PRESENT.load(Ordering::Acquire)
}
/// A program that has not presented and made no demand compile for this long is headless:
/// its startup phase ends (startup_main).
const STARTUP_QUIET_NS: u64 = 2_000_000_000;
static STARTUP_THREAD: std::sync::OnceLock<std::thread::Thread> = std::sync::OnceLock::new();

pub fn begin_startup_phase() {
    if std::env::var("MAKEPAD_RUN_STARTUP_PHASE").map_or(true, |v| v != "0") {
        STARTUP_PHASE.store(1, Ordering::Release);
    }
}

fn startup_phase() -> bool {
    STARTUP_PHASE.load(Ordering::Acquire) == 1
}

fn startup_holds_tier1() -> bool {
    matches!(STARTUP_PHASE.load(Ordering::Acquire), 1 | 2)
}

fn defer_startup_root(up: *mut Unit, id: u32, depth: u8) -> bool {
    if !startup_phase() {
        return false;
    }
    let mut q = STARTUP_ROOTS.lock().unwrap();
    if !startup_phase() {
        return false;
    }
    crate::zone_alloc::kept_with(|| q.push((up as usize, id, depth)));
    true
}

/// Whether the program has presented its first frame (the boot screen goes away then).
pub fn first_present_seen() -> bool { FIRST_PRESENT.load(Ordering::Acquire) }

extern "C" fn relax_first_present() {
    crate::domain::first_present();
    if !FIRST_PRESENT.swap(true, Ordering::AcqRel) {
        if EXEC_CHECK.load(Ordering::Relaxed) {
            exec_check_now("first frame");
        }
        FIRST_PRESENT_NS.store(wall_ns(), Ordering::Release);
        crate::solver_work::report();
        crate::exec_census::first_present();
        crate::tcx::Tcx::report_declaration_counters();
        if profile_path().is_some() { crate::exec_profile::first_present(); }
        // the startup manifest's window ends 1 s after the first present at the latest, so the
        // run's warm record is written soon after it (run.rs), not 5 s after main
        ORDER_UNTIL_NS.fetch_min(wall_ns() + 1_000_000_000, Ordering::Relaxed);
        crate::tui::finish(true);
        crate::tui::print_slowest();
        crate::fncost::mark("first present");
        o2_release_later();
        if o2_trace() {
            let n = |k: usize| O2_STATS[k].load(Ordering::Relaxed);
            let hit = BASE.lock().map_or(0, |g| g.hit.len());
            eprintln!("makepad-run O2 at first present: {} installed ({:.1} ms worker cpu), {} dropped; main thread waited {:.2} ms in {} unit-lock waits on the O2 worker; {} baseline slots sampled (queued)", n(0), n(1) as f64 / 1e6, n(2), n(3) as f64 / 1e6, n(4), hit);
        }
        crate::warm::first_present();
        if let Some(t) = STARTUP_THREAD.get() {
            t.unpark();
        }
        crate::exec_census::report();
        if profile_path().is_some() { crate::exec_profile::report(true); }
    }
}

/// Releases speculation, O1 inline candidates and O3 at the first present, never on wall
/// time (Stage needs 40-58 s from main to present; a clock released speculation in the middle
/// of startup, and main then waited seconds on the unit lock behind speculative builds). A
/// program that never presents (headless) releases once its demand compiles have been quiet
/// for STARTUP_QUIET_NS. No unit pointer is retained: an exiting program can retire its unit
/// while this thread sleeps.
pub fn startup_main(runtime: &crate::options::RuntimeOptions) {
    let main_at = wall_ns();
    let runtime = crate::zone_alloc::kept_clone(runtime);
    let release = move || {
        lower_worker_priority(&runtime);
        let _ = STARTUP_THREAD.set(std::thread::current());
        while !FIRST_PRESENT.load(Ordering::Acquire) {
            let quiet = if DEMAND_COMPILES.load(Ordering::SeqCst) == 0 {
                wall_ns().saturating_sub(DEMAND_LAST.load(Ordering::SeqCst).max(main_at))
            } else {
                0
            };
            // the live-edit gate (live_startup_done) waits for a present on its own: a
            // platform program that never presents (a headless bench, a hidden tool) still
            // releases speculation and O3 on quiet
            if quiet >= STARTUP_QUIET_NS {
                break;
            }
            std::thread::park_timeout(std::time::Duration::from_nanos(STARTUP_QUIET_NS.saturating_sub(quiet).max(50_000_000)));
        }
        LIVE_STARTUP_DONE.store(true, Ordering::Release);
        if !startup_phase() { return; }
        crate::fncost::mark(if FIRST_PRESENT.load(Ordering::Acquire) {
            "startup released: present"
        } else {
            "startup released: quiet"
        });
        crate::exec_census::startup_release(FIRST_PRESENT.load(Ordering::Acquire));
        STARTUP_PHASE.store(2, Ordering::Release);
        SPEC_WAKE.notify_all();
        set_worker();
        let roots = std::mem::take(&mut *STARTUP_ROOTS.lock().unwrap());
        let cached = std::mem::take(&mut *STARTUP_CACHED.lock().unwrap());
        for (unit, id, depth) in roots {
            let _job = compile_job();
            owner::call(owner::Class::Background, || {
                let up = SPEC_UNIT.load(Ordering::Acquire);
                if !SPEC_STOP.load(Ordering::SeqCst) && !up.is_null() && up as usize == unit {
                    unsafe {
                        spec_enqueue(up, id, depth);
                    }
                }
            });
        }
        for (unit, ids, depth) in cached {
            let _job = compile_job();
            owner::call(owner::Class::Background, || {
                let up = SPEC_UNIT.load(Ordering::Acquire);
                if !SPEC_STOP.load(Ordering::SeqCst) && !up.is_null() && up as usize == unit {
                    unsafe {
                        spec_enqueue_cached(up, &ids, depth);
                    }
                }
            });
        }
        {
            let _job = compile_job();
            owner::call(owner::Class::Background, || {
                let up = SPEC_UNIT.load(Ordering::Acquire);
                if !SPEC_STOP.load(Ordering::SeqCst) && !up.is_null() {
                    unsafe {
                        release_base(up);
                    }
                }
            });
        }
        STARTUP_PHASE.store(3, Ordering::Release);
        TIER_WAKE.notify_all();
        STARTUP_PHASE.store(0, Ordering::Release);
    };
    let _k = crate::zone_alloc::kept();
    if std::thread::Builder::new()
        .name("run-startup".into())
        .spawn(release)
        .is_err()
    {
        // Resource exhaustion must not leave a headless process gated forever.
        STARTUP_PHASE.store(0, Ordering::Release);
    }
}

/// Pipelining and speculation are on only where every compile goes through compile_slot
/// under the unit lock: `makepad run` turns them on (enable_pipelining). Drivers that build
/// without the lock (hrcheck, `makepad live` patches) leave them off. MAKEPAD_RUN_SPEC=0: off.
static PIPELINE: AtomicBool = AtomicBool::new(false);

pub fn enable_pipelining(options: &crate::options::Options) {
    // deterministic O3 (MAKEPAD_RUN_O3_PROFILE): no speculative builds -- a caller prepared while a
    // callee's build is in flight on a speculative worker does not inline it at O1, so O1
    // RIR (O3's input) would depend on the worker race
    PIPELINE.store(options.spec && options.tier1_profile.is_none(), Ordering::SeqCst);
}

fn pipelined() -> bool {
    PIPELINE.load(Ordering::Relaxed)
}

/// Bumped by every live patch and rollback (unit lock held): pipelined and O3 builds
/// whose snapshots predate it are dropped, as their back ends saw the old source.
static PATCH_EPOCH: AtomicU64 = AtomicU64::new(0);

/// A reload transaction is open: its new versions are committed and it releases the unit lock
/// (back ends of its compile batch) before its patch group installs -- or is refused. A O3
/// build must not install meanwhile: it may have inlined RIR built from the new versions into
/// a slot the group does not replace, code that stays if the edit is refused.
static PATCH_OPEN: AtomicBool = AtomicBool::new(false);

/// Opens a reload transaction (unit lock held): builds whose snapshots predate it are stale.
pub fn open_patch() {
    PATCH_OPEN.store(true, Ordering::SeqCst);
    bump_patch_epoch();
}

/// Closes the reload transaction (unit lock held), installed or refused: every O3 build that
/// overlapped any part of it is dropped at its install.
pub fn close_patch() {
    if PATCH_OPEN.swap(false, Ordering::SeqCst) {
        bump_patch_epoch();
    }
}

pub fn bump_patch_epoch() {
    PATCH_EPOCH.fetch_add(1, Ordering::SeqCst);
}

/// MAKEPAD_RUN_SPEC_ORDER=lifo: newest queued first (the callees of the latest compile)
fn spec_lifo(options: &crate::options::Options) -> bool {
    options.spec_order
}

/// MAKEPAD_RUN_SPEC_YIELD=0: worker fronts never give the lock up between bodies (measurement)
fn spec_yield(options: &crate::options::Options) -> bool {
    options.spec_yield
}

/// MAKEPAD_RUN_SPEC_HOT=1: speculate only on the fall-through path's calls
/// Default: fall-through only while the machine runs on battery (less work built ahead that
/// is never called), every warm call on mains power. MAKEPAD_RUN_SPEC_HOT=1/0 overrides.
pub fn spec_hot_only(options: &crate::options::Options) -> bool {
    options.spec_hot.unwrap_or_else(on_battery)
}

/// Is the machine running on battery power (read once, at the first speculation)?
pub fn on_battery() -> bool {
    os::proc::on_battery()
}

/// MAKEPAD_RUN_SPEC_HOT_FROM=n: from depth n on only fall-through calls are speculated (default 2)
fn spec_hot_from(options: &crate::options::Options) -> u8 {
    options.spec_hot_from
}

fn spec_depth_max(options: &crate::options::Options) -> u8 {
    options.spec_depth
}

fn spec_jobs(options: &crate::options::Options) -> usize {
    options.spec_jobs.unwrap_or_else(|| std::thread::available_parallelism().map_or(2, |n| n.get()).saturating_sub(1).clamp(1, 6))
}

static SPEC_UNIT: std::sync::atomic::AtomicPtr<Unit> = std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());
static SPEC_Q: relax_session::sync::Mutex<std::collections::VecDeque<u32>> = relax_session::sync::Mutex::new(std::collections::VecDeque::new());
static SPEC_WAKE: relax_session::sync::Condvar = relax_session::sync::Condvar::new();
static SPEC_START: std::sync::Once = std::sync::Once::new();
/// times a worker's front gave the lock to a waiting thread
pub static SPEC_YIELDS: AtomicU64 = AtomicU64::new(0);
/// calls not speculated because they sit on a cold path
pub static SPEC_COLD_SKIPPED: AtomicU64 = AtomicU64::new(0);
/// a worker's front whose build another thread took over while it was suspended
/// a background front that gave way to a program thread's request (queued again)
const YIELDED: &str = "(background build gave way)";
/// wall ns of split compiles on non-worker threads: [front (locked), back (unlocked),
/// relock wait, waits for in-flight builds]; and the same front/back cpu on workers
pub static SPLIT_NS: [AtomicU64; 6] = [const { AtomicU64::new(0) }; 6];

/// Queues the not-inlined direct callees of slot `id` (lock held), one level deeper.
unsafe fn spec_enqueue(up: *mut Unit, id: u32, depth: u8) {
    if pipelined() && defer_startup_root(up, id, depth) { return; }
    let session = unsafe { (*std::ptr::addr_of!((*up).prog.session)).clone() };
    let options = &session.options;
    if !pipelined() || depth >= spec_depth_max(options) {
        return;
    }
    let u = &mut *up;
    let rir = match &u.fns[id as usize].rir {
        Some(r) => r.clone(),
        None => return,
    };
    spec_enqueue_calls(up, &rir, &[], depth);
}

fn note_entered(_u: &Unit, id: u32) {
    crate::fncost::event("call", id);
}

/// A slot's key that stays the same across runs, and across edits that do not touch the fn
/// (the startup manifest, cache keys): a fn instance is its def path (Program::hint_path) plus its type arguments,
/// a closure its parent's key plus `{closure#n}` (n-th closure of its owner fn), glue its type;
/// closure types inside are named the same way (the explicit stable_closures formatting mode).
pub fn slot_key(u: &Unit, id: u32) -> String {
    slot_key_inner(u, id)
}

fn slot_key_inner(u: &Unit, id: u32) -> String {
    let f = &u.fns[id as usize];
    match &f.key {
        FnKey::Inst(d, args) => {
            let mut k = crate::program::hint_path(&u.prog.defs, &u.prog.syms, &u.prog.files, *d);
            if !args.is_empty() {
                let a: Vec<String> = args.iter().map(|t| crate::typeck::ty_to_string(true, &u.prog, &u.tcx, *t)).collect();
                k.push('<');
                k.push_str(&a.join(", "));
                k.push('>');
            }
            k
        }
        // the parent slot's key carries the instance; the closure's name its owner and ordinal
        FnKey::Closure(file, e, parent) => {
            let mut owner = *parent;
            let def = loop {
                match &u.fns[owner as usize].key {
                    FnKey::Inst(d, _) => break Some(*d),
                    FnKey::Closure(_, _, p) if *p != owner => owner = *p,
                    _ => break None,
                }
            };
            let name = crate::program::stable_closure_name(&u.prog, *file, *e, def);
            let tail = name.rsplit("::").next().unwrap_or(&name).to_string();
            format!("{}::{}", slot_key_inner(u, *parent), tail)
        }
        FnKey::Shim(shim) => crate::lower::shim_name(&u.prog, &u.tcx, shim),
        FnKey::Glue(g, t) => format!("glue{}<{}>", g, crate::typeck::ty_to_string(true, &u.prog, &u.tcx, *t)),
        FnKey::Sandbox(b) if *b == u32::MAX => "sandbox<invalid>".to_string(),
        FnKey::Sandbox(b) => format!("sandbox<{}>", slot_key(u, *b)),
    }
}

/// The cold blocks of `func`: they end in `unreachable` (after a diverging call), call a
/// panic / failure / formatting path, or lead only to cold blocks -- error paths the program
/// rarely takes. Speculation does not build their callees ahead, and O1 does not build them as
/// inline candidates (rustc's inliner skips cleanup blocks, inline.rs process_blocks).
fn cold_blocks(u: &Unit, func: &crate::rir::Func) -> Vec<bool> {
    use crate::rir::{Callee, Inst, Term};
    let mut cold: Vec<bool> = func.blocks.iter().map(|b| {
        matches!(b.term, Term::Unreachable)
            || b.insts.iter().any(|i| matches!(i, Inst::Call(Callee::Fn(c), _, _) if is_cold_fn(u, *c)))
    }).collect();
    loop {
        let mut changed = false;
        for (bi, b) in func.blocks.iter().enumerate() {
            if cold[bi] {
                continue;
            }
            let all_cold = match &b.term {
                Term::Jump(t) => cold[*t as usize],
                Term::Branch(_, t, f) => cold[*t as usize] && cold[*f as usize],
                _ => false,
            };
            if all_cold {
                cold[bi] = true;
                changed = true;
            }
        }
        if !changed {
            return cold;
        }
    }
}

/// Queues the slots `func` calls (but not `skip`) at depth + 1 (lock held).
unsafe fn spec_enqueue_calls(up: *mut Unit, func: &crate::rir::Func, skip: &[u32], depth: u8) {
    if startup_phase() { return; }
    let session = unsafe { (*std::ptr::addr_of!((*up).prog.session)).clone() };
    let options = &session.options;
    if !pipelined() || depth >= spec_depth_max(options) {
        return;
    }
    use crate::rir::{Callee, Inst, Term};
    let u = &mut *up;
    let nb = func.blocks.len();
    let cold = cold_blocks(u, func);
    // the fall-through path: from the entry, jumps, and at a branch its first warm target;
    // its callees go first in the queue
    let mut hot = vec![false; nb];
    let mut b = 0usize;
    while b < nb && !hot[b] && !cold[b] {
        hot[b] = true;
        b = match &func.blocks[b].term {
            Term::Jump(t) => *t as usize,
            Term::Branch(_, t, f) => {
                if !cold[*t as usize] {
                    *t as usize
                } else {
                    *f as usize
                }
            }
            _ => break,
        };
    }
    let mut front = Vec::new();
    let mut back = Vec::new();
    for (bi, b) in func.blocks.iter().enumerate() {
        if cold[bi] {
            SPEC_COLD_SKIPPED.fetch_add(b.insts.iter().filter(|i| matches!(i, Inst::Call(Callee::Fn(_), _, _))).count() as u64, Ordering::Relaxed);
            continue;
        }
        for i in &b.insts {
            if let Inst::Call(Callee::Fn(c), _, _) = i {
                if skip.contains(c) {
                    continue;
                }
                let f = &u.fns[*c as usize];
                if f.compiled || f.spec.is_some() || f.inflight || f.building || f.build_err.is_some() || f.spec_depth <= depth + 1 {
                    continue;
                }
                // drop / clone glue is not built ahead: most of it sits on scope exits of
                // values that are never live (None, moved-out), and it is cheap on demand
                let ok = match &f.key {
                    FnKey::Inst(d, _) => u.prog.def(*d).kind != DefKind::ForeignFn,
                    FnKey::Closure(cf, _, parent) => u.has_body(*parent, *cf),
                    FnKey::Shim(_) | FnKey::Glue(..) | FnKey::Sandbox(_) => false,
                };
                if !ok {
                    continue;
                }
                // deeper levels follow only the fall-through path (they are a guess on a guess)
                if !hot[bi] && (spec_hot_only(options) || depth + 1 >= spec_hot_from(options)) {
                    continue;
                }
                u.fns[*c as usize].spec_depth = depth + 1;
                if hot[bi] {
                    front.push(*c);
                } else {
                    back.push(*c);
                }
            }
        }
    }
    let added = front.len() + back.len();
    mark_spec_cached(u, &front);
    mark_spec_cached(u, &back);
    {
        let mut q = SPEC_Q.lock().unwrap();
        for c in front.into_iter().rev() {
            q.push_front(c);
        }
        q.extend(back);
    }
    if added > 0 {
        SPEC_START.call_once(|| {
            for k in 0..spec_jobs(options) {
                let session = session.clone();
            { let _k = crate::zone_alloc::kept();
            let _ = std::thread::Builder::new().name(format!("run-spec{}", k)).spawn(move || { let _z = crate::zone_alloc::compiler_thread(); spec_worker(&session.runtime, &session.options) });
            }
            }
        });
        SPEC_WAKE.notify_all();
    }
}

/// Initializers must enter through const_addr/static_addr: those reserve storage
/// and establish evaluation state before compiling. Seeding their function slot
/// directly can force a dependency back into the initializer already being built.
/// Keep them lazy, irrespective of which platform or hint source named them.
pub static SEED_INITIALIZERS_SKIPPED: AtomicU64 = AtomicU64::new(0);
pub static SEED_RECURSIVE_SKIPPED: AtomicU64 = AtomicU64::new(0);
/// spec_seed: thread CPU of the disk-cache probes of the queued slots, slots queued
pub static SEED_PROBE_NS: AtomicU64 = AtomicU64::new(0);
pub static SEED_QUEUED: AtomicU64 = AtomicU64::new(0);

fn skip_initializer_seed(u: &Unit, id: u32) -> bool {
    // Native anonymous const bodies have the same demand-evaluation boundary.
    if matches!(u.fns[id as usize].key, FnKey::Inst(d, _) if matches!(u.prog.def(d).kind, DefKind::Static | DefKind::Const | DefKind::AssocConst | DefKind::AnonConst)) {
        SEED_INITIALIZERS_SKIPPED.fetch_add(1, Ordering::Relaxed);
        true
    } else { false }
}

/// Queues `ids` for ahead-of-call compiles (lock held), behind what speculation queued: a
/// startup manifest. Seeds do not fan out further
/// (their depth is the speculation limit); slots already built or queued are skipped.
pub unsafe fn spec_seed(up: *mut Unit, ids: &[u32]) {
    for &id in ids { crate::fncost::hint(id); }
    let session = unsafe { (*std::ptr::addr_of!((*up).prog.session)).clone() };
    let options = &session.options;
    if !pipelined() {
        return;
    }
    SPEC_UNIT.store(up, Ordering::Release);
    let u = &mut *up;
    let max = spec_depth_max(options);
    let mut add = Vec::new();
    for &id in ids {
        if skip_initializer_seed(u, id) { continue; }
        let f = &mut u.fns[id as usize];
        if f.building {
            SEED_RECURSIVE_SKIPPED.fetch_add(1, Ordering::Relaxed);
            continue;
        }
        f.baseline_hint.store(true, Ordering::Relaxed);
        if f.compiled || f.spec.is_some() || f.inflight || f.building || f.build_err.is_some() || f.spec_depth != u8::MAX {
            continue;
        }
        f.spec_depth = max;
        add.push(id);
    }
    if add.is_empty() {
        return;
    }
    let probe = cpu_ns();
    mark_spec_cached(u, &add);
    SEED_PROBE_NS.fetch_add(cpu_ns() - probe, Ordering::Relaxed);
    SEED_QUEUED.fetch_add(add.len() as u64, Ordering::Relaxed);
    SPEC_Q.lock().unwrap().extend(add);
    SPEC_START.call_once(|| {
        for k in 0..spec_jobs(options) {
            let session = session.clone();
            { let _k = crate::zone_alloc::kept();
            let _ = std::thread::Builder::new().name(format!("run-spec{}", k)).spawn(move || { let _z = crate::zone_alloc::compiler_thread(); spec_worker(&session.runtime, &session.options) });
            }
        }
    });
    SPEC_WAKE.notify_all();
}

/// Teardown preparation: (slots built ahead, thread cpu ns)
pub static TEARDOWN_STATS: [AtomicU64; 2] = [AtomicU64::new(0), AtomicU64::new(0)];
/// Calls below a drop glue followed when preparing teardown (Drop impls and what they call:
/// `<Vec as Drop>::drop` -> its element loop, a platform's `Drop` -> its destroy fns).
const TEARDOWN_DEPTH: u8 = 3;

/// Builds the program's teardown ahead, on a background thread once the first frame is up: the
/// drop glue of every type the program has named so far (the locals main and the event loop
/// hold, the types made into trait objects -- a widget tree, the event handler closure), the
/// glue they reach and the Drop impls they call, a few calls deep. A quit otherwise compiles
/// all of it on the quitting thread (Calculator cold: ~1,600 builds, ~0.5 s, before the
/// process can end). Each build is an ordinary speculative one (compile_split): the first call
/// installs it, the disk cache records it. They are kept past MAKEPAD_RUN_SPEC_TTL_MS (spec_state 4):
/// teardown runs once, at the end. As a speculative worker's, each build takes the unit lock
/// only when no thread waits for it (lock_bg), at a lowered priority; the thread stops at exit.
pub unsafe fn prepare_teardown(up: *mut Unit) {
    if !pipelined() {
        return;
    }
    struct P(*mut Unit);
    unsafe impl Send for P {}
    let p = P(up);
    let session = unsafe { (*std::ptr::addr_of!((*up).prog.session)).clone() };
    { let _k = crate::zone_alloc::kept();
    let _ = std::thread::Builder::new().name("run-teardown".to_string()).spawn(move || { let _z = crate::zone_alloc::compiler_thread();
        let p = p;
        let up = p.0;
        ensure_altstack();
        set_worker();
        lower_worker_priority(&session.runtime);
        let stopped = || SPEC_STOP.load(Ordering::SeqCst) || crate::preempt::is_stopped() || owner::exit_tid() != 0;
        let c0 = cpu_ns();
        let mut seen: crate::fxhash::HashSet<u32> = Default::default();
        let mut work: Vec<(u32, u8)> = Vec::new();
        let mut scanned = 0usize;
        let mut built = 0u64;
        loop {
            if stopped() {
                break;
            }
            if work.is_empty() {
                // the drop glue slots made since the last scan (by the program or by the
                // builds below): new roots
                owner::call(owner::Class::Background, || {
                    let u = &*up;
                    for id in scanned..u.fns.len() {
                        if matches!(u.fns[id].key, FnKey::Glue(GLUE_DROP | GLUE_EXT_DROP, _)) && seen.insert(id as u32) {
                            work.push((id as u32, 0));
                        }
                    }
                    scanned = u.fns.len();
                });
                if work.is_empty() {
                    break;
                }
                work.reverse();
            }
            let Some((id, depth)) = work.pop() else { continue };
            let _job = compile_job();
            // the body's type check off the lock, as a speculative worker's (the slot table is
            // read only under the lock: it grows)
            if !(&*up).spec_cached.contains(&id) {
                typeck_ahead_slot(up, id);
            }
            if stopped() {
                break;
            }
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let fresh = owner::call(owner::Class::Background, || {
                    let f = &(&*up).fns[id as usize];
                    !(f.compiled || f.spec.is_some() || f.inflight || f.building || f.build_err.is_some())
                });
                if fresh && !spec_build(up, id) {
                    // gave way to a program thread: again later
                    work.push((id, depth));
                    return;
                }
                owner::call(owner::Class::Background, || {
                    let u = &mut *up;
                    if fresh && u.fns[id as usize].spec_state == 1 {
                        u.fns[id as usize].spec_state = 4;
                        built += 1;
                    }
                    // what it calls (its optimised body: calls O1 inlined are gone): glue at any
                    // depth, other fns a few calls below the nearest glue; not the cold ones
                    let u = &*up;
                    let Some(rir) = u.fns[id as usize].rir.clone() else { return };
                    for b in &rir.blocks {
                        for i in &b.insts {
                            let crate::rir::Inst::Call(crate::rir::Callee::Fn(c), _, _) = i else { continue };
                            let c = *c;
                            let d = match &u.fns[c as usize].key {
                                FnKey::Glue(GLUE_DROP | GLUE_EXT_DROP, _) => 0,
                                FnKey::Inst(d, _) if u.prog.def(*d).kind != DefKind::ForeignFn => depth + 1,
                                FnKey::Closure(cf, _, parent) if u.has_body(*parent, *cf) => depth + 1,
                                _ => continue,
                            };
                            let f = &u.fns[c as usize];
                            if d > TEARDOWN_DEPTH || f.compiled || f.spec.is_some() || is_cold_fn(u, c) || skip_initializer_seed(u, c) || !seen.insert(c) {
                                continue;
                            }
                            work.push((c, d));
                        }
                    }
                });
            }));
            if r.is_err() {
                SPEC_PANICS.fetch_add(1, Ordering::Relaxed);
            }
        }
        TEARDOWN_STATS[0].fetch_add(built, Ordering::Relaxed);
        TEARDOWN_STATS[1].fetch_add(cpu_ns() - c0, Ordering::Relaxed);
        if crate::run::VERBOSE.load(Ordering::Relaxed) {
            eprintln!("makepad-run teardown: {} slots built ahead ({} looked at), cpu {:.0} ms{}", built, seen.len(), (cpu_ns() - c0) as f64 / 1e6, if stopped() { " (stopped at exit)" } else { "" });
        }
    });
    }
}

/// O3 had to build a slot's RIR again (code linked from the disk cache has none):
/// (count, wall ns)
pub static TIER1_RIR_REBUILT: [AtomicU64; 2] = [AtomicU64::new(0), AtomicU64::new(0)];

/// Speculative results waiting for their first call: (slot, wall ns when built). A result
/// nobody called within MAKEPAD_RUN_SPEC_TTL_MS (default 2000; 0 = keep forever) is dropped with its
/// RIR (expire_spec): Stage builds ~10k slots ahead that are never called, and their code + RIR
/// stayed resident for the whole run.
static SPEC_BUILT: relax_session::sync::Mutex<Vec<(u32, u64)>> = relax_session::sync::Mutex::new(Vec::new());
/// (results dropped, bytes of code + RIR freed)
pub static SPEC_EXPIRED: [AtomicU64; 2] = [AtomicU64::new(0), AtomicU64::new(0)];

fn note_spec_built(options: &crate::options::Options, id: u32) {
    if spec_ttl_ns(options) != 0 {
        SPEC_BUILT.lock().unwrap().push((id, wall_ns()));
    }
}

pub fn spec_ttl_ns(options: &crate::options::Options) -> u64 {
    options.spec_ttl_ms * 1_000_000
}

/// Drops speculative results older than the TTL that were never called (their code, and their
/// RIR when no build inlined it and none is using it now): the slot is back to "not built"
/// and compiles on its first call. Only when the unit lock is free right now (never waits).
pub fn expire_spec(options: &crate::options::Options) {
    // The startup manifest's slots are preparation for this frame. Do not discard them before
    // it arrives and make the main thread compile them again. Normal TTL
    // accounting resumes at the first present, including time already spent waiting. A
    // headless program (released on quiet, startup_main) keeps what it built.
    if startup_phase() || !FIRST_PRESENT.load(Ordering::Acquire) { return; }
    let ttl = spec_ttl_ns(options);
    if ttl == 0 {
        return;
    }
    let now = wall_ns();
    {
        let g = SPEC_BUILT.lock().unwrap();
        if g.first().map_or(true, |x| now.saturating_sub(x.1) < ttl) {
            return;
        }
    }
    owner::call_within(owner::Class::Background, 0, || {
        let up = SPEC_UNIT.load(Ordering::Acquire);
        if !up.is_null() {
            let u = unsafe { &mut *up };
            let mut g = SPEC_BUILT.lock().unwrap();
            let cut = g.partition_point(|x| now.saturating_sub(x.1) >= ttl);
            let (mut n, mut bytes) = (0u64, 0u64);
            for &(id, _) in &g[..cut] {
                if id as usize >= u.fns.len() {
                    continue;
                }
                let f = &mut u.fns[id as usize];
                if f.compiled || f.spec_state != 1 || f.inflight || f.building {
                    continue;
                }
                if let Some(spec) = f.spec.take() {
                    let SpecCode { code, map, relocs, .. } = *spec;
                    u.reload_specs.remove(&id);
                    bytes += (std::mem::size_of::<SpecCode>() + code.capacity() + map.capacity() * 16 + relocs.capacity() * std::mem::size_of::<crate::rir::RelocAt>()) as u64;
                    f.spec_state = 3;
                    n += 1;
                    if f.history().inlined_into.is_empty() {
                        if let Some(r) = f.rir.take() {
                            bytes += crate::opt::inst_count(&r) as u64 * 80;
                        }
                    }
                }
            }
            g.drain(..cut);
            SPEC_EXPIRED[0].fetch_add(n, Ordering::Relaxed);
            SPEC_EXPIRED[1].fetch_add(bytes, Ordering::Relaxed);
        }
    });
}

fn spec_worker(runtime: &crate::options::RuntimeOptions, options: &crate::options::Options) {
    ensure_altstack();
    lower_worker_priority(runtime);
    set_worker();
    loop {
        let id = {
            let mut q = SPEC_Q.lock().unwrap();
            loop {
                if SPEC_STOP.load(Ordering::SeqCst) {
                    return;
                }
                let next = if spec_lifo(options) { q.pop_back() } else { q.pop_front() };
                if let Some(id) = next {
                    break id;
                }
                q = SPEC_WAKE.wait(q).unwrap();
            }
        };
        let _job = compile_job();
        let up = SPEC_UNIT.load(Ordering::Acquire);
        // the unit the queue was for was retired (retire_unit)
        if up.is_null() {
            continue;
        }
        // This also covers disk-manifest and callee speculation, not only explicit hints.
        if unsafe { skip_initializer_seed(&*up, id) } { continue; }
        // a panic escaping the build (outside its own catch_unwind) must not leave the unit
        // lock held by a thread that is gone: every lazy compile would wait for it forever
        let _census_context = unsafe { (&*up).exec_context(false) };
        let _cost = crate::fncost::Record::new(id, 0, crate::fncost::spec_origin(id));
        let _startup = StartupCompile::new();
        if !unsafe { (&*up).spec_cached.contains(&id) } {
            unsafe { typeck_ahead_slot(up, id) };
        }
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe { spec_build(up, id) }));
        match r {
            // gave way to a program thread: first in line again
            Ok(false) => SPEC_Q.lock().unwrap().push_front(id),
            Ok(true) => {}
            Err(_) => {
                SPEC_PANICS.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

/// speculative workers stop taking work (exit summary, Unit drop)
static SPEC_STOP: AtomicBool = AtomicBool::new(false);
/// background jobs that panicked outside their build's own recovery (lock released)
pub static SPEC_PANICS: AtomicU64 = AtomicU64::new(0);

/// The process is ending on this thread: stops the background compilers (speculative workers,
/// O3, preemptible checks); from now on the owner serves only this thread's requests (and its
/// exit helper's): every other thread asking for a compile waits until the process is gone.
pub fn begin_exit() {
    crate::preempt::stop_all();
    owner::begin_exit();
    SPEC_STOP.store(true, Ordering::SeqCst);
    TIER_STOP.store(true, Ordering::SeqCst);
    SPEC_WAKE.notify_all();
    TIER_WAKE.notify_all();
}

// ------------------------------------------------------------ allocator swaps

// Allocator epoch coordination: a program about to load a library that
// replaces the process allocator (Chromium's malloc shim, installed by a constructor of the CEF
// framework: its zone re-registration races any free on another thread) calls the loader's
// `relax_allocator_swap(0)` before and `(1)` after. Every compile job (lazy compiles on any
// thread, speculative workers, O3, ahead helpers, the sampler, the run monitor) holds a job
// guard; Before waits for running jobs and holds new ones at their start, After lets them go.
// After the swap Relax allocates from the new allocator like every library in the process
// (Chromium forwards frees of earlier pointers to their original zone).

static BG_PAUSE: AtomicBool = AtomicBool::new(false);
/// thread id (me()) of the thread that paused the jobs (0: none)
static BG_PAUSER: AtomicU64 = AtomicU64::new(0);
/// thread id (me()) of the one pause in progress (an allocator swap, a program's fork, a unit
/// retired, the settle swap); 0: none. Pauses take turns: a second one used to overwrite the
/// first's pauser and its resume opened the gate in the middle of the first.
static PAUSE_HOLDER: AtomicU64 = AtomicU64::new(0);

/// Takes the pause turn (waits for a pause in progress to end). A thread inside a compile job
/// is not counted as running while it waits: the pause in progress may be waiting for it.
fn take_pause_turn(mine: u32) {
    let me = me();
    if PAUSE_HOLDER.compare_exchange(0, me, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
        return;
    }
    if mine > 0 {
        BG_ACTIVE.fetch_sub(1, Ordering::SeqCst);
    }
    while PAUSE_HOLDER.compare_exchange(0, me, Ordering::SeqCst, Ordering::SeqCst).is_err() {
        std::thread::sleep(std::time::Duration::from_micros(200));
    }
    if mine > 0 {
        BG_ACTIVE.fetch_add(1, Ordering::SeqCst);
    }
}

/// The settle swap's turn (zone_swap.rs): taken only when no pause is in progress.
pub(crate) fn try_pause_turn() -> bool {
    PAUSE_HOLDER.compare_exchange(0, me(), Ordering::SeqCst, Ordering::SeqCst).is_ok()
}

pub(crate) fn end_pause_turn() {
    PAUSE_HOLDER.store(0, Ordering::SeqCst);
}
static BG_ACTIVE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
static BG_MX: relax_session::sync::Mutex<()> = relax_session::sync::Mutex::new(());
static BG_CV: relax_session::sync::Condvar = relax_session::sync::Condvar::new();
thread_local! {
    static JOB_DEPTH: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// The settle swap (zone_swap.rs) drains the compile jobs in two steps. DRAIN: no new outermost
/// job starts (a job that waits for the owner and comes back is let through: it must finish),
/// the owner serves everyone. HARD (the pause below, no 2 s escape): the owner serves only the
/// swap's thread. Jobs that wait for the owner (JobIdle) are counted in BG_IDLE: a swap needs
/// none, since such a job holds compiler state on its stack.
static SWAP_DRAIN: AtomicBool = AtomicBool::new(false);
static SWAP_HARD: AtomicBool = AtomicBool::new(false);
static BG_IDLE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
/// wall ns since which the program's main thread waits at the job gate (0: it does not)
static GATED_MAIN: AtomicU64 = AtomicU64::new(0);

/// A compile job in progress (outermost guard per thread counts).
pub struct JobGuard;

/// Starts a compile job: waits while an allocator swap is in progress.
pub fn compile_job() -> JobGuard {
    job_gate(false)
}

fn job_gate(reenter: bool) -> JobGuard {
    if JOB_DEPTH.with(|d| {
        let n = d.get();
        d.set(n + 1);
        n
    }) > 0
    {
        return JobGuard;
    }
    loop {
        BG_ACTIVE.fetch_add(1, Ordering::SeqCst);
        // the thread that paused the jobs compiles what it calls itself (its own swap or
        // fork code after the risky moment): it does not wait for its own resume; the owner
        // never waits here (while paused it serves only the pauser's requests: owner.rs pick)
        let closed = BG_PAUSE.load(Ordering::SeqCst) || SWAP_DRAIN.load(Ordering::SeqCst) && !reenter;
        if !closed || BG_PAUSER.load(Ordering::SeqCst) == me() || owner::is_owner() {
            return JobGuard;
        }
        BG_ACTIVE.fetch_sub(1, Ordering::SeqCst);
        let is_main = MAIN_TID.load(Ordering::Relaxed) == me();
        if is_main {
            let _ = GATED_MAIN.compare_exchange(0, wall_ns().max(1), Ordering::SeqCst, Ordering::SeqCst);
        }
        // A job never starts while an allocator swap is in progress, however long the swap
        // takes: it would allocate from the zone the swap is re-registering (a wall-clock
        // escape here turned a slow machine into a crash). The thread that began the swap
        // passes above (a callback into its own swap code), so the swap can finish.
        let mut g = BG_MX.lock().unwrap_or_else(|e| e.into_inner());
        let shut = || BG_PAUSE.load(Ordering::SeqCst) || SWAP_DRAIN.load(Ordering::SeqCst) && !reenter;
        while shut() {
            g = BG_CV.wait_timeout(g, std::time::Duration::from_millis(50)).map(|x| x.0).unwrap_or_else(|e| e.into_inner().0);
        }
        if is_main {
            GATED_MAIN.store(0, Ordering::SeqCst);
        }
        if shut() {
            drop(g);
            BG_ACTIVE.fetch_add(1, Ordering::SeqCst);
            return JobGuard;
        }
    }
}

/// A thread waiting for its work on the owner (owner::call) is not a running compile job
/// meanwhile: an allocator swap or a fork waits for the running ones, and the owner serves the
/// pauser.
pub(crate) struct JobIdle(bool);

impl JobIdle {
    pub(crate) fn enter() -> JobIdle {
        // only the outermost job of the thread counts (JOB_DEPTH 1): it is lifted for the wait
        let held = JOB_DEPTH.try_with(|d| d.get() == 1).unwrap_or(false);
        if held {
            BG_IDLE.fetch_add(1, Ordering::SeqCst);
            BG_ACTIVE.fetch_sub(1, Ordering::SeqCst);
        }
        JobIdle(held)
    }
}

impl Drop for JobIdle {
    fn drop(&mut self) {
        if self.0 {
            // back through the job gate: a pause that began meanwhile holds this job too (the
            // settle swap's drain lets it through: the job must finish before the swap)
            JOB_DEPTH.with(|d| d.set(d.get() - 1));
            std::mem::forget(job_gate(true));
            BG_IDLE.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

/// The thread compile jobs are paused for (an allocator swap, a fork), 0: none.
pub(crate) fn paused_for() -> u64 {
    // Only the pauser's requests are served while the jobs are paused, for as long as the pause
    // lasts: serving others after a time limit ran compile work inside the swap.
    if BG_PAUSE.load(Ordering::SeqCst) { BG_PAUSER.load(Ordering::SeqCst) } else { 0 }
}

impl Drop for JobGuard {
    fn drop(&mut self) {
        let outer = JOB_DEPTH.with(|d| {
            let n = d.get() - 1;
            d.set(n);
            n == 0
        });
        if outer {
            BG_ACTIVE.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

/// phase 0: an allocator swap begins (park compile jobs); 1: it is done (resume). Called by
/// the program on its own thread, outside any compile job of that thread.
/// Holds new compile jobs at their start and waits for running ones to finish (no time limit).
fn pause_compile_jobs(trace: bool) {
    let t0 = std::time::Instant::now();
    let mine = if JOB_DEPTH.with(|d| d.get()) > 0 { 1 } else { 0 };
    take_pause_turn(mine);
    BG_PAUSER.store(me(), Ordering::SeqCst);
    BG_PAUSE.store(true, Ordering::SeqCst);
    // the jobs running, and the request the owner serves now (unless it is the pauser's own)
    // Every running job must reach its end before the swap goes on: a job that frees memory
    // while the zone is re-registered corrupts the heap. Waiting is the only safe answer, so a
    // long wait is logged (the jobs, what they wait for), never cut short.
    let mut next_log = 5000u128;
    while BG_ACTIVE.load(Ordering::SeqCst) > mine || owner::busy_for_other(me()) {
        if t0.elapsed().as_millis() >= next_log {
            next_log += 5000;
            eprintln!("makepad: the allocator swap waits for compile jobs ({:.0} s): jobs active {}, waiting for the owner {}, owner busy for another thread {}",
                t0.elapsed().as_secs_f64(), BG_ACTIVE.load(Ordering::SeqCst), BG_IDLE.load(Ordering::SeqCst), owner::busy_for_other(me()));
        }
        std::thread::sleep(std::time::Duration::from_micros(200));
    }
    if trace {
        eprintln!("makepad: allocator swap: compile jobs parked in {:.1} ms", t0.elapsed().as_secs_f64() * 1000.0);
    }
}

fn resume_compile_jobs(_trace: bool) {
    {
        let _g = BG_MX.lock().unwrap_or_else(|e| e.into_inner());
        BG_PAUSE.store(false, Ordering::SeqCst);
        BG_PAUSER.store(0, Ordering::SeqCst);
    }
    BG_CV.notify_all();
    // requests other threads queued during the pause: the owner sleeps until woken (it served
    // only the pauser meanwhile), so a pause that ended without waking it left them queued until
    // some later request came (a domain thread and a test hung on 165 with the owner idle)
    owner::resume();
    end_pause_turn();
}

pub fn allocator_swap(phase: u32) {
    let trace = std::env::var("MAKEPAD_RUN_TRACE").map_or(false, |v| v.split(',').any(|x| x == "startup"));
    if phase == 0 {
        pause_compile_jobs(trace);
        // the compiler leaves the default zone before it becomes the program's new allocator
        crate::zone_alloc::use_own_zone();
        SWAP_T0.store(wall_ns(), Ordering::Relaxed);
    } else {
        resume_compile_jobs(trace);
        if trace {
            let ns = wall_ns().saturating_sub(SWAP_T0.load(Ordering::Relaxed));
            eprintln!("makepad: allocator swap: compile jobs resumed after {:.1} ms", ns as f64 / 1e6);
        }
    }
}
static SWAP_T0: AtomicU64 = AtomicU64::new(0);

extern "C" fn relax_allocator_swap(phase: u32) {
    allocator_swap(phase);
}

/// What `makepad run` does at exit (its summary: background compilers stopped, cache written);
/// set by run.rs, called by __relax_exit.
static EXIT_HOOK: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

pub fn set_exit_hook(f: extern "C" fn()) {
    EXIT_HOOK.store(f as usize, Ordering::SeqCst);
}

/// The program's std::process::exit on Windows (Relax std process_windows.rs): what unix's
/// exit() does for a Relax program -- the at-exit hook -- then the end of the process without
/// the exiting thread's fiber-local destructors: ExitProcess ran them into JIT code whose lazy
/// compile waited forever for the unit lock of a thread ExitProcess had already ended (Amp never
/// exited after its window closed).
pub extern "C" fn __relax_exit(code: i32) {
    crate::domain::exit_current(code);
    crate::tui::exit_code(code);
    let h = EXIT_HOOK.load(Ordering::SeqCst);
    if h != 0 {
        let f: extern "C" fn() = unsafe { std::mem::transmute(h) };
        f();
    }
    os::proc::exit(code)
}

#[cfg(unix)]
extern "C" fn recorded_exit(code: i32) -> ! {
    // an app domain's exit ends that domain only (domain.rs)
    crate::domain::exit_current(code);
    crate::tui::exit_code(code);
    os::proc::exit(code)
}

/// Symbols the loader provides to the programs it runs (found through dlsym).
fn loader_symbol(name: &[u8]) -> Option<usize> {
    match name {
        #[cfg(unix)]
        b"exit" => Some(recorded_exit as *const () as usize),
        #[cfg(unix)]
        b"fork" => Some(relax_fork as *const () as usize),
        b"relax_allocator_swap" => Some(relax_allocator_swap as *const () as usize),
        b"relax_first_present" => Some(relax_first_present as *const () as usize),
        b"relax_quit_requested" => Some(crate::run::relax_quit_requested as *const () as usize),
        b"__relax_guard_enter" => Some(__relax_guard_enter as *const () as usize),
        b"__relax_guard_leave" => Some(__relax_guard_leave as *const () as usize),
        // Relax's std reads virtual files (git runs) through these (vfs.rs)
        b"relax_vfs_stat" => Some(crate::vfs::relax_vfs_stat as *const () as usize),
        b"relax_vfs_read" => Some(crate::vfs::relax_vfs_read as *const () as usize),
        b"relax_vfs_list" => Some(crate::vfs::relax_vfs_list as *const () as usize),
        b"relax_vfs_mounts" => Some(crate::vfs::relax_vfs_mounts as *const () as usize),
        b"relax_vfs_release" => Some(crate::vfs::relax_vfs_release as *const () as usize),
        b"relax_vfs_edit" => Some(crate::vfs::relax_vfs_edit as *const () as usize),
        b"relax_store_file" => Some(crate::vfs::relax_store_file as *const () as usize),
        // `"..".parse::<TokenStream>()` in JIT-run proc macros (std proc_macro; pmhost.rs)
        b"relax_pm_parse" => Some(crate::pmhost::relax_pm_parse as *const () as usize),
        b"__relax_exit" => Some(__relax_exit as *const () as usize),
        // makepad inspect's timeline: makepad's UI phases (platform thread/ui_hang.rs), Relax std's
        // futex wakes and the recording flag they check
        // makepad inspect's retry boundaries (makepad's event dispatch)
        b"relax_retry_on" => Some(&crate::inspect::events::MAKEPAD_RUN_RETRY_ON as *const _ as usize),
        b"relax_retry_enter" => Some(crate::inspect::events::relax_retry_enter as *const () as usize),
        b"relax_retry_leave" => Some(crate::inspect::events::relax_retry_leave as *const () as usize),
        b"relax_retry_take" => Some(crate::inspect::events::relax_retry_take as *const () as usize),
        // makepad inspect's probes: std::relax::probe (Relax std relax.rs)
        b"__relax_probe_emit" => Some(crate::inspect::probes::relax_probe_emit as *const () as usize),
        b"__relax_probe_count" => Some(crate::inspect::probes::relax_probe_count as *const () as usize),
        b"__relax_probe_hist" => Some(crate::inspect::probes::relax_probe_hist as *const () as usize),
        b"__relax_probe_sample" => Some(crate::inspect::probes::relax_probe_sample as *const () as usize),
        b"__relax_probe_pause" => Some(crate::inspect::probes::relax_probe_pause as *const () as usize),
        b"relax_register_inspect" => Some(crate::inspect::relax_register_inspect as *const () as usize),
        b"relax_register_remote_service" => Some(crate::inspect::relax_register_remote_service as *const () as usize),
        b"relax_gpu_span" => Some(crate::inspect::timeline::relax_gpu_span as *const () as usize),
        b"relax_span_begin" => Some(crate::inspect::timeline::relax_span_begin as *const () as usize),
        b"relax_span_end" => Some(crate::inspect::timeline::relax_span_end as *const () as usize),
        b"__relax_wake" => Some(crate::inspect::timeline::relax_wake as *const () as usize),
        b"__relax_recording" | b"relax_recording" => Some(&crate::inspect::timeline::MAKEPAD_RUN_RECORDING as *const _ as usize),
        // std::relax::realtime: code a real-time thread runs is compiled ahead
        b"__relax_realtime" => Some(__relax_realtime as *const () as usize),
        // makepad inspect's wait registry: program lock / condvar / park waits (Relax's std sync/raw.rs)
        b"__relax_wait_enter" => Some(crate::inspect::relax_wait_enter as *const () as usize),
        b"__relax_wait_leave" => Some(crate::inspect::relax_wait_leave as *const () as usize),
        // the domain a thread runs in (domain.rs): a thread std starts inherits its parent's
        b"__relax_domain_base" => Some(crate::domain::relax_domain_base as *const () as usize),
        b"__relax_domain_id" => Some(crate::domain::relax_domain_id as *const () as usize),
        b"__relax_domain_enter" => Some(crate::domain::relax_domain_enter as *const () as usize),
        b"__relax_domain_reserve" => Some(crate::domain::relax_domain_reserve as *const () as usize),
        b"__relax_domain_failed" => Some(crate::domain::thread_finished as *const () as usize),
        b"__relax_domain_name" => Some(crate::domain::relax_domain_name as *const () as usize),
        b"__relax_key_create" => Some(crate::domain_threads::key_create as *const () as usize),
        b"__relax_key_delete" => Some(crate::domain_threads::key_delete as *const () as usize),
        b"__relax_key_get" => Some(crate::domain_threads::key_get as *const () as usize),
        b"__relax_key_set" => Some(crate::domain_threads::key_set as *const () as usize),
        // run-time modules (modules.rs; Relax's std::relax); the C allocator through relax
        // inspect's heap veneers (one indirect jump; tracking patches the word)
        _ => crate::inspect::heap::veneer(name).or_else(|| crate::modules::symbol(name)),
    }
}

thread_local! {
    /// guarded run-time module calls this thread is inside (std::relax::guard)
    static GUARD_DEPTH: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}
crate::gen_local! {
    static GUARD_MARKS: std::cell::RefCell<Vec<crate::modules::GuardMark>>;
}

/// std::relax::guard: enters a guarded call (faults inside it are contained), with a deadline
/// in ms (0: none) after which the hang watchdog unwinds the call. Returns the guard deadline token.
pub extern "C" fn __relax_guard_enter(deadline_ms: u64) -> u64 {
    GUARD_DEPTH.with(|g| g.set(g.get() + 1));
    let m = crate::modules::guard_mark();
    GUARD_MARKS.with(|g| g.borrow_mut().push(m));
    watchdog(deadline_ms)
}

pub extern "C" fn __relax_guard_leave(token: u64) {
    GUARD_DEPTH.with(|g| g.set(g.get().saturating_sub(1)));
    // the sandbox state as it was at the guard's start (a fault or deadline left no wrapper)
    if let Some(m) = GUARD_MARKS.with(|g| g.borrow_mut().pop()) {
        crate::modules::guard_reset(m);
    }
    watchdog_remove(token);
    if WATCHDOG_EXPIRED.with(|t| t.get() != 0) {
        // The nearest recovery point belonged to an inner guard. It has now restored
        // its arena and deadline stack; continue to the guard whose deadline expired.
        // This uses the same contained exit as the watchdog, without skipping guard
        // bookkeeping or falsely returning an inner timeout to its caller.
        PANIC.with(|p| *p.borrow_mut() = Some(PanicInfo {
            kind: "hang".to_string(), message: "watchdog: the call did not return in time".to_string(),
            site: u64::MAX, frames: Vec::new(), fault_addr: 0, location: String::new(),
        }));
        unsafe {
            let leave: rt_fn_ty!(fn(*mut u64, u64) -> !) = std::mem::transmute((&*UNIT).leave as usize);
            leave(ctx_ptr(), 3)
        }
    }
}

/// The unit the program runs in (run-time module loading, modules.rs).
pub fn current_unit() -> *mut Unit {
    unsafe { UNIT }
}

/// The program's own symbol lookups (dlsym; Windows: GetProcAddress) go through the loader, which
/// answers its loader symbols first: a program finds the loader's hooks with the platform's usual
/// call, and a rustc build of it finds none.
fn lookup_interposer(link: &str) -> Option<u64> {
    match link {
        "dlsym" => Some(relax_dlsym as *const () as usize as u64),
        #[cfg(windows)]
        "GetProcAddress" => Some(relax_get_proc_address as *const () as usize as u64),
        _ => None,
    }
}

#[cfg(windows)]
extern "system" fn relax_get_proc_address(module: os::win32::HANDLE, name: *const u8) -> *mut u8 {
    // an ordinal (MAKEINTRESOURCE) is not a name
    if name as usize > 0xffff {
        let n = unsafe { std::ffi::CStr::from_ptr(name as *const std::ffi::c_char) }.to_bytes();
        if let Some(a) = loader_symbol(n) {
            return a as *mut u8;
        }
    }
    unsafe { os::win32::GetProcAddress(module, name) }
}

extern "C" fn relax_dlsym(handle: *mut u8, name: *const u8) -> *mut u8 {
    if !name.is_null() {
        let n = unsafe { std::ffi::CStr::from_ptr(name as *const std::ffi::c_char) }.to_bytes();
        if let Some(a) = loader_symbol(n) {
            return a as *mut u8;
        }
    }
    unsafe { os::proc::lookup(handle, name) }
}



/// The front end of a split compile (lock held): the slot's RIR exists already (an inline
/// candidate) -> code generation only; else a preparation for optimize_rir.
enum Front {
    Codegen(std::sync::Arc<crate::rir::Func>),
    Opt(Prep, u64),
}

/// Hot reload: builds a set of slots with the unit lock held only for their front ends (type
/// check + lowering); optimisation and code generation run with the lock released, on up to
/// `threads` threads, so program threads keep compiling and running meanwhile. Nothing is
/// installed. The caller holds the unit lock (as it does on return). Slots in flight are marked
/// (a program thread needing one waits for its build, as for pipelined compiles).
pub(crate) unsafe fn compile_batch_off_lock(up: *mut Unit, ids: &[u32], threads: usize) -> Vec<(u32, Result<(Vec<u8>, Vec<(u32, u64)>, Vec<crate::rir::RelocAt>), String>)> {
    let census_unit = (&*up).front.census_id;
    let session = unsafe { (*std::ptr::addr_of!((*up).prog.session)).clone() };
    let options = &session.options;
    let mut out: Vec<(u32, Result<(Vec<u8>, Vec<(u32, u64)>, Vec<crate::rir::RelocAt>), String>)> = Vec::new();
    // fronts, under the lock; each front's slots go in flight at once (no other thread starts
    // them meanwhile). A later front of this batch that inlines one of them does not wait for it
    // (wait_inflight skips BATCH_FLIGHT): its back end starts only after the last front.
    let mut work: Vec<(u32, Front, String)> = Vec::new();
    let mut flight: Vec<u32> = Vec::new();
    struct ClearBatch;
    impl Drop for ClearBatch { fn drop(&mut self) { let _ = BATCH_FLIGHT.try_with(|b| b.borrow_mut().clear()); } }
    let clear_batch = ClearBatch;
    for &id in ids {
        match split_front(up, id) {
            Ok(f) => {
                let mut slots = vec![id];
                if let Front::Opt(p, _) = &f {
                    deferred_slots(p, &mut slots);
                }
                let u = &mut *up;
                for &x in &slots {
                    if !u.fns[x as usize].inflight {
                        u.fns[x as usize].inflight = true;
                        flight.push(x);
                        BATCH_FLIGHT.with(|b| b.borrow_mut().push(x));
                    }
                }
                work.push((id, f, u.fns[id as usize].name.to_string()));
            }
            Err(e) => out.push((id, Err(e))),
        }
    }
    drop(clear_batch);
    let (env, fold, rotate) = {
        let u = &*up;
        let e = &u.env;
        (x64::Env { slots: e.slots, tls_key: e.tls_key, tls_slow: e.tls_slow, resume: e.resume, opt_regs: !TIER1_ENABLED.load(Ordering::Relaxed), syms: e.syms, sym_class: e.sym_class }, u.opt.fold, u.opt.layout)
    };
    // back ends, off the unit (on helper threads; the owner serves requests meanwhile)
    type Built = (u32, Result<(Option<(std::sync::Arc<crate::rir::Func>, Vec<(u32, usize)>, u64)>, Vec<u8>, Vec<(u32, u64)>, Vec<crate::rir::RelocAt>), String>, Vec<Deferred>);
    let next = std::sync::atomic::AtomicUsize::new(0);
    let results: relax_session::sync::Mutex<Vec<Built>> = relax_session::sync::Mutex::new(Vec::new());
    let work_cells: Vec<relax_session::sync::Mutex<Option<(u32, Front, String)>>> = work.into_iter().map(|w| relax_session::sync::Mutex::new(Some(w))).collect();
    let finished = std::sync::atomic::AtomicUsize::new(0);
    let helpers = threads.max(1).min(work_cells.len().max(1));
    std::thread::scope(|sc| {
        for _ in 0..helpers {
            sc.spawn(|| { let _z = crate::zone_alloc::compiler_thread();
                // counted when this helper is done, also if it panics (the owner waits for all)
                struct Done<'a>(&'a std::sync::atomic::AtomicUsize);
                impl Drop for Done<'_> {
                    fn drop(&mut self) {
                        self.0.fetch_add(1, Ordering::Release);
                        owner::wake();
                    }
                }
                let _done = Done(&finished);
                loop {
                    let k = next.fetch_add(1, Ordering::Relaxed);
                    if k >= work_cells.len() {
                        break;
                    }
                    let Some((id, front, name)) = work_cells[k].lock().unwrap().take() else { continue };
                    let _census_context = crate::exec_census::Context::new(census_unit, false);
                    let _cost = crate::fncost::Record::new(id, 0, "ahead");
                    crate::fncost::name(id, &name);
                    let mut done: Vec<Deferred> = Vec::new();
                    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<_, String> {
                        let t0 = cpu_ns();
                        let (rir, opt) = match front {
                            Front::Codegen(r) => (r, None),
                            Front::Opt(p, prep_ns) => {
                                let (f, inl) = optimize_tree(options, id, p, &mut done)?;
                                let f = std::sync::Arc::new(f);
                                let ns = cpu_ns() - t0 + prep_ns;
                                (f.clone(), Some((f, inl, ns)))
                            }
                        };
                        let cg = tier0_codegen_rir(options, &rir, fold, rotate);
                        let e = &env;
                        let env = x64::Env { slots: e.slots, tls_key: e.tls_key, tls_slow: e.tls_slow, resume: e.resume, opt_regs: e.opt_regs && !rir.baseline, syms: e.syms, sym_class: e.sym_class };
                        let c = x64::compile(options, &cg, &env)?;
                        note_frame(id, c.frame_size, slot_bytes(&cg), false);
                        dump_o1(&cg, &c);
                        Ok((opt, c.code, c.pc_map, c.relocs))
                    }));
                    let r = match r {
                        Ok(r) => r,
                        Err(p) => {
                            let msg = p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_else(|| "panic".to_string());
                            Err(if let Some(fatal) = p.downcast_ref::<crate::infer::diagnostics::FatalError>() { fatal.0.clone() } else { format!("internal compiler error in {}: {} -- makepad run bug, report it", name, msg) })
                        }
                    };
                    results.lock().unwrap().push((id, r, done));
                }
            });
        }
        // the helpers are pure: the owner keeps serving (constants for type checks, back ends'
        // finishes) until all are done, then the scope's join does not wait
        owner::pump_wait(&|| finished.load(Ordering::Acquire) == helpers);
    });
    // results (unit)
    for (id, r, done) in results.into_inner().unwrap() {
        for (c, rr) in done {
            match rr {
                Ok((f, inl)) => {
                    if (&*up).fns[c as usize].rir.is_none() {
                        store_rir(up, c, f, inl, 0);
                    }
                }
                Err(e) => {
                    (&mut *up).reload_errors.insert(c);
                    (&mut *up).fns[c as usize].build_err = Some(e.into_boxed_str());
                },
            }
        }
        match r {
            Ok((opt, code, map, relocs)) => {
                if let Some((f, inl, ns)) = opt {
                    if (&*up).fns[id as usize].rir.is_none() {
                        let f = std::sync::Arc::try_unwrap(f).unwrap_or_else(|a| (*a).clone());
                        store_rir(up, id, f, inl, ns);
                    }
                }
                out.push((id, Ok((code, map, relocs))));
            }
            Err(e) => out.push((id, Err(e))),
        }
    }
    {
        let u = &mut *up;
        for &x in &flight {
            u.fns[x as usize].inflight = false;
        }
    }
    out
}

unsafe fn split_front(up: *mut Unit, id: u32) -> Result<Front, String> {
    {
        let u = &mut *up;
        if let Some(r) = &u.fns[id as usize].rir {
            return Ok(Front::Codegen(r.clone()));
        }
        if let Some(e) = &u.fns[id as usize].build_err {
            return Err(e.to_string());
        }
    }
    if let Some(r) = &(&*up).fns[id as usize].rir {
        return Ok(Front::Codegen(r.clone()));
    }
    if let Some(e) = &(&*up).fns[id as usize].build_err {
        return Err(e.to_string());
    }
    begin_build(up, id)?;
    let depth0 = BUILD_STACK.try_with(|s| s.borrow().len()).unwrap_or(0);
    let child0 = BUILD_CHILD.try_with(|c| c.borrow().len()).unwrap_or(0);
    child_push();
    let t0 = cpu_ns();
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| prepare_rir(up, id, true)));
    if r.is_err() {
        let open: Vec<u32> = BUILD_STACK.try_with(|s| s.borrow()[depth0.min(s.borrow().len())..].to_vec()).unwrap_or_default();
        for x in open.into_iter().rev() {
            end_build(up, x);
        }
        let _ = BUILD_CHILD.try_with(|c| c.borrow_mut().truncate(child0 + 1));
    }
    let dt = cpu_ns() - t0;
    let nested = child_pop(dt);
    let u = &mut *up;
    u.fns[id as usize].build_ns = dt.saturating_sub(nested);
    end_build(up, id);
    let u = &mut *up;
    match r {
        Ok(Ok((p, ns))) => Ok(Front::Opt(p, ns)),
        Ok(Err(e)) => Err(e),
        Err(p) if p.is::<crate::preempt::OwnerBusy>() => std::panic::resume_unwind(p),
        Err(p) => {
            let msg = p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_else(|| "panic".to_string());
            Err(if let Some(fatal) = p.downcast_ref::<crate::infer::diagnostics::FatalError>() { fatal.0.clone() } else { format!("internal compiler error in {}: {}{}", u.fns[id as usize].name, msg, take_ice_location()) })
        }
    }
}

/// A split compile's back end: inlining, optimisation and code generation of a front's result.
/// It reads only what the front handed it (no unit state), so it runs on any thread: the
/// requesting program thread, a speculative worker.
pub(crate) struct BackWork {
    id: u32,
    spec: bool,
    front: Front,
    /// the slot and its deferred inline candidates, in flight until the finish
    slots: Vec<u32>,
    epoch: u64,
    env: x64::Env,
    fold: bool,
    rotate: bool,
    session: std::sync::Arc<crate::Session>,
    name: String,
    /// wall ns when the front ended (SPLIT_NS)
    w1: u64,
}

/// What a back end made, for its finish (a unit call).
pub(crate) struct BackDone {
    id: u32,
    spec: bool,
    slots: Vec<u32>,
    epoch: u64,
    built: Result<(Option<(std::sync::Arc<crate::rir::Func>, Vec<(u32, usize)>, u64)>, Vec<u8>, Vec<(u32, u64)>, Vec<crate::rir::RelocAt>, u64), String>,
    done: Vec<Deferred>,
}

/// The front of a split compile (a unit call): Done when there is nothing to run off the unit.
pub(crate) enum Begin {
    Done(Split),
    Back(BackWork),
}

/// The front of a split compile of slot `id` (unit call, outermost): the disk cache first for a
/// speculative build, then type check + lower (+ inline candidates' fronts); the slot and its
/// deferred candidates are in flight until `split_finish`.
unsafe fn split_begin(up: *mut Unit, id: u32, spec: bool) -> Begin {
    let _why = why_compile(&*up, id);
    cost_name(up, id);
    let session = unsafe { (*std::ptr::addr_of!((*up).prog.session)).clone() };
    let options = &session.options;
    // the disk cache first (unit lock held): a speculative build of a cached fn only links it
    if spec {
        let t0 = wall_ns();
        let r = cache_lookup(up, id);
        if (&*up).fns[id as usize].compiled { return Begin::Done(Split::Spec); }
        if r.is_some() {
            if let Some(c) = (&mut *up).cache.as_mut() {
                c.stats.spec_hits += 1;
                c.stats.spec_link_ns += wall_ns() - t0;
            }
        }
        if let Some((code, map, crelocs)) = r {
            let u = &mut *up;
            let callees = u.cache.as_mut().map(|c| std::mem::take(&mut c.last_callees)).unwrap_or_default();
            let t1 = u.cache.as_mut().and_then(|c| c.last_tier1.take());
            if u.cache.as_mut().is_some_and(|c| std::mem::take(&mut c.last_o2)) {
                o2_cached(up, id);
            }
            if !u.fns[id as usize].compiled && u.fns[id as usize].spec.is_none() {
                // cached O3 code: no call counter once installed, inlined slots know it
                if let Some(inl) = &t1 {
                    cached_tier1(up, id, inl);
                }
                let u = &mut *up;
                u.reload_specs.insert(id);
                u.fns[id as usize].spec = Some(Box::new(SpecCode { code, map, relocs: crelocs, record: false }));
                u.fns[id as usize].spec_state = 1;
                u.exec_note(id, crate::exec_profile::SPEC);
                note_spec_built(options, id);
            }
            let u = &mut *up;
            let d = u.fns[id as usize].spec_depth;
            spec_enqueue_cached(up, &callees, d);
            return Begin::Done(Split::Spec);
        }
    }
    let epoch = PATCH_EPOCH.load(Ordering::SeqCst);
    let w0 = wall_ns();
    let c0 = if spec { 0 } else { cpu_ns() };
    let k = if spec { 4 } else { 0 };
    let front = match split_front(up, id) {
        Ok(f) => f,
        Err(e) => return Begin::Done(Split::Failed(e)),
    };
    let mut slots = vec![id];
    if let Front::Opt(p, _) = &front {
        deferred_slots(p, &mut slots);
        // the callees go to the speculative workers now, not after this back end: the ones
        // O1 will inline (ready or deferred candidates) are left out
        let mut skip: Vec<u32> = p.callees.iter().map(|x| x.0).collect();
        skip.extend(p.deferred.iter().map(|x| x.0));
        let depth = if spec { (&*up).fns[id as usize].spec_depth } else { 0 };
        if !pipelined() || !defer_startup_root(up, id, depth) {
            spec_enqueue_calls(up, &p.func, &skip, depth);
        }
    }
    let u = &mut *up;
    for &x in &slots {
        u.fns[x as usize].inflight = true;
    }
    let e = &u.env;
    let env = x64::Env { slots: e.slots, tls_key: e.tls_key, tls_slow: e.tls_slow, resume: e.resume, opt_regs: !TIER1_ENABLED.load(Ordering::Relaxed), syms: e.syms, sym_class: e.sym_class };
    let (fold, rotate) = (u.opt.fold, u.opt.layout);
    let name = u.fns[id as usize].name.to_string();
    let w1 = wall_ns();
    SPLIT_NS[k].fetch_add(w1 - w0, Ordering::Relaxed);
    if !spec {
        let c1 = cpu_ns();
        main_rec(|r| {
            r.ns[2] += w1 - w0;
            r.cpu[0] += c1 - c0;
        });
    }
    Begin::Back(BackWork { id, spec, front, slots, epoch, env, fold, rotate, session, name, w1 })
}

impl BackWork {
    /// Optimises and generates the code (any thread, no unit state).
    pub(crate) fn run(self) -> BackDone {
        let BackWork { id, spec, front, slots, epoch, env, fold, rotate, session, name, w1 } = self;
        let options = &session.options;
        let runtime = &session.runtime;
        let done_out: relax_session::sync::Mutex<Vec<Deferred>> = relax_session::sync::Mutex::new(Vec::new());
        let built = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<_, String> {
            let t0 = cpu_ns();
            let (rir, opt) = match front {
                Front::Codegen(r) => (r, None),
                Front::Opt(p, prep_ns) => {
                    let mut done: Vec<Deferred> = Vec::new();
                    if !spec {
                        let n = crate::opt::inst_count(&p.func) as u32;
                        main_rec(|r| r.insts[0] += n);
                    }
                    let r = optimize_tree(options, id, p, &mut done);
                    *done_out.lock().unwrap() = done;
                    let (f, inl) = r?;
                    let f = std::sync::Arc::new(f);
                    let ns = cpu_ns() - t0 + prep_ns;
                    (f.clone(), Some((f, inl, ns)))
                }
            };
            let t1 = cpu_ns();
            let cg = tier0_codegen_rir(options, &rir, fold, rotate);
            let mut env = env;
            env.opt_regs &= !rir.baseline;
            compile_watch(Some((&format!("codegen {}", cg.name), crate::opt::inst_count(&cg))));
            let mps = crate::memstat::peak_start();
            let c = crate::memstat::tagged(crate::memstat::Tag::Codegen, || {
                let _scratch = crate::zone_alloc::scratch(runtime);
                x64::compile(options, &cg, &env)
            });
            crate::memstat::note_big("codegen", &cg.name, mps, || cg_shape(&cg));
            compile_watch(None);
            let c = c?;
            note_frame(id, c.frame_size, slot_bytes(&cg), false);
            dump_o1(&cg, &c);
            code_sig(id, &cg, &c.code, c.alloc_sig);
            if let Some(u) = crate::syms::unit() {
                crate::syms::reloc_sig(u, || crate::syms::stable_fn_name(u, id), &c.code, &c.relocs, &c.pc_map);
            }
            let t2 = cpu_ns();
            if !spec {
                let n = crate::opt::inst_count(&cg) as u32;
                main_rec(|r| {
                    r.cpu[1] += t1 - t0;
                    r.cpu[2] += t2 - t1;
                    r.insts[1] += n;
                });
            }
            Ok((opt, c.code, c.pc_map, c.relocs, t2 - t1))
        }));
        let w2 = wall_ns();
        SPLIT_NS[if spec { 5 } else { 1 }].fetch_add(w2 - w1, Ordering::Relaxed);
        if !spec {
            main_rec(|r| r.ns[3] += w2 - w1);
        }
        let built = match built {
            Ok(r) => r,
            Err(p) => {
                let msg = p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_else(|| "panic".to_string());
                Err(if let Some(fatal) = p.downcast_ref::<crate::infer::diagnostics::FatalError>() { fatal.0.clone() } else { format!("internal compiler error in {}: {}", name, msg) })
            }
        };
        BackDone { id, spec, slots, epoch, built, done: done_out.into_inner().unwrap_or_default() }
    }
}

/// The finish of a split compile (unit call): the deferred candidates' RIR is stored, the slots
/// are no longer in flight, and the code is installed (or kept for the first call when
/// speculative). Restart: a live patch landed while the back end ran (build again from the new
/// source). Never runs a front.
unsafe fn split_finish(up: *mut Unit, d: BackDone) -> Split {
    let BackDone { id, spec, slots, epoch, built, done } = d;
    let session = unsafe { (*std::ptr::addr_of!((*up).prog.session)).clone() };
    let options = &session.options;
    let _cost_install = crate::fncost::phase(6);
    let stale = PATCH_EPOCH.load(Ordering::SeqCst) != epoch;
    {
        // the deferred inline candidates' RIR; then the slots are no longer in flight
        let done = if stale { Vec::new() } else { done };
        for (c, r) in done {
            match r {
                Ok((f, inl)) => {
                    if (&*up).fns[c as usize].rir.is_none() {
                        store_rir(up, c, f, inl, 0);
                    }
                }
                Err(e) => {
                    (&mut *up).reload_errors.insert(c);
                    (&mut *up).fns[c as usize].build_err = Some(e.into_boxed_str());
                },
            }
        }
        let u = &mut *up;
        for &x in &slots {
            u.fns[x as usize].inflight = false;
        }
    }
    if stale {
        // a live patch landed while this back end ran: start over from the new source
        return if spec { Split::Spec } else { Split::Restart };
    }
    match built {
        Ok((opt, code, map, relocs, cg_ns)) => {
            let u = &mut *up;
            if let Some((f, inl, ns)) = opt {
                if u.fns[id as usize].rir.is_none() {
                    let f = std::sync::Arc::try_unwrap(f).unwrap_or_else(|a| (*a).clone());
                    store_rir(up, id, f, inl, ns);
                }
            }
            let u = &mut *up;
            u.stats.codegen_ns += cg_ns;
            u.fns[id as usize].cg_ns = cg_ns;
            if u.fns[id as usize].compiled {
                // built meanwhile by a nested compile: that code stays
                return if spec { Split::Spec } else { Split::Installed(u.fns[id as usize].code) };
            }
            u.stats.compiled += 1;
            COMPILED_FNS.fetch_add(1, Ordering::Relaxed);
            if spec {
                // a speculative build is valid code whether or not it gets called: cache it now
                // (not an O2 fn's O1 code: the O2 worker records its entry)
                if marked_hot(u, id) {
                } else if let Some(mut c) = u.cache.take() {
                    c.record(u, id, &code, &relocs, &map);
                    u.cache = Some(c);
                }
                let u = &mut *up;
                if u.fns[id as usize].spec.is_none() {
                    u.reload_specs.insert(id);
                    u.fns[id as usize].spec = Some(Box::new(SpecCode { code, map, relocs, record: true }));
                    u.fns[id as usize].spec_state = 1;
                    u.exec_note(id, crate::exec_profile::SPEC);
                    note_spec_built(options, id);
                    let d = u.fns[id as usize].spec_depth;
                    spec_enqueue(up, id, d);
                }
                Split::Spec
            } else {
                Split::Installed(install_compiled(up, id, &code, map, &relocs, true))
            }
        }
        // a speculative build's error is reported when (if) the program calls it; a lazy
        // compile's is installed as the slot's error stub
        Err(_) if spec => Split::Spec,
        Err(e) => Split::Installed(install_error(up, id, &e)),
    }
}

/// A speculative build of slot `id` driven from a background thread: its front and finish are
/// unit calls, its back end runs here. Nothing when the slot needs no build any more. False:
/// the build gave way to a program thread's request (the caller queues it again).
unsafe fn spec_build(up: *mut Unit, id: u32) -> bool {
    let b = owner::call(owner::Class::Background, || {
        if SPEC_STOP.load(Ordering::SeqCst) {
            return Begin::Done(Split::Spec);
        }
        let f = &(&*up).fns[id as usize];
        if f.building { SEED_RECURSIVE_SKIPPED.fetch_add(1, Ordering::Relaxed); }
        if f.compiled || f.spec.is_some() || f.inflight || f.building || f.build_err.is_some() {
            return Begin::Done(Split::Spec);
        }
        COMPILE_LAST.store(wall_ns(), Ordering::Relaxed);
        split_begin(up, id, true)
    });
    match b {
        Begin::Back(w) => {
            let d = w.run();
            owner::call(owner::Class::Finish, || split_finish(up, d));
            COMPILE_LAST.store(wall_ns(), Ordering::Relaxed);
            true
        }
        Begin::Done(Split::Failed(e)) if e == YIELDED => false,
        Begin::Done(_) => true,
    }
}

// ------------------------------------------------------------ front end ahead (off the unit lock)
//
// Type checking needs only the program and the type context, both shared (`&Program`, `&Tcx`,
// Sync): a thread about to build a slot type-checks its body -- and the bodies of the small core
// callees O1 will build as inline candidates -- before it takes the unit lock, so the lock
// is held for lowering only and other threads' type checks run in parallel. The bodies wait in
// definition-keyed typeck query until lowering instantiates them. Changes
// to signatures or sources (live patches, dropping the unit) take FRONT_RW for writing;
// type checks ahead hold it for reading. Declaration changes
// invalidates query results, including diagnostics. MAKEPAD_RUN_TYCK_AHEAD=0: off.
// Helpers also lower instantiated bodies until first present.

/// Readers: type checks and lowering off the unit lock. Writers (unit lock held too): changes to the program
/// or to collected signatures / types.
pub static FRONT_RW: std::sync::RwLock<()> = std::sync::RwLock::new(());

/// The front end's write lock within `ms`, or None: the exit's last writes give up rather than
/// wait for a reader (an app thread that parked for good holding a type check's read lock:
/// EXITING parks every other thread that asks for the unit lock).
pub fn front_write_within(ms: u64) -> Option<std::sync::RwLockWriteGuard<'static, ()>> {
    let _waiting = crate::preempt::WriterWaiting::new();
    let t0 = std::time::Instant::now();
    loop {
        match FRONT_RW.try_write() {
            Ok(g) => return Some(g),
            Err(std::sync::TryLockError::Poisoned(e)) => return Some(e.into_inner()),
            Err(std::sync::TryLockError::WouldBlock) => {}
        }
        if t0.elapsed().as_millis() as u64 >= ms {
            return None;
        }
        if owner::is_owner() {
            // (a reader may wait for one of the owner's answers)
            owner::pump_wait(&|| FRONT_RW.try_write().is_ok() || t0.elapsed().as_millis() as u64 >= ms);
        } else {
            std::thread::sleep(std::time::Duration::from_micros(200));
        }
    }
}

pub fn front_write() -> std::sync::RwLockWriteGuard<'static, ()> {
    // background checks that opted in (Gandalf's type checks) stop at their next preemption
    // point instead of finishing first (relax_types::preempt)
    let _waiting = crate::preempt::WriterWaiting::new();
    if !owner::is_owner() {
        return FRONT_RW.write().unwrap_or_else(|e| e.into_inner());
    }
    // the owner: a reader may be waiting for one of its answers (a constant for a type check),
    // so it serves them while it waits; a request served nested inside another's wait gives
    // way instead (that request's own readers may be what holds the front)
    owner::busy_if_nested();
    loop {
        match FRONT_RW.try_write() {
            Ok(g) => return g,
            Err(std::sync::TryLockError::Poisoned(e)) => return e.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => owner::pump_wait(&|| FRONT_RW.try_write().is_ok()),
        }
    }
}

/// MAKEPAD_RUN_FRONT_TRACE=1: the front read holders by site (diagnostics of writer waits): each
/// hold longer than 50 ms is printed when it ends, with its site and thread.
fn front_trace() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("MAKEPAD_RUN_FRONT_TRACE").is_some())
}
pub struct FrontRead {
    _g: std::sync::RwLockReadGuard<'static, ()>,
    traced: Option<(&'static str, std::time::Instant)>,
}
impl Drop for FrontRead {
    fn drop(&mut self) {
        if let Some((site, t)) = self.traced {
            let ms = t.elapsed().as_secs_f64() * 1e3;
            if ms > 50.0 {
                eprintln!("front-hold: {} ms by {} on {:?} (ended t={:.0} ms)", ms as u64, site, std::thread::current().name().unwrap_or("?"), wall_ns() as f64 / 1e6);
            }
        }
    }
}
pub fn front_read(site: &'static str) -> FrontRead {
    let g = FRONT_RW.read().unwrap_or_else(|e| e.into_inner());
    FrontRead { _g: g, traced: front_trace().then(|| (site, std::time::Instant::now())) }
}
/// What type checks ahead need of a unit, reachable without its lock: the program and type
/// context (shared, Sync), slot keys, and the checked bodies waiting for their builds.
pub struct FrontShared {
    census_id: usize,
    session: std::sync::Arc<crate::Session>,
    prog: std::sync::atomic::AtomicPtr<Program>,
    tcx: std::sync::atomic::AtomicPtr<Tcx>,
    unit: std::sync::atomic::AtomicPtr<Unit>,
    core_crate: std::sync::atomic::AtomicU32,
    src_toks: std::sync::atomic::AtomicU32,
    /// set once the unit sits at its final address; cleared when it is dropped
    alive: AtomicBool,
    pub(crate) warm_cache: AtomicBool,
    owner_verdicts: crate::conc::MemoMap<u64, std::sync::Arc<crate::cache::OwnerVerdicts>>,
    /// slot -> key (the fns table moves when it grows)
    slot_keys: crate::conc::MemoMap<u32, FnKey>,
    /// checked bodies with the collect epoch they were checked at
    ahead: crate::conc::ShardMap<AheadKey, (u64, std::sync::Arc<Body>)>,
    /// Claims and the publish/take handoff share one short lock. No type checking under it.
    inprog: relax_session::sync::Mutex<crate::fxhash::HashSet<AheadKey>>,
    /// Instances whose build consumed their body: no new ahead checks.
    built: crate::conc::MemoMap<AheadKey, ()>,
    /// A one-use handoff from lowering helpers. None reserves a slot; removing it lets the
    /// normal build take over without waiting. No slot's build state is owned by a helper.
    lowered: relax_session::sync::Mutex<HashMap<u32, Option<LoweredAhead>>>,
    /// Until the first present, `helpers` threads type-check every new slot's body ahead (queue
    /// of slots): a cold start's main thread finds most bodies it calls checked
    /// (MAKEPAD_RUN_AHEAD_HELPERS=n; default ahead_helpers())
    helpers: u32,
    queue: relax_session::sync::Mutex<std::collections::VecDeque<AheadWork>>,
    wake: relax_session::sync::Condvar,
}

impl FrontShared {
    /// Memory report for the shared slot index and pending ahead-work queue.
    pub fn mem_lines(&self) -> Vec<(String, u64)> {
        let (sn, sb) = self.slot_keys.mem_stats(|_, _| 0);
        let queue = self.queue.lock().unwrap_or_else(|e| e.into_inner());
        vec![(format!("front: slot keys ({})", sn), sb as u64),
             (format!("front: queued slots ({})", queue.len()), (queue.capacity() * std::mem::size_of::<u32>()) as u64)]
    }

    fn new(sess: &std::sync::Arc<crate::Session>) -> Self {
        let options = &sess.options;
        FrontShared {
            census_id: crate::exec_census::new_unit(),
            session: sess.clone(),
            prog: Default::default(),
            tcx: Default::default(),
            unit: Default::default(),
            core_crate: Default::default(),
            src_toks: Default::default(),
            alive: AtomicBool::new(false),
            warm_cache: AtomicBool::new(false),
            owner_verdicts: Default::default(),
            slot_keys: crate::conc::MemoMap::with_bits(16),
            ahead: Default::default(),
            inprog: Default::default(),
            built: crate::conc::MemoMap::with_bits(16),
            lowered: Default::default(),
            helpers: if options.tyck_ahead { options.ahead_helpers.unwrap_or_else(ahead_helpers) } else { 0 },
            queue: Default::default(),
            wake: Default::default(),
        }
    }
}

/// The front state of the last unit set up (diagnostics: MAKEPAD_RUN_CODE_SIG_CANON).
static LAST_FRONT: relax_session::sync::Mutex<Option<std::sync::Arc<FrontShared>>> = relax_session::sync::Mutex::new(None);

/// MAKEPAD_RUN_CODE_SIG_CANON=1: `Fn(slot)` in a RIR dump becomes the slot's instance (path and
/// argument types as text), so signatures compare across runs whose slot numbering differs
/// (ahead type checking can evaluate constants in a different order).
fn canon_slots(d: String) -> String {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if !*ON.get_or_init(|| std::env::var("MAKEPAD_RUN_CODE_SIG_CANON").is_ok()) {
        return d;
    }
    let f = match LAST_FRONT.lock().unwrap_or_else(|e| e.into_inner()).clone() {
        Some(f) => f,
        None => return d,
    };
    let _r = front_read("canon_slots");
    let (prog, tcx) = unsafe { (&*f.prog.load(Ordering::Relaxed), &*f.tcx.load(Ordering::Relaxed)) };
    let name_of = |id: u32| -> Option<String> {
        Some(match f.slot_keys.get(&id)? {
            FnKey::Inst(d, a) => {
                let mut s = prog.def_path(d);
                for t in a {
                    s.push(',');
                    s.push_str(&crate::typeck::ty_to_string(false, prog, tcx, t));
                }
                s
            }
            FnKey::Closure(file, e, _) => format!("closure:{}:{}", file, e),
            FnKey::Shim(shim) => crate::lower::shim_name(prog, tcx, &shim),
            FnKey::Glue(g, t) => format!("glue{}:{}", g, crate::typeck::ty_to_string(false, prog, tcx, t)),
            FnKey::Sandbox(b) => format!("sandbox:{}", b),
        })
    };
    // `Fn(N)` / `CFn(N, ..)` callees and `FnAddr(VReg(v), N)`; a closure's name carries its parent's
    // slot (`{closure#314}@15917`): the slot is dropped
    let mut out = String::with_capacity(d.len());
    for line in d.lines() {
        let mut l = line.to_string();
        while let Some(i) = l.find("}@") {
            let st = i + 2;
            let en = l[st..].find(|ch: char| !ch.is_ascii_digit()).map_or(l.len(), |x| st + x);
            if en == st {
                break;
            }
            l.replace_range(i + 1..en, "");
        }
        if let Some(i) = l.find("FnAddr(VReg(") {
            if let Some(c) = l[i..].find("), ") {
                let st = i + c + 3;
                let en = l[st..].find(|ch: char| !ch.is_ascii_digit()).map_or(l.len(), |x| st + x);
                if let Some(n) = l[st..en].parse::<u32>().ok().and_then(|id| name_of(id)) {
                    l.replace_range(st..en, &n);
                }
            }
        }
        let mut res = String::new();
        let mut rest = l.as_str();
        while let Some(i) = rest.find("Fn(") {
            res.push_str(&rest[..i + 3]);
            rest = &rest[i + 3..];
            let j = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
            match rest[..j].parse::<u32>().ok().and_then(|id| name_of(id)) {
                Some(n) if j > 0 => res.push_str(&n),
                _ => res.push_str(&rest[..j]),
            }
            rest = &rest[j..];
        }
        res.push_str(rest);
        out.push_str(&res);
        out.push('\n');
    }
    out
}

fn set_shared_front(u: &mut Unit, core_crate: u32) {
    let options = &u.prog.session.clone().options;
    *LAST_FRONT.lock().unwrap_or_else(|e| e.into_inner()) = Some(u.front.clone());
    let tier1 = u.tier1.is_some();
    let up = u as *mut Unit;
    let f = &u.front;
    f.unit.store(up, Ordering::SeqCst);
    f.prog.store(&u.prog as *const Program as *mut Program, Ordering::SeqCst);
    f.tcx.store(&u.tcx as *const Tcx as *mut Tcx, Ordering::SeqCst);
    f.core_crate.store(core_crate, Ordering::SeqCst);
    f.src_toks.store(inline_src_toks(options, tier1).max(inline_src_toks(options, !tier1)), Ordering::SeqCst);
    f.alive.store(true, Ordering::SeqCst);
    for i in 0..if startup_phase() { f.helpers } else { 0 } {
        let f = u.front.clone();
        { let _k = crate::zone_alloc::kept();
        let _ = std::thread::Builder::new().name(format!("run-ahead{}", i)).spawn(move || { let _z = crate::zone_alloc::compiler_thread(); ahead_helper(f) });
        }
    }
}

/// Ahead helpers by default: the main thread's lazy compiles spend about a third of a cold
/// start's main -> first present in type checks; a few threads keep ahead of them.
fn ahead_helpers() -> u32 {
    std::thread::available_parallelism().map_or(2, |n| n.get()).saturating_sub(2).clamp(1, 4) as u32
}

/// A helper thread: checks and lowers queued slots' bodies ahead until the first present (then
/// speculative workers check what they build ahead) or until the unit is dropped.
fn ahead_helper(f: std::sync::Arc<FrontShared>) {
    loop {
        let id = {
            let mut q = f.queue.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                if !f.alive.load(Ordering::SeqCst) || !startup_phase() {
                    q.clear();
                    drop(q);
                    // Unused raw RIR is startup work only. Drop it on a helper, outside the
                    // queue/publication locks; in-flight helpers cannot republish removed slots.
                    let unused = std::mem::take(&mut *f.lowered.lock().unwrap_or_else(|e| e.into_inner()));
                    drop(unused);
                    return;
                }
                if let Some(id) = q.pop_front() {
                    break id;
                }
                q = f.wake.wait_timeout(q, std::time::Duration::from_millis(50)).map(|x| x.0).unwrap_or_else(|e| e.into_inner().0);
            }
        };
        // a validated warm start links cached code: no bodies to check (typeck_ahead_slot's rule)
        if f.warm_cache.load(Ordering::Relaxed) && crate::reload::edited_crates().is_empty() {
            continue;
        }
        let (d, args, slot, normal) = match id {
            AheadWork::Slot(id) => match f.slot_keys.get(&id) {
                Some(FnKey::Inst(d, args)) => (d, args, Some(id), false),
                _ => continue,
            },
            AheadWork::Body((d, args), normal) => (d, args, None, normal),
        };
        let _job = compile_job();
        let _census_context = crate::exec_census::Context::new(f.census_id, false);
        let _cost = slot.map(|id| crate::fncost::Record::new(id, 0, "ahead"));
        let t0 = cpu_ns();
        typeck_ahead(&f, d, args, 1, true, normal);
        AHEAD_STATS[4].fetch_add(cpu_ns() - t0, Ordering::Relaxed);
        if let Some(id) = slot {
            lower_ahead(&f, id);
        }
    }
}

struct LoweredAhead {
    epoch: u64,
    file: u32,
    func: crate::rir::Func,
    /// Closures share their owner's body, installed only by the locked build.
    body: Option<std::sync::Arc<Body>>,
    /// New callees in the order normal lowering would discover them. Helpers do not create
    /// their slots: doing that early changes check's traversal and inline preparation order.
    callees: Vec<FnKey>,
}

thread_local! {
    static LOWERING_AHEAD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
crate::gen_local! {
    static LOWER_AHEAD_IDS: std::cell::RefCell<AheadFnIds>;
}

#[derive(Default)]
struct AheadFnIds {
    keys: Vec<FnKey>,
    ids: HashMap<FnKey, u32>,
}

struct LowerAheadGuard;
impl Drop for LowerAheadGuard {
    fn drop(&mut self) {
        LOWERING_AHEAD.with(|a| a.set(false));
        LOWER_AHEAD_IDS.with(|a| *a.borrow_mut() = AheadFnIds::default());
    }
}

/// Temporary function references, descending from u32::MAX, exist only in a helper's raw
/// RIR. The locked front resolves them in discovery order before any optimiser sees it.
fn lower_ahead_fn_id(u: &Unit, key: &FnKey) -> Option<u32> {
    if !LOWERING_AHEAD.try_with(|a| a.get()).unwrap_or(false) { return None; }
    // A real slot can be published during this lowering. Keep using its first temporary
    // identity so repeated calls and per-callee metadata stay identical to locked lowering.
    if let Some(id) = LOWER_AHEAD_IDS.with(|a| a.borrow().ids.get(key).copied()) { return Some(id); }
    if let Some(id) = u.fn_map.get(key) { return Some(id); }
    // Constructor and coroutine-poll keys can become a different kind of slot in fn_id.
    // Leave those rewrites (and their track_caller identity) on the ordinary path.
    if let FnKey::Inst(d, args) = key {
        if !matches!(u.prog.def(*d).kind, DefKind::Fn | DefKind::AssocFn)
            || args.first().is_some_and(|t| u.tcx.is_coroutine(*t))
        { lower_ahead_needs_unit(); }
    }
    Some(LOWER_AHEAD_IDS.with(|a| {
        let mut a = a.borrow_mut();
        if let Some(&id) = a.ids.get(key) { return id; }
        let id = u32::MAX - a.keys.len() as u32;
        a.keys.push(key.clone());
        a.ids.insert(key.clone(), id);
        id
    }))
}

/// Resolve temporary callee metadata for caller tracking and RustCall argument spreading.
pub(crate) fn lower_ahead_fn_key(id: u32) -> Option<FnKey> {
    if !LOWERING_AHEAD.try_with(|a| a.get()).unwrap_or(false) { return None; }
    LOWER_AHEAD_IDS.with(|a| a.borrow().keys.get((u32::MAX - id) as usize).cloned())
}

/// FRONT_RW readers must never acquire the unit lock: its owner may be waiting for their
/// read guard to re-collect types. Abort *before* acquiring it, including when uncontended.
/// resume_unwind avoids the panic hook; the helper leaves the checked body for a normal build.
/// NeedsUnit is a cancellation: a constant evaluation it crosses (the solver normalizing an
/// array length) is not cached as that constant's failure.
#[inline]
pub(crate) fn lower_ahead_needs_unit() {
    if LOWERING_AHEAD.try_with(|a| a.get()).unwrap_or(false) {
        std::panic::resume_unwind(Box::new(crate::preempt::NeedsUnit));
    }
}

fn lower_ahead(f: &FrontShared, id: u32) {
    let _r = front_read("lower_ahead");
    if !f.alive.load(Ordering::SeqCst) || !startup_phase() { return; }
    let Some(FnKey::Inst(d, args)) = f.slot_keys.get(&id) else { return; };
    let key = (d, args);
    if f.built.contains(&key) { return; }
    let Some((epoch, body)) = f.ahead.get(&key) else { return; };
    let u = unsafe { &*f.unit.load(Ordering::Relaxed) };
    let fn_key = FnKey::Inst(d, key.1.clone());
    // fn_map publishes after the slot's fields (including a decoded image identity).
    // The ahead queue can contain the id before fn_id/name_slot finishes publishing it.
    if epoch != u.tcx.collect_epoch.load(Ordering::SeqCst) || u.fn_map.get(&fn_key) != Some(id) { return; }
    let def = u.prog.def(d);
    // Trait declarations may lower as virtual/derived shims rather than from their body.
    // Provided functions likewise use the locked path's bridge selection.
    if !matches!(def.kind, DefKind::Fn | DefKind::AssocFn)
        || (def.parent != crate::program::NO_DEF && u.prog.def(def.parent).kind == DefKind::Trait)
        || crate::sandbox::is_wrapper_crate(def.krate)
        || !u.provided.is_empty() && u.provided.iter().any(|(kr, _)| *kr == def.krate)
    { return; }
    // These are evaluated before ordinary lowering, including uses on unreachable paths.
    // Do not skip that preparation or inspect the unit-lock-owned constant/static maps.
    if body.res.values().chain(body.pat_res.values()).chain(body.fmt_named.values()).any(|r| {
        matches!(r, Res::Def(d) if matches!(u.prog.def(*d).kind, DefKind::Const | DefKind::AssocConst | DefKind::Static))
    }) { return; }
    {
        let mut pending = f.lowered.lock().unwrap_or_else(|e| e.into_inner());
        if f.built.contains(&key) || pending.contains_key(&id) { return; }
        pending.insert(id, None);
    }
    let t0 = cpu_ns();
    let _cost_lower = crate::fncost::phase(3);
    let _mem = crate::memstat::tag(crate::memstat::Tag::Lower);
    let _ = crate::program::take_missing();
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        LOWERING_AHEAD.with(|a| a.set(true));
        let _guard = LowerAheadGuard;
        let body = crate::typeck::codegen_body(&u.prog, &u.tcx, d, &key.1);
        let (func, closures) = lower_checked_body(u, id, &fn_key, &body)?;
        let callees = LOWER_AHEAD_IDS.with(|a| std::mem::take(&mut a.borrow_mut().keys));
        Ok::<_, String>(LoweredAhead { epoch, file: body.file, func, body: closures.then_some(body), callees })
    }));
    let missing = !crate::program::take_missing().is_empty();
    let ready = match r {
        Ok(Ok(r)) if !missing => Some(r),
        Err(p) => {
            if !p.is::<crate::preempt::NeedsUnit>() && std::env::var_os("MAKEPAD_RUN_AHEAD_TRACE").is_some() {
                eprintln!("ahead-lower panic: {}", u.prog.def_path(d));
            }
            None
        }
        _ => None,
    };
    if std::env::var_os("MAKEPAD_RUN_AHEAD_TRACE").is_some() {
        eprintln!("ahead-lower {} {} us: {}", if ready.is_some() { "ready" } else { "deferred" }, (cpu_ns() - t0) / 1000, u.prog.def_path(d));
    }
    let mut pending = f.lowered.lock().unwrap_or_else(|e| e.into_inner());
    if f.built.contains(&key) {
        pending.remove(&id);
    } else if let Some(slot) = pending.get_mut(&id) {
        *slot = ready;
    }
}

/// Unit lock held. Never wait for lowering: taking an unfinished reservation cancels its
/// publication, and this build uses the already checked body through the ordinary path.
fn take_lowered_ahead(u: &mut Unit, id: u32) -> Option<crate::rir::Func> {
    let mut lowered = u.front.lowered.lock().unwrap_or_else(|e| e.into_inner()).remove(&id)??;
    if crate::boundary::on()
        || lowered.epoch != u.tcx.collect_epoch.load(Ordering::SeqCst)
        || lowered.file != u.fns[id as usize].file
    { return None; }
    if let FnKey::Inst(d, args) = &u.fns[id as usize].key {
        let key = (*d, args.clone());
        u.front.built.insert(key.clone(), ());
        u.front.ahead.remove(&key);
    }
    if let Some(body) = lowered.body { u.put_body(id, body); }
    if !lowered.callees.is_empty() {
        let mut ids = Vec::with_capacity(lowered.callees.len());
        let resolved = |id: u32, ids: &[u32]| ids.get((u32::MAX - id) as usize).copied().unwrap_or(id);
        for mut key in lowered.callees {
            if let FnKey::Closure(_, _, parent) | FnKey::Sandbox(parent) = &mut key {
                *parent = resolved(*parent, &ids);
            }
            ids.push(u.fn_id(key));
        }
        for block in &mut lowered.func.blocks {
            for inst in &mut block.insts {
                use crate::rir::{Inst, Callee};
                if let Inst::Call(Callee::Fn(c) | Callee::CFn(c, _), _, _) | Inst::FnAddr(_, c) = inst {
                    *c = resolved(*c, &ids);
                }
            }
        }
        for callee in &mut lowered.func.sret_callees {
            *callee = resolved(*callee, &ids);
        }
        for (callee, _) in &mut lowered.func.nounwind_inputs {
            *callee = resolved(*callee, &ids);
        }
    }
    if std::env::var_os("MAKEPAD_RUN_AHEAD_TRACE").is_some() {
        eprintln!("ahead-lower taken: {}", u.fns[id as usize].name);
    }
    Some(lowered.func)
}

type AheadKey = (DefId, Vec<TyId>);

enum AheadWork {
    Slot(u32),
    /// An inline candidate (normal O1), before lowering creates its slot.
    Body(AheadKey, bool),
}

fn publish_ahead(f: &FrontShared, key: AheadKey, epoch: u64, body: std::sync::Arc<Body>) {
    let before = crate::memstat::live_total();
    let claim = f.inprog.lock().unwrap_or_else(|e| e.into_inner());
    if !f.built.contains(&key) {
        f.ahead.insert(key, (epoch, body));
    } else {
        // The build took over. Drop outside the claim lock, including large bodies.
        drop(claim);
        drop(body);
        DONE_BODY_BYTES.fetch_add(freed_since(before), Ordering::Relaxed);
    }
}

/// bodies checked ahead, taken by a build, checked with errors (dropped); cpu ns of the
/// checks ahead on callers' threads and on the speculative workers
pub static AHEAD_STATS: [AtomicU64; 5] = [AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0)];

fn ahead_on(options: &crate::options::Options) -> bool {
    options.tyck_ahead
}

/// The checked-ahead body of an instance, if one waits (unit lock held).
fn take_ahead(prog: &Program, f: &FrontShared, tcx: &Tcx, d: DefId, args: &[TyId]) -> Result<std::sync::Arc<Body>, &'static str> {
    if !ahead_on(&prog.session.options) {
        return Err("off");
    }
    let key = (d, args.to_vec());
    let claim = f.inprog.lock().unwrap_or_else(|e| e.into_inner());
    f.built.insert(key.clone(), ());
    let result = f.ahead.remove(&key).ok_or_else(|| if claim.contains(&key) { "inprog" } else { "absent" });
    drop(claim);
    let (epoch, b) = result?;
    // checked before signatures were re-collected: check again
    if epoch != tcx.collect_epoch.load(Ordering::SeqCst) {
        return Err("epoch");
    }
    AHEAD_STATS[1].fetch_add(1, Ordering::Relaxed);
    if *b.args == *args { return Ok(b); }
    Ok(crate::typeck::codegen_body(prog, tcx, d, args))
}

/// CTFE's existing const-fn body can also supply code generation.
fn ctfe_checked_body(u: &Unit, d: DefId, args: &[TyId]) -> Option<std::sync::Arc<Body>> {
    let cached = u.ctfe_fn_bodies.lock().unwrap_or_else(|e| e.into_inner()).get(&(d, args.to_vec())).cloned();
    let (epoch, b) = cached?;
    (epoch == u.tcx.collect_epoch.load(Ordering::SeqCst)).then_some(b)
}

/// An initializer outside the interpreter's subset still has a checked body for its JIT
/// fallback. Keep the check's original epoch: evaluation may have re-collected signatures.
pub(crate) fn ctfe_fallback_body(u: &Unit, epoch: Option<u64>, b: std::sync::Arc<Body>) {
    let Some(epoch) = epoch else { return };
    if ahead_on(&u.prog.session.options) && b.deferred_misses.is_empty() {
        publish_ahead(&u.front, (b.def, b.args.to_vec()), epoch, b);
    }
}

fn ahead_check_diag(u: &Unit, id: u32, reason: &str, ns: u64) {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ON.get_or_init(|| std::env::var_os("MAKEPAD_RUN_AHEAD_MISSES").is_some()) {
        let mut trace = format!("ahead-build\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n", reason, ns, id,
            MAIN_TID.load(Ordering::Relaxed) == me(), startup_phase(),
            u.tcx.collect_epoch.load(Ordering::Relaxed), u.fns[id as usize].name);
        if reason != "hit" && reason != "ctfe" {
            BUILD_STACK.with(|s| {
                let stack = s.borrow();
                let parent = stack.iter().rev().copied().find(|p| *p != id);
                if let Some(parent) = parent {
                    trace.push_str(&format!("ahead-parent\t{}\t{}\t{}\n", id, reason, u.fns[parent as usize].name));
                }
            });
        }
        // The hosted app can write to fd 2 without this Rust runtime's stderr lock.
        use std::io::Write;
        let _ = std::io::stderr().lock().write_all(trace.as_bytes());
    }
}

/// Type-checks slot `id`'s body ahead (no unit lock held). `up` only reaches the unit's
/// FrontShared (a field never written after the unit is set up).
pub unsafe fn typeck_ahead_slot(up: *mut Unit, id: u32) {
    if up.is_null() { return; }
    let _census_context = (&*up).exec_context(crate::exec_census::active() && crate::exec_profile::is_call(up as usize, id));

    let _cost = crate::fncost::Record::ensure(id, 0, "ahead");
    let session = unsafe { (*std::ptr::addr_of!((*up).prog.session)).clone() };
    let options = &session.options;
    if !ahead_on(options) {
        return;
    }
    let f: std::sync::Arc<FrontShared> = (*std::ptr::addr_of!((*up).front)).clone();
    // A few legitimate warm misses must not speculatively type-check their
    // cached callees. The normal build checks a missing body on demand. After a
    // live edit the edited code is not cached: speculation helps again.
    if f.warm_cache.load(Ordering::Relaxed) && crate::reload::edited_crates().is_empty() { return; }
    if let Some(FnKey::Inst(d, args)) = f.slot_keys.get(&id) {
        let t0 = cpu_ns();
        let key = (d, args.clone());
        typeck_ahead(&f, d, args, 3, false, false);
        // a helper is checking this very body: wait for its result rather than check it again
        // under the unit lock (not while holding that lock: a constant evaluation's call)
        if !owner::is_owner() && f.inprog.lock().unwrap_or_else(|e| e.into_inner()).contains(&key) {
            let mut g = AHEAD_DONE.0.lock().unwrap_or_else(|e| e.into_inner());
            while f.inprog.lock().unwrap_or_else(|e| e.into_inner()).contains(&key) && f.alive.load(Ordering::SeqCst) {
                g = AHEAD_DONE.1.wait_timeout(g, std::time::Duration::from_millis(5)).map(|x| x.0).unwrap_or_else(|e| e.into_inner().0);
            }
        }
        let w = IS_WORKER.try_with(|w| w.get()).unwrap_or(false);
        AHEAD_STATS[if w { 4 } else { 3 }].fetch_add(cpu_ns() - t0, Ordering::Relaxed);
    }
}

/// Signalled when a check ahead ends (typeck_ahead_slot waits for a helper's check in flight).
static AHEAD_DONE: (relax_session::sync::Mutex<()>, relax_session::sync::Condvar) = (relax_session::sync::Mutex::new(()), relax_session::sync::Condvar::new());

fn inprog_end(f: &FrontShared, key: &AheadKey) {
    f.inprog.lock().unwrap_or_else(|e| e.into_inner()).remove(key);
    let _g = AHEAD_DONE.0.lock().unwrap_or_else(|e| e.into_inner());
    AHEAD_DONE.1.notify_all();
}

fn typeck_ahead(f: &FrontShared, d: DefId, args: Vec<TyId>, depth: u32, queued: bool, normal: bool) {
    let _census_context = crate::exec_census::Context::new(f.census_id, false);
    let core = f.core_crate.load(Ordering::Relaxed);
    let src_toks = f.src_toks.load(Ordering::Relaxed);
    let mut todo = vec![(d, args, depth, normal)];
    while let Some((d, args, depth, normal)) = todo.pop() {
        let key = (d, args);
        {
            let mut claim = f.inprog.lock().unwrap_or_else(|e| e.into_inner());
            if f.built.contains(&key) || f.ahead.contains_key(&key) || !claim.insert(key.clone()) {
                continue;
            }
        }
        // the program and its types stay unchanged while the read guard is held
        let _r = front_read("typeck_ahead");
        if !f.alive.load(Ordering::SeqCst) {
            inprog_end(f, &key);
            return;
        }
        if f.built.contains(&key) {
            inprog_end(f, &key);
            continue;
        }
        let (prog, tcx) = unsafe { (&*f.prog.load(Ordering::Relaxed), &*f.tcx.load(Ordering::Relaxed)) };
        if !matches!(prog.def(key.0).kind, DefKind::Fn | DefKind::AssocFn) {
            inprog_end(f, &key);
            continue;
        }
        let epoch = tcx.collect_epoch.load(Ordering::SeqCst);
        let _o = crate::clock::origin("ahead");
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _p = crate::preempt::Preemptible::on();
            crate::preempt::point();
            tcx.typeck(prog, key.0)
        }));
        if r.as_ref().is_err_and(|p| crate::preempt::is_preempted(&**p)) {
            inprog_end(f, &key);
            if crate::preempt::is_stopped() { return; }
            todo.push((key.0, key.1, depth, normal));
            drop(_r);
            std::thread::sleep(std::time::Duration::from_millis(5));
            continue;
        }
        if std::env::var_os("MAKEPAD_RUN_AHEAD_TRACE").is_some() {
            match &r {
                Err(p) => eprintln!("ahead-panic {} : {}", prog.def_path(key.0), p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_default()),
                Ok(b) if !b.errors.is_empty() => eprintln!("ahead-error {} : {}", prog.def_path(key.0), b.errors[0].chars().take(160).collect::<String>()),
                _ => {}
            }
        }
        let b = match r {
            Ok(b) if b.errors.is_empty() && b.deferred_misses.is_empty() => b,
            _ => {
                AHEAD_STATS[2].fetch_add(1, Ordering::Relaxed);
                inprog_end(f, &key);
                continue;
            }
        };
        // Discover only direct inline candidates; publish before checking any of them.
        let callees = if depth > 0 {
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| body_callees(prog, tcx, &b, true))) {
                Ok(c) => c,
                Err(_) => Vec::new(),
            }
        } else { Vec::new() };
        AHEAD_STATS[0].fetch_add(1, Ordering::Relaxed);
        publish_ahead(f, key.clone(), epoch, b);
        inprog_end(f, &key);
        let mut next = Vec::new();
        for (c, mut cargs) in callees {
            let cd = prog.def(c);
            if cd.krate != core || !matches!(cd.kind, DefKind::Fn | DefKind::AssocFn) {
                continue;
            }
            // what prepare_rir_from does not take as an inline candidate is not checked ahead
            if inline_never(prog, c) || is_cold_def(prog, tcx, c) || is_cold_name(&prog.def_path(c)) {
                continue;
            }
            let attr = inline_attrs(prog, c);
            let it = prog.files[cd.file as usize].ast.item(cd.item);
            let toks = &prog.files[cd.file as usize].toks;
            let src = (toks.partition_point(|t| t.lo < it.hi) - toks.partition_point(|t| t.lo < it.lo)) as u32;
            let baseline = !normal && startup_phase() && f.session.options.t0_baseline != crate::options::Tier0Baseline::Off;
            let want = if baseline { src <= BASE_INLINE_SRC_TOKS } else {
                attr & crate::opt::ATTR_ALWAYS != 0 || src <= src_toks || (attr & crate::opt::ATTR_HINT != 0 && src <= INLINE_HINT_SRC_TOKS)
            };
            if want {
                // fn_id keys codegen instances with erased regions too.
                cargs.iter_mut().for_each(|t| *t = tcx.tys.erase_regions(*t));
                if queued {
                    // Baseline inlines a candidate's normal O1 body, whose own
                    // inline candidates use the normal source-size rule too.
                    next.push(AheadWork::Body((c, cargs), true));
                } else {
                    todo.push((c, cargs, depth - 1, true));
                }
            }
        }
        if !next.is_empty() {
            let mut q = f.queue.lock().unwrap_or_else(|e| e.into_inner());
            for work in next.into_iter().rev() { q.push_front(work); }
            f.wake.notify_all();
        }
    }
}

/// The fn instances a checked body calls, as lowering will most likely key them (trait
/// methods resolved to their impls); open or erroneous instances are left out. A guess: a
/// miss only means the build checks that body itself. `warm_only`: calls inside a diverging
/// call's expression (`panic!(..)`'s formatting, `unwrap_failed(..)`) are left out, as
/// prepare_rir_from leaves the callees of cold blocks out of its inline candidates.
pub(crate) fn body_callees(prog: &Program, tcx: &Tcx, b: &Body, warm_only: bool) -> Vec<(DefId, Vec<TyId>)> {
    use crate::ast::ExprKind;
    let ast = &prog.files[b.file as usize].ast;
    let cold: Vec<(u32, u32)> = if warm_only {
        (0..b.expr_ty.len())
            .filter(|&i| b.expr_ty[i] == tcx.tys.never)
            .filter_map(|i| {
                let e = ast.expr(b.expr_at(i));
                matches!(e.kind, ExprKind::Call(..) | ExprKind::MethodCall { .. } | ExprKind::Mac(..)).then_some((e.lo, e.hi))
            })
            .collect()
    } else {
        Vec::new()
    };
    let in_cold = |k: u32| {
        !cold.is_empty() && b.has_expr(crate::ast::ExprId(k)) && {
            let e = ast.expr(crate::ast::ExprId(k));
            cold.iter().any(|&(lo, hi)| lo <= e.lo && e.hi <= hi)
        }
    };
    let mut out: Vec<(DefId, Vec<TyId>)> = Vec::new();
    let mut add = |k: u32, d: DefId, args: &[TyId]| {
        // rustc_ty_utils/src/instance.rs:187-198 @8d1a764: instance
        // discovery is a codegen boundary, including non-trait callees.
        let mut erased = Vec::with_capacity(args.len());
        for &ty in args {
            let Ok(ty) = tcx.try_normalize_after_erasing_regions(prog, crate::tcx::TypingEnv::fully_monomorphized(&prog), ty) else { return };
            erased.push(ty);
        }
        let args = erased.as_slice();
        if !args.iter().all(|t| tcx.tys.is_concrete(*t)) || in_cold(k) {
            return;
        }
        let Ok(instance) = tcx.resolve_instance_raw(prog, crate::tcx::TypingEnv::fully_monomorphized(&prog), d, args) else { return };
        // Ahead checking handles source bodies only; generated bodies are built at their slots.
        let InstanceKind::Item(rd) = instance.def else { return };
        if prog.def(rd).kind == DefKind::Closure { return; }
        let ra = instance.compact_args(tcx);
        if !out.iter().any(|x| x.0 == rd && x.1 == ra) {
            out.push((rd, ra));
        }
    };
    for (k, r) in &b.res {
        if let Res::Def(d) = r {
            if matches!(prog.def(*d).kind, DefKind::Fn | DefKind::AssocFn) {
                let a = b.res_args.get(k).map_or(&[][..], |&v| tcx.tys.list(v));
                add(*k, *d, a);
            }
        }
    }
    for (k, m) in b.methods.iter().chain(b.binops.iter()) {
        add(*k, m.def, tcx.tys.list(m.args));
    }
    for ((k, _), (d, a)) in &b.ov_derefs {
        add(*k, *d, tcx.tys.list(*a));
    }
    for (k, (d, a)) in b.ov_index.iter().chain(b.for_next.iter()) {
        add(*k, *d, tcx.tys.list(*a));
    }
    for (k, (d, a, _)) in &b.for_into {
        add(*k, *d, tcx.tys.list(*a));
    }
    out
}

/// Bump allocation in [*next, end): `size` bytes at `align` (lock-free: lowering threads and
/// the lock holder allocate concurrently).
fn bump(next: &AtomicU64, size: u64, align: u64, end: u64, full: &str) -> u64 {
    let a = align.max(1);
    let mut cur = next.load(Ordering::Relaxed);
    loop {
        let at = (cur + a - 1) / a * a;
        if at + size > end {
            panic!("{}", full);
        }
        match next.compare_exchange_weak(cur, at + size, Ordering::AcqRel, Ordering::Relaxed) {
            Ok(_) => {
                // where pages are committed on demand (Windows), the range becomes usable here
                os::mem::commit(at, size as usize);
                return at;
            }
            Err(x) => cur = x,
        }
    }
}

/// A unit's own address (see Unit::self_ptr).
struct UnitPtr(*mut Unit);
unsafe impl Send for UnitPtr {}
unsafe impl Sync for UnitPtr {}

/// A constant's stable name (its data object's identity, syms.rs).
pub fn const_name(u: &Unit, d: DefId, args: &[TyId]) -> String {
    let mut s = format!("const:{}", u.prog.def_path(d));
    for t in args {
        s.push(',');
        s.push_str(&crate::syms::stable_ty(u, *t));
    }
    s
}

/// Live sessions: functions a patch reached and took from the disk cache instead of compiling.
pub static CACHE_AHEAD: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn count_cache_ahead() {
    CACHE_AHEAD.fetch_add(1, Ordering::Relaxed);
}

/// The disk cache's code for slot `id`, linked (unit lock held); None: compile it.
pub(crate) unsafe fn cache_lookup(up: *mut Unit, id: u32) -> Option<(Vec<u8>, Vec<(u32, u64)>, Vec<crate::rir::RelocAt>)> {
    let _mem = crate::memstat::tag(crate::memstat::Tag::CacheLoad);
    let u = &mut *up;
    if matches!(u.fns[id as usize].key, FnKey::Inst(d, _) if u.prog.def(d).kind == DefKind::ForeignFn) {
        crate::warm::reason(id, "foreign veneer");
        return None;
    }
    if u.cache.is_none() { crate::warm::reason(id, "cache absent during nested lookup"); }
    crate::cache::lookup_unit(up, id)
}

/// Before replacing a file, restore any evicted closure owners while the old definition
/// still names its source. Compiled closures may later need RIR too (retier / inlining).
/// put_body moves these to old_bodies when a new version is checked.
pub(crate) unsafe fn preserve_closure_bodies(up: *mut Unit, files: &[u32]) -> Result<(), String> {
    let mut owners: Vec<(u32, u32)> = (&*up).closures_of_files(files).into_iter().filter_map(|id| match (&*up).fns[id as usize].key {
        FnKey::Closure(file, _, parent) if files.contains(&file) => Some((parent, file)),
        _ => None,
    }).collect();
    owners.sort_unstable();
    owners.dedup();
    for (parent, file) in owners {
        ensure_parent_body(up, parent, file, 0)?;
        let u = &mut *up;
        let body = u.body_of(parent, file).unwrap().clone();
        // Freeze shared identity tables before their definitions are replaced.
        // Old closures must never normalize an old alias against the new source.
        if std::sync::Arc::ptr_eq(&body, &u.tcx.typeck(&u.prog, body.def)) {
            let state = crate::tcx::EvalState::new();
            let cx = crate::tcx::EvalCx::new(&u.prog, &u.tcx, crate::tcx::TypingEnv::fully_monomorphized(&u.prog), &state);
            let frozen = crate::typeck::instantiate_body(&cx, &u.prog, &u.tcx, body.def, &body.args);
            u.put_body(parent, std::sync::Arc::new(frozen));
        }
    }
    Ok(())
}

/// The type-checked body closures of slot `id` are lowered from (a fn whose code came from the
/// disk cache had no front end in this run).
/// `file`: the file version the body is needed for (hot reload; u32::MAX: the current one). A
/// version that is no longer current cannot be checked again: reload preserves its closure
/// owners' bodies before replacing it (preserve_closure_bodies), and its closures are lowered
/// from those on their first call.
unsafe fn ensure_parent_body(up: *mut Unit, id: u32, file: u32, depth: u32) -> Result<(), String> {
    if depth > 32 || (&*up).has_body(id, file) {
        return Ok(());
    }
    let key = (&*up).fns[id as usize].key.clone();
    let body = match key {
        FnKey::Inst(d, args) => {
            let u = &mut *up;
            if file != u32::MAX && u.prog.def(d).file != file {
                return Err("closure parent body missing (its file version was replaced)".to_string());
            }
            let _o = crate::clock::origin("closure-owner");
            match take_ahead(&u.prog, &u.front, &u.tcx, d, &args) {
                Ok(b) => b,
                Err(_) => ctfe_checked_body(u, d, &args).unwrap_or_else(|| crate::typeck::codegen_body(&u.prog, &u.tcx, d, &args)),
            }
        }
        FnKey::Sandbox(_) => return Err("closure parent is a sandbox twin".to_string()),
        FnKey::Closure(_, _, grand) => {
            ensure_parent_body(up, grand, file, depth + 1)?;
            match (&*up).body_of(grand, file) {
                Some(b) => b.clone(),
                None => return Err("closure parent body missing".to_string()),
            }
        }
        FnKey::Shim(_) | FnKey::Glue(..) => return Err("closure parent is glue".to_string()),
    };
    if !body.errors.is_empty() {
        return Err(body.errors[0].clone());
    }
    (&mut *up).put_body(id, body);
    Ok(())
}

/// Queues the cached callees of code linked from the disk cache for speculative linking at
/// depth + 1 (linking is cheap: callees without an entry are left to their first call).
unsafe fn spec_enqueue_cached(up: *mut Unit, ids: &[u32], depth: u8) {
    if startup_phase() && pipelined() {
        let mut q = STARTUP_CACHED.lock().unwrap();
        if startup_phase() {
            crate::zone_alloc::kept_with(|| q.push((up as usize, ids.to_vec(), depth)));
            return;
        }
    }
    let session = unsafe { (*std::ptr::addr_of!((*up).prog.session)).clone() };
    let options = &session.options;
    if !pipelined() || ids.is_empty() || depth >= spec_depth_max(options) {
        return;
    }
    let u = &mut *up;
    let mut c = match u.cache.take() {
        Some(c) => c,
        None => return,
    };
    let mut add = Vec::new();
    for &id in ids {
        let f = &u.fns[id as usize];
        if f.compiled || f.spec.is_some() || f.inflight || f.building || f.build_err.is_some() || f.spec_depth <= depth + 1 {
            continue;
        }
        if matches!(f.key, FnKey::Inst(d, _) if u.prog.def(d).kind == DefKind::ForeignFn) || add.contains(&id) || !c.has(u, id) {
            continue;
        }
        u.fns[id as usize].spec_depth = depth + 1;
        u.spec_cached.insert(id, ());
        add.push(id);
    }
    u.cache = Some(c);
    if add.is_empty() {
        return;
    }
    SPEC_UNIT.store(up, Ordering::Release);
    {
        let mut q = SPEC_Q.lock().unwrap();
        for id in add.into_iter().rev() {
            q.push_front(id);
        }
    }
    SPEC_START.call_once(|| {
        for k in 0..spec_jobs(options) {
            let session = session.clone();
            { let _k = crate::zone_alloc::kept();
            let _ = std::thread::Builder::new().name(format!("run-spec{}", k)).spawn(move || { let _z = crate::zone_alloc::compiler_thread(); spec_worker(&session.runtime, &session.options) });
            }
        }
    });
    SPEC_WAKE.notify_all();
}

/// Zeroed read-write memory (`n` bytes, page granular).
pub fn alloc_rw(n: usize) -> u64 {
    os::mem::alloc_rw(n)
}

// ---------------------------------------------------------------- borrowck lane: lazy hooks

/// The background borrow checker's view of a unit: its program and type context under the
/// front end's read lock (as the type checks ahead read them), the compile-quiet gate of
/// O3, and the background workers' priority.
struct BorrowckFront(std::sync::Arc<FrontShared>);

/// MAKEPAD_RUN_GANDALF_TRACE=1: one line per body handed to the background borrow checker, with the
/// verdict a previous run stored for it ("check": none, the body is checked in this run).
fn trace_stored_verdict(prog: &Program, def: DefId, what: &str, stored: Option<&crate::borrowck::lazy::StoredVerdict>) {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if !*ON.get_or_init(|| std::env::var_os("MAKEPAD_RUN_GANDALF_TRACE").is_some_and(|v| v != "0")) { return; }
    let state = match stored {
        Some(crate::borrowck::lazy::StoredVerdict::Clean) => "clean",
        Some(crate::borrowck::lazy::StoredVerdict::Unchecked(_)) => "unchecked",
        None => "check",
    };
    eprintln!("gandalf-verdict: {} {} {}", what, state, prog.def_path(def));
}

impl crate::borrowck::lazy::FrontSource for BorrowckFront {
    fn owner_verdict(&self, prog: &Program, def: DefId, epoch: u64) -> (Option<u128>, Option<crate::borrowck::lazy::StoredVerdict>) {
        let found = self.0.owner_verdicts.get(&epoch).map_or((None, None), |v| v.lookup(prog, def, epoch));
        trace_stored_verdict(prog, def, "owner", found.1.as_ref());
        found
    }
    fn wait_start(&self) {
        while STARTUP_PHASE.load(Ordering::Acquire) != 0 && !SPEC_STOP.load(Ordering::SeqCst) {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
    fn identity(&self) -> usize {
        std::sync::Arc::as_ptr(&self.0) as usize
    }
    fn with_front(&self, f: &mut dyn FnMut(&Program, &Tcx, &crate::tcx::EvalCx<'_>)) -> bool {
        let _job = compile_job();
        let _r = front_read("gandalf");
        if !self.0.alive.load(Ordering::SeqCst) {
            return false;
        }
        let (prog, tcx) = unsafe { (&*self.0.prog.load(Ordering::Relaxed), &*self.0.tcx.load(Ordering::Relaxed)) };
        let cx_state = crate::tcx::EvalState::new();
        let cx = crate::tcx::EvalCx::new(prog, &tcx, crate::tcx::TypingEnv::fully_monomorphized(prog), &cx_state);
        f(prog, tcx, &cx);
        true
    }
    fn quiet_ns(&self) -> u64 {
        wall_ns().saturating_sub(COMPILE_LAST.load(Ordering::Relaxed))
    }
    fn alive(&self) -> bool {
        self.0.alive.load(Ordering::SeqCst)
    }
    fn lower_priority(&self) {
        lower_worker_priority(&self.0.session.runtime);
    }
}

/// Lead decision 53.2: a run's coherence check (orphan rules, overlapping impls: E0116/E0117/
/// E0210/E0390, E0119, E0592) is not on its startup path. It runs once, in the background, after
/// the first present (as Gandalf's checks: after the startup phase, at worker priority, under
/// the front end's read lock, preempted by a live patch's writer and then run again); its errors
/// go to the diagnostics context and are reported with the run's other compile errors.
/// Selection never reads coherence verdicts, so nothing waits for it (`makepad check` checks it
/// synchronously, Tcx::check_coherence).
pub fn start_background_coherence(u: &Unit) {
    if std::env::var("MAKEPAD_RUN_COHERENCE").is_ok_and(|v| v == "0") { return; }
    // A validated warm start whose cache holds this program's clean verdict (the same crate texts,
    // cfg, Relax build and impl files) does not check again: the check type-checks and collects
    // the signatures of the whole impl set, 300+ ms and tens of MB for nothing it can find.
    // Without errors and warnings this run, the verdict is stored for the next one.
    let key = u.cache.as_ref().and_then(|c| c.coherence_key(&u.prog));
    if let (Some(k), Some(c)) = (key, u.cache.as_ref()) {
        if u.front.warm_cache.load(Ordering::Relaxed) && matches!(c.borrowck_stored(k), Some(crate::borrowck::lazy::StoredVerdict::Clean)) {
            if u.prog.session.options.verbose { eprintln!("makepad-run coherence: clean verdict from the cache"); }
            return;
        }
    }
    let front = u.front.clone();
    { let _k = crate::zone_alloc::kept();
    let _ = std::thread::Builder::new().name("run-coherence".into()).spawn(move || { let _z = crate::zone_alloc::compiler_thread();
        let _mem = crate::memstat::tag(crate::memstat::Tag::Borrowck);
        lower_worker_priority(&front.session.runtime);
        while STARTUP_PHASE.load(Ordering::Acquire) != 0 && !SPEC_STOP.load(Ordering::SeqCst) {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let mut orphan_done = false;
        let emitted = crate::infer::diagnostics::DiagCtxt::thread_emitted();
        loop {
            if SPEC_STOP.load(Ordering::SeqCst) || crate::preempt::is_stopped() { return; }
            let r = {
                let _job = compile_job();
                let _r = front_read("coherence");
                if !front.alive.load(Ordering::SeqCst) { return; }
                let (prog, tcx) = unsafe { (&*front.prog.load(Ordering::Relaxed), &*front.tcx.load(Ordering::Relaxed)) };
                let t0 = std::time::Instant::now();
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let _p = crate::preempt::Preemptible::on();
                    crate::preempt::point();
                    tcx.report_coherence(prog, &mut orphan_done);
                }));
                if r.is_ok() && front.session.options.verbose { eprintln!("makepad-run coherence: checked in {:.1} ms (background)", t0.elapsed().as_secs_f64() * 1e3); }
                r
            };
            match r {
                Ok(()) => {
                    if let Some(k) = key {
                        // a live edit meanwhile: the key names the text this run started with
                        if crate::infer::diagnostics::DiagCtxt::thread_emitted() == emitted && crate::reload::edited_crates().is_empty() {
                            crate::borrowck::lazy::push_verdict_key(k, crate::borrowck::lazy::StoredVerdict::Clean);
                        }
                    }
                    return;
                }
                Err(p) if crate::preempt::is_preempted(&*p) => std::thread::sleep(std::time::Duration::from_millis(50)),
                Err(p) => {
                    let m = p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default();
                    eprintln!("makepad-run coherence: checker failure (makepad run bug, please report): {}", m);
                    return;
                }
            }
        }
    });
    }
}

/// MAKEPAD_RUN_MEM_REPORT's size line of a function: instructions, vregs, blocks, slots.
fn cg_shape(f: &crate::rir::Func) -> String {
    format!("{} insts, {} vregs, {} blocks, {} slots", crate::opt::inst_count(f), f.vregs.len(), f.blocks.len(), f.slots.len())
}

/// Notes which of `ids` (about to be queued for speculation; unit lock held) the disk cache
/// holds: their workers skip the type check ahead (the cache key is memoized for the lookup).
fn mark_spec_cached(u: &mut Unit, ids: &[u32]) {
    let mut c = match u.cache.take() {
        Some(c) => c,
        None => return,
    };
    for &id in ids {
        if !matches!(u.fns[id as usize].key, FnKey::Inst(d, _) if u.prog.def(d).kind == DefKind::ForeignFn) && c.has(u, id) {
            u.spec_cached.insert(id, ());
        }
    }
    u.cache = Some(c);
}

/// A function's pc -> source position map ((code offset, (file << 32) | position) entries,
/// ascending), kept compact: per entry a varint offset delta and a zigzag varint position delta
/// (2-4 bytes instead of 16; read only for reports, backtraces and the O3 sampler).
#[derive(Default, Clone)]
pub struct PcMap(PcBytes);

/// A pc map's bytes: its own, or a startup image's read in place (cache/image.rs), whose
/// positions name the image's file indices (`PcFiles` translates them).
#[derive(Clone)]
enum PcBytes {
    Own(Box<[u8]>),
    Mapped(&'static [u8], std::sync::Arc<PcFiles>),
}

impl Default for PcBytes {
    fn default() -> Self {
        PcBytes::Own(Box::default())
    }
}

/// A startup image's files in this run (u32::MAX: a file version this run has not loaded: its
/// positions are unknown), and what keeps the image mapped.
pub struct PcFiles {
    pub files: Vec<u32>,
    pub keep: Box<dyn std::any::Any + Send + Sync>,
}

impl PcMap {
    /// An installed code body's map: identity state, built in the kept heap.
    pub fn new(v: &[(u32, u64)]) -> PcMap {
        let _k = crate::zone_alloc::kept();
        PcMap::encode(v.len(), v.iter().copied())
    }

    /// The map of `n` entries (a capacity hint) from `v`, encoded as they come.
    pub fn encode(n: usize, v: impl Iterator<Item = (u32, u64)>) -> PcMap {
        let mut b = Vec::with_capacity(n * 3);
        PcMap::encode_to(&mut b, v);
        PcMap(PcBytes::Own(b.into_boxed_slice()))
    }

    /// Appends the bytes of the map of `v` to `b`.
    pub fn encode_to(b: &mut Vec<u8>, v: impl Iterator<Item = (u32, u64)>) {
        let (mut po, mut pp) = (0u32, 0u64);
        let put = |b: &mut Vec<u8>, mut x: u64| loop {
            if x < 0x80 {
                b.push(x as u8);
                break;
            }
            b.push((x as u8 & 0x7f) | 0x80);
            x >>= 7;
        };
        for (o, p) in v {
            put(b, o.wrapping_sub(po) as u64);
            let d = p.wrapping_sub(pp) as i64;
            put(b, ((d << 1) ^ (d >> 63)) as u64);
            po = o;
            pp = p;
        }
    }

    /// The entries of the map whose bytes (`encode_to`'s) are `b`.
    pub fn decode(b: &[u8]) -> Vec<(u32, u64)> {
        PcMap(PcBytes::Own(b.into())).entries()
    }

    /// What keeps the image a mapped map is read from mapped.
    pub fn keep(&self) -> Option<std::sync::Arc<PcFiles>> {
        match &self.0 {
            PcBytes::Mapped(_, f) => Some(f.clone()),
            PcBytes::Own(_) => None,
        }
    }

    /// The map whose bytes (`encode_to`'s) are `b`, read in place, its files indices into `files`.
    pub fn mapped(b: &'static [u8], files: std::sync::Arc<PcFiles>) -> PcMap {
        PcMap(PcBytes::Mapped(b, files))
    }

    fn for_each(&self, mut f: impl FnMut(u32, u64) -> bool) {
        let (b, files): (&[u8], Option<&PcFiles>) = match &self.0 {
            PcBytes::Own(b) => (b, None),
            PcBytes::Mapped(b, files) => (b, Some(files)),
        };
        let mut i = 0;
        let get = |i: &mut usize| -> u64 {
            let mut x = 0u64;
            let mut sh = 0;
            while *i < b.len() {
                let c = b[*i];
                *i += 1;
                x |= ((c & 0x7f) as u64) << sh;
                if c < 0x80 {
                    break;
                }
                sh += 7;
            }
            x
        };
        let (mut o, mut p) = (0u32, 0u64);
        while i < b.len() {
            o = o.wrapping_add(get(&mut i) as u32);
            let z = get(&mut i);
            p = p.wrapping_add((((z >> 1) as i64) ^ -((z & 1) as i64)) as u64);
            let q = match files {
                Some(t) => (t.files.get((p >> 32) as usize).copied().unwrap_or(u32::MAX) as u64) << 32 | (p & 0xffff_ffff),
                None => p,
            };
            if !f(o, q) {
                break;
            }
        }
    }

    /// The position of the last entry at or before code offset `off`.
    pub fn lookup(&self, off: u32) -> Option<u64> {
        let mut r = None;
        self.for_each(|o, p| {
            if o <= off {
                r = Some(p);
                true
            } else {
                false
            }
        });
        r
    }

    pub fn entries(&self) -> Vec<(u32, u64)> {
        let mut v = Vec::new();
        self.for_each(|o, p| {
            v.push((o, p));
            true
        });
        v
    }

    pub fn bytes(&self) -> usize {
        match &self.0 {
            PcBytes::Own(b) => b.len(),
            PcBytes::Mapped(..) => 0,
        }
    }
}

#[cfg(test)]
#[path = "settle_tests.rs"]
mod settle_tests;

#[cfg(test)]
mod allocator_swap_tests {
    use super::*;

    /// the tests below pause the process's compile jobs: one at a time
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// The swap waits for a running compile job however long it runs. A job that outlives the
    /// old 5 s cap (a slow machine, a big O3 build) must still be finished when the swap goes on;
    /// a job that starts during the swap waits for its end, however long that takes.
    #[test]
    fn the_swap_waits_for_a_job_longer_than_five_seconds() {
        isolated_test!(the_swap_waits_for_a_job_longer_than_five_seconds);
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let job_over = std::sync::Arc::new(AtomicBool::new(false));
        let (running_tx, running_rx) = std::sync::mpsc::channel();
        let flag = job_over.clone();
        let job = std::thread::spawn(move || {
            let guard = compile_job();
            running_tx.send(()).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(5600));
            flag.store(true, Ordering::SeqCst);
            drop(guard);
        });
        running_rx.recv().unwrap();
        let begun = std::time::Instant::now();
        allocator_swap(0);
        let waited = begun.elapsed();
        assert!(job_over.load(Ordering::SeqCst), "the swap went ahead with a compile job still running ({waited:?})");
        assert!(waited.as_millis() >= 5000, "it did not wait the job out ({waited:?})");
        // a job that starts during the swap is held until the swap ends, however long
        let started = std::sync::Arc::new(AtomicBool::new(false));
        let s2 = started.clone();
        let late = std::thread::spawn(move || {
            let _guard = compile_job();
            s2.store(true, Ordering::SeqCst);
        });
        std::thread::sleep(std::time::Duration::from_millis(2600));
        assert!(!started.load(Ordering::SeqCst), "a job started inside the swap after 2 s");
        allocator_swap(1);
        late.join().unwrap();
        assert!(started.load(Ordering::SeqCst));
        job.join().unwrap();
    }

    /// Two pauses take turns: a unit retired (or a program's fork) while an allocator swap is in
    /// progress waits for that swap's end, and the swap's resume does not open the gate under it.
    #[test]
    fn pauses_take_turns() {
        isolated_test!(pauses_take_turns);
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let wait_for = |f: &AtomicBool| {
            let t0 = std::time::Instant::now();
            while !f.load(Ordering::SeqCst) {
                assert!(t0.elapsed().as_secs() < 30, "never happened");
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        };
        allocator_swap(0);
        let b_paused = std::sync::Arc::new(AtomicBool::new(false));
        let b_go = std::sync::Arc::new(AtomicBool::new(false));
        let (bp, bg) = (b_paused.clone(), b_go.clone());
        let b = std::thread::spawn(move || {
            pause_compile_jobs(false);
            bp.store(true, Ordering::SeqCst);
            while !bg.load(Ordering::SeqCst) {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            resume_compile_jobs(false);
        });
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(!b_paused.load(Ordering::SeqCst), "a second pause began inside the first");
        allocator_swap(1);
        wait_for(&b_paused);
        let c_started = std::sync::Arc::new(AtomicBool::new(false));
        let cs = c_started.clone();
        let c = std::thread::spawn(move || {
            let _job = compile_job();
            cs.store(true, Ordering::SeqCst);
        });
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(!c_started.load(Ordering::SeqCst), "a job started inside the second pause");
        b_go.store(true, Ordering::SeqCst);
        b.join().unwrap();
        c.join().unwrap();
        assert!(c_started.load(Ordering::SeqCst));
    }

    /// A request another thread queued with the owner during a pause is served when the pause
    /// ends: the end wakes the owner (it slept with the request queued, and nothing else came).
    #[test]
    fn a_request_queued_during_a_pause_is_served_after_it() {
        isolated_test!(a_request_queued_during_a_pause_is_served_after_it);
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        owner::start();
        allocator_swap(0);
        let served = std::sync::Arc::new(AtomicBool::new(false));
        let s2 = served.clone();
        let other = std::thread::spawn(move || {
            owner::call(owner::Class::App, || ());
            s2.store(true, Ordering::SeqCst);
        });
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(!served.load(Ordering::SeqCst), "the owner served another thread inside the pause");
        allocator_swap(1);
        let t0 = std::time::Instant::now();
        while !served.load(Ordering::SeqCst) {
            assert!(t0.elapsed().as_secs() < 30, "the request queued during the pause was never served");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        other.join().unwrap();
    }

    /// The fork the loader hands programs (`fork` through loader_symbol / dlsym) still parks the
    /// compile jobs: it returns only after a running job ended, and the child (which may compile
    /// lazily) starts with no job counted elsewhere.
    #[cfg(unix)]
    #[test]
    fn the_programs_fork_waits_for_running_jobs() {
        isolated_test!(the_programs_fork_waits_for_running_jobs);
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let addr = loader_symbol(b"fork").expect("the loader provides fork");
        assert_eq!(addr, relax_fork as *const () as usize);
        let fork: extern "C" fn() -> i32 = unsafe { std::mem::transmute(addr) };
        let job_over = std::sync::Arc::new(AtomicBool::new(false));
        let (running_tx, running_rx) = std::sync::mpsc::channel();
        let flag = job_over.clone();
        let job = std::thread::spawn(move || {
            let guard = compile_job();
            running_tx.send(()).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(800));
            flag.store(true, Ordering::SeqCst);
            drop(guard);
        });
        running_rx.recv().unwrap();
        let pid = fork();
        if pid == 0 {
            // the child: nothing counted, nothing paused
            let ok = BG_ACTIVE.load(Ordering::SeqCst) == 0 && !BG_PAUSE.load(Ordering::SeqCst);
            os::thread::exit_now(if ok { 0 } else { 1 });
        }
        assert!(pid > 0, "fork failed");
        assert!(job_over.load(Ordering::SeqCst), "the program's fork went ahead with a compile job running");
        extern "C" {
            fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
        }
        let mut status = 0i32;
        assert_eq!(unsafe { waitpid(pid, &mut status, 0) }, pid);
        assert_eq!(status, 0, "the child saw a job counted or the jobs paused");
        job.join().unwrap();
    }
}

static EXEC_CHECK: AtomicBool = AtomicBool::new(false);

/// No-codegen mode (or MAKEPAD_RUN_EXEC_CHECK=1): reports, once, whether the process has
/// executable memory that no loaded image backs -- code anything generated. Called when the run
/// ends (run.rs finish_run) and at exit.
pub fn exec_check() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if EXEC_CHECK.load(Ordering::Relaxed) && !DONE.swap(true, Ordering::AcqRel) {
        exec_check_now("at exit");
    }
}

fn exec_check_now(when: &str) {
    #[cfg(target_os = "macos")]
    {
        let r = os::mem::anonymous_exec_regions();
        if r.is_empty() {
            eprintln!("makepad-run exec check ({}): no generated executable memory in the process", when);
        } else {
            eprintln!("makepad-run exec check ({}): {} executable regions no image backs: {}", when, r.len(), r.iter().map(|(a, n, p)| format!("{:#x}+{:#x} prot {}", a, n, p)).collect::<Vec<_>>().join(", "));
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = when;
}

/// no-codegen mode's precompiled runtime glue (MAKEPAD_RUN_NO_CODEGEN)
#[cfg(all(target_arch = "aarch64", target_vendor = "apple"))]
#[path = "static_glue.rs"]
mod static_glue;
/// bytes per precompiled slot thunk / stub (no-codegen mode)
const NC_STRIDE: usize = 16;

fn present_files() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("MAKEPAD_RUN_PRESENT_FILES").is_some())
}


// ------------------------------------------------------------ real-time code
//
// A real-time thread (an audio device's callback) must never wait for a compile: a lazy stub there
// stalls the thread for the compile's length and the device plays garbage. The program marks the
// code such a thread runs (std::relax::realtime: the platform's audio seams mark the callbacks they
// install); everything it can reach is then compiled on the owner thread before the first call:
// direct calls, fn addresses taken, the functions constant data points at (vtables, fn tables),
// the bodies inlined into it, and the methods of every trait object it calls through -- the
// vtables that exist now and, as the program makes new ones of those types, those too (after the
// request that made them). The slots this reaches are recorded in the disk cache (rt-<crate>.keys)
// and compiled first at the next run's mark. A lazy compile a real-time thread still meets is
// reported, compiles everything its slot reaches at once and is recorded too.

/// MAKEPAD_RUN_RT_AHEAD=0: no real-time code compiled ahead (A/B: lazy compiles on real-time threads)
fn realtime_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("MAKEPAD_RUN_RT_AHEAD").map_or(true, |v| v != "0"))
}

/// The real-time reach of the program's unit (owner only; a mutex for the static).
#[derive(Default)]
struct RealTime {
    /// slots reached, in order (what the disk cache records)
    set: Vec<u32>,
    seen: crate::fxhash::HashSet<u32>,
    /// trait-object types real-time code calls through (TyId), and the vtables already followed
    dyns: crate::fxhash::HashSet<u32>,
    vtables: crate::fxhash::HashSet<u64>,
    /// the earlier runs' set was compiled
    loaded: bool,
}

static REALTIME: relax_session::sync::Mutex<Option<RealTime>> = relax_session::sync::Mutex::new(None);
/// any real-time trait-object type known (vtable() checks this before the mutex)
static RT_DYNS: AtomicBool = AtomicBool::new(false);
/// vtables made for a real-time trait-object type, waiting for realtime_drain
static RT_PENDING: relax_session::sync::Mutex<Vec<u64>> = relax_session::sync::Mutex::new(Vec::new());
/// slots compiled ahead for real-time code, and the owner's wall ns spent on it
pub static RT_AHEAD: [AtomicU64; 2] = [AtomicU64::new(0), AtomicU64::new(0)];

thread_local! {
    /// the RIR of what real-time reach compiles stays (its references are read from it)
    static RT_KEEP: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// std::relax::realtime: `root` is the address of `__realtime_root::<T>` (its slot names T), `data`
/// the value's address, `meta` a trait object's vtable (0: sized).
extern "C" fn __relax_realtime(root: usize, data: usize, meta: usize) {
    let up = unsafe { UNIT };
    if up.is_null() || !realtime_on() {
        return;
    }
    let upn = up as usize;
    owner::call(owner::program_class(), move || unsafe { realtime_mark(upn as *mut Unit, root as u64, data as u64, meta as u64) });
}

unsafe fn realtime_mark(up: *mut Unit, root: u64, data: u64, meta: u64) {
    let u = &mut *up;
    let mut roots = Vec::new();
    let mut dyns = Vec::new();
    if let Some(FnKey::Inst(_, args)) = u.thunk_slot(root).map(|r| u.fns[r as usize].key.clone()) {
        if let Some(&t) = args.first() {
            let t = u.tcx.tys.erase_regions(t);
            match u.tcx.tys.kind(t).clone() {
                TyKind::Dyn(..) => dyns.push(t.0),
                TyKind::Closure(..) => roots.extend(u.closure_fn_id(t)),
                TyKind::FnPtr(..) if data != 0 => roots.extend(u.thunk_slot(*(data as *const u64))),
                TyKind::FnDef(d, a, _) => roots.push(u.fn_id(FnKey::Inst(d, u.tcx.tys.list(a).to_vec()))),
                _ => {}
            }
            if meta == 0 && u.needs_drop_glue(t) {
                roots.push(u.fn_id(FnKey::Glue(GLUE_DROP, t)));
            }
        }
    }
    // a trait object (or a struct ending in one): the methods of its vtable
    if meta != 0 {
        crate::syms::data_thunks(u, meta, &mut roots);
    }
    {
        let mut g = REALTIME.lock().unwrap_or_else(|e| e.into_inner());
        let rt = g.get_or_insert_with(Default::default);
        for d in dyns {
            crate::zone_alloc::kept_with(|| rt.dyns.insert(d));
            RT_DYNS.store(true, Ordering::Release);
        }
    }
    realtime_reach(up, roots, "mark");
}

/// Compiles everything `roots` reach (owner): see the section comment. `why`: the verbose line.
pub(crate) unsafe fn realtime_reach(up: *mut Unit, roots: Vec<u32>, why: &str) {
    let t0 = wall_ns();
    let mut todo = roots;
    // the earlier runs' set, once, first: on a warm run most of it links from the disk cache
    let first = {
        let mut g = REALTIME.lock().unwrap_or_else(|e| e.into_inner());
        let rt = g.get_or_insert_with(Default::default);
        !std::mem::replace(&mut rt.loaded, true)
    };
    if first {
        // (their code is compiled, their references are not walked again: what they reached
        // then is in the set; an edited slot's key no longer resolves and is walked from what
        // calls it)
        let root = crate::layout::root_crate();
        let keys = (&*up).cache.as_ref().map(|c| c.rt_set_keys(&*up, root)).unwrap_or_default();
        let n = keys.len();
        let earlier: Vec<u32> = keys.iter().filter_map(|k| crate::cache::slot_for_bytes_unit(up, k)).collect();
        if std::env::var_os("MAKEPAD_RUN_RT_TRACE").is_some() {
            eprintln!("makepad-run rt: the earlier runs' set: {} keys, {} resolved", n, earlier.len());
        }
        let mut compiled = 0u64;
        for id in earlier {
            {
                let mut g = REALTIME.lock().unwrap_or_else(|e| e.into_inner());
                let rt = g.get_or_insert_with(Default::default);
                if !crate::zone_alloc::kept_with(|| rt.seen.insert(id)) {
                    continue;
                }
                crate::zone_alloc::kept_with(|| rt.set.push(id));
            }
            if !(&*up).fns[id as usize].compiled {
                let _ = compile_slot_locked(up, id, false);
                compiled += 1;
            }
        }
        RT_AHEAD[0].fetch_add(compiled, Ordering::Relaxed);
    }
    let n0 = REALTIME.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map_or(0, |r| r.set.len());
    let mut compiled = 0u64;
    let mut new_dyns = Vec::new();
    while let Some(id) = todo.pop() {
        {
            let mut g = REALTIME.lock().unwrap_or_else(|e| e.into_inner());
            let rt = g.get_or_insert_with(Default::default);
            if !crate::zone_alloc::kept_with(|| rt.seen.insert(id)) {
                continue;
            }
            crate::zone_alloc::kept_with(|| rt.set.push(id));
        }
        if (id as usize) >= (&*up).fns.len() {
            continue;
        }
        let was = (&*up).fns[id as usize].compiled;
        let (slots, dyns) = realtime_refs(up, id);
        if !was && (&*up).fns[id as usize].compiled {
            compiled += 1;
        }
        todo.extend(slots);
        new_dyns.extend(dyns);
        // trait-object types newly called through: every vtable of each, now and later
        if todo.is_empty() && !new_dyns.is_empty() {
            let u = &*up;
            let mut g = REALTIME.lock().unwrap_or_else(|e| e.into_inner());
            let rt = g.get_or_insert_with(Default::default);
            let fresh: Vec<u32> = new_dyns.drain(..).filter(|d| crate::zone_alloc::kept_with(|| rt.dyns.insert(*d))).collect();
            if !fresh.is_empty() {
                RT_DYNS.store(true, Ordering::Release);
            }
            let dyns = rt.dyns.clone();
            let addrs: Vec<u64> = u.vtables.snapshot().into_iter().filter(|((_, dy), a)| dyns.contains(&dy.0) && !rt.vtables.contains(a)).map(|(_, a)| a).collect();
            for a in addrs {
                crate::zone_alloc::kept_with(|| rt.vtables.insert(a));
                crate::syms::data_thunks(u, a, &mut todo);
            }
        }
    }
    let set = REALTIME.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|r| r.set.clone()).unwrap_or_default();
    // the top tier for all of it: an O2 build replaces whatever code it has (baseline or O1 code
    // made before it was known to be real-time; the sampler never sees a device's thread, so
    // nothing else would) and is the slot's disk cache entry, so later runs link O2 code at the
    // mark; the O3 tier follows its call counts as anywhere
    realtime_top_tier(up, &set);
    let u = &mut *up;
    if set.len() > n0 {
        if let Some(mut c) = u.cache.take() {
            c.rt_set_store(u, crate::layout::root_crate(), &set);
            u.cache = Some(c);
        }
    }
    let dt = wall_ns() - t0;
    RT_AHEAD[0].fetch_add(compiled, Ordering::Relaxed);
    RT_AHEAD[1].fetch_add(dt, Ordering::Relaxed);
    if crate::run::VERBOSE.load(Ordering::Relaxed) || std::env::var_os("MAKEPAD_RUN_RT_TRACE").is_some() {
        eprintln!("makepad-run rt: {}: real-time code reaches {} slots ({} new), {} compiled now, {:.1} ms", why, set.len(), set.len() - n0, compiled, dt as f64 / 1e6);
    }
}

/// Real-time slots `ids` get their O2 build (once; o2_hot / o2_base) unless their code is O2 or
/// O3 already. MAKEPAD_RUN_RT_TRACE: the tiers they run at now.
unsafe fn realtime_top_tier(up: *mut Unit, ids: &[u32]) {
    let (mut base, mut o3, mut o2, mut o1) = (0, 0, 0, 0);
    for &id in ids {
        let u = &*up;
        if (id as usize) >= u.fns.len() || !u.fns[id as usize].compiled || matches!(u.fns[id as usize].key, FnKey::Glue(..) | FnKey::Sandbox(_)) {
            continue;
        }
        if u.fns[id as usize].o3.load(Ordering::Relaxed) {
            o3 += 1;
            continue;
        }
        let seen = O2_SEEN.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|m| m.contains(&(up as usize, id)));
        if u.fns[id as usize].base_code.load(Ordering::Relaxed) {
            base += 1;
            o2_base(up, id);
        } else if seen {
            o2 += 1;
        } else {
            o1 += 1;
            o2_hot(up, id);
        }
    }
    if std::env::var_os("MAKEPAD_RUN_RT_TRACE").is_some() {
        eprintln!("makepad-run rt: tiers of {} real-time slots: {} baseline, {} O1 (both queued for O2), {} O2, {} O3", ids.len(), base, o1, o2, o3);
    }
}

/// Compiles slot `id` (if it has no code) and returns what its code can reach: slots, and the
/// trait-object types it calls through. From its RIR when it has one (a build here keeps it),
/// else from its disk cache entry, else from a fresh build of its RIR.
unsafe fn realtime_refs(up: *mut Unit, id: u32) -> (Vec<u32>, Vec<u32>) {
    let was = RT_KEEP.with(|k| k.replace(true));
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if !(&*up).fns[id as usize].compiled {
            if let Some(c) = (&mut *up).cache.as_mut() {
                c.last_callees.clear();
                c.last_thunks.clear();
                c.last_data.clear();
            }
            let _ = compile_slot_locked(up, id, false);
        } else if (&*up).fns[id as usize].rir.is_none() && (&*up).cache.is_some() && !matches!((&*up).fns[id as usize].key, FnKey::Sandbox(_)) {
            // installed from the disk cache (or the startup image): its entry names its references
            let _ = cache_lookup(up, id);
        }
        let u = &*up;
        let mut slots = Vec::new();
        let mut dyns = Vec::new();
        if u.fns[id as usize].rir.is_none() && !u.fns[id as usize].compiled {
            return (slots, dyns);
        }
        if let Some(r) = u.fns[id as usize].rir.clone() {
            realtime_rir_refs(u, &r, &mut slots, &mut dyns);
        } else if let Some(c) = u.cache.as_ref().filter(|c| !c.last_callees.is_empty() || !c.last_thunks.is_empty() || !c.last_data.is_empty()) {
            slots.extend(c.last_callees.iter().copied());
            slots.extend(c.last_thunks.iter().copied());
            for &a in &c.last_data {
                crate::syms::data_thunks(u, a, &mut slots);
            }
        } else if matches!(u.fns[id as usize].key, FnKey::Inst(..) | FnKey::Closure(..)) && build_rir(up, id).is_ok() {
            let u = &*up;
            if let Some(r) = u.fns[id as usize].rir.clone() {
                realtime_rir_refs(u, &r, &mut slots, &mut dyns);
            }
        }
        // bodies inlined into it: their trait-object calls are its calls now
        if let Some(deps) = (&*up).body_deps.get(&id) {
            slots.extend(deps.iter().copied());
        }
        (slots, dyns)
    }));
    RT_KEEP.with(|k| k.set(was));
    r.unwrap_or_default()
}

fn realtime_rir_refs(u: &Unit, f: &crate::rir::Func, slots: &mut Vec<u32>, dyns: &mut Vec<u32>) {
    use crate::rir::{Callee, Inst};
    for b in &f.blocks {
        for i in &b.insts {
            match i {
                Inst::Call(Callee::Fn(c) | Callee::CFn(c, _), _, _) | Inst::FnAddr(_, c) => slots.push(*c),
                Inst::Addr(_, sym) => match u.syms.kind(sym.id) {
                    Some(crate::syms::SymKind::Thunk(c)) => slots.push(c),
                    Some(crate::syms::SymKind::Data(_)) => crate::syms::data_thunks(u, u.syms.value(crate::rir::Sym { id: sym.id, off: 0 }), slots),
                    _ => {}
                },
                _ => {}
            }
        }
    }
    dyns.extend(f.dyn_calls.iter().copied());
}

/// A vtable was made (Unit::vtable): one of a trait-object type real-time code calls through is
/// compiled after the current request.
fn realtime_vtable(dy: TyId, addr: u64) {
    if !RT_DYNS.load(Ordering::Acquire) {
        return;
    }
    let known = REALTIME.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|r| r.dyns.contains(&dy.0) && !r.vtables.contains(&addr));
    if known {
        crate::zone_alloc::kept_with(|| RT_PENDING.lock().unwrap_or_else(|e| e.into_inner()).push(addr));
    }
}

/// The startup manifest's order (cache.rs record_manifest) and its before-present length: the
/// real-time set goes before the first present, so the next run's startup image links it
/// before main (a device opened right after the first frame finds its code installed).
pub fn manifest_order(order: Vec<u32>, present: usize) -> (Vec<u32>, usize) {
    let set = REALTIME.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|r| r.set.clone()).unwrap_or_default();
    if set.is_empty() {
        return (order, present);
    }
    let present = present.min(order.len());
    let head: crate::fxhash::HashSet<u32> = order[..present].iter().copied().collect();
    let add: Vec<u32> = set.into_iter().filter(|id| !head.contains(id)).collect();
    let addset: crate::fxhash::HashSet<u32> = add.iter().copied().collect();
    let mut out = Vec::with_capacity(order.len() + add.len());
    out.extend_from_slice(&order[..present]);
    out.extend(add.iter().copied());
    let n = out.len();
    out.extend(order[present..].iter().copied().filter(|id| !addset.contains(id)));
    (out, n)
}

/// Owner, between requests: the methods of the vtables realtime_vtable noted.
pub(crate) fn realtime_drain() {
    if !RT_DYNS.load(Ordering::Acquire) {
        return;
    }
    let addrs = std::mem::take(&mut *RT_PENDING.lock().unwrap_or_else(|e| e.into_inner()));
    if addrs.is_empty() {
        return;
    }
    let up = unsafe { UNIT };
    if up.is_null() {
        return;
    }
    let mut roots = Vec::new();
    {
        let u = unsafe { &*up };
        let mut g = REALTIME.lock().unwrap_or_else(|e| e.into_inner());
        let rt = g.get_or_insert_with(Default::default);
        for a in addrs {
            if crate::zone_alloc::kept_with(|| rt.vtables.insert(a)) {
                crate::syms::data_thunks(u, a, &mut roots);
            }
        }
    }
    if !roots.is_empty() {
        unsafe { realtime_reach(up, roots, "new vtable") };
    }
}

/// rustc's E0080 message for a failed evaluation (rustc_const_eval errors.rs): the checks the
/// compiler inserts keep their text (overflow, bounds, division); a panic of the program is
/// "evaluation panicked: <message>".
fn ctfe_error_message(m: &str) -> String {
    let builtin = ["attempt to ", "index out of bounds", "divide by zero", "remainder by zero", "misaligned pointer", "null pointer"];
    // (core's runtime text says `len`; the evaluator's says `length`)
    if builtin.iter().any(|b| m.starts_with(b)) { m.replacen("the len is", "the length is", 1) } else { format!("evaluation panicked: {}", m) }
}

/// Per provided crate present in `provided`: its opaque (handle) types with rustc's drop, their
/// layouts, and the plain types whose layout must be checked against rustc's.
fn provided_type_tables(prog: &Program, provided: &[(u32, crate::provided::ProvidedCrate)])
    -> (HashMap<DefId, u64>, HashMap<DefId, (u32, u32)>, Vec<(DefId, u32, u32, String, &'static [u64])>)
{
    let mut provided_types = HashMap::new();
    let mut opaque_layouts = HashMap::new();
    let mut check_layouts = Vec::new();
    for (k, pc) in provided {
        for (i, def) in prog.defs.iter().enumerate() {
            if def.krate == *k && matches!(def.kind, DefKind::Struct | DefKind::Enum) {
                let path = crate::provided::norm(&prog.def_path(DefId(i as u32)));
                if let Some((size, align, drop)) = pc.types.get(path.as_str()) {
                    provided_types.insert(DefId(i as u32), drop.map_or(0, |f| f as usize as u64));
                    opaque_layouts.insert(DefId(i as u32), (*size as u32, *align as u32));
                } else if let Some((size, align, offs)) = pc.layouts.get(path.as_str()) {
                    check_layouts.push((DefId(i as u32), *size as u32, *align as u32, path, *offs));
                }
            }
        }
    }
    (provided_types, opaque_layouts, check_layouts)
}

impl Unit {
    /// Binds the provided crates among `new` (crate indices a run-time load just added, e.g. the
    /// crates an app domain brings that the booted program did not have): the same tables
    /// `Unit::new` builds for the boot program's provided crates, so their `#[relax_api]` ops are
    /// ops here too, not compiled from source. Only crates whose source matched
    /// (`provided::check_sources`) bind. On the owner thread, with the front-end write lock.
    pub(crate) unsafe fn bind_added_provided(&mut self, new: &[u32]) {
        let added = crate::provided::take_for_crates(&self.prog, new);
        if added.is_empty() { return; }
        let (types, opaque, check_layouts) = provided_type_tables(&self.prog, &added);
        crate::zone_alloc::kept_with(|| {
            self.provided.extend(added.iter().cloned());
            self.provided_types.extend(types);
        });
        self.lay.opaque.extend(opaque);
        // rustc's layout must be Relax's for a plain type, else it is used through the crate's fns only
        let probe = crate::layout::Layouts::new();
        for (d, size, align, path, offs) in check_layouts {
            let t = self.tcx.tys.intern(TyKind::Adt(d, crate::types::List::EMPTY, crate::types::RegionArgsId::EMPTY));
            let l = probe.of(&crate::layout::LayoutCx::new(&self.prog), &self.tcx, t);
            let offs_ok = offs.is_empty() || (offs.len() == l.fields.len() && offs.iter().zip(&l.fields).all(|(a, b)| *a == *b as u64));
            if l.size != size || l.align != align || !offs_ok {
                eprintln!("makepad: provided type {}: rustc lays it out in {} bytes (align {}, field offsets {:?}), makepad run in {} (align {}, {:?}): opaque, used through the crate's fns only", path, size, align, offs, l.size, l.align, l.fields);
                self.lay.opaque.insert(d, (size, align));
                crate::zone_alloc::kept_with(|| self.provided_types.insert(d, 0));
            }
        }
        let ks: Vec<u32> = added.iter().map(|(k, _)| *k).collect();
        let shared = {
            let pt = &self.provided_types;
            let provided = &self.provided;
            crate::provided::crossing_types(&self.prog, &self.tcx, &ks, &|d| {
                let k = self.prog.def(d).krate;
                crate::provided::unbound_reason(&self.prog, &self.tcx, k, d, &|a| pt.contains_key(&a)).is_none() && {
                    let key = crate::provided::key(&self.prog, &self.tcx, d);
                    provided.iter().any(|(pk, pc)| *pk == k && pc.shims.contains_key(key.as_str()))
                }
            })
        };
        crate::zone_alloc::kept_with(|| self.provided_shared.extend(shared));
    }
}
