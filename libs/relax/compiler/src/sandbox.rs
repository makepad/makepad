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
//! - in a domain that meters fuel (`Domain::fuel`), pre-optimization RIR blocks spend
//!   deterministic instruction fuel at entry.
//!
//! Host code reaches a module only through generated wrapper functions (modules.rs) that enter
//! the module's arena, copy reference arguments into it, call the twin and leave. Plain-data
//! aggregate results also use an arena slot (lower.rs), copied out before leaving: a twin's
//! sret stores must never receive a host destination.

include!("rt_abi.rs");

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
/// per-thread segments [SEGS, SEGS + NSEG * SEG): one per thread inside the module (claimed at its
/// first entry, given back when its last frame there leaves; mapped on first use; a thread that
/// finds all in use waits for one). Each is a guard (never mapped), the thread's scratch for
/// runtime results the module reads (formatted text), its thread-locals, then its shadow stack,
/// growing down. Threads never share shadow frames, wrapper argument copies, result slots or
/// thread-locals (MEMRECLAIM.REVIEW F4).
const SEGS: u32 = 128 << 20;
const SEG: u32 = 8 << 20;
const NSEG: u32 = 64;
const SEG_GUARD: u32 = 64 << 10;
const SCRATCH_LEN: u32 = 1 << 20;
/// a segment's thread-locals: [SEG_GUARD + SCRATCH_LEN, STACK_LO), laid out per unit (`tls_offset`),
/// zero when the segment is claimed. Its first two words stand for the thread block's (std's
/// LocalKey reads them): no domain, no runtime record. The third is the thread's errno (sb_errno).
const TLS_LEN: u32 = 1 << 20;
const TLS_ERRNO: u32 = 16;
/// a segment's shadow stack: [STACK_LO, SEG)
const STACK_LO: u32 = SEG_GUARD + SCRATCH_LEN + TLS_LEN;
const HEAP_BASE: u32 = SEGS + NSEG * SEG;
const HEAP_END: u32 = 0xffff_0000;
/// sandbox thunks: one per twin taken as a fn value, so never more than the unit has slots; the
/// table reserves that many (the unit's code area holds every slot's own thunk: its size over a
/// thunk's, to a power of two, which an indirect call is masked into), held, and written a chunk
/// at a time (trap thunks first) as fn values reach it
const SBT_CHUNK: u32 = 1 << 12;

/// Offsets of the sandbox words in each thread's Relax TLS block (relative to `Sb::tls`):
/// the current arena base, the shadow stack pointer, the fuel at entry (budget accounting), the
/// host arena state, the fuel left (the twins' meter: per thread, out of the module's reach), the
/// shadow stack's lower bound.
const W_ARENA: i64 = 0;
const W_SSP: i64 = 8;
const W_FUEL: i64 = 32;
const W_SSLO: i64 = 40;
/// sandbox words per thread (`Saved`)
const NWORDS: usize = 6;

/// One module's arena, as the host keeps it (the module never sees this).
pub struct Arena {
    pub base: u64,
    pub module: u32,
    /// the module's name (fault reports)
    pub name: String,
    heap: Mutex<Heap>,
    /// image bytes / relocations already copied into this window
    synced: Mutex<(u32, u32, usize, usize)>,
    /// fuel per call (0: none)
    pub fuel: i64,
    pub alive: std::sync::atomic::AtomicBool,
    /// per-thread segments in use / ever mapped (bit k: segment k)
    segs: std::sync::atomic::AtomicU64,
    segs_mapped: std::sync::atomic::AtomicU64,
    /// the thunk table of its unit's twins and its span in bytes (catch_unwind masks a target
    /// into it: no unit there)
    sbt_base: u64,
    sbt_span: u64,
    /// the bytes of a segment's thread-local area in use (its unit's layout): zeroed at a claim
    tls_used: Arc<std::sync::atomic::AtomicU32>,
    /// std's runtime keys of its code (`relax_key_*`) and each segment's values; the segments
    /// that have a value (bit k: segment k; a call that sets none never takes the lock)
    keys: Mutex<Keys>,
    keyed: std::sync::atomic::AtomicU64,
    /// a thread has waited for a segment (said once per arena)
    waited: std::sync::atomic::AtomicBool,
}

