//! Sandboxed run-time modules: memory confinement (local/plans/hotreload.md §12.3, option B
//! "one shared sandbox": MAIN 2026-10-05).
//!
//! Every function a sandboxed module runs is a *twin* (`FnKey::Sandbox(base)`): the base slot's
//! RIR (built as usual, shared by all modules) rewritten by `transform` before code generation:
//! - every load, store, copy and atomic goes through `A + zext32(p)`, where A is the calling
//!   module's 4 GB-aligned arena window (read at entry from this thread's sandbox state), so no
//!   address the module computes can leave its window; guard pages around it turn the rest into
//!   faults, which a guarded call contains;
//! - stack slots move to a shadow stack inside the window (return addresses and spills stay on
//!   the native stack, out of the module's reach); its overflow is a contained panic;
//! - data the code names (literals, consts, vtables, statics, promoted temporaries) is placed in
//!   one sandbox data image every arena holds a copy of, pointers inside it relocated per arena;
//!   mutable data starts from its declared initializer, never the host's running state. Thus
//!   core's panic hook/count remain module-local and unwind through the audited panic runtime,
//!   without importing std's installed host TLS callbacks;
//! - calls go to twins (std and module code) or, for the host's API crates, to the host's
//!   non-generic functions; fn pointers are sandbox thunks, and an indirect call masks its
//!   target into the sandbox thunk table; host runtime and C calls are allow-listed and the
//!   memory ones replaced by arena-checked versions (malloc & co. allocate in the arena);
//! - pre-optimization RIR blocks spend deterministic instruction fuel at entry.
//!
//! Host code reaches a module only through generated wrapper functions (modules.rs) that enter
//! the module's arena, copy reference arguments into it, call the twin and leave. Plain-data
//! aggregate results also use an arena slot (lower.rs), copied out before leaving: a twin's
//! sret stores must never receive a host destination.

use crate::jit::{self, FnKey, Unit};
use crate::program::DefKind;
use relax_backend::rir::{Callee, Cls, Cond, Conv, Func, IOp, Inst, Sym, Term, VReg, U32, U64};
use crate::syms::SymKind;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use relax_session::sync::Mutex;

#[cfg(target_arch = "aarch64")]
use relax_backend::arm64 as tgt;
#[cfg(not(target_arch = "aarch64"))]
use relax_backend::x64 as tgt;

pub const WINDOW: u64 = 1 << 32;
/// unmapped bytes on both sides of a window, and the largest offset a masked access may add
const GUARD: u64 = 1 << 16;
/// window layout (offsets): [0, 64K) never mapped (null and small dangling pointers fault)
const RO_BASE: u32 = 64 << 10;
const RO_END: u32 = 96 << 20;
const RW_BASE: u32 = 96 << 20;
const RW_END: u32 = 128 << 20;
/// shadow stack [SS_LO, SS_HI), growing down
const SS_LO: u32 = 128 << 20;
const SS_HI: u32 = 136 << 20;
/// scratch for runtime results the module reads (formatted text)
const SCRATCH: u32 = 136 << 20;
const SCRATCH_LEN: u32 = 1 << 20;
const HEAP_BASE: u32 = 256 << 20;
/// The module's instruction fuel word inside the window's image region.
const FUEL_OFF: u32 = RW_END - 64;
const HEAP_END: u32 = 0xffff_0000;
/// sandbox thunks (a power of two of them)
const SBT_N: u32 = 1 << 16;

/// Offsets of the sandbox words in each thread's Relax TLS block (relative to `Sb::tls`):
/// the current arena base, the shadow stack pointer, fuel left, the host arena state.
const W_ARENA: i64 = 0;
const W_SSP: i64 = 8;

/// One module's arena, as the host keeps it (the module never sees this).
pub struct Arena {
    pub base: u64,
    pub module: u32,
    heap: Mutex<Heap>,
    /// image bytes / relocations already copied into this window
    synced: Mutex<(u32, u32, usize, usize)>,
    /// fuel per call (0: none)
    pub fuel: i64,
    pub alive: std::sync::atomic::AtomicBool,
}

struct Heap {
    top: u32,
    mapped: u32,
    budget: u64,
    used: u64,
    free: Vec<Vec<u32>>,
    large_free: Vec<(u32, u32)>,
    sizes: HashMap<u32, u32>,
}

/// Process-wide sandbox state (one unit hosts modules).
struct Sb {
    /// TLS block offset of the sandbox words, and the symbol TlsAddr reads it from
    tls_sym: Sym,
    sbt_base: u64,
    sbt_n: u32,
    sbt_of: HashMap<u32, u32>,
    /// base slot -> twin slot
    twins: HashMap<u32, u32>,
    /// crates whose code always runs as twins (std's and the modules')
    sandboxed: HashSet<u32>,
    /// crates of generated host wrappers (their calls into modules go to twins)
    wrappers: HashSet<u32>,
    /// the data image
    placed: HashMap<u32, u32>,
    ro: Vec<u8>,
    rw: Vec<u8>,
    /// (window offset, target window offset): words holding an address inside the window
    rel: Vec<(u32, u32)>,
    /// (window offset, absolute value): words holding a code address (sandbox thunks)
    abs: Vec<(u32, u64)>,
    arenas: Vec<Arc<Arena>>,
    /// twin -> slots its code refers to without a relocation (fn pointer constants)
    refs: HashMap<u32, Vec<u32>>,
    /// the two code spaces twins are installed in: (base, size, next); `active` gets new code,
    /// a recycle rebuilds every live twin into the other one and swaps
    spaces: [(u64, u64, u64); 2],
    active: usize,
    /// crates of unloaded modules (their twins are dead)
    dead_crates: HashSet<u32>,
    recycles: u32,
}

/// bytes of each sandbox code space
const SPACE: u64 = 64 << 20;
/// dead twin code (replaced or of unloaded modules) that starts a recycle
static DEAD_BYTES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
const RECYCLE_AT: u64 = 16 << 20;
/// wrapper calls in flight (a space retired by a recycle is reused only when none were)
static INFLIGHT: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);
static QUIESCED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
static RECYCLING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

static SB: Mutex<Option<Sb>> = Mutex::new(None);
/// The TLS block offset of the sandbox words + 1 (0: no sandbox yet), read off the unit.
static TLS_OFF: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
/// Module index -> its arena (a lock-free lookup for every wrapper call).
const MAX_MODULES: usize = 1 << 16;
static ARENAS: [std::sync::atomic::AtomicPtr<Arena>; MAX_MODULES] = [const { std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()) }; MAX_MODULES];

#[cfg(unix)]
extern "C" {
    fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
    fn munmap(addr: *mut u8, len: usize) -> i32;
    fn mprotect(addr: *mut u8, len: usize, prot: i32) -> i32;
}
// Windows: no arena windows yet (the reservation is carved with partial munmaps, which
// VirtualFree cannot do); new_arena reports that instead of mapping
#[cfg(windows)]
unsafe fn mmap(_addr: *mut u8, _len: usize, _prot: i32, _flags: i32, _fd: i32, _off: i64) -> *mut u8 {
    usize::MAX as *mut u8
}
#[cfg(windows)]
unsafe fn munmap(_addr: *mut u8, _len: usize) -> i32 {
    -1
}
#[cfg(windows)]
unsafe fn mprotect(_addr: *mut u8, _len: usize, _prot: i32) -> i32 {
    -1
}
#[cfg(windows)]
const MAP_ANON_PRIV: i32 = 0;
#[cfg(any(target_os = "linux", target_os = "android"))]
const MAP_ANON_PRIV: i32 = 0x22 | 0x4000; // MAP_PRIVATE | MAP_ANONYMOUS | MAP_NORESERVE
#[cfg(target_os = "macos")]
const MAP_ANON_PRIV: i32 = 0x1002;

fn map_rw(addr: u64, len: u64) -> bool {
    unsafe { mprotect(addr as *mut u8, len as usize, 3) == 0 }
}

/// Sets up the sandbox state on first use (on the owner thread).
unsafe fn sb_init(up: *mut Unit) -> Result<(), String> {
    if SB.lock().unwrap_or_else(|e| e.into_inner()).is_some() {
        return Ok(());
    }
    let u = &mut *up;
    let tls = u.tls_alloc(32, 8);
    let tls_sym = u.syms.intern(SymKind::Rt("sb_tls"), tls as u64);
    let ts = jit::thunk_size() as u64;
    let bytes = (SBT_N as u64 * ts) as usize;
    let sbt_base = crate::os::mem::reserve(bytes, 0);
    // the slot every unused sandbox thunk jumps through: its code panics with the reason
    let trap_slot = u.fn_id(FnKey::Sandbox(u32::MAX));
    let s0 = u.reserve_code(SPACE);
    let s1 = u.reserve_code(SPACE);
    let trap = tgt::thunk(u.slots.table(trap_slot), ts as usize);
    let mut all = Vec::with_capacity(bytes);
    for _ in 0..SBT_N {
        all.extend_from_slice(&trap);
    }
    crate::os::mem::write_code(sbt_base, &all);
    TLS_OFF.store(tls + 1, std::sync::atomic::Ordering::Release);
    *SB.lock().unwrap_or_else(|e| e.into_inner()) = Some(Sb {
        tls_sym,
        sbt_base,
        sbt_n: 1,
        sbt_of: HashMap::new(),
        twins: HashMap::new(),
        sandboxed: HashSet::new(),
        wrappers: HashSet::new(),
        placed: HashMap::new(),
        ro: Vec::new(),
        rw: Vec::new(),
        rel: Vec::new(),
        abs: Vec::new(),
        arenas: Vec::new(),
        refs: HashMap::new(),
        spaces: [(s0, SPACE, s0), (s1, SPACE, s1)],
        active: 0,
        dead_crates: HashSet::new(),
        recycles: 0,
    });
    Ok(())
}

fn with_sb<R>(f: impl FnOnce(&mut Sb) -> R) -> R {
    let mut g = SB.lock().unwrap_or_else(|e| e.into_inner());
    f(g.as_mut().expect("sandbox not initialised"))
}

/// Marks crates whose code runs only as twins (a module's crate; std's are added with the first).
pub unsafe fn add_sandboxed_crate(up: *mut Unit, krate: u32) -> Result<(), String> {
    sb_init(up)?;
    let u = &*up;
    let std_crates: Vec<u32> = (0..u.prog.crates.len() as u32).filter(|&c| matches!(u.prog.syms.str(u.prog.crates[c as usize].name), "core" | "alloc" | "std")).collect();
    with_sb(|s| {
        s.sandboxed.insert(krate);
        s.sandboxed.extend(std_crates);
    });
    Ok(())
}

pub fn add_wrapper_crate(krate: u32) {
    with_sb(|s| {
        s.wrappers.insert(krate);
    });
}

/// Places a twin's code in the active sandbox code space (None: full, the unit's own code
/// memory takes it; a recycle is due).
pub fn emit_twin(u: &Unit, code: &[u8], relocs: &[relax_backend::rir::RelocAt]) -> Option<u64> {
    let at = {
        let mut g = SB.lock().unwrap_or_else(|e| e.into_inner());
        let s = g.as_mut()?;
        let sp = &mut s.spaces[s.active];
        let at = (sp.2 + 15) & !15;
        if at + code.len() as u64 > sp.0 + sp.1 {
            DEAD_BYTES.store(RECYCLE_AT, std::sync::atomic::Ordering::Relaxed);
            return None;
        }
        sp.2 = at + code.len() as u64;
        at
    };
    let c = u.link_at(at, code, relocs).ok()?;
    unsafe { crate::os::mem::write_code(at, &c) };
    Some(at)
}

/// Twin code that stopped being used (replaced by a patch).
pub fn note_dead(bytes: u64) {
    DEAD_BYTES.fetch_add(bytes, std::sync::atomic::Ordering::Relaxed);
}

/// A module's crate was unloaded: its twins are dead code.
pub unsafe fn retire_crate(up: *mut Unit, krate: u32) {
    let u = &*up;
    let mut bytes = 0;
    {
        let mut g = SB.lock().unwrap_or_else(|e| e.into_inner());
        let Some(s) = g.as_mut() else { return };
        s.dead_crates.insert(krate);
        for (&base, &t) in s.twins.iter() {
            if crate_of(u, base) == Some(krate) && u.fns[t as usize].compiled {
                bytes += u.fns[t as usize].code_len as u64;
            }
        }
    }
    note_dead(bytes);
}

fn crate_of(u: &Unit, id: u32) -> Option<u32> {
    match &u.fns[id as usize].key {
        FnKey::Inst(d, _) => Some(u.prog.def(*d).krate),
        FnKey::Closure(f, _, _) => u.prog.file_crate.get(*f as usize).copied(),
        _ => None,
    }
}

/// Whether a recycle is due (dead twin code past the threshold) and none is running.
pub fn recycle_due() -> bool {
    DEAD_BYTES.load(std::sync::atomic::Ordering::Relaxed) >= RECYCLE_AT && !RECYCLING.load(std::sync::atomic::Ordering::Relaxed)
}

/// Starts a recycle on a background thread when one is due.
pub fn maybe_recycle(up: *mut Unit) {
    let session = unsafe { (*std::ptr::addr_of!((*up).prog.session)).clone() };
    if !recycle_due() || RECYCLING.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    struct P(*mut Unit);
    unsafe impl Send for P {}
    let p = P(up);
    std::thread::Builder::new()
        .name("run-sandbox-recycle".to_string())
        .spawn(move || {
            let runtime = &session.runtime;
            let p = p;
            jit::lower_this_thread_priority(runtime);
            let r = unsafe { recycle(p.0) };
            if let Err(e) = r {
                eprintln!("makepad: sandbox recycle skipped: {}", e);
            }
            RECYCLING.store(false, std::sync::atomic::Ordering::SeqCst);
        })
        .ok();
}

/// Rebuilds every live twin into the other sandbox code space and swaps (a unit call
/// here): dead twins (unloaded modules, replaced versions) are left behind, and the space they
/// were in is reused by the next recycle. Returns (twins moved, bytes now used).
pub unsafe fn recycle(up: *mut Unit) -> Result<(usize, u64), String> {
    crate::owner::call(crate::owner::Class::App, || recycle_locked(up))
}

unsafe fn recycle_locked(up: *mut Unit) -> Result<(usize, u64), String> {
    if INFLIGHT.load(std::sync::atomic::Ordering::SeqCst) != 0 && !QUIESCED.load(std::sync::atomic::Ordering::SeqCst) {
        return Err("the space to reuse may still hold running frames".to_string());
    }
    let (target, tbase, tsize, live, dead) = {
        let g = SB.lock().unwrap_or_else(|e| e.into_inner());
        let s = g.as_ref().ok_or("no sandbox")?;
        let target = 1 - s.active;
        let u = &*up;
        let mut live = Vec::new();
        let mut dead = Vec::new();
        for (&base, &t) in s.twins.iter() {
            if !u.fns[t as usize].compiled {
                continue;
            }
            if crate_of(u, base).map_or(false, |k| s.dead_crates.contains(&k)) {
                dead.push(t);
            } else {
                live.push(t);
            }
        }
        (target, s.spaces[target].0, s.spaces[target].1, live, dead)
    };
    // the target space's old code is gone from every index (its frames are quiescent)
    {
        let u = &mut *up;
        u.unindex_range(tbase, tbase + tsize);
        for f in u.fns.iter_mut() {
            if let Some(h) = &mut f.history {
                h.prev.retain(|p| p.0 < tbase || p.0 >= tbase + tsize);
                h.retired.retain(|p| p.0 < tbase || p.0 >= tbase + tsize);
            }
        }
    }
    // build everything first: nothing changes unless every live twin rebuilt
    let mut built: Vec<(u32, u64, Vec<u8>, Vec<(u32, u64)>, Option<relax_backend::rir::UnwindInfo>)> = Vec::new();
    let mut next = tbase;
    for &t in &live {
        let (code, map, relocs) = jit::compile_fn(up, t)?;
        let (n, info) = relax_backend::rir::split_unwind_info(&code);
        let code = &code[..n];
        let at = (next + 15) & !15;
        if at + code.len() as u64 > tbase + tsize {
            return Err("the live twins do not fit one sandbox code space".to_string());
        }
        next = at + code.len() as u64;
        let u = &*up;
        let linked = u.link_at(at, code, &relocs)?;
        built.push((t, at, linked, map, info));
    }
    // swap: each live twin's table word points to its new code; dead twins back to their stubs
    let u = &mut *up;
    for (t, at, code, map, info) in built {
        crate::os::mem::write_code(at, &code);
        u.index_code(at, code.len() as u32, t, info);
        let f = &mut u.fns[t as usize];
        let old = (f.code, f.code_len, std::mem::take(&mut f.pc_map), f.patch_gen);
        crate::zone_alloc::kept_with(|| {
            let h = f.history.get_or_insert_with(Default::default);
            h.retired.push((old.0, old.1, old.2));
            h.prev.clear();
        });
        f.code = at;
        f.code_len = code.len() as u32;
        f.pc_map = crate::jit::PcMap::new(&map);
        f.version += 1;
        let slot = u.slots.table(t) as *const std::sync::atomic::AtomicU64;
        (*slot).store(at, std::sync::atomic::Ordering::SeqCst);
        u.rearm_sandbox_counter(t, at);
    }
    for t in dead {
        let stub = u.stub_of(t);
        let f = &mut u.fns[t as usize];
        f.compiled = false;
        f.rir = None;
        f.version += 1;
        if let Some(h) = &mut f.history { h.prev.clear(); }
        let slot = u.slots.table(t) as *const std::sync::atomic::AtomicU64;
        (*slot).store(stub, std::sync::atomic::Ordering::SeqCst);
    }
    let used = next - tbase;
    let n = live.len();
    with_sb(|s| {
        s.active = target;
        s.spaces[target].2 = next;
        s.recycles += 1;
    });
    DEAD_BYTES.store(0, std::sync::atomic::Ordering::SeqCst);
    QUIESCED.store(INFLIGHT.load(std::sync::atomic::Ordering::SeqCst) == 0, std::sync::atomic::Ordering::SeqCst);
    Ok((n, used))
}

/// (recycles done, bytes used in the active code space, dead bytes pending).
pub fn code_stats() -> (u32, u64, u64) {
    let g = SB.lock().unwrap_or_else(|e| e.into_inner());
    match g.as_ref() {
        Some(s) => (s.recycles, s.spaces[s.active].2 - s.spaces[s.active].0, DEAD_BYTES.load(std::sync::atomic::Ordering::Relaxed)),
        None => (0, 0, 0),
    }
}

/// Whether slot `id` holds code of a sandboxed crate (never compiled or inlined as host code).
pub fn is_sandboxed_slot(u: &Unit, id: u32) -> bool {
    // no module loaded yet: one atomic load (these sit on every compile's path)
    if !sb_ready() {
        return false;
    }
    let g = SB.lock().unwrap_or_else(|e| e.into_inner());
    let Some(s) = g.as_ref() else { return false };
    let k = match &u.fns[id as usize].key {
        FnKey::Inst(d, _) => u.prog.def(*d).krate,
        FnKey::Closure(f, _, _) => match u.prog.file_crate.get(*f as usize) {
            Some(k) => *k,
            None => return false,
        },
        _ => return false,
    };
    // std code is host code too: only module crates are exclusively sandboxed
    let kname = u.prog.syms.str(u.prog.crates[k as usize].name);
    s.sandboxed.contains(&k) && !matches!(kname, "core" | "alloc" | "std")
}