/// The thread-local keys an arena's code made (std's LocalKey with lazy init or drop glue).
#[derive(Default)]
struct Keys {
    /// key -> (its destructor, a sandbox thunk address; late: the value has no drop glue)
    made: Vec<(u64, bool)>,
    /// segment -> (key -> value, the keys in the order first set): a claim's values
    vals: Vec<(Vec<u64>, Vec<u32>)>,
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

/// One unit's sandbox: the confinement domain of the code that unit runs confined -- its twins,
/// their thunk table, the data image, the arenas and what its code may call beyond memory and
/// math (`allow`). Made on the unit's first confined crate (`init`). A macro world is a unit of
/// its own and the program's unit may host modules at the same time: slot ids, crate indexes
/// and symbols are a unit's, so nothing here is shared between units. The thread words
/// (TLS_OFF, ENTRIES, ACCOUNTS) are every unit's: a thread is in one arena at a time.
pub struct State {
    /// `sb` is made: one atomic load on every compile's path of a unit with no sandbox
    ready: std::sync::atomic::AtomicBool,
    sb: Mutex<Option<Box<Sb>>>,
}

impl Default for State {
    fn default() -> State {
        State { ready: std::sync::atomic::AtomicBool::new(false), sb: Mutex::new(None) }
    }
}

impl State {
    /// A retired unit's code goes (Unit's drop): its thunk table with it.
    pub(crate) fn release(&mut self) {
        if let Some(s) = self.sb.get_mut().unwrap_or_else(|e| e.into_inner()).take() {
            crate::os::mem::release(s.sbt_base, s.sbt_span as usize, 0);
        }
    }
}

/// What a confinement domain's code may call beyond what every domain may (`c_policy`): the C
/// function `name` kept (Some(None)) or replaced by the domain's version (Some(Some((symbol
/// name, address)))); None: not this domain's. Run-time modules add nothing (`MODULES`).
pub type Allow = fn(&str) -> Option<Option<(&'static str, u64)>>;

/// A confinement domain: its name, what its code may call beyond what every domain may, and
/// whether its code meters fuel. The name is part of its twins' cache keys: code one domain's
/// policy made is never linked into another domain's unit (cache.rs enc_fnkey); `allow` and
/// `fuel` are fixed per domain, so the name covers them.
#[derive(Clone, Copy)]
pub struct Domain {
    pub name: &'static str,
    pub allow: Allow,
    /// its twins charge each block's instructions to the thread's fuel word (`add_fuel`): an
    /// arena's per-call budget (`Arena::fuel`) can only end a call in a domain that meters
    pub fuel: bool,
}

/// The run-time modules' domain: what every domain may call, nothing more; metered (a module's
/// arena may set a per-call budget).
pub const MODULES: Domain = Domain { name: "modules", allow: modules_allow, fuel: true };

fn modules_allow(_: &str) -> Option<Option<(&'static str, u64)>> {
    None
}

struct Sb {
    /// the symbol TlsAddr reads the sandbox words' offset from (this unit's; the offset is TLS_OFF)
    tls_sym: Sym,
    /// the domain's name, own C entries and whether its twins meter fuel
    domain: &'static str,
    allow: Allow,
    fuel: bool,
    sbt_base: u64,
    /// the table's bytes (a power of two: the indirect-call mask)
    sbt_span: u64,
    /// thunks given out (index 0 is the trap) / written (the rest of the reservation is held)
    sbt_n: u32,
    sbt_filled: u32,
    /// twin -> its thunk's index (only twins taken as fn values have one)
    sbt_of: HashMap<u32, u32>,
    /// the thunk every unused index holds: a jump through the trap slot
    trap: Vec<u8>,
    /// thread-locals of the twins: symbol -> offset in a segment's thread-local area; the bytes
    /// in use (shared with the arenas, which zero them when a segment is claimed)
    tls: HashMap<u32, u32>,
    tls_used: Arc<std::sync::atomic::AtomicU32>,
    /// base slot -> twin slot
    twins: HashMap<u32, u32>,
    /// crates whose code always runs as twins (std's and the modules')
    sandboxed: HashSet<u32>,
    /// of those, the crates whose code is host code too (both forms: std's, and in a macro world
    /// the crates a host-run macro uses as well); the rest are only ever twins
    host_too: HashSet<u32>,
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
    /// host API fns checked at the boundary ((def, collect epoch) -> the refusal, None: allowed)
    api_checked: HashMap<(u32, u64), Option<String>>,
}


/// The TLS block offset of the sandbox words + 1 (0: no unit has a sandbox yet). One offset for
/// every unit: the thread block's bytes are allocated across units (`Unit::tls_alloc`).
static TLS_OFF: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
/// held while the sandbox words are placed in the thread block (the first unit's `init`)
static TLS_PLACING: Mutex<()> = Mutex::new(());
/// unloaded arenas: tombstones, never freed (free_arena), whichever unit they were made in
static DEAD: Mutex<Vec<Arc<Arena>>> = Mutex::new(Vec::new());
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
#[cfg(target_vendor = "apple")]
const MAP_ANON_PRIV: i32 = 0x1002;

fn map_rw(addr: u64, len: u64) -> bool {
    unsafe { mprotect(addr as *mut u8, len as usize, 3) == 0 }
}

/// Sets up unit `up`'s sandbox on first use (on the owner thread), in `domain`. A unit has one
/// domain: a second call keeps the first's.
unsafe fn sb_init(up: *mut Unit, domain: Domain) -> Result<(), String> {
    let u = &mut *up;
    if u.sandbox.ready.load(std::sync::atomic::Ordering::Acquire) {
        return Ok(());
    }
    if cfg!(windows) {
        // (no arena windows: the reservation is carved with partial munmaps, which VirtualFree
        // cannot do)
        return Err("sandboxed code is not available on Windows yet".to_string());
    }
    let tls = {
        let _placing = TLS_PLACING.lock().unwrap_or_else(|e| e.into_inner());
        match TLS_OFF.load(std::sync::atomic::Ordering::Acquire) {
            0 => {
                let tls = u.tls_alloc(8 * NWORDS as u32, 8);
                TLS_OFF.store(tls + 1, std::sync::atomic::Ordering::Release);
                tls
            }
            t => {
                // (wasm export sizes its thread block by tls_next)
                u.tls_next = u.tls_next.max(t - 1 + 8 * NWORDS as u32);
                t - 1
            }
        }
    };
    let tls_sym = u.syms.intern(SymKind::Rt("sb_tls"), tls as u64);
    let ts = jit::thunk_size() as u64;
    let sbt_span = (u.exec_size / ts).next_power_of_two() * ts;
    let sbt_base = crate::os::mem::reserve(sbt_span as usize, 0);
    // held until a chunk is written: zero bytes would run as code (x86: `add [rax], al`). (No
    // twin exists yet, so no code can call into the table before this.)
    #[cfg(unix)]
    if !crate::os::mem::code_held(sbt_base, sbt_span as usize, true) {
        crate::os::mem::release(sbt_base, sbt_span as usize, 0);
        return Err("cannot reserve the sandbox thunk table".to_string());
    }
    // the slot every unused sandbox thunk jumps through: its code panics with the reason
    let trap_slot = u.fn_id(FnKey::Sandbox(u32::MAX));
    let trap = tgt::thunk(u.slots.table(trap_slot));
    *u.sandbox.sb.lock().unwrap_or_else(|e| e.into_inner()) = Some(Box::new(Sb {
        tls_sym,
        domain: domain.name,
        allow: domain.allow,
        fuel: domain.fuel,
        sbt_base,
        sbt_span,
        sbt_n: 1,
        sbt_filled: 0,
        sbt_of: HashMap::new(),
        trap,
        tls: HashMap::new(),
        // (words 0 and 1: the thread block's two words)
        tls_used: Arc::new(std::sync::atomic::AtomicU32::new(TLS_ERRNO + 8)),
        twins: HashMap::new(),
        sandboxed: HashSet::new(),
        host_too: HashSet::new(),
        wrappers: HashSet::new(),
        placed: HashMap::new(),
        ro: Vec::new(),
        rw: Vec::new(),
        rel: Vec::new(),
        abs: Vec::new(),
        arenas: Vec::new(),
        refs: HashMap::new(),
        api_checked: HashMap::new(),
    }));
    u.sandbox.ready.store(true, std::sync::atomic::Ordering::Release);
    Ok(())
}

fn with_sb<R>(u: &Unit, f: impl FnOnce(&mut Sb) -> R) -> R {
    let mut g = u.sandbox.sb.lock().unwrap_or_else(|e| e.into_inner());
    f(g.as_mut().expect("sandbox not initialised"))
}

/// Marks crates of unit `up` whose code runs only as twins (a module's crate; std's are added
/// with the first), in the run-time modules' domain.
pub unsafe fn add_sandboxed_crate(up: *mut Unit, krate: u32) -> Result<(), String> {
    sb_init(up, MODULES)?;
    let u = &*up;
    let std_crates: Vec<u32> = (0..u.prog.crates.len() as u32).filter(|&c| matches!(u.prog.syms.str(u.prog.crates[c as usize].name), "core" | "alloc" | "std")).collect();
    with_sb(u, |s| {
        s.sandboxed.insert(krate);
        s.sandboxed.extend(std_crates.iter().copied());
        s.host_too.extend(std_crates);
    });
    Ok(())
}

/// Unit `up` becomes confinement domain `domain`: every crate of it runs as twins when reached
/// from twins (nothing in it is a host API), and `host_too` are host code as well.
pub unsafe fn confine_unit(up: *mut Unit, domain: Domain, host_too: &[u32]) -> Result<(), String> {
    sb_init(up, domain)?;
    let u = &*up;
    let all = 0..u.prog.crates.len() as u32;
    with_sb(u, |s| {
        s.sandboxed.extend(all);
        s.host_too.extend(host_too.iter().copied());
    });
    Ok(())
}

/// The address twin code of unit `up` sees for slot `base` as a fn value: its twin's sandbox
/// thunk (on the owner thread).
pub unsafe fn fn_value(up: *mut Unit, base: u32) -> Result<u64, String> {
    let t = twin(up, base);
    sbthunk(up, t)
}

pub fn add_wrapper_crate(u: &Unit, krate: u32) {
    with_sb(u, |s| {
        s.wrappers.insert(krate);
    });
}

/// Twin code bytes installed so far (append-only: every version stays mapped).
pub fn twin_code_bytes() -> u64 {
    jit::CODE_GROWTH[jit::GROW_MODULE].load(std::sync::atomic::Ordering::Relaxed)
}

/// Whether slot `id` holds code of a sandboxed crate (never compiled or inlined as host code).
pub fn is_sandboxed_slot(u: &Unit, id: u32) -> bool {
    // no module loaded yet: one atomic load (these sit on every compile's path)
    if !u.sandbox.ready.load(std::sync::atomic::Ordering::Acquire) {
        return false;
    }
    let g = u.sandbox.sb.lock().unwrap_or_else(|e| e.into_inner());
    let Some(s) = g.as_ref() else { return false };
    // a lowering ahead's temporary callee identity (descending from u32::MAX) names its key there
    let ahead = crate::jit::lower_ahead_fn_key(id);
    let key = match &ahead {
        Some(k) => k,
        None if (id as usize) < u.fns.len() => &u.fns[id as usize].key,
        None => return false,
    };
    let k = match key {
        FnKey::Inst(d, _) => u.prog.def(*d).krate,
        FnKey::Closure(f, _, _) => match u.prog.file_crate.get(*f as usize) {
            Some(k) => *k,
            None => return false,
        },
        _ => return false,
    };
    // std code is host code too: only module crates are exclusively sandboxed
    s.sandboxed.contains(&k) && !s.host_too.contains(&k)
}

/// As is_sandboxed_slot, from a key (the inliner's view): `sb` and `prog` are one unit's.
pub fn is_sandboxed_crate_key(sb: &State, prog: &crate::program::Program, key: &FnKey) -> bool {
    // no module loaded yet: one atomic load (these sit on every compile's path)
    if !sb.ready.load(std::sync::atomic::Ordering::Acquire) {
        return false;
    }
    let g = sb.sb.lock().unwrap_or_else(|e| e.into_inner());
    let Some(s) = g.as_ref() else { return false };
    let k = match key {
        FnKey::Inst(d, _) => prog.def(*d).krate,
        FnKey::Closure(f, _, _) => match prog.file_crate.get(*f as usize) {
            Some(k) => *k,
            None => return false,
        },
        _ => return false,
    };
    s.sandboxed.contains(&k) && !s.host_too.contains(&k)
}

/// Whether host code of unit `u`'s crate `krate` is a generated wrapper (its calls into modules
/// go to twins).
pub fn is_wrapper_crate(u: &Unit, krate: u32) -> bool {
    // no module loaded yet: one atomic load (these sit on every compile's path)
    if !u.sandbox.ready.load(std::sync::atomic::Ordering::Acquire) {
        return false;
    }
    let g = u.sandbox.sb.lock().unwrap_or_else(|e| e.into_inner());
    g.as_ref().map_or(false, |s| s.wrappers.contains(&krate))
}

/// The twin of slot `base` (created on first use; its sandbox thunk when it is taken as a value).
pub unsafe fn twin(up: *mut Unit, base: u32) -> u32 {
    if let Some(t) = with_sb(&*up, |s| s.twins.get(&base).copied()) {
        return t;
    }
    let u = &mut *up;
    let t = u.fn_id(FnKey::Sandbox(base));
    with_sb(u, |s| s.twins.insert(base, t));
    t
}

/// The twin of unit `u`'s slot `base`, if it has one.
pub fn twin_of(u: &Unit, base: u32) -> Option<u32> {
    // no module loaded yet: one atomic load (these sit on every compile's path)
    if !u.sandbox.ready.load(std::sync::atomic::Ordering::Acquire) {
        return None;
    }
    let g = u.sandbox.sb.lock().unwrap_or_else(|e| e.into_inner());
    g.as_ref().and_then(|s| s.twins.get(&base).copied())
}

/// The sandbox thunk address of twin `t` (a fn pointer value in sandboxed code), given out on its
/// first use as a value: the table's next chunk is written (trap thunks) when it is reached, and
/// becomes executable only then.
unsafe fn sbthunk(up: *mut Unit, t: u32) -> Result<u64, String> {
    let u = &*up;
    let ts = jit::thunk_size() as u64;
    let table = u.slots.table(t);
    with_sb(u, |s| {
        if let Some(&k) = s.sbt_of.get(&t) {
            return Ok(s.sbt_base + k as u64 * ts);
        }
        let k = s.sbt_n;
        // (a twin is a slot, and the table has room for every slot the unit's code area can hold)
        if k as u64 * ts >= s.sbt_span {
            return Err(format!("the sandbox thunk table has more entries than the unit has slots ({})", k));
        }
        while k >= s.sbt_filled {
            let at = s.sbt_base + s.sbt_filled as u64 * ts;
            let len = (SBT_CHUNK as u64 * ts).min(s.sbt_span - s.sbt_filled as u64 * ts) as usize;
            let trap = s.trap.clone();
            let fill = |b: &mut [u8]| {
                for c in b.chunks_mut(trap.len()) {
                    c.copy_from_slice(&trap);
                }
            };
            #[cfg(unix)]
            if !crate::os::mem::code_unhold_filled(at, len, fill) {
                return Err("cannot map more of the sandbox thunk table".to_string());
            }
            #[cfg(not(unix))]
            crate::os::mem::fill_code(at, len, fill);
            s.sbt_filled += (len as u64 / ts) as u32;
        }
        s.sbt_n += 1;
        s.sbt_of.insert(t, k);
        let at = s.sbt_base + k as u64 * ts;
        crate::os::mem::write_code(at, &tgt::thunk(table));
        Ok(at)
    })
}

/// Slots twin `t` of unit `u` refers to without a relocation (fn pointer constants).
pub fn extra_refs(u: &Unit, t: u32) -> Vec<u32> {
    let g = u.sandbox.sb.lock().unwrap_or_else(|e| e.into_inner());
    g.as_ref().and_then(|s| s.refs.get(&t).cloned()).unwrap_or_default()
}

// ------------------------------------------------------------------ twin code's symbols

// What twin code holds of this run (thunk addresses, data and thread-local offsets, the thunk
// table, the domain's host entries) it names by these symbols, never as immediates: the disk
// cache stores a twin's code like any other and its link resolves them again here, placing the
// data, thread-locals and thunks the code needs as the transform did (cache.rs link_symbol).

/// Whether unit `u` has a sandbox (a twin's key decodes and its symbols link only then).
pub fn ready(u: &Unit) -> bool {
    u.sandbox.ready.load(std::sync::atomic::Ordering::Acquire)
}

/// Unit `u`'s confinement domain (its sandbox must be set up).
pub(crate) fn domain(u: &Unit) -> &'static str {
    with_sb(u, |s| s.domain)
}

/// The symbol of twin `t`'s sandbox thunk (given out on first use).
pub(crate) unsafe fn thunk_sym(up: *mut Unit, t: u32) -> Result<Sym, String> {
    let a = sbthunk(up, t)?;
    Ok((&*up).syms.intern(SymKind::SbThunk(t), a))
}

/// The symbol of the image offset of the data symbol `sym` names (placed on first use).
pub(crate) unsafe fn place_sym(up: *mut Unit, sym: u32) -> Result<Sym, String> {
    let o = place(up, sym, 0)?;
    Ok((&*up).syms.intern(SymKind::SbPlace(sym), o as u64))
}

/// The symbol of the segment thread-local offset of thread-local symbol `sym` (laid out on first use).
pub(crate) unsafe fn tls_offset_sym(up: *mut Unit, sym: u32) -> Result<Sym, String> {
    let o = tls_offset(up, Sym::new(sym))?;
    Ok((&*up).syms.intern(SymKind::SbTls(sym), o as u64))
}

/// The symbol of unit `u`'s sandbox thunk table.
pub(crate) fn table_sym(u: &Unit) -> Sym {
    let base = with_sb(u, |s| s.sbt_base);
    u.syms.intern(SymKind::SbTable, base)
}

/// The symbol of the version of host entry `name` (`c:<C name>`, `rt:<runtime entry>`) unit `u`'s
/// domain gives its twins in place of it. None: the domain keeps none (the entry itself, or no
/// call at all).
pub(crate) fn host_sym(u: &Unit, name: &str) -> Option<Sym> {
    let p = match name.split_once(':')? {
        ("c", n) => c_policy(with_sb(u, |s| s.allow), n),
        ("rt", n) => rt_policy(n),
        _ => None,
    };
    match p {
        Some(Some((nn, addr))) if nn != "abort" => Some(u.syms.intern(SymKind::SbHost(name.to_string()), addr)),
        _ => None,
    }
}

// ------------------------------------------------------------------ the data image

/// The window offset of the data a symbol names (placed, with what it points to, on first use).
unsafe fn place(up: *mut Unit, sym: u32, depth: u32) -> Result<u32, String> {
    if let Some(o) = with_sb(&*up, |s| s.placed.get(&sym).copied()) {
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
    let off = with_sb(u, |s| {
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
                let v = (sbthunk(up, tw)? as i64 + t.off) as u64;
                with_sb(&*up, |s| s.abs.push((pos, v)));
                with_sb(&*up, |s| s.refs.entry(u32::MAX).or_default().push(tw));
            }
            Some(SymKind::Data(_)) | Some(SymKind::Static(_)) | Some(SymKind::Rw(_)) => {
                let to = place(up, t.id, depth + 1)?;
                with_sb(&*up, |s| s.rel.push((pos, (to as i64 + t.off) as u32)));
            }
            Some(SymKind::Site(_)) => {}
            Some(SymKind::Rt("null")) => {}
            other => return Err(format!("data `{}` points to {:?}, which a sandboxed module cannot reach", crate::syms::describe(&*up, Sym::new(sym)), other)),
        }
    }
    sync_all(&*up);
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
    let pw = u.ptr_bytes() as usize;
    let mut i = 0;
    while i + pw <= b.len() {
        let w = crate::syms::word_in(u, b, i);
        if (if pw == 4 { w & 0xffff_ffff } else { w }) > 0x10000 {
            if let Some(s) = u.syms.symbol_at(w, u) {
                out.push((i as u32, s));
            }
        }
        i += pw;
    }
    out
}

/// Copies what unit `u`'s image gained since into each of its arenas.
fn sync_all(u: &Unit) {
    let g = u.sandbox.sb.lock().unwrap_or_else(|e| e.into_inner());
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

/// Does unit `u` have a sandbox, or does any sandbox module live?
pub fn any_live(u: &Unit) -> bool {
    u.sandbox.ready.load(std::sync::atomic::Ordering::Acquire) || ARENAS.iter().any(|a| !a.load(std::sync::atomic::Ordering::Acquire).is_null())
}

/// Creates an arena in unit `up`'s sandbox (`mem` bytes of heap budget) for module `module`
/// (registered for `enter_module` when it is below MAX_MODULES).
pub fn new_arena(up: *mut Unit, module: u32, name: &str, mem: u64, fuel: i64) -> Result<Arc<Arena>, String> {
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
        if !map_rw(base + RO_BASE as u64, (RW_END - RO_BASE) as u64) {
            munmap(lo as *mut u8, (hi - lo) as usize);
            return Err("cannot map an arena".to_string());
        }
        let u = &*up;
        let mut g = u.sandbox.sb.lock().unwrap_or_else(|e| e.into_inner());
        let Some(s) = g.as_mut() else {
            munmap(lo as *mut u8, (hi - lo) as usize);
            return Err("sandbox not initialised".to_string());
        };
        let a = Arc::new(Arena {
            base,
            module,
            name: name.to_string(),
            heap: Mutex::new(Heap { top: HEAP_BASE, mapped: HEAP_BASE, budget: mem, used: 0, free: vec![Vec::new(); 17], large_free: Vec::new(), sizes: HashMap::new() }),
            synced: Mutex::new((0, 0, 0, 0)),
            fuel: if u.prog.session.options.sandbox_fuel { fuel } else { 0 },
            alive: std::sync::atomic::AtomicBool::new(true),
            segs: std::sync::atomic::AtomicU64::new(0),
            segs_mapped: std::sync::atomic::AtomicU64::new(0),
            sbt_base: s.sbt_base,
            sbt_span: s.sbt_span,
            tls_used: s.tls_used.clone(),
            keys: Mutex::new(Keys::default()),
            keyed: std::sync::atomic::AtomicU64::new(0),
            waited: std::sync::atomic::AtomicBool::new(false),
        });
        sync_arena(s, &a);
        s.arenas.push(a.clone());
        if (module as usize) < MAX_MODULES {
            ARENAS[module as usize].store(Arc::as_ptr(&a) as *mut Arena, std::sync::atomic::Ordering::SeqCst);
        }
        Ok(a)
    }
}

/// Unloads a module's arena. Its address range is never used again (proving that no stale
/// pointer into it is left -- a parked thread's frames, a host copy of a result pointer -- is not
/// tractable): its pages go back to the OS and the range stays reserved, PROT_NONE, for the life
/// of the process, so a late access traps and can never reach memory a later allocation got. Such
/// a fault is never contained: the thread parks, the app pauses and the user decides (stale.rs).
/// The `Arena` stays too, as a tombstone that refuses entry: a pointer to it (the module index, a
/// thread's sandbox words) never dangles.
pub fn free_arena(u: &Unit, a: &Arc<Arena>) {
    let mut g = u.sandbox.sb.lock().unwrap_or_else(|e| e.into_inner());
    a.alive.store(false, std::sync::atomic::Ordering::SeqCst);
    if (a.module as usize) < MAX_MODULES {
        let _ = ARENAS[a.module as usize].compare_exchange(Arc::as_ptr(a) as *mut Arena, std::ptr::null_mut(), std::sync::atomic::Ordering::SeqCst, std::sync::atomic::Ordering::SeqCst);
    }
    // registered before the range turns inaccessible: a fault there finds its module
    let why = if (a.module as usize) < MAX_MODULES { format!("Module::unload of module #{} (`{}`)", a.module, a.name) } else { format!("{} was freed", a.name) };
    crate::stale::register(a.base - GUARD, a.base + WINDOW + GUARD, "unloaded module", a.name.clone(), a.module as u64, format!("{}: its window is reserved and inaccessible for good", why));
    // the window's pages go, its addresses stay taken
    if !retire_window(a.base - GUARD, WINDOW + 2 * GUARD) {
        eprintln!("relax: an unloaded module's memory at {:#x} could not be made inaccessible", a.base);
        std::process::abort();
    }
    // (the heap's bookkeeping is the only sizeable part of the tombstone)
    *a.heap.lock().unwrap_or_else(|e| e.into_inner()) = Heap { top: HEAP_END, mapped: HEAP_END, budget: 0, used: 0, free: Vec::new(), large_free: Vec::new(), sizes: HashMap::new() };
    if let Some(s) = g.as_mut() {
        let had = s.arenas.len();
        s.arenas.retain(|x| !Arc::ptr_eq(x, a));
        if s.arenas.len() != had {
            DEAD.lock().unwrap_or_else(|e| e.into_inner()).push(a.clone());
        }
    }
}

/// [at, at + len) gives its pages back and stays reserved, inaccessible: one mmap over it
/// (MAP_FIXED replaces the mapping in one step, NORESERVE: no commit charge).
fn retire_window(at: u64, len: u64) -> bool {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    const MAP_FIXED_NORESERVE: i32 = 0x10 | 0x4000;
    #[cfg(target_vendor = "apple")]
    const MAP_FIXED_NORESERVE: i32 = 0x10 | 0x40;
    #[cfg(windows)]
    const MAP_FIXED_NORESERVE: i32 = 0;
    unsafe { mmap(at as *mut u8, len as usize, 0, MAP_ANON_PRIV | MAP_FIXED_NORESERVE, -1, 0) as u64 == at }
}

/// What lies at `off` in a module's (window + guards) range: names the data a stale pointer
/// pointed to (stale.rs).
pub fn describe_window_offset(off: u64) -> String {
    if off < GUARD || off >= GUARD + WINDOW {
        return "a guard page next to the window".to_string();
    }
    let o = (off - GUARD) as u32;
    if o < RO_BASE {
        "the null page".to_string()
    } else if o < RO_END {
        format!("read-only data image +{:#x}", o - RO_BASE)
    } else if o < RW_END {
        format!("statics image +{:#x}", o - RW_BASE)
    } else if o < SEGS + NSEG * SEG {
        let k = (o - SEGS) / SEG;
        let w = (o - SEGS) % SEG;
        let part = if w < SEG_GUARD { "guard" } else if w < SEG_GUARD + SCRATCH_LEN { "runtime scratch" } else if w < STACK_LO { "thread-locals" } else { "shadow stack" };
        format!("thread segment {} {} +{:#x}", k, part, w)
    } else {
        format!("heap +{:#x}", o - HEAP_BASE)
    }
}

/// Unloaded modules' windows (reserved, inaccessible, never reused) (memory report, tests).
pub fn dead_arenas() -> usize {
    DEAD.lock().unwrap_or_else(|e| e.into_inner()).len()
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

/// The state a guarded call or a wrapper restores (the sandbox words, W_*).
#[derive(Clone, Copy)]
pub struct Saved(pub [i64; NWORDS]);

struct Account {
    base: u64,
    out: std::rc::Rc<std::cell::Cell<[u64; 4]>>,
    // remaining fuel, allocation limit, consumed growth, error (1 fuel / 2 alloc)
    data: [u64; 4],
}

thread_local! {
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
            let left = *w.add(4);
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
            *w.add(4) = left;
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

/// This thread's wrapper entries in progress.
pub fn entry_depth() -> u64 { ENTRIES.try_with(|es| es.borrow().len() as u64).unwrap_or(0) }
/// A contained fault or deadline unwound past the wrappers: their frames are gone (the words
/// were restored by the guard; the entries and segments of the frames that left go too).
pub fn entry_recover(depth: u64) {
    let _ = ENTRIES.try_with(|es| {
        let mut es = es.borrow_mut();
        while es.len() as u64 > depth {
            pop_entry(&mut es);
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
        return Saved([0; NWORDS]);
    }
    let w = words();
    let mut s = [0; NWORDS];
    for (k, v) in s.iter_mut().enumerate() {
        *v = unsafe { *w.add(k) };
    }
    Saved(s)
}

pub fn restore(s: Saved) {
    if !sb_ready() {
        return;
    }
    budget_flush();
    let w = words();
    unsafe {
        for k in 0..NWORDS {
            *w.add(k) = s.0[k];
        }
    }
    budget_reload();
}

/// Why an entry failed.
pub enum EnterError {
    Unloaded,
    /// the thread is past its thread-local destructors, or its segment cannot be mapped
    NoSegment,
}

/// One entry of this thread into a module (a wrapper's call), innermost last. The words before
/// it live here, not in the wrapper's frame: a leave restores them, and a re-entry finds the
/// arena's live shadow stack and fuel in them.
struct Entry {
    prev: [i64; NWORDS],
    arena: *const Arena,
    /// the arena's segment this entry runs on
    seg: u32,
    /// this entry claimed the segment (its first frame in the arena on this thread)
    claimed: bool,
    /// where the fuel this entry spends goes back to when it leaves (it continues that call)
    carry: Carry,
}

#[derive(Clone, Copy)]
enum Carry {
    /// a fresh call: its own fuel
    None,
    /// nested in a call into the same arena: the words this entry restores
    Same,
    /// a re-entry (A -> host -> B -> host -> A) continues the state saved in that outer entry's `prev`
    Outer(usize),
}

thread_local! {
    static ENTRIES: std::cell::RefCell<Vec<Entry>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Enters module `idx`'s arena on this thread (a wrapper's call). Lock-free.
pub fn enter_module(idx: u32) -> Result<(), EnterError> {
    if idx as usize >= MAX_MODULES {
        return Err(EnterError::Unloaded);
    }
    // (an `Arena` is never freed: an unloaded one is a tombstone that refuses entry)
    let p = ARENAS[idx as usize].load(std::sync::atomic::Ordering::Acquire);
    if p.is_null() {
        return Err(EnterError::Unloaded);
    }
    enter_raw(unsafe { &*p })
}

/// A host call enters arena `a` (a macro world's): as a wrapper's entry, left with `leave`.
pub fn enter_arena(a: &Arena) -> Result<(), EnterError> {
    enter_raw(a)
}

/// `len` bytes of the current arena's heap, aligned to `align` (0: none left, or no arena).
pub fn arena_alloc(len: u64, align: u64) -> u64 {
    sb_alloc(len, align)
}

/// Gives back what `arena_alloc` (or the arena's code) allocated in the current arena.
pub fn arena_free(p: u64) {
    sb_free(p)
}

/// The current arena's bytes [p, p + len), when the range lies inside its window.
pub unsafe fn arena_bytes<'a>(p: u64, len: u64) -> Option<&'a [u8]> {
    let a = cur_arena()?;
    if p & !0xffff_ffff != a.base || !fits(p, len) {
        return None;
    }
    Some(std::slice::from_raw_parts(p as *const u8, len as usize))
}

/// The current arena's address of a module pointer `p` with `len` bytes behind it (0: out of
/// the window).
pub fn arena_ptr(p: u64, len: u64) -> u64 {
    let pm = m(p);
    if pm == 0 || !fits(pm, len) { 0 } else { pm }
}

thread_local! {
    /// the last buffer `give` handed this thread's code (arena base, address): freed by the next
    static GIVEN: std::cell::Cell<(u64, u64)> = const { std::cell::Cell::new((0, 0)) };
}

/// Hands `b` to the current arena's code, in its heap: valid until this thread's next `give`
/// in the same arena (the code copies it right away). 0: no room.
pub fn give(b: &[u8]) -> u64 {
    let Some(a) = cur_arena() else { return 0 };
    let p = sb_alloc(b.len().max(1) as u64, 16);
    if p == 0 {
        return 0;
    }
    unsafe { std::ptr::copy_nonoverlapping(b.as_ptr(), p as *mut u8, b.len()) };
    let (base, last) = GIVEN.with(|g| g.replace((a.base, p)));
    if base == a.base && last != 0 {
        sb_free(last);
    }
    p
}

/// The entry proper.
fn enter_raw(a: &Arena) -> Result<(), EnterError> {
    if !a.alive.load(std::sync::atomic::Ordering::SeqCst) {
        return Err(EnterError::Unloaded);
    }
    budget_flush();
    let w = words();
    let prev = save().0;
    let fuel = if a.fuel > 0 { a.fuel } else { i64::MAX };
    let r = ENTRIES.try_with(|es| {
        let mut es = es.borrow_mut();
        // where this thread's state in the arena is: the current one (a nested entry), saved in an
        // outer entry (A -> host -> B -> host -> A: continue below A's live frames), or none yet
        let (ssp, sslo, left, seg, claimed, carry) = if prev[0] == a.base as i64 {
            let seg = es.last().map_or(0, |e| e.seg);
            (prev[1], prev[5], prev[4], seg, false, Carry::Same)
        } else if let Some(i) = es.iter().rposition(|e| e.prev[0] == a.base as i64) {
            // (es[i - 1] is the entry into the arena that es[i] left)
            let e = &es[i];
            let seg = es[..i].iter().rev().find(|o| std::ptr::eq(o.arena, a)).map_or(0, |o| o.seg);
            (e.prev[1], e.prev[5], e.prev[4], seg, false, Carry::Outer(i))
        } else {
            let seg = claim_seg_waiting(a)?;
            let at = a.base + SEGS as u64 + seg as u64 * SEG as u64;
            // its thread-locals start as zeros (their first-access flags clear)
            let used = a.tls_used.load(std::sync::atomic::Ordering::Acquire).min(TLS_LEN);
            unsafe { std::ptr::write_bytes((at + (SEG_GUARD + SCRATCH_LEN) as u64) as *mut u8, 0, used as usize) };
            ((at + SEG as u64) as i64, (at + STACK_LO as u64) as i64, fuel, seg, true, Carry::None)
        };
        es.push(Entry { prev, arena: a as *const Arena, seg, claimed, carry });
        unsafe {
            *w = a.base as i64;
            *w.add(1) = ssp;
            *w.add(2) = fuel;
            *w.add(3) = a as *const Arena as i64;
            // (a fresh entry starts with the module's fuel per call; a nested or resumed one
            // continues the outer call's)
            *w.add(4) = left;
            *w.add(5) = sslo;
        }
        Ok(())
    });
    match r {
        Ok(Ok(())) => {
            budget_reload();
            Ok(())
        }
        Ok(Err(e)) => Err(e),
        // (this thread is past its TLS destructors)
        Err(_) => Err(EnterError::NoSegment),
    }
}

/// A free segment of arena `a` for this thread: when every one is in use, the thread waits until
/// another thread's last frame there leaves (an entry never fails for that).
fn claim_seg_waiting(a: &Arena) -> Result<u32, EnterError> {
    let mut said = false;
    loop {
        match claim_seg(a) {
            Ok(k) => return Ok(k),
            Err(false) => return Err(EnterError::NoSegment),
            Err(true) => {}
        }
        if !a.alive.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(EnterError::Unloaded);
        }
        if !said {
            said = true;
            if !a.waited.swap(true, std::sync::atomic::Ordering::Relaxed) {
                eprintln!("relax: all {} thread segments of `{}` are in use: threads entering it wait for one", NSEG, a.name);
            }
        }
        std::thread::yield_now();
    }
}

/// A free segment of arena `a` for this thread, mapped (Err(true): all in use; Err(false): it
/// cannot be mapped).
fn claim_seg(a: &Arena) -> Result<u32, bool> {
    use std::sync::atomic::Ordering;
    let mut cur = a.segs.load(Ordering::Acquire);
    let k = loop {
        if cur == u64::MAX {
            return Err(true);
        }
        let k = (!cur).trailing_zeros();
        match a.segs.compare_exchange_weak(cur, cur | 1 << k, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => break k,
            Err(c) => cur = c,
        }
    };
    if a.segs_mapped.load(Ordering::Acquire) & 1 << k == 0 {
        let at = a.base + SEGS as u64 + k as u64 * SEG as u64;
        if !map_rw(at + SEG_GUARD as u64, (SEG - SEG_GUARD) as u64) {
            a.segs.fetch_and(!(1 << k), Ordering::AcqRel);
            return Err(false);
        }
        a.segs_mapped.fetch_or(1 << k, Ordering::AcqRel);
    }
    Ok(k)
}

/// Pops this thread's innermost entry (its words are restored by the caller). A claim's end: its
/// thread-local key values go (a fault that unwound past it ran no destructors).
fn pop_entry(es: &mut Vec<Entry>) -> Option<[i64; NWORDS]> {
    let e = es.pop()?;
    if e.claimed {
        // (an `Arena` is never freed)
        let a = unsafe { &*e.arena };
        let bit = 1u64 << e.seg;
        if a.keyed.load(std::sync::atomic::Ordering::Acquire) & bit != 0 {
            if let Some(v) = a.keys.lock().unwrap_or_else(|e| e.into_inner()).vals.get_mut(e.seg as usize) {
                *v = Default::default();
            }
            a.keyed.fetch_and(!bit, std::sync::atomic::Ordering::AcqRel);
        }
        a.segs.fetch_and(!(1u64 << e.seg), std::sync::atomic::Ordering::AcqRel);
    }
    Some(e.prev)
}

/// The innermost entry's claim ends with it: the destructors of its thread-locals' runtime keys
/// run first, in the arena (the entry is still this thread's state), in std's order
/// (domain_threads.rs `finish`): keys with drop glue last-set first -- a destructor that sets
/// another key adds it, and it runs next -- then the late keys, whose destructors only free their
/// slots. A destructor is code of the arena, called through its thunk table.
fn run_key_destructors() {
    let Some((a, seg)) = ENTRIES.try_with(|es| es.borrow().last().filter(|e| e.claimed).map(|e| (e.arena, e.seg))).ok().flatten() else { return };
    let a = unsafe { &*a };
    if a.keyed.load(std::sync::atomic::Ordering::Acquire) & (1u64 << seg) == 0 {
        return;
    }
    // a call that unwinds out of the module drops its thread-locals' values with the claim (they
    // live in the arena): no module code runs during an unwind
    if !a.alive.load(std::sync::atomic::Ordering::SeqCst) || jit::unwinding() {
        return;
    }
    // under a recovery point, as the module's own catch_unwind (a panicking destructor, or one
    // out of fuel, is reported and the next one runs; std aborts the process there)
    let call = |dtor: u64, value: u64| {
        let s = save();
        let g: rt_fn_ty!(fn(u64, u64) -> u64) = unsafe { std::mem::transmute(jit::rt_entry("catch_panic") as usize) };
        g(mask_thunk(dtor), m(value));
        restore(s);
    };
    loop {
        let next = {
            let mut g = a.keys.lock().unwrap_or_else(|e| e.into_inner());
            let Keys { made, vals } = &mut *g;
            let Some((values, order)) = vals.get_mut(seg as usize) else { return };
            let mut next = None;
            while let Some(key) = order.pop() {
                match made.get(key as usize) {
                    Some(&(dtor, false)) if dtor != 0 => {
                        let value = values.get(key as usize).copied().unwrap_or(0);
                        if value != 0 {
                            next = Some((dtor, value));
                            break;
                        }
                    }
                    _ => {}
                }
            }
            next
        };
        let Some((dtor, value)) = next else { break };
        call(dtor, value);
    }
    let late: Vec<(u64, u64)> = {
        let g = a.keys.lock().unwrap_or_else(|e| e.into_inner());
        let Some((values, _)) = g.vals.get(seg as usize) else { return };
        values.iter().enumerate().filter_map(|(key, &value)| match g.made.get(key) {
            Some(&(dtor, true)) if dtor != 0 && value != 0 => Some((dtor, value)),
            _ => None,
        }).collect()
    };
    for (dtor, value) in late {
        call(dtor, value);
    }
}

/// A wrapper's call returns (or unwinds): the state before it comes back.
pub fn leave() {
    run_key_destructors();
    budget_flush();
    let w = words();
    let prev = ENTRIES.try_with(|es| {
        let mut es = es.borrow_mut();
        let carry = es.last().map_or(Carry::None, |e| e.carry);
        let mut prev = pop_entry(&mut es);
        // a nested or resumed call's fuel left goes back to the call it continued
        let left = unsafe { *w.add(4) };
        match carry {
            Carry::None => {}
            Carry::Same => {
                if let Some(p) = prev.as_mut() {
                    p[4] = left;
                }
            }
            Carry::Outer(i) => {
                if let Some(o) = es.get_mut(i) {
                    o.prev[4] = left;
                }
            }
        }
        prev
    }).ok().flatten();
    if let Some(p) = prev {
        restore(Saved(p));
    }
}

/// Shadow-stack bytes for a wrapper's argument copy or result slot (0: no room).
pub fn alloca(size: u64, align: u64) -> u64 {
    let w = words();
    unsafe {
        let a = *w as u64;
        let sp = *w.add(1) as u64;
        let lo = *w.add(5) as u64;
        let at = (sp.saturating_sub(size)) & !(align.max(1) - 1);
        if a == 0 || at < lo {
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

// Callee::Host entries (rt_policy, the Copy transform) are in Relax's convention; the same
// operations as C functions (c_policy) are C: they differ on Windows (SysV vs Win64)
rt_fn! {
fn sb_rt_memcpy(d: u64, s: u64, n: u64) -> u64 {
    memcpy_in(d, s, n)
}
}
rt_fn! {
fn sb_rt_memset(d: u64, c: u64, n: u64) -> u64 {
    memset_in(d, c, n)
}
}

extern "C" fn sb_memcpy(d: u64, s: u64, n: u64) -> u64 {
    memcpy_in(d, s, n)
}

fn memcpy_in(d: u64, s: u64, n: u64) -> u64 {
    // faults here are the module's (an unmapped arena page, the guard write below): contained
    // like its own code's; nothing here holds a lock or half-made state
    let _safe = crate::jit::FaultSafe::enter();
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
    memset_in(d, c, n)
}

fn memset_in(d: u64, c: u64, n: u64) -> u64 {
    let _safe = crate::jit::FaultSafe::enter();
    let dm = m(d);
    if n > 0 && fits(dm, n) {
        unsafe { std::ptr::write_bytes(dm as *mut u8, c as u8, n as usize) };
    } else if n > 0 {
        unsafe { std::ptr::write_volatile((dm | 0xffff_ffff) as *mut u8, 0) };
    }
    dm
}

extern "C" fn sb_memcmp(a: u64, b: u64, n: u64) -> i32 {
    let _safe = crate::jit::FaultSafe::enter();
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
    let _safe = crate::jit::FaultSafe::enter();
    let pm = m(p);
    let mut n = 0;
    while fits(pm, n + 1) && unsafe { *((pm + n) as *const u8) } != 0 {
        n += 1;
    }
    n
}

/// Fills [p, p + len) of the current arena with random bytes (false: out of the window).
fn fill_random(p: u64, len: u64) -> bool {
    use std::hash::{BuildHasher, Hasher};
    let pm = m(p);
    if pm == 0 || !fits(pm, len) {
        return false;
    }
    let out = unsafe { std::slice::from_raw_parts_mut(pm as *mut u8, len as usize) };
    let seed = std::collections::hash_map::RandomState::new();
    for (k, c) in out.chunks_mut(8).enumerate() {
        let mut h = seed.build_hasher();
        h.write_usize(k);
        c.copy_from_slice(&h.finish().to_le_bytes()[..c.len()]);
    }
    true
}
extern "C" fn sb_getrandom(buf: u64, len: u64, _flags: u32) -> i64 {
    if fill_random(buf, len) {
        len as i64
    } else {
        set_errno(EFAULT)
    }
}
extern "C" fn sb_arc4random_buf(buf: u64, len: u64) {
    fill_random(buf, len);
}
extern "C" fn sb_cc_random(buf: u64, len: u64) -> i32 {
    if fill_random(buf, len) { 0 } else { -1 }
}

// errno: a word of this thread's segment (its thread-locals' third), what the C functions here
// set on failure
extern "C" fn sb_errno() -> u64 {
    unsafe { *words().add(5) as u64 - TLS_LEN as u64 + TLS_ERRNO as u64 }
}
/// Sets this thread's errno to `e`; -1, the C functions' failure.
fn set_errno(e: i32) -> i64 {
    unsafe { *(sb_errno() as *mut i32) = e };
    -1
}
const EFAULT: i32 = 14;
#[cfg(unix)]
const EINVAL: i32 = 22;

// std's futex (sys/linux.rs, sys/macos.rs): waits and wakes on a word of the arena, nothing else
// of the kernel. Linux reaches it through syscall(2); macOS through __ulock_* (std's dlsym
// lookup of os_sync_* finds nothing here: sb_no_symbol).
#[cfg(any(target_os = "linux", target_os = "android"))]
extern "C" fn sb_syscall(n: i64, a1: u64, a2: u64, a3: u64, a4: u64, a5: u64, a6: u64) -> i64 {
    extern "C" {
        fn syscall(n: i64, ...) -> i64;
    }
    #[cfg(target_arch = "x86_64")]
    const SYS_FUTEX: i64 = 202;
    #[cfg(not(target_arch = "x86_64"))]
    const SYS_FUTEX: i64 = 98;
    const ENOSYS: i32 = 38;
    // FUTEX_WAIT_BITSET and FUTEX_WAKE, process-private
    const WAIT: u64 = 9 | 128;
    const WAKE: u64 = 1 | 128;
    if n != SYS_FUTEX {
        return set_errno(ENOSYS);
    }
    let w = arena_ptr(a1, 4);
    if w == 0 || w % 4 != 0 {
        return set_errno(EFAULT);
    }
    let r = match a2 {
        WAIT if a5 == 0 => {
            let ts = if a4 == 0 { 0 } else { arena_ptr(a4, 16) };
            if a4 != 0 && ts == 0 {
                return set_errno(EFAULT);
            }
            unsafe { syscall(n, w, a2, a3 as u32, ts, 0u64, a6 as u32) }
        }
        WAKE => unsafe { syscall(n, w, a2, a3 as u32) },
        _ => return set_errno(EINVAL),
    };
    if r < 0 {
        set_errno(std::io::Error::last_os_error().raw_os_error().unwrap_or(EINVAL))
    } else {
        r
    }
}
#[cfg(target_vendor = "apple")]
extern "C" {
    fn __ulock_wait(op: u32, addr: *mut u8, value: u64, timeout_us: u32) -> i32;
    fn __ulock_wake(op: u32, addr: *mut u8, wake_value: u64) -> i32;
}
/// UL_COMPARE_AND_WAIT, with ULF_WAKE_ALL for a wake
#[cfg(target_vendor = "apple")]
fn ulock_op(op: u32, wake: bool) -> bool {
    op == 1 || (wake && op == 1 | 0x100)
}
#[cfg(target_vendor = "apple")]
extern "C" fn sb_ulock_wait(op: u32, addr: u64, value: u64, timeout_us: u32) -> i32 {
    let w = arena_ptr(addr, 4);
    if w == 0 || w % 4 != 0 || !ulock_op(op, false) {
        return set_errno(if w == 0 { EFAULT } else { EINVAL }) as i32;
    }
    let r = unsafe { __ulock_wait(op, w as *mut u8, value, timeout_us) };
    if r < 0 {
        set_errno(std::io::Error::last_os_error().raw_os_error().unwrap_or(EINVAL));
    }
    r
}
#[cfg(target_vendor = "apple")]
extern "C" fn sb_ulock_wake(op: u32, addr: u64, wake_value: u64) -> i32 {
    let w = arena_ptr(addr, 4);
    if w == 0 || w % 4 != 0 || !ulock_op(op, true) {
        return set_errno(if w == 0 { EFAULT } else { EINVAL }) as i32;
    }
    let r = unsafe { __ulock_wake(op, w as *mut u8, wake_value) };
    if r < 0 {
        set_errno(std::io::Error::last_os_error().raw_os_error().unwrap_or(EINVAL));
    }
    r
}
extern "C" fn sb_no_symbol() -> u64 {
    0
}
/// What a twin's call leaves out: makepad inspect's wait registry and wake timeline (std
/// sync/raw.rs), the runtime's panic hook (std sys/rt.rs)
extern "C" fn sb_unrecorded() {}

/// The current arena and this thread's segment of it (from its shadow stack's floor, W_SSLO).
fn cur_seg() -> Option<(&'static Arena, usize)> {
    let a = cur_arena()?;
    let lo = unsafe { *words().add(5) } as u64;
    let rel = lo.checked_sub(a.base + SEGS as u64 + STACK_LO as u64)?;
    let k = rel / SEG as u64;
    (rel % SEG as u64 == 0 && k < NSEG as u64).then_some((a, k as usize))
}

// std's runtime keys (thread/local.rs LazyKey) for an arena's code: the keys are the arena's,
// each segment (the claim of one thread) has its own values; their destructors run when the
// claim ends (`run_key_destructors`). `dtor` is a fn value of the module (a sandbox thunk).
extern "C" fn sb_key_create(dtor: u64, late: u32) -> u32 {
    let Some(a) = cur_arena() else { return 0 };
    let mut g = a.keys.lock().unwrap_or_else(|e| e.into_inner());
    g.made.push((dtor, late != 0));
    (g.made.len() - 1) as u32
}
extern "C" fn sb_key_delete(key: u32) {
    let Some(a) = cur_arena() else { return };
    // (keys are never reused: a deleted one keeps its number and loses its destructor)
    if let Some(k) = a.keys.lock().unwrap_or_else(|e| e.into_inner()).made.get_mut(key as usize) {
        k.0 = 0;
    }
}
extern "C" fn sb_key_get(key: u32) -> u64 {
    let Some((a, seg)) = cur_seg() else { return 0 };
    let g = a.keys.lock().unwrap_or_else(|e| e.into_inner());
    g.vals.get(seg).and_then(|v| v.0.get(key as usize)).copied().unwrap_or(0)
}
extern "C" fn sb_key_set(key: u32, value: u64) {
    let Some((a, seg)) = cur_seg() else { return };
    let mut g = a.keys.lock().unwrap_or_else(|e| e.into_inner());
    if key as usize >= g.made.len() {
        return;
    }
    if g.vals.len() <= seg {
        g.vals.resize_with(seg + 1, Default::default);
    }
    let (values, order) = &mut g.vals[seg];
    if values.len() <= key as usize {
        values.resize(key as usize + 1, 0);
    }
    if values[key as usize] == 0 && value != 0 {
        order.push(key);
    }
    values[key as usize] = value;
    if value != 0 {
        a.keyed.fetch_or(1u64 << seg, std::sync::atomic::Ordering::AcqRel);
    }
}

// Thread identity: a module's code sees no host thread or process ids. Its threads are numbered
// by segment from 1, its process is 0, and none of them is the program's main thread.
extern "C" fn sb_main_np() -> i32 {
    0
}
extern "C" fn sb_gettid() -> i32 {
    cur_seg().map_or(0, |(_, k)| k as i32 + 1)
}
extern "C" fn sb_getpid() -> i32 {
    0
}

rt_fn! {
fn sb_fmt_str(p: u64, len: u64, spec: u64) {
    let pm = m(p);
    let len = if fits(pm, len) { len } else { 0 };
    let f: rt_fn_ty!(fn(u64, u64, u64)) = unsafe { std::mem::transmute(jit::rt_entry("fmt_str") as usize) };
    f(pm, len, spec)
}
}

rt_fn! {
fn sb_str_eq(p1: u64, l1: u64, p2: u64, l2: u64) -> u64 {
    let (a, b) = (m(p1), m(p2));
    if l1 != l2 || !fits(a, l1) || !fits(b, l2) {
        return 0;
    }
    let (x, y) = unsafe { (std::slice::from_raw_parts(a as *const u8, l1 as usize), std::slice::from_raw_parts(b as *const u8, l2 as usize)) };
    (x == y) as u64
}
}

rt_fn! {
/// fmt_pop for a module: the text is copied into the window's scratch (larger text into its heap).
fn sb_fmt_pop() -> jit::StrRet {
    let f: rt_fn_ty!(fn() -> jit::StrRet) = unsafe { std::mem::transmute(jit::rt_entry("fmt_pop") as usize) };
    let r = f();
    let Some(a) = cur_arena() else { return r };
    let (p, n) = r.parts();
    // this thread's scratch: below its thread-locals, below its shadow stack (W_SSLO)
    let scratch = unsafe { *words().add(5) } as u64 - TLS_LEN as u64 - SCRATCH_LEN as u64;
    let dst = if n <= SCRATCH_LEN as u64 { scratch } else { a.heap.lock().unwrap_or_else(|e| e.into_inner()).alloc(a.base, n, 16) };
    if dst == 0 {
        return jit::StrRet::new(scratch, 0);
    }
    unsafe { std::ptr::copy_nonoverlapping(p as *const u8, dst as *mut u8, n as usize) };
    jit::StrRet::new(dst, n)
}
}

rt_fn! {
/// catch_unwind inside a module: the target and data are masked; the shadow stack comes back.
fn sb_catch_panic(f: u64, data: u64) -> u64 {
    let s = save();
    let fm = mask_thunk(f);
    let g: rt_fn_ty!(fn(u64, u64) -> u64) = unsafe { std::mem::transmute(jit::rt_entry("catch_panic") as usize) };
    let r = g(fm, m(data));
    restore(s);
    r
}
}

/// A fn pointer of the current arena's code masked into its unit's thunk table (0: no arena).
fn mask_thunk(f: u64) -> u64 {
    let Some(a) = cur_arena() else { return 0 };
    let (base, span, ts) = (a.sbt_base, a.sbt_span, jit::thunk_size() as u64);
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
        "memcpy" | "memmove" => Some(("sb_rt_memcpy", sb_rt_memcpy as *const () as u64)),
        "memset" => Some(("sb_rt_memset", sb_rt_memset as *const () as u64)),
        "catch_panic" => Some(("sb_catch_panic", sb_catch_panic as *const () as u64)),
        _ => return None,
    })
}

/// C functions a twin of a domain allowing `allow` may call: math (scalars), memory/allocation
/// replaced by arena versions, and the domain's own entries.
fn c_policy(allow: Allow, name: &str) -> Option<Option<(&'static str, u64)>> {
    let n = name.trim_start_matches('_');
    if let Some(own) = allow(n) {
        return Some(own);
    }
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
        // std's hash seeds (RandomState): random bytes, nothing else of the host
        "getrandom" => Some(("sb_getrandom", sb_getrandom as *const () as u64)),
        "arc4random_buf" => Some(("sb_arc4random_buf", sb_arc4random_buf as *const () as u64)),
        "CCRandomGenerateBytes" => Some(("sb_cc_random", sb_cc_random as *const () as u64)),
        #[cfg(any(target_os = "linux", target_os = "android"))]
        "errno_location" | "errno" => Some(("sb_errno", sb_errno as *const () as u64)),
        #[cfg(target_vendor = "apple")]
        "error" => Some(("sb_errno", sb_errno as *const () as u64)),
        #[cfg(any(target_os = "linux", target_os = "android"))]
        "syscall" => Some(("sb_syscall", sb_syscall as *const () as u64)),
        #[cfg(target_vendor = "apple")]
        "ulock_wait" => Some(("sb_ulock_wait", sb_ulock_wait as *const () as u64)),
        #[cfg(target_vendor = "apple")]
        "ulock_wake" => Some(("sb_ulock_wake", sb_ulock_wake as *const () as u64)),
        "dlsym" => Some(("sb_no_symbol", sb_no_symbol as *const () as u64)),
        "relax_wait_enter" | "relax_wait_leave" | "relax_wake" => Some(("sb_unrecorded", sb_unrecorded as *const () as u64)),
        // std's runtime panic hook and payload end (sys/rt.rs install_core_hook): host code never
        // calls into twins; their panics record their payload through core's hook in the arena,
        // and a runtime check's failure in a twin is a contained fault
        "relax_set_panic_hook" | "relax_set_payload_end" => Some(("sb_unrecorded", sb_unrecorded as *const () as u64)),
        "sin" | "cos" | "tan" | "asin" | "acos" | "atan" | "atan2" | "sinh" | "cosh" | "tanh" | "asinh" | "acosh" | "atanh" | "exp" | "exp2" | "expm1" | "log" | "log2" | "log10" | "log1p" | "pow" | "sqrt" | "cbrt" | "hypot" | "fmod" | "floor" | "ceil" | "round" | "trunc" | "fma" | "fmax" | "fmin" | "fdim" | "erf" | "erfc" | "tgamma" | "lgamma" | "ldexp" | "nextafter" | "copysign" | "rint" | "nearbyint" | "sinf" | "cosf" | "tanf" | "asinf" | "acosf" | "atanf" | "atan2f" | "sinhf" | "coshf" | "tanhf" | "asinhf" | "acoshf" | "atanhf" | "expf" | "exp2f" | "expm1f" | "logf" | "log2f" | "log10f" | "log1pf" | "powf" | "sqrtf" | "cbrtf" | "hypotf" | "fmodf" | "floorf" | "ceilf" | "roundf" | "truncf" | "fmaf" | "fmaxf" | "fminf" | "fdimf" | "erff" | "erfcf" | "tgammaf" | "lgammaf" | "ldexpf" | "nextafterf" | "copysignf" | "rintf" | "nearbyintf" => None,
        // std's thread-locals with lazy init or drop glue (thread/local.rs LazyKey): the arena's
        // own keys, a value per segment
        "relax_key_create" => Some(("sb_key_create", sb_key_create as *const () as u64)),
        "relax_key_delete" => Some(("sb_key_delete", sb_key_delete as *const () as u64)),
        "relax_key_get" => Some(("sb_key_get", sb_key_get as *const () as u64)),
        "relax_key_set" => Some(("sb_key_set", sb_key_set as *const () as u64)),
        // thread identity (std::thread::current): no host thread or process ids
        "pthread_main_np" => Some(("sb_main_np", sb_main_np as *const () as u64)),
        "gettid" => Some(("sb_gettid", sb_gettid as *const () as u64)),
        "getpid" => Some(("sb_getpid", sb_getpid as *const () as u64)),
        "abort" => Some(("abort", 0)),
        // std's catch_unwind names its recovery point (jit.rs relax_catch_caught: a thread-local flag)
        "relax_catch_caught" => None,
        _ => return None,
    })
}

// ------------------------------------------------------------------ the transform

/// Rewrites a twin's RIR for sandboxed execution (on the owner thread). Errors name what a module
/// cannot do (a C function, data it cannot reach), never a crash.
pub unsafe fn transform(up: *mut Unit, f: &mut Func, twin_id: u32) -> Result<(), String> {
    let session = unsafe { crate::jit::unit_session(up) };
    let options = &session.options;
    let u = &mut *up;
    if !u.sandbox.ready.load(std::sync::atomic::Ordering::Acquire) {
        return Err(format!("{}: a twin of a unit with no sandbox", f.name));
    }
    let (tls_sym, sandboxed, allow) = with_sb(u, |s| (s.tls_sym, s.sandboxed.clone(), s.allow));
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
    let mut b0 = relax_backend::rir::Block { insts: Vec::new(), pos: Vec::new(), term: Term::Jump(entry), term_pos: pos0, count: -1.0 };
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
    let site_nounwind = u.site_sym(f.file, (pos0 & 0xffff_ffff) as u32, "sandboxed module: a panic reached the edge of a function that cannot unwind (`extern \"C\"`)");
    let panic_site = u.rs.panic_site;
    let mut new_blocks: Vec<relax_backend::rir::Block> = Vec::new();
    let nb = f.blocks.len() as u32 + 1; // index of the first appended block (after block 0 is inserted)
    let trap = |f: &mut Func, nbs: &mut Vec<relax_backend::rir::Block>, site: Sym| -> u32 {
        let v = f.vreg(Cls::I);
        nbs.push(relax_backend::rir::Block { insts: vec![Inst::Addr(v, site), Inst::Call(Callee::Host(panic_site), vec![v], Vec::new())], pos: vec![pos0, pos0], term: Term::Unreachable, term_pos: pos0, count: -1.0 });
        nb + nbs.len() as u32 - 1
    };
    if has_frame {
        push0(&mut b0, Inst::IBinI(IOp::Add, U64, t_ssp, t_arena, W_SSP - W_ARENA));
        push0(&mut b0, Inst::Load(relax_backend::rir::Mem::Int(8, false), ssp0, t_ssp, 0));
        push0(&mut b0, Inst::IBinI(IOp::Sub, U64, nsp, ssp0, frame as i64));
        // this thread's segment: nsp - lo must be in [0, stack size] (below lo wraps; a thread
        // with no entry has ssp = lo = 0, which wraps too)
        let lo = f.vreg(Cls::I);
        let rel = f.vreg(Cls::I);
        let c = f.vreg(Cls::I);
        push0(&mut b0, Inst::Load(relax_backend::rir::Mem::Int(8, false), lo, t_arena, (W_SSLO - W_ARENA) as i32));
        push0(&mut b0, Inst::IBin(IOp::Sub, U64, rel, nsp, lo));
        push0(&mut b0, Inst::ICmpI(Cond::Gt, false, c, rel, (SEG - STACK_LO) as i64));
        let t = trap(f, &mut new_blocks, site_stack);
        // the store of the new shadow sp happens in a continuation block
        let cont = relax_backend::rir::Block { insts: vec![Inst::Store(relax_backend::rir::Mem::Int(8, false), t_ssp, 0, nsp)], pos: vec![pos0], term: Term::Jump(entry + 1), term_pos: pos0, count: -1.0 };
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
                            let o = f.vreg(Cls::I);
                            emit(Inst::Addr(o, place_sym(up, sym.id)?.plus(sym.off)), &mut insts, &mut pos);
                            emit(Inst::IBin(IOp::Add, U64, d, ar, o), &mut insts, &mut pos);
                        }
                        Some(SymKind::Thunk(id)) => {
                            let t = fn_target(up, id, &sandboxed)?;
                            refs.push(t);
                            emit(Inst::Addr(d, thunk_sym(up, t)?.plus(sym.off)), &mut insts, &mut pos);
                        }
                        Some(SymKind::Site(_)) | Some(SymKind::Rt("null")) => emit(Inst::Addr(d, sym), &mut insts, &mut pos),
                        // std's "makepad inspect records" flag (sync/raw.rs): never for twins, a
                        // word of the thread's that stays zero (its thread block's first)
                        Some(SymKind::Foreign(n)) if n == "__relax_recording" => {
                            let lo = f.vreg(Cls::I);
                            emit(Inst::Load(relax_backend::rir::Mem::Int(8, false), lo, t_arena, (W_SSLO - W_ARENA) as i32), &mut insts, &mut pos);
                            emit(Inst::IBinI(IOp::Add, U64, d, lo, -(TLS_LEN as i64)), &mut insts, &mut pos);
                        }
                        other => return Err(format!("{}: refers to {:?}, which a sandboxed module cannot reach", f.name, other)),
                    }
                }
                Inst::FnAddr(d, id) => {
                    let t = fn_target(up, id, &sandboxed)?;
                    refs.push(t);
                    emit(Inst::Addr(d, thunk_sym(up, t)?), &mut insts, &mut pos);
                }
                // a thread-local: in the thread's segment, below its shadow stack (W_SSLO)
                Inst::TlsAddr(d, sym) | Inst::TlsStatic(d, sym) => {
                    let lo = f.vreg(Cls::I);
                    emit(Inst::Load(relax_backend::rir::Mem::Int(8, false), lo, t_arena, (W_SSLO - W_ARENA) as i32), &mut insts, &mut pos);
                    if matches!(u.syms.kind(sym.id), Some(SymKind::Rt("tls_base"))) {
                        emit(Inst::IBinI(IOp::Add, U64, d, lo, sym.off - TLS_LEN as i64), &mut insts, &mut pos);
                    } else {
                        let o = f.vreg(Cls::I);
                        let x = f.vreg(Cls::I);
                        emit(Inst::Addr(o, tls_offset_sym(up, sym.id).map_err(|e| format!("{}: {}", f.name, e))?.plus(sym.off)), &mut insts, &mut pos);
                        emit(Inst::IBin(IOp::Add, U64, x, lo, o), &mut insts, &mut pos);
                        emit(Inst::IBinI(IOp::Add, U64, d, x, -(TLS_LEN as i64)), &mut insts, &mut pos);
                    }
                }
                Inst::VGather(..) | Inst::VTbl(..) => return Err(format!("{}: vector gathers are not available to sandboxed code", f.name)),
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
                        let s = host_sym(u, "rt:memcpy").ok_or_else(|| format!("{}: no sandboxed memcpy", f.name))?;
                        emit(Inst::Call(Callee::Host(s), vec![dst, src, n], vec![r]), &mut insts, &mut pos);
                    } else {
                        let (d2, _) = mask(f, &mut insts, &mut pos, &mut cache, dst, 0);
                        let (s2, _) = mask(f, &mut insts, &mut pos, &mut cache, src, 0);
                        emit(Inst::Copy(d2, s2, len), &mut insts, &mut pos);
                    }
                }
                Inst::Call(callee, args, rets) => {
                    let callee = match callee {
                        Callee::Asm(_) => return Err(format!("{}: inline assembly, which sandboxed code cannot run", f.name)),
                        Callee::Fn(id) => Callee::Fn(call_target(up, id, &sandboxed, allow)?),
                        Callee::CFn(id, sig) => Callee::CFn(call_target(up, id, &sandboxed, allow)?, sig),
                        Callee::Indirect(v) => Callee::Indirect(mask_fn(&*up, f, &mut insts, &mut pos, p, v)),
                        Callee::CIndirect(v, sig) => Callee::CIndirect(mask_fn(&*up, f, &mut insts, &mut pos, p, v), sig),
                        Callee::Host(s) => match u.syms.kind(s.id) {
                            // the terminate pad of an `extern "C"` body (rustc aborts the process
                            // there): the module's call ends with that fault instead
                            Some(SymKind::Rt("cannot_unwind")) => {
                                let v = f.vreg(Cls::I);
                                emit(Inst::Addr(v, site_nounwind), &mut insts, &mut pos);
                                emit(Inst::Call(Callee::Host(panic_site), vec![v], Vec::new()), &mut insts, &mut pos);
                                continue;
                            }
                            Some(SymKind::Rt(name)) => match rt_policy(name) {
                                Some(None) => Callee::Host(s),
                                Some(Some(_)) => Callee::Host(host_sym(u, &format!("rt:{}", name)).ok_or_else(|| format!("{}: no sandboxed `{}`", f.name, name))?),
                                None => return Err(format!("{}: calls the runtime's `{}`, which sandboxed code cannot use", f.name, name)),
                            },
                            other => return Err(format!("{}: calls {:?}, which sandboxed code cannot use", f.name, other)),
                        },
                        Callee::CHost(s, sig) => {
                            let name = match u.syms.kind(s.id) {
                                Some(SymKind::Foreign(n)) => n,
                                Some(SymKind::Rt(n)) => n.to_string(),
                                other => return Err(format!("{}: calls {:?}, which sandboxed code cannot use", f.name, other)),
                            };
                            match c_policy(allow, &name) {
                                Some(Some(("abort", _))) => {
                                    let v = f.vreg(Cls::I);
                                    emit(Inst::Addr(v, site_abort), &mut insts, &mut pos);
                                    emit(Inst::Call(Callee::Host(panic_site), vec![v], Vec::new()), &mut insts, &mut pos);
                                    continue;
                                }
                                Some(Some(_)) => Callee::CHost(host_sym(u, &format!("c:{}", name)).ok_or_else(|| format!("{}: no sandboxed `{}`", f.name, name))?, sig),
                                Some(None) => Callee::CHost(s, sig),
                                None => return Err(format!("{}: reaches the C function `{}`, which sandboxed code cannot call", f.name, name)),
                            }
                        }
                    };
                    emit(Inst::Call(callee, args, rets), &mut insts, &mut pos);
                    cache.clear();
                }
                // the FP control register is the host thread's (rounding, flush-to-zero): a module
                // may read it, not change it under the host
                Inst::FpEnv(true, _) => return Err(format!("{}: writes the floating-point control register (_mm_setcsr / set_fpcr), which sandboxed code cannot", f.name)),
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
        out_blocks.push(relax_backend::rir::Block { insts, pos, term: b.term, term_pos: b.term_pos, count: b.count });
    }
    let _ = nblocks;
    // assemble: block 0, the rewritten blocks, then the made ones
    f.blocks = Vec::with_capacity(out_blocks.len() + new_blocks.len() + 1);
    f.blocks.push(b0);
    f.blocks.extend(out_blocks);
    f.blocks.extend(new_blocks);
    f.slots.clear();
    // (a domain without fuel pays nothing per block: the meter's load and store of the thread's
    // fuel word chained every block of its code through memory)
    if with_sb(&*up, |s| s.fuel) {
        add_fuel(f, t_arena, site_fuel, panic_site, pos0, &costs);
    }
    with_sb(&*up, |s| s.refs.insert(twin_id, refs));
    f.verify().map_err(|e| format!("{}: sandbox transform made invalid RIR: {}", f.name, e))?;
    // the masks the transform added: common ones merged, loop-invariant ones hoisted. Only
    // under O1 alone (RELAX_O3=0, O2 pipeline): with O3 on, O1 keeps
    // registers unoptimised and the hoisted masks cost more than they save (mini, particle
    // kernel: +25% with the clean-up, +0.5% without). Hot twins retain this confined RIR
    // for O3, whose full pipeline and register allocation perform the clean-up there.
    // RELAX_SANDBOX_OPT=0/1 forces the O1 clean-up.
    let opt = options.sandbox_opt.unwrap_or(!jit::TIER1_ENABLED.load(std::sync::atomic::Ordering::Relaxed));
    if opt {
        relax_backend::passes::simplify(options, f, None, true, true);
        relax_backend::passes::licm(options, f);
        relax_backend::passes::simplify(options, f, None, true, true);
        f.verify().map_err(|e| format!("{}: sandbox clean-up made invalid RIR: {}", f.name, e))?;
    }
    Ok(())
}

/// Where `sym` (a `#[thread_local]` static, or the thread block itself) is in a segment's
/// thread-locals: the block's two words first (zero: std's LocalKey takes the runtime call), each
/// static laid out once per unit with its first-access flag byte.
unsafe fn tls_offset(up: *mut Unit, sym: Sym) -> Result<u32, String> {
    let u = &*up;
    let d = match u.syms.kind(sym.id) {
        Some(SymKind::Rt("tls_base")) => return Ok(0),
        Some(SymKind::ThreadLocal(d)) => d,
        other => return Err(format!("refers to the thread-local {:?}, which a sandboxed module cannot reach", other)),
    };
    if let Some(o) = with_sb(u, |s| s.tls.get(&sym.id).copied()) {
        return Ok(o);
    }
    let t = u.tcx.const_ty(&u.prog, d).ok_or_else(|| format!("the type of thread-local `{}` is not known", u.prog.def_path(d)))?;
    let l = u.layout_of(t);
    let (size, align) = (l.size.max(1) as u32 + 1, l.align.max(1) as u32);
    with_sb(u, |s| {
        let used = s.tls_used.load(std::sync::atomic::Ordering::Acquire);
        let off = (used + align - 1) / align * align;
        if off as u64 + size as u64 > TLS_LEN as u64 {
            return Err(format!("thread-local `{}` does not fit: sandboxed code has {} bytes of thread-locals per thread", u.prog.def_path(d), TLS_LEN));
        }
        s.tls_used.store(off + size, std::sync::atomic::Ordering::Release);
        s.tls.insert(sym.id, off);
        Ok(off)
    })
}