/// As is_sandboxed_slot, from a key (the inliner's view).
pub fn is_sandboxed_crate_key(prog: &crate::program::Program, key: &FnKey) -> bool {
    // no module loaded yet: one atomic load (these sit on every compile's path)
    if !sb_ready() {
        return false;
    }
    let g = SB.lock().unwrap_or_else(|e| e.into_inner());
    let Some(s) = g.as_ref() else { return false };
    let k = match key {
        FnKey::Inst(d, _) => prog.def(*d).krate,
        FnKey::Closure(f, _, _) => match prog.file_crate.get(*f as usize) {
            Some(k) => *k,
            None => return false,
        },
        _ => return false,
    };
    s.sandboxed.contains(&k) && !matches!(prog.syms.str(prog.crates[k as usize].name), "core" | "alloc" | "std")
}

/// Whether host code of crate `krate` is a generated wrapper (its calls into modules go to twins).
pub fn is_wrapper_crate(krate: u32) -> bool {
    // no module loaded yet: one atomic load (these sit on every compile's path)
    if !sb_ready() {
        return false;
    }
    let g = SB.lock().unwrap_or_else(|e| e.into_inner());
    g.as_ref().map_or(false, |s| s.wrappers.contains(&krate))
}

/// The twin of slot `base` (created on first use, with its sandbox thunk).
pub unsafe fn twin(up: *mut Unit, base: u32) -> u32 {
    if let Some(t) = with_sb(|s| s.twins.get(&base).copied()) {
        return t;
    }
    let u = &mut *up;
    let t = u.fn_id(FnKey::Sandbox(base));
    let ts = jit::thunk_size() as u64;
    let (k, sbt) = with_sb(|s| {
        s.twins.insert(base, t);
        let k = s.sbt_n;
        s.sbt_n += 1;
        s.sbt_of.insert(t, k);
        (k, s.sbt_base)
    });
    if k >= SBT_N {
        panic!("makepad: sandbox thunk table full ({} functions)", SBT_N);
    }
    crate::os::mem::write_code(sbt + k as u64 * ts, &tgt::thunk(u.slots.table(t), ts as usize));
    t
}

/// The twin of `base`, if it has one.
pub fn twin_of(base: u32) -> Option<u32> {
    // no module loaded yet: one atomic load (these sit on every compile's path)
    if !sb_ready() {
        return None;
    }
    let g = SB.lock().unwrap_or_else(|e| e.into_inner());
    g.as_ref().and_then(|s| s.twins.get(&base).copied())
}

/// The sandbox thunk address of twin `t` (a fn pointer value in sandboxed code).
fn sbthunk(t: u32) -> u64 {
    with_sb(|s| s.sbt_base + *s.sbt_of.get(&t).unwrap_or(&0) as u64 * jit::thunk_size() as u64)
}

/// Slots twin `t`'s code refers to without a relocation (fn pointer constants).
pub fn extra_refs(t: u32) -> Vec<u32> {
    let g = SB.lock().unwrap_or_else(|e| e.into_inner());
    g.as_ref().and_then(|s| s.refs.get(&t).cloned()).unwrap_or_default()
}

// ------------------------------------------------------------------ the data image

/// The window offset of the data a symbol names (placed, with what it points to, on first use).
unsafe fn place(up: *mut Unit, sym: u32, depth: u32) -> Result<u32, String> {
    if let Some(o) = with_sb(|s| s.placed.get(&sym).copied()) {
        return Ok(o);
    }
    if depth > 64 {
        return Err("data nested too deep for the sandbox image".to_string());
    }
    let u = &mut *up;
    let kind = u.syms.kind(sym).ok_or("unknown symbol")?;
    // (bytes, align, writable, pointer words (offset -> target symbol))
    let (bytes, align, rw, ptrs): (Vec<u8>, u64, bool, Vec<(u32, Sym)>) = match kind {
        SymKind::Data(i) => {
            let (start, len, align, mutable) = u.syms.obj_info(i).ok_or("unknown data object")?;
            if mutable {
                let b = initial_bytes(u, sym)?;
                let words = provenance_words(u, &b);
                (b, align, true, words)
            } else {
                let words = crate::syms::obj_pointer_words(u, i).ok_or_else(|| format!("data object `{}` holds pointers the sandbox cannot relocate", crate::syms::describe(u, Sym::new(sym))))?;
                (std::slice::from_raw_parts(start as *const u8, len as usize).to_vec(), align, false, words)
            }
        }
        SymKind::Static(d) => {
            let t = u.tcx.const_ty(&u.prog, d).ok_or("static without a type")?;
            let align = u.layout_of(t).align.max(1) as u64;
            let b = initial_bytes(u, sym)?;
            let words = provenance_words(u, &b);
            (b, align, true, words)
        }
        SymKind::Rw(i) => {
            let (size, align) = u.syms.rw_blocks.lock().unwrap_or_else(|e| e.into_inner()).get(i as usize).copied().ok_or("unknown block")?;
            // Lowered promotions are filled by code when it runs. Only promotions filled
            // by a const initializer have nonzero initial bytes; never copy host writes.
            let b = initial_bytes(u, sym).unwrap_or_else(|_| vec![0; size.max(1) as usize]);
            let words = provenance_words(u, &b);
            (b, align.max(1), true, words)
        }
        k => return Err(format!("{:?} is not data a sandboxed module can reach", k)),
    };
    // reserve the place first (cycles: data pointing to itself or to each other)
    let off = with_sb(|s| {
        let (img, base, end) = if rw { (&mut s.rw, RW_BASE, RW_END) } else { (&mut s.ro, RO_BASE, RO_END) };
        let at = (img.len() as u64 + align - 1) / align * align;
        if base as u64 + at + bytes.len() as u64 > end as u64 {
            return Err("sandbox data image full".to_string());
        }
        img.resize(at as usize, 0);
        img.extend_from_slice(&bytes);
        let off = base + at as u32;
        s.placed.insert(sym, off);
        Ok(off)
    })?;
    for (o, t) in ptrs {
        let tk = u.syms.kind(t.id);
        let pos = off + o;
        match tk {
            Some(SymKind::Thunk(id)) => {
                let tw = twin(up, id);
                let v = (sbthunk(tw) as i64 + t.off) as u64;
                with_sb(|s| s.abs.push((pos, v)));
                with_sb(|s| s.refs.entry(u32::MAX).or_default().push(tw));
            }
            Some(SymKind::Data(_)) | Some(SymKind::Static(_)) | Some(SymKind::Rw(_)) => {
                let to = place(up, t.id, depth + 1)?;
                with_sb(|s| s.rel.push((pos, (to as i64 + t.off) as u32)));
            }
            Some(SymKind::Site(_)) => {}
            Some(SymKind::Rt("null")) => {}
            other => return Err(format!("data `{}` points to {:?}, which a sandboxed module cannot reach", crate::syms::describe(&*up, Sym::new(sym)), other)),
        }
    }
    sync_all();
    Ok(off)
}

/// Freeze a completed initializer and its promoted objects before any host code can
/// mutate them. References to other statics retain their symbolic identity; each static
/// captures its own initializer (including cycles). No runtime heap/TLS is sampled.
pub(crate) fn capture_initial_data(u: &Unit, root: Sym) {
    let mut pending = vec![root];
    while let Some(sym) = pending.pop() {
        if u.syms.initial_data.lock().unwrap_or_else(|e| e.into_inner()).contains_key(&sym.id) {
            continue;
        }
        let size = match u.syms.kind(sym.id) {
            Some(SymKind::Static(d)) => match u.tcx.const_ty(&u.prog, d) {
                // the layout the static's storage was sized with (Unit::layout_of normalizes const aliases
                // in array lengths: `[u8; N]` with `const N` is N bytes, not an unevaluated length)
                Some(t) => u.layout_of(t).size.max(1) as usize,
                None => continue,
            },
            Some(SymKind::Data(i)) => match u.syms.obj_info(i) {
                Some((_, len, _, _)) => len as usize,
                None => continue,
            },
            Some(SymKind::Rw(i)) => u.syms.rw_blocks.lock().unwrap_or_else(|e| e.into_inner())[i as usize].0.max(1) as usize,
            _ => continue,
        };
        // identity state (the declared initial bytes): the program's heap, not the compiler arena's
        let bytes = crate::zone_alloc::kept_with(|| unsafe { std::slice::from_raw_parts(u.syms.addr(Sym::new(sym.id)) as *const u8, size).to_vec() });
        // the data objects its words point into: what provenance_words would attribute to a
        // data object (only those are captured; the thunk / static / runtime / C attributions
        // it tries after are never data, and its C declaration scan is not needed here)
        let words = u.syms.data_words(&bytes);
        crate::zone_alloc::kept_with(|| u.syms.initial_data.lock().unwrap_or_else(|e| e.into_inner()).insert(sym.id, bytes));
        pending.extend(words);
    }
}

pub(crate) fn initial_bytes(u: &Unit, sym: u32) -> Result<Vec<u8>, String> {
    u.syms.initial_data.lock().unwrap_or_else(|e| e.into_inner()).get(&sym).cloned()
        .ok_or_else(|| format!("sandbox: missing declared initializer for {}", crate::syms::describe(u, Sym::new(sym))))
}

/// Pointer words of initializer bytes by provenance (a word equal to the address of a
/// known data object, static or thunk). Never inspect a running host static here.
pub(crate) fn provenance_words(u: &Unit, b: &[u8]) -> Vec<(u32, Sym)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 8 <= b.len() {
        let w = u64::from_le_bytes(b[i..i + 8].try_into().unwrap());
        if w > 0x10000 {
            if let Some(s) = u.syms.symbol_at(w, u) {
                out.push((i as u32, s));
            }
        }
        i += 8;
    }
    out
}

/// Copies what the image gained since into every arena.
fn sync_all() {
    let g = SB.lock().unwrap_or_else(|e| e.into_inner());
    let s = g.as_ref().unwrap();
    for a in &s.arenas {
        if a.alive.load(std::sync::atomic::Ordering::Relaxed) {
            sync_arena(s, a);
        }
    }
}