/// An indirect call's target masked into unit `u`'s sandbox thunk table. (The span is the code
/// area's slots' (Unit::exec_size, a compile-time constant): an immediate like any other.)
fn mask_fn(u: &Unit, f: &mut Func, insts: &mut Vec<Inst>, pos: &mut Vec<u64>, p: u64, v: VReg) -> VReg {
    let span = with_sb(u, |s| s.sbt_span);
    let ts = jit::thunk_size() as u64;
    let b = f.vreg(Cls::I);
    let t1 = f.vreg(Cls::I);
    let t2 = f.vreg(Cls::I);
    let t3 = f.vreg(Cls::I);
    insts.push(Inst::Addr(b, table_sym(u)));
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
unsafe fn call_target(up: *mut Unit, id: u32, sandboxed: &HashSet<u32>, allow: Allow) -> Result<u32, String> {
    let u = &*up;
    match &u.fns[id as usize].key {
        FnKey::Inst(d, args) => {
            let def = u.prog.def(*d);
            if def.kind == DefKind::ForeignFn {
                // a foreign fn reached through its slot: only allow-listed math
                let name = u.prog.syms.str(def.name).to_string();
                return match c_policy(allow, &name) {
                    Some(None) => Ok(id),
                    _ => Err(format!("reaches the C function `{}`, which sandboxed code cannot call", name)),
                };
            }
            if !sandboxed.contains(&def.krate) {
                if !args.is_empty() {
                    return Err(format!("calls the generic host API fn `{}`: API fns called by modules must be concrete", u.prog.def_path(*d)));
                }
                if let Some(why) = host_api_escape(u, *d) {
                    return Err(why);
                }
                return Ok(id);
            }
            Ok(twin(up, id))
        }
        FnKey::Sandbox(_) => Ok(id),
        _ => Ok(twin(up, id)),
    }
}

/// Whether calling host API fn `d` could hand module code to the host: a parameter whose type
/// holds a fn pointer, a trait object or a closure anywhere inside (through references, boxes,
/// fields). The host could keep such a value and call it later -- on any thread, outside every
/// wrapper -- and twin code would run with no arena entered (its masked accesses relative to
/// arena 0, i.e. host memory) and no reclaim record (frames the code reclaimer cannot see).
/// So the boundary refuses them: modules pass data to the host; the host calls modules only
/// through their exports' wrappers. (MEMRECLAIM.REVIEW F3.) Some(the refusal) or None.
fn host_api_escape(u: &Unit, d: crate::program::DefId) -> Option<String> {
    // (keyed by the collect epoch too: a host edit may change the signature)
    let key = (d.0, u.tcx.collect_epoch.load(std::sync::atomic::Ordering::Relaxed));
    {
        let g = u.sandbox.sb.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(r) = g.as_ref().and_then(|s| s.api_checked.get(&key)) {
            return r.clone();
        }
    }
    let r = match u.tcx.fn_sig(&u.prog, d) {
        None => Some(format!("calls the host API fn `{}`, whose signature is not known", u.prog.def_path(d))),
        Some(sig) => sig.params.iter().enumerate().find_map(|(k, &t)| {
            let mut seen = HashSet::new();
            carries_code(u, t, &mut seen).map(|what| format!(
                "passes {} to the host API fn `{}` (parameter {}, `{}`): module code cannot be handed to the host -- pass data, and let the host call the module through its exports",
                what, u.prog.def_path(d), k + 1, crate::typeck::type_name(&u.prog, &u.tcx, t)))
        }),
    };
    let mut g = u.sandbox.sb.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(s) = g.as_mut() {
        s.api_checked.insert(key, r.clone());
    }
    r
}

/// What in type `t` can carry callable module code (None: plain data, references to it, boxes of it).
fn carries_code(u: &Unit, t: crate::types::TyId, seen: &mut HashSet<crate::types::TyId>) -> Option<&'static str> {
    use crate::types::TyKind;
    if !seen.insert(t) {
        return None;
    }
    match u.tcx.tys.kind(t).clone() {
        TyKind::Bool | TyKind::Char | TyKind::Str | TyKind::Int(_) | TyKind::Float(_) | TyKind::Never | TyKind::Foreign(_) | TyKind::Const(_) => None,
        TyKind::FnPtr(..) => Some("a fn pointer"),
        TyKind::FnDef(..) => Some("a fn item"),
        TyKind::Closure(..) => Some("a closure"),
        TyKind::Dyn(..) => Some("a trait object (`dyn`)"),
        TyKind::Array(e, _) | TyKind::Slice(e) | TyKind::Ref(_, e, _) | TyKind::Ptr(_, e) => carries_code(u, e, seen),
        TyKind::Tuple(v) => u.tcx.tys.list(v).iter().find_map(|x| carries_code(u, *x, seen)),
        TyKind::Adt(d, args, _) => {
            let Some(a) = u.tcx.adts.get(&d) else { return Some("a value of a type not known concretely") };
            let args = u.tcx.tys.list(args);
            let env = crate::tcx::TypingEnv::fully_monomorphized(&u.prog);
            a.variants.iter().enumerate().find_map(|(vi, v)| (0..v.fields.len()).find_map(|fi| carries_code(u, u.tcx.field_ty(&u.prog, env, d, vi as u32, fi as u32, args), seen)))
        }
        // not concrete: refused rather than guessed
        _ => Some("a value of a type not known concretely"),
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
/// instructions have no charge. Stores/checks remain observable in both tiers. The fuel word is
/// this thread's (W_FUEL in its sandbox words, `words` the TLS address of W_ARENA).
fn add_fuel(f: &mut Func, words: VReg, site: Sym, panic_site: Sym, pos0: u64, costs: &[i64]) {
    let trap_ix = f.blocks.len() as u32;
    let tv = f.vreg(Cls::I);
    f.blocks.push(relax_backend::rir::Block { insts: vec![Inst::Addr(tv, site), Inst::Call(Callee::Host(panic_site), vec![tv], Vec::new())], pos: vec![pos0, pos0], term: Term::Unreachable, term_pos: pos0, count: -1.0 });
    for (k, &cost) in costs.iter().enumerate() {
        let b = k + 1; // block zero is the confinement prologue
        let t = f.vreg(Cls::I);
        let v = f.vreg(Cls::I);
        let v2 = f.vreg(Cls::I);
        let c = f.vreg(Cls::I);
        let cont_ix = f.blocks.len() as u32;
        let insts = vec![
            Inst::IBinI(IOp::Add, U64, t, words, W_FUEL - W_ARENA),
            Inst::Load(relax_backend::rir::Mem::Int(8, true), v, t, 0),
            Inst::IBinI(IOp::Sub, U64, v2, v, cost),
            Inst::Store(relax_backend::rir::Mem::Int(8, true), t, 0, v2),
            Inst::ICmpI(Cond::Lt, true, c, v2, 0),
        ];
        let meter = relax_backend::rir::Block { pos: vec![pos0; insts.len()], insts, term: Term::Branch(c, trap_ix, cont_ix), term_pos: pos0, count: -1.0 };
        let old = std::mem::replace(&mut f.blocks[b], meter);
        f.blocks.push(old);
    }
}

fn sandbox_dump() -> Option<&'static str> {
    static VALUE: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    VALUE.get_or_init(|| crate::zone_alloc::kept_with(|| std::env::var("RELAX_SANDBOX_DUMP").ok())).as_deref()
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
    // RELAX_SANDBOX_DUMP=<substring>: the transformed RIR of matching twins
    if let Some(pat) = sandbox_dump() {
        if f.name.contains(pat) {
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

/// Bytes of unit `u`'s image (read-only, read-write) and its arenas live (memory report).
pub fn stats(u: &Unit) -> (usize, usize, usize) {
    let g = u.sandbox.sb.lock().unwrap_or_else(|e| e.into_inner());
    g.as_ref().map_or((0, 0, 0), |s| (s.ro.len(), s.rw.len(), s.arenas.len()))
}

/// What this module holds of a run, as a fresh process has it (`run_end`). Nothing of the run is left that could see it.
pub(crate) unsafe fn reset_statics() {
    use relax_session::reset_static as reset;
    reset(&TLS_OFF, std::sync::atomic::AtomicU32::new(0));
    reset(&DEAD, Mutex::new(Vec::new()));
    reset(&ARENAS, [const { std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()) }; MAX_MODULES]);
}