fn sync_arena(s: &Sb, a: &Arena) {
    let mut g = a.synced.lock().unwrap_or_else(|e| e.into_inner());
    let (ro, rw, rel, abs) = *g;
    unsafe {
        if (ro as usize) < s.ro.len() {
            std::ptr::copy_nonoverlapping(s.ro.as_ptr().add(ro as usize), (a.base + RO_BASE as u64 + ro as u64) as *mut u8, s.ro.len() - ro as usize);
        }
        if (rw as usize) < s.rw.len() {
            std::ptr::copy_nonoverlapping(s.rw.as_ptr().add(rw as usize), (a.base + RW_BASE as u64 + rw as u64) as *mut u8, s.rw.len() - rw as usize);
        }
        for &(pos, to) in &s.rel[rel..] {
            *((a.base + pos as u64) as *mut u64) = a.base + to as u64;
        }
        for &(pos, v) in &s.abs[abs..] {
            *((a.base + pos as u64) as *mut u64) = v;
        }
    }
    *g = (s.ro.len() as u32, s.rw.len() as u32, s.rel.len(), s.abs.len());
}

// ------------------------------------------------------------------ arenas

/// Creates a module's arena (`mem` bytes of heap budget). Returns its index.
/// Does any sandbox module live (the settle swap is refused meanwhile)?
pub fn any_live() -> bool {
    SB.lock().map(|g| g.is_some()).unwrap_or(true) || ARENAS.iter().any(|a| !a.load(std::sync::atomic::Ordering::Acquire).is_null())
}

pub fn new_arena(module: u32, mem: u64, fuel: i64) -> Result<Arc<Arena>, String> {
    if cfg!(windows) {
        return Err("sandboxed run-time modules are not available on Windows yet".to_string());
    }
    unsafe {
        let span = 2 * WINDOW + 2 * GUARD;
        let p = mmap(std::ptr::null_mut(), span as usize, 0, MAP_ANON_PRIV, -1, 0);
        if p as isize == -1 || p.is_null() {
            return Err("cannot reserve an arena window".to_string());
        }
        let p = p as u64;
        let base = (p + GUARD + WINDOW - 1) & !(WINDOW - 1);
        // keep [base - GUARD, base + WINDOW + GUARD), give back the rest
        let lo = base - GUARD;
        let hi = base + WINDOW + GUARD;
        if lo > p {
            munmap(p as *mut u8, (lo - p) as usize);
        }
        if p + span > hi {
            munmap(hi as *mut u8, (p + span - hi) as usize);
        }
        if !map_rw(base + RO_BASE as u64, (RW_END - RO_BASE) as u64) || !map_rw(base + SS_LO as u64, (SCRATCH + SCRATCH_LEN - SS_LO) as u64) {
            munmap(lo as *mut u8, (hi - lo) as usize);
            return Err("cannot map an arena".to_string());
        }
        let a = Arc::new(Arena {
            base,
            module,
            heap: Mutex::new(Heap { top: HEAP_BASE, mapped: HEAP_BASE, budget: mem, used: 0, free: vec![Vec::new(); 17], large_free: Vec::new(), sizes: HashMap::new() }),
            synced: Mutex::new((0, 0, 0, 0)),
            fuel: if (&*jit::current_unit()).prog.session.options.sandbox_fuel { fuel } else { 0 },
            alive: std::sync::atomic::AtomicBool::new(true),
        });
        let mut g = SB.lock().unwrap_or_else(|e| e.into_inner());
        let s = g.as_mut().ok_or("sandbox not initialised")?;
        sync_arena(s, &a);
        s.arenas.push(a.clone());
        if (module as usize) < MAX_MODULES {
            ARENAS[module as usize].store(Arc::as_ptr(&a) as *mut Arena, std::sync::atomic::Ordering::Release);
        }
        Ok(a)
    }
}

/// Frees a module's arena (its window is unmapped: stale pointers into it fault).
pub fn free_arena(a: &Arc<Arena>) {
    a.alive.store(false, std::sync::atomic::Ordering::SeqCst);
    if (a.module as usize) < MAX_MODULES {
        ARENAS[a.module as usize].store(std::ptr::null_mut(), std::sync::atomic::Ordering::Release);
    }
    unsafe {
        munmap((a.base - GUARD) as *mut u8, (WINDOW + 2 * GUARD) as usize);
    }
    let mut g = SB.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(s) = g.as_mut() {
        s.arenas.retain(|x| !Arc::ptr_eq(x, a));
    }
}

impl Heap {
    fn class(size: u32) -> Option<usize> {
        if size > 1 << 20 {
            return None;
        }
        let c = size.max(16).next_power_of_two();
        Some(c.trailing_zeros() as usize - 4)
    }

    fn alloc(&mut self, base: u64, size: u64, align: u64) -> u64 {
        if size == 0 || size > (HEAP_END - HEAP_BASE) as u64 || align > 1 << 20 {
            return 0;
        }
        let size = size as u32;
        let (sz, off) = match Self::class(size.max(align as u32)) {
            Some(c) => {
                let sz = 16u32 << c;
                if self.used + sz as u64 > self.budget {
                    return 0;
                }
                match self.free[c].pop() {
                    Some(o) => (sz, o),
                    None => match self.bump(base, sz, sz) {
                        Some(o) => (sz, o),
                        None => return 0,
                    },
                }
            }
            None => {
                let sz = (size + 0xffff) & !0xffff;
                if self.used + sz as u64 > self.budget {
                    return 0;
                }
                match self.large_free.iter().position(|&(_, l)| l == sz) {
                    Some(i) => (sz, self.large_free.swap_remove(i).0),
                    None => match self.bump(base, sz, 1 << 16) {
                        Some(o) => (sz, o),
                        None => return 0,
                    },
                }
            }
        };
        self.used += sz as u64;
        self.sizes.insert(off, sz);
        base + off as u64
    }

    fn bump(&mut self, base: u64, sz: u32, align: u32) -> Option<u32> {
        let at = (self.top as u64 + align as u64 - 1) / align as u64 * align as u64;
        let end = at + sz as u64;
        if end > HEAP_END as u64 {
            return None;
        }
        let growth = end - self.top as u64;
        if !budget_growth(base, growth, false) { return None; }
        if end > self.mapped as u64 {
            let new_mapped = ((end + (16 << 20) - 1) & !((16 << 20) - 1)).min(HEAP_END as u64);
            if !map_rw(base + self.mapped as u64, new_mapped - self.mapped as u64) {
                return None;
            }
            self.mapped = new_mapped as u32;
        }
        budget_growth(base, growth, true);
        self.top = end as u32;
        Some(at as u32)
    }

    fn free(&mut self, off: u32) {
        if let Some(sz) = self.sizes.remove(&off) {
            self.used -= sz as u64;
            match Self::class(sz) {
                Some(c) if sz == 16 << c => self.free[c].push(off),
                _ => self.large_free.push((off, sz)),
            }
        }
    }

    fn size_of(&self, off: u32) -> Option<u32> {
        self.sizes.get(&off).copied()
    }
}

// ------------------------------------------------------------------ this thread's sandbox words

fn words() -> *mut i64 {
    let tls = TLS_OFF.load(std::sync::atomic::Ordering::Acquire) - 1;
    (jit::tls_block() + tls as u64) as *mut i64
}

fn sb_ready() -> bool {
    TLS_OFF.load(std::sync::atomic::Ordering::Acquire) != 0
}

fn cur_arena() -> Option<&'static Arena> {
    let p = unsafe { *words().add(3) } as *const Arena;
    if p.is_null() {
        None
    } else {
        Some(unsafe { &*p })
    }
}

/// Masks a module pointer into the current window.
fn m(p: u64) -> u64 {
    match cur_arena() {
        Some(a) => a.base | (p & 0xffff_ffff),
        None => 0,
    }
}

/// `len` bytes from masked `p` stay inside the window.
fn fits(p: u64, len: u64) -> bool {
    (p & 0xffff_ffff) + len <= WINDOW
}

/// The state a guarded call or a wrapper restores (arena, shadow sp, fuel, arena state).
#[derive(Clone, Copy)]
pub struct Saved(pub [i64; 4]);

struct Account {
    base: u64,
    out: std::rc::Rc<std::cell::Cell<[u64; 4]>>,
    // remaining fuel, allocation limit, consumed growth, error (1 fuel / 2 alloc)
    data: [u64; 4],
}

thread_local! {
    static ENTRY_DEPTH: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    static ACCOUNTS: std::cell::RefCell<Vec<Account>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Flush before changing arenas, including non-local exits through guard recovery.
fn budget_flush() {
    if !sb_ready() { return; }
    ACCOUNTS.with(|accounts| {
        let mut accounts = accounts.borrow_mut();
        if accounts.is_empty() { return; }
        unsafe {
            let w = words();
            let base = *w as u64;
            if base == 0 { return; }
            let left = *((base + FUEL_OFF as u64) as *const i64);
            let spent = (*w.add(2)).saturating_sub(left).max(0) as u64;
            for a in accounts.iter_mut().filter(|a| a.base == base) {
                a.data[0] = a.data[0].saturating_sub(spent);
                if left < 0 && a.data[3] == 0 { a.data[3] = 1; }
                a.out.set(a.data);
            }
            *w.add(2) = left;
        }
    });
}

fn budget_reload() {
    if !sb_ready() { return; }
    ACCOUNTS.with(|accounts| unsafe {
        let w = words();
        let base = *w as u64;
        if let Some(left) = accounts.borrow().iter().filter(|a| a.base == base).map(|a| a.data[0]).min() {
            let left = left.min(i64::MAX as u64) as i64;
            *((base + FUEL_OFF as u64) as *mut i64) = left;
            *w.add(2) = left;
        }
    });
}

pub fn budget_begin(a: &Arena, token: u64) {
    budget_flush();
    let out = unsafe {
        let p = token as *const std::cell::Cell<[u64; 4]>;
        std::rc::Rc::increment_strong_count(p);
        std::rc::Rc::from_raw(p)
    };
    let mut data = out.get();
    data[3] = 0;
    out.set(data);
    ACCOUNTS.with(|s| s.borrow_mut().push(Account { base: a.base, out, data }));
    budget_reload();
}

pub fn budget_end() -> [u64; 4] {
    budget_flush();
    let data = ACCOUNTS.with(|s| s.borrow_mut().pop().map_or([0; 4], |a| a.data));
    budget_reload();
    data
}

/// Charge heap high-water growth, including alignment, only after mapping succeeds.
/// Free-list reuse does not advance the high-water mark and is therefore free.
fn budget_growth(base: u64, bytes: u64, commit: bool) -> bool {
    ACCOUNTS.with(|s| {
        let mut s = s.borrow_mut();
        let mut ok = true;
        for a in s.iter_mut().filter(|a| a.base == base) {
            if bytes > a.data[1].saturating_sub(a.data[2]) {
                if a.data[3] == 0 { a.data[3] = 2; }
                ok = false;
            }
        }
        if ok && commit {
            for a in s.iter_mut().filter(|a| a.base == base) { a.data[2] += bytes; }
        }
        for a in s.iter() { a.out.set(a.data); }
        ok
    })
}

pub fn entry_depth() -> u64 { ENTRY_DEPTH.with(|d| d.get()) }
pub fn entry_recover(depth: u64) {
    ENTRY_DEPTH.with(|d| {
        let skipped = d.get().saturating_sub(depth);
        d.set(depth);
        if skipped != 0 && INFLIGHT.fetch_sub(skipped as i64, std::sync::atomic::Ordering::SeqCst) == skipped as i64 {
            QUIESCED.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    });
}

pub fn budget_depth() -> usize { ACCOUNTS.with(|s| s.borrow().len()) }
pub fn budget_recover(depth: usize) {
    budget_flush();
    ACCOUNTS.with(|s| s.borrow_mut().truncate(depth));
    budget_reload();
}

pub fn current_module() -> Option<u32> {
    if !sb_ready() { return None; }
    cur_arena().map(|a| a.module)
}

/// Copy an owned string to host storage, then release its arena allocation.
pub unsafe fn string_copy(p: u64, len: u64, out: *mut u8) -> bool {
    let Some(a) = cur_arena() else { return false; };
    if len == 0 { sb_free(p); return true; }
    if p >> 32 != a.base >> 32 { return false; }
    let mut heap = a.heap.lock().unwrap_or_else(|e| e.into_inner());
    let off = (p - a.base) as u32;
    if !heap.size_of(off).is_some_and(|size| len <= size as u64) { return false; }
    std::ptr::copy_nonoverlapping(p as *const u8, out, len as usize);
    heap.free(off);
    true
}

pub fn save() -> Saved {
    if !sb_ready() {
        return Saved([0; 4]);
    }
    let w = words();
    unsafe { Saved([*w, *w.add(1), *w.add(2), *w.add(3)]) }
}

pub fn restore(s: Saved) {
    if !sb_ready() {
        return;
    }
    budget_flush();
    let w = words();
    unsafe {
        for k in 0..4 {
            *w.add(k) = s.0[k];
        }
    }
    budget_reload();
}

/// Enters module `idx`'s arena on this thread (a wrapper's call): the state before, or None
/// for an unloaded module. Lock-free.
pub fn enter_module(idx: u32) -> Option<Saved> {
    if idx as usize >= MAX_MODULES {
        return None;
    }
    let p = ARENAS[idx as usize].load(std::sync::atomic::Ordering::Acquire);
    if p.is_null() {
        return None;
    }
    // the Arc in the module table keeps it alive until free_arena clears this entry first
    enter_raw(unsafe { &*p }).ok()
}

/// Enters arena `a` on this thread (a wrapper's call into its module).
pub fn enter(a: &Arc<Arena>) -> Result<Saved, String> {
    enter_raw(a)
}

fn enter_raw(a: &Arena) -> Result<Saved, String> {
    if !a.alive.load(std::sync::atomic::Ordering::Acquire) {
        return Err("the module was unloaded".to_string());
    }
    budget_flush();
    INFLIGHT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    ENTRY_DEPTH.with(|d| d.set(d.get() + 1));
    let w = words();
    let prev = unsafe { Saved([*w, *w.add(1), *w.add(2), *w.add(3)]) };
    unsafe {
        // nested entry into the same arena continues its shadow stack
        let same = *w == a.base as i64;
        *w = a.base as i64;
        if !same {
            *w.add(1) = (a.base + SS_HI as u64) as i64;
        }
        *w.add(2) = if a.fuel > 0 { a.fuel } else { i64::MAX };
        *w.add(3) = a as *const Arena as i64;
        // the twins' fuel word (in the window: the module could only raise its own budget, and
        // only with unsafe code the compile-time check refuses)
        if !same {
            *((a.base + FUEL_OFF as u64) as *mut i64) = if a.fuel > 0 { a.fuel } else { i64::MAX };
        }
    }
    budget_reload();
    Ok(prev)
}

/// A wrapper's call returns (or unwinds): the state before it comes back.
pub fn leave(prev: Saved) {
    restore(prev);
    ENTRY_DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
    if INFLIGHT.fetch_sub(1, std::sync::atomic::Ordering::SeqCst) == 1 {
        QUIESCED.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Shadow-stack bytes for a wrapper's argument copy or result slot (0: no room).
pub fn alloca(size: u64, align: u64) -> u64 {
    let w = words();
    unsafe {
        let a = *w as u64;
        let sp = *w.add(1) as u64;
        let at = (sp.saturating_sub(size)) & !(align.max(1) - 1);
        if a == 0 || at < a + SS_LO as u64 {
            return 0;
        }
        *w.add(1) = at as i64;
        at
    }
}

// ------------------------------------------------------------------ runtime entries for twins

// Rust and C program allocations must stay inside the same sandbox window. The layout
// ABI preserves over-alignment on growth without trusting a caller's claimed old size.
extern "C" fn sb_alloc(size: u64, align: u64) -> u64 {
    let Some(a) = cur_arena() else { return 0 };
    if !align.is_power_of_two() { return 0; }
    a.heap.lock().unwrap_or_else(|e| e.into_inner()).alloc(a.base, size, align)
}
extern "C" fn sb_alloc_zeroed(size: u64, align: u64) -> u64 {
    let p = sb_alloc(size, align);
    if p != 0 { unsafe { std::ptr::write_bytes(p as *mut u8, 0, size as usize) }; }
    p
}
extern "C" fn sb_dealloc(p: u64, _size: u64, _align: u64) {
    sb_free(p);
}
extern "C" fn sb_resize(p: u64, _old: u64, align: u64, size: u64) -> u64 {
    let Some(a) = cur_arena() else { return 0 };
    let old = a.heap.lock().unwrap_or_else(|e| e.into_inner()).size_of((p & 0xffff_ffff) as u32);
    let Some(old) = old else { return 0 };
    let q = sb_alloc(size, align);
    if q != 0 {
        unsafe { std::ptr::copy_nonoverlapping(m(p) as *const u8, q as *mut u8, (old as u64).min(size) as usize) };
        sb_free(p);
    }
    q
}

extern "C" fn sb_malloc(size: u64) -> u64 {
    let Some(a) = cur_arena() else { return 0 };
    a.heap.lock().unwrap_or_else(|e| e.into_inner()).alloc(a.base, size, 16)
}

extern "C" fn sb_calloc(n: u64, size: u64) -> u64 {
    let Some(t) = n.checked_mul(size) else { return 0 };
    let p = sb_malloc(t);
    if p != 0 {
        unsafe { std::ptr::write_bytes(p as *mut u8, 0, t as usize) };
    }
    p
}

extern "C" fn sb_free(p: u64) {
    let Some(a) = cur_arena() else { return };
    if p == 0 {
        return;
    }
    a.heap.lock().unwrap_or_else(|e| e.into_inner()).free((p & 0xffff_ffff) as u32);
}

extern "C" fn sb_realloc(p: u64, size: u64) -> u64 {
    let Some(a) = cur_arena() else { return 0 };
    if p == 0 {
        return sb_malloc(size);
    }
    let old = a.heap.lock().unwrap_or_else(|e| e.into_inner()).size_of((p & 0xffff_ffff) as u32);
    let Some(old) = old else { return 0 };
    let q = sb_malloc(size);
    if q != 0 {
        unsafe { std::ptr::copy_nonoverlapping(m(p) as *const u8, q as *mut u8, (old as u64).min(size) as usize) };
        sb_free(p);
    }
    q
}

extern "C" fn sb_posix_memalign(out: u64, align: u64, size: u64) -> i32 {
    let Some(a) = cur_arena() else { return 12 };
    let p = a.heap.lock().unwrap_or_else(|e| e.into_inner()).alloc(a.base, size, align);
    if p == 0 {
        return 12;
    }
    unsafe { *(m(out) as *mut u64) = p };
    0
}

extern "C" fn sb_memcpy(d: u64, s: u64, n: u64) -> u64 {
    let (dm, sm) = (m(d), m(s));
    if n > 0 && fits(dm, n) && fits(sm, n) {
        unsafe { std::ptr::copy(sm as *const u8, dm as *mut u8, n as usize) };
    } else if n > 0 {
        // out of the window: the module faults where its own code would have
        unsafe { std::ptr::write_volatile((dm | 0xffff_ffff) as *mut u8, 0) };
    }
    dm
}

extern "C" fn sb_memset(d: u64, c: u64, n: u64) -> u64 {
    let dm = m(d);
    if n > 0 && fits(dm, n) {
        unsafe { std::ptr::write_bytes(dm as *mut u8, c as u8, n as usize) };
    } else if n > 0 {
        unsafe { std::ptr::write_volatile((dm | 0xffff_ffff) as *mut u8, 0) };
    }
    dm
}

extern "C" fn sb_memcmp(a: u64, b: u64, n: u64) -> i32 {
    let (am, bm) = (m(a), m(b));
    if n == 0 || !fits(am, n) || !fits(bm, n) {
        return 0;
    }
    let (x, y) = unsafe { (std::slice::from_raw_parts(am as *const u8, n as usize), std::slice::from_raw_parts(bm as *const u8, n as usize)) };
    match x.cmp(y) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }
}

extern "C" fn sb_strlen(p: u64) -> u64 {
    let pm = m(p);
    let mut n = 0;
    while fits(pm, n + 1) && unsafe { *((pm + n) as *const u8) } != 0 {
        n += 1;
    }
    n
}

extern "C" fn sb_fmt_str(p: u64, len: u64, spec: u64) {
    let pm = m(p);
    let len = if fits(pm, len) { len } else { 0 };
    let f: extern "C" fn(u64, u64, u64) = unsafe { std::mem::transmute(jit::rt_entry("fmt_str") as usize) };
    f(pm, len, spec)
}

extern "C" fn sb_str_eq(p1: u64, l1: u64, p2: u64, l2: u64) -> u64 {
    let (a, b) = (m(p1), m(p2));
    if l1 != l2 || !fits(a, l1) || !fits(b, l2) {
        return 0;
    }
    let (x, y) = unsafe { (std::slice::from_raw_parts(a as *const u8, l1 as usize), std::slice::from_raw_parts(b as *const u8, l2 as usize)) };
    (x == y) as u64
}

/// fmt_pop for a module: the text is copied into the window's scratch (larger text into its heap).
extern "C" fn sb_fmt_pop() -> jit::StrRet {
    let f: extern "C" fn() -> jit::StrRet = unsafe { std::mem::transmute(jit::rt_entry("fmt_pop") as usize) };
    let r = f();
    let Some(a) = cur_arena() else { return r };
    let (p, n) = r.parts();
    let dst = if n <= SCRATCH_LEN as u64 { a.base + SCRATCH as u64 } else { a.heap.lock().unwrap_or_else(|e| e.into_inner()).alloc(a.base, n, 16) };
    if dst == 0 {
        return jit::StrRet::new(a.base + SCRATCH as u64, 0);
    }
    unsafe { std::ptr::copy_nonoverlapping(p as *const u8, dst as *mut u8, n as usize) };
    jit::StrRet::new(dst, n)
}

/// catch_unwind inside a module: the target and data are masked; the shadow stack comes back.
extern "C" fn sb_catch_panic(f: u64, data: u64) -> u64 {
    let s = save();
    let fm = mask_thunk(f);
    let g: extern "C" fn(u64, u64) -> u64 = unsafe { std::mem::transmute(jit::rt_entry("catch_panic") as usize) };
    let r = g(fm, m(data));
    restore(s);
    r
}

fn mask_thunk(f: u64) -> u64 {
    let (base, ts) = with_sb(|s| (s.sbt_base, jit::thunk_size() as u64));
    let span = SBT_N as u64 * ts;
    base + ((f.wrapping_sub(base)) & (span - 1) & !(ts - 1))
}

/// The runtime entries a twin may call: kept (scalar arguments or no memory access), or
/// replaced by an arena-checked version.
fn rt_policy(name: &str) -> Option<Option<(&'static str, u64)>> {
    Some(match name {
        "fmt_int" | "fmt_float" | "fmt_push" | "fmt_print" | "panicking" | "rethrow" | "fmod" | "fmodf" | "null" | "panic_site" | "panic_bounds" | "panic_impl" | "resume" | "land" | "divrem128" | "wide_f32" | "wide_f64" => None,
        "fmt_str" => Some(("sb_fmt_str", sb_fmt_str as *const () as u64)),
        "fmt_pop" => Some(("sb_fmt_pop", sb_fmt_pop as *const () as u64)),
        "str_eq" => Some(("sb_str_eq", sb_str_eq as *const () as u64)),
        "memcpy" | "memmove" => Some(("sb_memcpy", sb_memcpy as *const () as u64)),
        "memset" => Some(("sb_memset", sb_memset as *const () as u64)),
        "catch_panic" => Some(("sb_catch_panic", sb_catch_panic as *const () as u64)),
        _ => return None,
    })
}

/// C functions a twin may call: math (scalars), or memory/allocation replaced by arena versions.
fn c_policy(name: &str) -> Option<Option<(&'static str, u64)>> {
    let n = name.trim_start_matches('_');
    Some(match n {
        "relax_alloc" => Some(("sb_alloc", sb_alloc as *const () as u64)),
        "relax_alloc_zeroed" => Some(("sb_alloc_zeroed", sb_alloc_zeroed as *const () as u64)),
        "relax_dealloc" => Some(("sb_dealloc", sb_dealloc as *const () as u64)),
        "relax_realloc" => Some(("sb_resize", sb_resize as *const () as u64)),
        "malloc" => Some(("sb_malloc", sb_malloc as *const () as u64)),
        "calloc" => Some(("sb_calloc", sb_calloc as *const () as u64)),
        "realloc" => Some(("sb_realloc", sb_realloc as *const () as u64)),
        "free" => Some(("sb_free", sb_free as *const () as u64)),
        "posix_memalign" => Some(("sb_posix_memalign", sb_posix_memalign as *const () as u64)),
        "memcpy" | "memmove" => Some(("sb_memcpy", sb_memcpy as *const () as u64)),
        "memset" => Some(("sb_memset", sb_memset as *const () as u64)),
        "memcmp" | "bcmp" => Some(("sb_memcmp", sb_memcmp as *const () as u64)),
        "strlen" => Some(("sb_strlen", sb_strlen as *const () as u64)),
        "sin" | "cos" | "tan" | "asin" | "acos" | "atan" | "atan2" | "sinh" | "cosh" | "tanh" | "asinh" | "acosh" | "atanh" | "exp" | "exp2" | "expm1" | "log" | "log2" | "log10" | "log1p" | "pow" | "sqrt" | "cbrt" | "hypot" | "fmod" | "floor" | "ceil" | "round" | "trunc" | "fma" | "fmax" | "fmin" | "fdim" | "erf" | "erfc" | "tgamma" | "lgamma" | "ldexp" | "nextafter" | "copysign" | "rint" | "nearbyint" | "sinf" | "cosf" | "tanf" | "asinf" | "acosf" | "atanf" | "atan2f" | "sinhf" | "coshf" | "tanhf" | "asinhf" | "acoshf" | "atanhf" | "expf" | "exp2f" | "expm1f" | "logf" | "log2f" | "log10f" | "log1pf" | "powf" | "sqrtf" | "cbrtf" | "hypotf" | "fmodf" | "floorf" | "ceilf" | "roundf" | "truncf" | "fmaf" | "fmaxf" | "fminf" | "fdimf" | "erff" | "erfcf" | "tgammaf" | "lgammaf" | "ldexpf" | "nextafterf" | "copysignf" | "rintf" | "nearbyintf" => None,
        "abort" => Some(("abort", 0)),
        _ => return None,
    })
}

// ------------------------------------------------------------------ the transform

/// Rewrites a twin's RIR for sandboxed execution (on the owner thread). Errors name what a module
/// cannot do (a C function, data it cannot reach), never a crash.
pub unsafe fn transform(up: *mut Unit, f: &mut Func, twin_id: u32) -> Result<(), String> {
    let session = unsafe { (*std::ptr::addr_of!((*up).prog.session)).clone() };
    let options = &session.options;
    sb_init(up)?;
    let u = &mut *up;
    let (tls_sym, sandboxed) = with_sb(|s| (s.tls_sym, s.sandboxed.clone()));
    // Count the canonical lowered RIR, before confinement and either tier's optimizer.
    // A terminator costs one; a trusted host/runtime call costs 32 instead of one.
    let mut costs: Vec<i64> = f.blocks.iter().map(|b| 1 + b.insts.iter().map(|i| {
        match i {
            Inst::Call(Callee::Host(_) | Callee::CHost(..), ..) => 32,
            Inst::Call(Callee::Fn(id) | Callee::CFn(id, _), ..)
                if matches!(&u.fns[*id as usize].key, FnKey::Inst(d, _) if u.prog.def(*d).kind == DefKind::ForeignFn || !sandboxed.contains(&u.prog.def(*d).krate)) => 32,
            _ => 1,
        }
    }).sum::<i64>()).collect();
    let mut refs: Vec<u32> = Vec::new();
    // the old entry moves to the end: the new block 0 runs the prologue
    let n = f.blocks.len() as u32;
    costs.rotate_left(1);
    let old_entry = f.blocks.remove(0);
    f.blocks.push(old_entry);
    let remap = |b: u32| if b == 0 { n - 1 } else { b - 1 };
    for b in f.blocks.iter_mut() {
        b.term.map_succs(|t| remap(t));
    }
    let entry = n - 1;
    let pos0 = f.blocks[entry as usize].pos.first().copied().unwrap_or(0);
    // shadow frame
    let mut offs = Vec::with_capacity(f.slots.len());
    let mut frame: u64 = 0;
    for s in &f.slots {
        let a = s.align.max(1) as u64;
        frame = (frame + a - 1) / a * a;
        offs.push(frame);
        frame += s.size as u64;
    }
    let frame = (frame + 15) & !15;
    let has_frame = !f.slots.is_empty();
    let t_arena = f.vreg(Cls::I);
    let ar = f.vreg(Cls::I);
    let t_ssp = f.vreg(Cls::I);
    let ssp0 = f.vreg(Cls::I);
    let nsp = f.vreg(Cls::I);
    let mut b0 = relax_backend::rir::Block { insts: Vec::new(), pos: Vec::new(), term: Term::Jump(entry), term_pos: pos0 };
    let push0 = |b: &mut relax_backend::rir::Block, i: Inst| {
        b.insts.push(i);
        b.pos.push(pos0);
    };
    push0(&mut b0, Inst::TlsAddr(t_arena, tls_sym.plus(W_ARENA)));
    push0(&mut b0, Inst::Load(relax_backend::rir::Mem::Int(8, false), ar, t_arena, 0));
    // trap blocks (made on demand)
    let site_stack = u.site_sym(f.file, (pos0 & 0xffff_ffff) as u32, "sandboxed module: shadow stack overflow (recursion too deep)");
    let site_fuel = u.site_sym(f.file, (pos0 & 0xffff_ffff) as u32, "sandboxed module: instruction budget (fuel) exhausted");
    let site_abort = u.site_sym(f.file, (pos0 & 0xffff_ffff) as u32, "sandboxed module: aborted (out of memory?)");
    let panic_site = u.rs.panic_site;
    let mut new_blocks: Vec<relax_backend::rir::Block> = Vec::new();
    let nb = f.blocks.len() as u32 + 1; // index of the first appended block (after block 0 is inserted)
    let trap = |f: &mut Func, nbs: &mut Vec<relax_backend::rir::Block>, site: Sym| -> u32 {
        let v = f.vreg(Cls::I);
        nbs.push(relax_backend::rir::Block { insts: vec![Inst::Addr(v, site), Inst::Call(Callee::Host(panic_site), vec![v], Vec::new())], pos: vec![pos0, pos0], term: Term::Unreachable, term_pos: pos0 });
        nb + nbs.len() as u32 - 1
    };
    if has_frame {
        push0(&mut b0, Inst::TlsAddr(t_ssp, tls_sym.plus(W_SSP)));
        push0(&mut b0, Inst::Load(relax_backend::rir::Mem::Int(8, false), ssp0, t_ssp, 0));
        push0(&mut b0, Inst::IBinI(IOp::Sub, U64, nsp, ssp0, frame as i64));
        let rel = f.vreg(Cls::I);
        let c = f.vreg(Cls::I);
        push0(&mut b0, Inst::IBin(IOp::Sub, U64, rel, nsp, ar));
        push0(&mut b0, Inst::ICmpI(Cond::Lt, false, c, rel, SS_LO as i64));
        let t = trap(f, &mut new_blocks, site_stack);
        // the store of the new shadow sp happens in a continuation block
        let cont = relax_backend::rir::Block { insts: vec![Inst::Store(relax_backend::rir::Mem::Int(8, false), t_ssp, 0, nsp)], pos: vec![pos0], term: Term::Jump(entry + 1), term_pos: pos0 };
        new_blocks.push(cont);
        let cont_ix = nb + new_blocks.len() as u32 - 1;
        b0.term = Term::Branch(c, t, cont_ix);
    } else {
        b0.term = Term::Jump(entry + 1);
    }
    // every block index shifts by one once block 0 is inserted
    for b in f.blocks.iter_mut() {
        b.term.map_succs(|t| t + 1);
    }
    // rewrite the blocks
    let nblocks = f.blocks.len();
    let mut out_blocks: Vec<relax_backend::rir::Block> = Vec::with_capacity(nblocks);
    let blocks = std::mem::take(&mut f.blocks);
    for b in blocks {
        let mut insts = Vec::with_capacity(b.insts.len() * 2);
        let mut pos = Vec::with_capacity(b.insts.len() * 2);
        let mut cache: HashMap<u32, VReg> = HashMap::new();
        for (k, inst) in b.insts.into_iter().enumerate() {
            let p = b.pos.get(k).copied().unwrap_or(pos0);
            let emit = |i: Inst, insts: &mut Vec<Inst>, pos: &mut Vec<u64>| {
                insts.push(i);
                pos.push(p);
            };
            // masks a base (with its offset) into the window
            let mask = |f: &mut Func, insts: &mut Vec<Inst>, pos: &mut Vec<u64>, cache: &mut HashMap<u32, VReg>, base: VReg, off: i32| -> (VReg, i32) {
                let (base, off) = if (off as i64).unsigned_abs() >= GUARD {
                    let b2 = f.vreg(Cls::I);
                    insts.push(Inst::IBinI(IOp::Add, U64, b2, base, off as i64));
                    pos.push(p);
                    (b2, 0)
                } else {
                    (base, off)
                };
                if let Some(&mv) = cache.get(&base.0) {
                    return (mv, off);
                }
                let t = f.vreg(Cls::I);
                let mv = f.vreg(Cls::I);
                insts.push(Inst::Conv(Conv::IntToInt(U32), t, base));
                insts.push(Inst::IBin(IOp::Add, U64, mv, t, ar));
                pos.push(p);
                pos.push(p);
                cache.insert(base.0, mv);
                (mv, off)
            };
            let mut defs: Vec<VReg> = Vec::new();
            relax_backend::regalloc::for_defs(&inst, |d| defs.push(d));
            match inst {
                Inst::SlotAddr(d, s) => emit(Inst::IBinI(IOp::Add, U64, d, nsp, offs[s as usize] as i64), &mut insts, &mut pos),
                Inst::Addr(d, sym) => {
                    let kind = u.syms.kind(sym.id);
                    match kind {
                        Some(SymKind::Data(_)) | Some(SymKind::Static(_)) | Some(SymKind::Rw(_)) => {
                            let o = place(up, sym.id, 0)?;
                            emit(Inst::IBinI(IOp::Add, U64, d, ar, o as i64 + sym.off), &mut insts, &mut pos);
                        }
                        Some(SymKind::Thunk(id)) => {
                            let t = fn_target(up, id, &sandboxed)?;
                            refs.push(t);
                            emit(Inst::Iconst(d, (sbthunk(t) as i64).wrapping_add(sym.off)), &mut insts, &mut pos);
                        }
                        Some(SymKind::Site(_)) | Some(SymKind::Rt("null")) => emit(Inst::Addr(d, sym), &mut insts, &mut pos),
                        other => return Err(format!("{}: refers to {:?}, which a sandboxed module cannot reach", f.name, other)),
                    }
                }
                Inst::FnAddr(d, id) => {
                    let t = fn_target(up, id, &sandboxed)?;
                    refs.push(t);
                    emit(Inst::Iconst(d, sbthunk(t) as i64), &mut insts, &mut pos);
                }
                Inst::TlsAddr(..) => return Err(format!("{}: thread-local storage is not available to sandboxed modules", f.name)),
                Inst::VGather(..) | Inst::VTbl(..) => return Err(format!("{}: vector gathers are not available to sandboxed modules", f.name)),
                Inst::Load(mm, d, base, off) => {
                    let (b2, o2) = mask(f, &mut insts, &mut pos, &mut cache, base, off);
                    emit(Inst::Load(mm, d, b2, o2), &mut insts, &mut pos);
                }
                Inst::Store(mm, base, off, src) => {
                    let (b2, o2) = mask(f, &mut insts, &mut pos, &mut cache, base, off);
                    emit(Inst::Store(mm, b2, o2, src), &mut insts, &mut pos);
                }
                Inst::VLoad(d, base, off) => {
                    let (b2, o2) = mask(f, &mut insts, &mut pos, &mut cache, base, off);
                    emit(Inst::VLoad(d, b2, o2), &mut insts, &mut pos);
                }
                Inst::VStore(base, off, src) => {
                    let (b2, o2) = mask(f, &mut insts, &mut pos, &mut cache, base, off);
                    emit(Inst::VStore(b2, o2, src), &mut insts, &mut pos);
                }
                Inst::LoadX(..) | Inst::StoreX(..) => return Err(format!("{}: indexed address forms before the sandbox transform", f.name)),
                Inst::AtomicLoad(mm, o, d, a) => {
                    let (b2, _) = mask(f, &mut insts, &mut pos, &mut cache, a, 0);
                    emit(Inst::AtomicLoad(mm, o, d, b2), &mut insts, &mut pos);
                }
                Inst::AtomicStore(mm, o, a, src) => {
                    let (b2, _) = mask(f, &mut insts, &mut pos, &mut cache, a, 0);
                    emit(Inst::AtomicStore(mm, o, b2, src), &mut insts, &mut pos);
                }
                Inst::AtomicRmw(op, mm, o, d, a, src) => {
                    let (b2, _) = mask(f, &mut insts, &mut pos, &mut cache, a, 0);
                    emit(Inst::AtomicRmw(op, mm, o, d, b2, src), &mut insts, &mut pos);
                }
                Inst::AtomicCas(mm, o1, o2, d, a, e, nw) => {
                    let (b2, _) = mask(f, &mut insts, &mut pos, &mut cache, a, 0);
                    emit(Inst::AtomicCas(mm, o1, o2, d, b2, e, nw), &mut insts, &mut pos);
                }
                Inst::Copy(dst, src, len) => {
                    if len as u64 >= GUARD {
                        let n = f.vreg(Cls::I);
                        let r = f.vreg(Cls::I);
                        emit(Inst::Iconst(n, len as i64), &mut insts, &mut pos);
                        let s = u.syms.intern(SymKind::Rt("sb_memcpy"), sb_memcpy as *const () as u64);
                        emit(Inst::Call(Callee::Host(s), vec![dst, src, n], vec![r]), &mut insts, &mut pos);
                    } else {
                        let (d2, _) = mask(f, &mut insts, &mut pos, &mut cache, dst, 0);
                        let (s2, _) = mask(f, &mut insts, &mut pos, &mut cache, src, 0);
                        emit(Inst::Copy(d2, s2, len), &mut insts, &mut pos);
                    }
                }
                Inst::Call(callee, args, rets) => {
                    let callee = match callee {
                        Callee::Asm(_) => return Err(format!("{}: inline assembly, which sandboxed modules cannot run", f.name)),
                        Callee::Fn(id) => Callee::Fn(call_target(up, id, &sandboxed)?),
                        Callee::CFn(id, sig) => Callee::CFn(call_target(up, id, &sandboxed)?, sig),
                        Callee::Indirect(v) => Callee::Indirect(mask_fn(f, &mut insts, &mut pos, p, v)),
                        Callee::CIndirect(v, sig) => Callee::CIndirect(mask_fn(f, &mut insts, &mut pos, p, v), sig),
                        Callee::Host(s) => match u.syms.kind(s.id) {
                            Some(SymKind::Rt(name)) => match rt_policy(name) {
                                Some(None) => Callee::Host(s),
                                Some(Some((nn, addr))) => Callee::Host(u.syms.intern(SymKind::Rt(nn), addr)),
                                None => return Err(format!("{}: calls the runtime's `{}`, which sandboxed modules cannot use", f.name, name)),
                            },
                            other => return Err(format!("{}: calls {:?}, which sandboxed modules cannot use", f.name, other)),
                        },
                        Callee::CHost(s, sig) => {
                            let name = match u.syms.kind(s.id) {
                                Some(SymKind::Foreign(n)) => n,
                                Some(SymKind::Rt(n)) => n.to_string(),
                                other => return Err(format!("{}: calls {:?}, which sandboxed modules cannot use", f.name, other)),
                            };
                            match c_policy(&name) {
                                Some(Some(("abort", _))) => {
                                    let v = f.vreg(Cls::I);
                                    emit(Inst::Addr(v, site_abort), &mut insts, &mut pos);
                                    emit(Inst::Call(Callee::Host(panic_site), vec![v], Vec::new()), &mut insts, &mut pos);
                                    continue;
                                }
                                Some(Some((nn, addr))) => Callee::CHost(u.syms.intern(SymKind::Rt(nn), addr), sig),
                                Some(None) => Callee::CHost(s, sig),
                                None => return Err(format!("{}: reaches the C function `{}`, which sandboxed modules cannot call", f.name, name)),
                            }
                        }
                    };
                    emit(Inst::Call(callee, args, rets), &mut insts, &mut pos);
                    cache.clear();
                }
                // the FP control register is the host thread's (rounding, flush-to-zero): a module
                // may read it, not change it under the host
                Inst::FpEnv(true, _) => return Err(format!("{}: writes the floating-point control register (_mm_setcsr / set_fpcr), which sandboxed modules cannot", f.name)),
                other => emit(other, &mut insts, &mut pos),
            }
            for d in defs {
                cache.remove(&d.0);
            }
        }
        // a return gives the shadow frame back
        if has_frame {
            if let Term::Ret(_) = b.term {
                insts.push(Inst::Store(relax_backend::rir::Mem::Int(8, false), t_ssp, 0, ssp0));
                pos.push(b.term_pos);
            }
        }
        out_blocks.push(relax_backend::rir::Block { insts, pos, term: b.term, term_pos: b.term_pos });
    }
    let _ = nblocks;
    // assemble: block 0, the rewritten blocks, then the made ones
    f.blocks = Vec::with_capacity(out_blocks.len() + new_blocks.len() + 1);
    f.blocks.push(b0);
    f.blocks.extend(out_blocks);
    f.blocks.extend(new_blocks);
    f.slots.clear();
    add_fuel(f, ar, site_fuel, panic_site, pos0, &costs);
    with_sb(|s| s.refs.insert(twin_id, refs));
    f.verify().map_err(|e| format!("{}: sandbox transform made invalid RIR: {}", f.name, e))?;
    // the masks the transform added: common ones merged, loop-invariant ones hoisted. Only
    // under O1 alone (MAKEPAD_RUN_O3=0, O2 pipeline): with O3 on, O1 keeps
    // registers unoptimised and the hoisted masks cost more than they save (mini, particle
    // kernel: +25% with the clean-up, +0.5% without). Hot twins retain this confined RIR
    // for O3, whose full pipeline and register allocation perform the clean-up there.
    // MAKEPAD_RUN_SANDBOX_OPT=0/1 forces the O1 clean-up.
    let opt = options.sandbox_opt.unwrap_or(!jit::TIER1_ENABLED.load(std::sync::atomic::Ordering::Relaxed));
    if opt {
        relax_backend::passes::simplify(options, f, None, true, true);
        relax_backend::passes::licm(options, f);
        relax_backend::passes::simplify(options, f, None, true, true);
        f.verify().map_err(|e| format!("{}: sandbox clean-up made invalid RIR: {}", f.name, e))?;
    }
    Ok(())
}

/// An indirect call's target masked into the sandbox thunk table.
fn mask_fn(f: &mut Func, insts: &mut Vec<Inst>, pos: &mut Vec<u64>, p: u64, v: VReg) -> VReg {
    let (base, ts) = with_sb(|s| (s.sbt_base, jit::thunk_size() as u64));
    let span = SBT_N as u64 * ts;
    let b = f.vreg(Cls::I);
    let t1 = f.vreg(Cls::I);
    let t2 = f.vreg(Cls::I);
    let t3 = f.vreg(Cls::I);
    insts.push(Inst::Iconst(b, base as i64));
    insts.push(Inst::IBin(IOp::Sub, U64, t1, v, b));
    insts.push(Inst::IBinI(IOp::And, U64, t2, t1, ((span - 1) & !(ts - 1)) as i64));
    insts.push(Inst::IBin(IOp::Add, U64, t3, t2, b));
    for _ in 0..4 {
        pos.push(p);
    }
    t3
}

/// What a call to slot `id` from a twin goes to: the host's own function for a host API
/// crate (non-generic only), else the callee's twin.
unsafe fn call_target(up: *mut Unit, id: u32, sandboxed: &HashSet<u32>) -> Result<u32, String> {
    let u = &*up;
    match &u.fns[id as usize].key {
        FnKey::Inst(d, args) => {
            let def = u.prog.def(*d);
            if def.kind == DefKind::ForeignFn {
                // a foreign fn reached through its slot: only allow-listed math
                let name = u.prog.syms.str(def.name).to_string();
                return match c_policy(&name) {
                    Some(None) => Ok(id),
                    _ => Err(format!("reaches the C function `{}`, which sandboxed modules cannot call", name)),
                };
            }
            if !sandboxed.contains(&def.krate) {
                if !args.is_empty() {
                    return Err(format!("calls the generic host API fn `{}`: API fns called by modules must be concrete", u.prog.def_path(*d)));
                }
                return Ok(id);
            }
            Ok(twin(up, id))
        }
        FnKey::Sandbox(_) => Ok(id),
        _ => Ok(twin(up, id)),
    }
}

/// A fn value in a twin: the twin's sandbox thunk (host API fns cannot be taken as values).
unsafe fn fn_target(up: *mut Unit, id: u32, sandboxed: &HashSet<u32>) -> Result<u32, String> {
    let u = &*up;
    if let FnKey::Inst(d, _) = &u.fns[id as usize].key {
        let def = u.prog.def(*d);
        if def.kind == DefKind::ForeignFn || !sandboxed.contains(&def.krate) {
            return Err(format!("takes `{}` as a fn value: host and C functions cannot be fn values in a sandboxed module", u.prog.def_path(*d)));
        }
    }
    Ok(twin(up, id))
}

/// Charge each original block before executing it. Metering blocks and confinement
/// instructions have no charge. Stores/checks remain observable in both tiers.
fn add_fuel(f: &mut Func, arena: VReg, site: Sym, panic_site: Sym, pos0: u64, costs: &[i64]) {
    let trap_ix = f.blocks.len() as u32;
    let tv = f.vreg(Cls::I);
    f.blocks.push(relax_backend::rir::Block { insts: vec![Inst::Addr(tv, site), Inst::Call(Callee::Host(panic_site), vec![tv], Vec::new())], pos: vec![pos0, pos0], term: Term::Unreachable, term_pos: pos0 });
    for (k, &cost) in costs.iter().enumerate() {
        let b = k + 1; // block zero is the confinement prologue
        let t = f.vreg(Cls::I);
        let v = f.vreg(Cls::I);
        let v2 = f.vreg(Cls::I);
        let c = f.vreg(Cls::I);
        let cont_ix = f.blocks.len() as u32;
        let insts = vec![
            Inst::IBinI(IOp::Add, U64, t, arena, FUEL_OFF as i64),
            Inst::Load(relax_backend::rir::Mem::Int(8, true), v, t, 0),
            Inst::IBinI(IOp::Sub, U64, v2, v, cost),
            Inst::Store(relax_backend::rir::Mem::Int(8, true), t, 0, v2),
            Inst::ICmpI(Cond::Lt, true, c, v2, 0),
        ];
        let meter = relax_backend::rir::Block { pos: vec![pos0; insts.len()], insts, term: Term::Branch(c, trap_ix, cont_ix), term_pos: pos0 };
        let old = std::mem::replace(&mut f.blocks[b], meter);
        f.blocks.push(old);
    }
}

/// The code of a twin (on the owner thread): the base slot's RIR, transformed.
pub unsafe fn twin_rir(up: *mut Unit, id: u32, base: u32) -> Result<Func, String> {
    if base == u32::MAX {
        return Err("a sandboxed module called an invalid function pointer".to_string());
    }
    {
        let u = &*up;
        if let FnKey::Inst(d, _) = &u.fns[base as usize].key {
            if u.prog.def(*d).kind == DefKind::ForeignFn {
                return Err(format!("a sandboxed module reached the C function `{}`", u.prog.def_path(*d)));
            }
        }
    }
    // Keep base-slot dependency/type-check metadata used by reload invalidation.
    // Metering itself never uses that tier-dependent optimized body.
    jit::build_rir(up, base)?;
    let mut f = jit::sandbox_source_rir(up, base)?;
    transform(up, &mut f, id)?;
    // MAKEPAD_RUN_SANDBOX_DUMP=<substring>: the transformed RIR of matching twins
    if let Ok(pat) = std::env::var("MAKEPAD_RUN_SANDBOX_DUMP") {
        if f.name.contains(&pat) {
            eprintln!("sandbox twin {} ({} insts):\n{}", f.name, relax_codegen::opt::inst_count(&f), f.dump());
        }
    }
    Ok(f)
}

/// Rewrites a wrapper (host code)'s calls into sandboxed crates to their twins.
pub unsafe fn redirect_wrapper(up: *mut Unit, f: &mut Func) {
    for b in f.blocks.iter_mut() {
        for i in b.insts.iter_mut() {
            if let Inst::Call(Callee::Fn(id), _, _) = i {
                let u = &*up;
                if is_sandboxed_slot(u, *id) {
                    *id = twin(up, *id);
                }
            }
        }
    }
}

/// Bytes of image (read-only, read-write) and arenas live (memory report).
pub fn stats() -> (usize, usize, usize) {
    let g = SB.lock().unwrap_or_else(|e| e.into_inner());
    g.as_ref().map_or((0, 0, 0), |s| (s.ro.len(), s.rw.len(), s.arenas.len()))
}

/// Heap bytes a module's arena has handed out.
pub fn heap_used(a: &Arena) -> u64 {
    a.heap.lock().unwrap_or_else(|e| e.into_inner()).used
}
