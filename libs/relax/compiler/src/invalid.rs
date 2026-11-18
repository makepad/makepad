//! Invalidated code (user rule 2026-10-23): JIT code is append-only, and code that is merely
//! *superseded* (a tier-up, a same-semantics recompile, a body patch) stays executable forever. Code
//! that is *invalidated* must never run again: a call, return or jump into it is a stale access
//! (stale.rs: the thread parks, the app pauses, the user restarts or stops), never silent wrong
//! results or corrupt memory. Invalidated are:
//! - the earlier patch versions of the slots a layout-changing patch rebuilt (code built for the
//!   old field list or variants);
//! - a rolled-back patch generation: the borrow checker rejected it after it ran (SPEC-V), or it
//!   panicked or faulted (M7) -- its O3 code and its unverified-mode twins included;
//! - an unloaded module's code (the twins of its crate's fns, a trusted module's fns).
//!
//! Such code is allocated in groups (`Group`): one slot's code from one patch generation, one
//! module's code from one generation. Every range is registered with `stale::register` before it
//! is made unrunnable, and addresses are never reused. Code of a run's first generation (every
//! slot's first code, std's twins, glue) stays bump-shared: it is never invalidated.
//! - A module group's code lies in page-aligned chunks that hold nothing else: its unload (or a
//!   rollback of its generation) is one `mprotect(PROT_NONE)` per chunk (the kernel's TLB
//!   shootdown makes it immediate on every core).
//! - A patch group's code lies in its generation's arena (`Arena`): page-aligned chunks that hold
//!   that generation's patch groups only, one after the other (a page per group took 6.3 MB of
//!   chunks for 0.9 MB of code in a session of layout edits). A chunk whose groups are all dead
//!   (a rollback kills its whole generation) is made PROT_NONE as above. A group that dies while
//!   others in its chunk live (a layout patch kills the earlier versions of the slots it rebuilt)
//!   is trap-filled (`trap_fill`): its bytes become an instruction that faults at any offset a
//!   fetch can start (a call, a return, a jump into it is a stale access, by SIGILL), written
//!   while its pages are out of every thread's reach. A thread that touches them meanwhile waits
//!   in its fault handler for the fill to end and runs its instruction again (`transit_wait`):
//!   live neighbours run on.
//!
//! Code built after a patch from that patch's source is the patch's code too (`group_of`): a fn
//! first compiled after a patch of its file, and an unpatched caller whose lazy first compile or
//! O3 tier-up inlined patched bodies (the latest generation over what it took in). A rollback of
//! the generation (or a later layout patch) invalidates it with the rest.
//!
//! First code cannot be made inaccessible (it shares pages). A layout-changing patch therefore
//! pauses the program, and with a thread in code of a fn it rebuilds (`frame_in`) it does not
//! install: the edit is a restart, offered, and the running code goes on. Under the same pause it
//! installs, and a thread found in such code after all (one the pause did not stop entered it) is
//! held for good (`hold_old_frames`, stale.rs `park_held`): it never returns there, and the app
//! runs on without it until the restart that is offered.

use crate::jit::{FnKey, Unit};
use std::sync::atomic::{AtomicU64, Ordering};

/// A group of invalidatable code.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Group {
    /// slot `slot`'s code built from patch generation `gen`'s source (> 0): the patch install, its
    /// later O3 code, the lazily built unverified-mode twin of a patched body
    Patch { slot: u32, gen: u32 },
    /// a run-time module's code (crate `krate`), of generation `gen` (0: its load; > 0: a patch)
    Module { krate: u32, gen: u32 },
}

impl Group {
    pub fn gen(self) -> u32 {
        match self {
            Group::Patch { gen, .. } | Group::Module { gen, .. } => gen,
        }
    }
}

/// One install into a group.
#[derive(Clone, Copy)]
pub struct Install {
    pub slot: u32,
    pub at: u64,
    pub len: u32,
    /// the slot's code version it became (FnEntry::version)
    pub version: u32,
    pub t_ns: u64,
}

/// A group's code and installs.
#[derive(Default)]
pub struct GroupCode {
    /// a module group's chunks: [lo, hi), page-aligned, never shared with other code
    pub chunks: Vec<(u64, u64)>,
    next: u64,
    end: u64,
    /// a patch group's ranges in its generation's arena: [lo, hi), 16-byte units
    pub ranges: Vec<(u64, u64)>,
    pub installs: Vec<Install>,
    pub code_bytes: u64,
    /// why it was invalidated (None: live)
    pub invalidated: Option<String>,
}

/// A patch generation's code: its chunks, the last one filled from `next`.
#[derive(Default)]
struct Arena {
    chunks: Vec<Chunk>,
    next: u64,
    end: u64,
}

struct Chunk {
    lo: u64,
    hi: u64,
    /// the live groups' ranges in it
    live: u32,
    /// nothing in it lives: PROT_NONE, nothing is placed in it again
    closed: bool,
}

/// The unit's invalidatable code (Unit::code_groups).
#[derive(Default)]
pub struct CodeGroups {
    groups: std::collections::HashMap<Group, GroupCode>,
    arenas: std::collections::HashMap<u32, Arena>,
}

/// Totals for `makepad inspect <app> code`: groups, chunk bytes (what the groups took of the code
/// window: page rounding and arena tails included), code bytes in them, invalidated groups and
/// their bytes, the chunk bytes made PROT_NONE, the bytes trap-filled.
pub static GROUPS: AtomicU64 = AtomicU64::new(0);
pub static CHUNK_BYTES: AtomicU64 = AtomicU64::new(0);
pub static GROUP_CODE_BYTES: AtomicU64 = AtomicU64::new(0);
pub static DEAD_GROUPS: AtomicU64 = AtomicU64::new(0);
pub static DEAD_BYTES: AtomicU64 = AtomicU64::new(0);
pub static PROT_BYTES: AtomicU64 = AtomicU64::new(0);
pub static FILLED_BYTES: AtomicU64 = AtomicU64::new(0);
static ARENAS: AtomicU64 = AtomicU64::new(0);

/// a module group's chunks, and a generation's arena chunks, grow to this size
const MODULE_CHUNK_MAX: u64 = 64 << 10;

/// The group new code of slot `id` goes to (`gen`: the install's patch generation, 0 outside a
/// patch; `inlined`: the slots an O3 build inlined, beside the slot's body_deps), None:
/// bump-shared code (never invalidated).
pub(crate) fn group_of(u: &Unit, id: u32, gen: u32, inlined: &[u32]) -> Option<Group> {
    let f = &u.fns[id as usize];
    // the generation whose source the code is built from: a patch's own, a slot's last patch (its
    // O3 code, a lazy rebuild), a twin's base's
    let own = if gen > 0 {
        gen
    } else {
        match f.key {
            // (a twin's base may be no slot: the sandbox's invalid-fn-pointer stand-in)
            FnKey::Plain(b) | FnKey::Sandbox(b) | FnKey::Ctfe(b) if (b as usize) < u.fns.len() => source_gen(u, id).max(source_gen(u, b)),
            _ => source_gen(u, id),
        }
    };
    // ... or the latest generation of code it inlined (code built after a patch that took in a
    // patched body: an unpatched caller's lazy first compile, its O3 tier-up): it is that
    // generation's code too, so a rollback of it (or a later layout patch) invalidates it
    let g = own.max(inlined_gen(u, id, inlined));
    if crate::modules::any_module_crate() {
        let base = match f.key {
            FnKey::Sandbox(b) | FnKey::Ctfe(b) => b,
            FnKey::Inst(..) | FnKey::Closure(..) => id,
            _ => u32::MAX,
        };
        if (base as usize) < u.fns.len() {
            if let Some(k) = slot_crate(u, base).filter(|&k| crate::modules::is_module_crate(k)) {
                return Some(Group::Module { krate: k, gen: g });
            }
        }
    }
    (g > 0).then_some(Group::Patch { slot: id, gen: g })
}

/// The patch generation whose source slot `id`'s next code is built from: its last patch, or
/// the generation that made its item's file version (a fn first built after a patch of its file:
/// a lazy first compile, an O3 build of a fn the patch did not rebuild).
fn source_gen(u: &Unit, id: u32) -> u32 {
    let f = &u.fns[id as usize];
    if u.file_gen.is_empty() {
        return f.patch_gen;
    }
    f.patch_gen.max(u.file_gen.get(&u.key_file(&f.key)).copied().unwrap_or(0))
}

/// The latest patch generation among the slots whose RIR the code of `id` took in: `inlined`
/// and `Unit::body_deps` (of `id`, a twin's base, transitively; body_deps only grows, so this may
/// name a generation the code no longer holds: that costs a page, never a missed invalidation),
/// by `source_gen` (an inlined fn may never have had code of its own: an `#[inline(always)]`
/// helper, so its patch_gen alone does not say which source it was built from).
/// 0 before the first patch.
pub(crate) fn inlined_gen(u: &Unit, id: u32, inlined: &[u32]) -> u32 {
    if u.patch_gen == 0 {
        return 0;
    }
    let base = match u.fns[id as usize].key {
        FnKey::Plain(b) | FnKey::Sandbox(b) | FnKey::Ctfe(b) if (b as usize) < u.fns.len() => Some(b),
        _ => None,
    };
    let mut stack: Vec<u32> = inlined.iter().copied().chain(u.body_deps.get(&id).into_iter().flatten().copied()).chain(base.into_iter().flat_map(|b| u.body_deps.get(&b).into_iter().flatten().copied())).collect();
    if stack.is_empty() {
        return 0;
    }
    let mut seen = crate::fxhash::HashSet::default();
    seen.insert(id);
    let mut g = 0;
    while let Some(c) = stack.pop() {
        if c as usize >= u.fns.len() || !seen.insert(c) {
            continue;
        }
        g = g.max(source_gen(u, c));
        if let Some(v) = u.body_deps.get(&c) {
            stack.extend(v.iter().copied());
        }
    }
    g
}

fn slot_crate(u: &Unit, id: u32) -> Option<u32> {
    match &u.fns[id as usize].key {
        FnKey::Inst(d, _) => Some(u.prog.def(*d).krate),
        FnKey::Closure(f, _, _) => u.prog.file_crate.get(*f as usize).copied(),
        _ => None,
    }
}

/// `len` bytes of code memory in group `g`'s chunks (a new chunk from the code window when the
/// group's last one is full). Err: the group was invalidated (nothing installs into it again).
pub(crate) fn alloc(u: &Unit, g: Group, len: u64, chunk: impl FnOnce(u64, u64) -> u64) -> Result<u64, String> {
    let mut map = u.code_groups.lock().unwrap_or_else(|e| e.into_inner());
    let _k = crate::zone_alloc::kept();
    let CodeGroups { groups, arenas } = &mut *map;
    let e = groups.entry(g).or_insert_with(|| {
        GROUPS.fetch_add(1, Ordering::Relaxed);
        GroupCode::default()
    });
    if let Some(r) = &e.invalidated {
        return Err(format!("its code group was invalidated ({})", r));
    }
    let need = (len.max(1) + 15) & !15;
    let page = crate::os::mem::page_size();
    // chunks double up to 64 KB: a module's own, a generation's for its patch groups
    let size = |chunks: u64| (need.max(chunks.clamp(page, MODULE_CHUNK_MAX)) + page - 1) / page * page;
    let at = match g {
        Group::Module { .. } => {
            if e.next + need > e.end {
                let size = size(e.chunks.iter().map(|c| c.1 - c.0).sum());
                let at = chunk(size, page);
                CHUNK_BYTES.fetch_add(size, Ordering::Relaxed);
                e.chunks.push((at, at + size));
                e.next = at;
                e.end = at + size;
            }
            let at = e.next;
            e.next += need;
            at
        }
        Group::Patch { gen, .. } => {
            let a = arenas.entry(gen).or_insert_with(|| {
                ARENAS.fetch_add(1, Ordering::Relaxed);
                Arena::default()
            });
            if a.next + need > a.end {
                let size = size(a.chunks.iter().map(|c| c.hi - c.lo).sum());
                let at = chunk(size, page);
                CHUNK_BYTES.fetch_add(size, Ordering::Relaxed);
                a.chunks.push(Chunk { lo: at, hi: at + size, live: 0, closed: false });
                a.next = at;
                a.end = at + size;
            }
            let at = a.next;
            a.next += need;
            a.chunks.last_mut().expect("an arena chunk").live += 1;
            e.ranges.push((at, at + need));
            at
        }
    };
    e.code_bytes += len;
    GROUP_CODE_BYTES.fetch_add(len, Ordering::Relaxed);
    Ok(at)
}

/// Records an install into group `g` (after the slot took the code: its version is known).
pub(crate) fn note_install(u: &Unit, g: Group, slot: u32, at: u64, len: u32, version: u32) {
    let mut map = u.code_groups.lock().unwrap_or_else(|e| e.into_inner());
    let _k = crate::zone_alloc::kept();
    if let Some(e) = map.groups.get_mut(&g) {
        e.installs.push(Install { slot, at, len, version, t_ns: crate::clock::wall_ns() });
    }
}

/// Whether group `g` was invalidated (an O3 build for it is dropped).
pub(crate) fn is_dead(u: &Unit, g: Group) -> bool {
    u.code_groups.lock().unwrap_or_else(|e| e.into_inner()).groups.get(&g).is_some_and(|e| e.invalidated.is_some())
}

/// What invalidated a range (stale.rs names it; inspect `stale` explains it).
pub struct Why {
    pub lo: u64,
    pub hi: u64,
    pub group: Group,
    /// the patch generation whose event caused it (0: a module unload)
    pub by_gen: u32,
    pub installs: Vec<Install>,
}

static WHYS: relax_session::sync::Mutex<Vec<Why>> = relax_session::sync::Mutex::new(Vec::new());

/// The invalidation record of the range holding `addr`.
pub fn why_at<R>(addr: u64, f: impl FnOnce(&Why) -> R) -> Option<R> {
    let w = WHYS.lock().unwrap_or_else(|e| e.into_inner());
    w.iter().find(|w| addr >= w.lo && addr < w.hi).map(f)
}

/// Invalidates every live group `pick` selects: each range is registered as a stale range, then
/// made unrunnable -- a module's chunks and the arena chunks nothing lives in any more
/// inaccessible, a patch group's ranges in a chunk that holds live code too trap-filled
/// (`trap_fill`). A slot of a patch group whose current code is in it goes back to its lazy stub
/// (it builds again from the source it runs now); a module's slots keep their words (a call of an
/// unloaded module's code is a stale access). Returns (groups, bytes).
/// `kind` / `name` are what a stale access into it says ("stale access into the <kind> `<name>`"):
/// "invalidated code" and the fns it held, or "unloaded module" and the module's name.
pub(crate) fn invalidate(u: &mut Unit, pick: impl Fn(Group) -> bool, by_gen: u32, kind: &'static str, name: Option<&str>, reason: &str) -> (usize, u64) {
    // (group, its ranges: a module's chunks, a patch group's arena ranges; installs)
    let mut dead: Vec<(Group, Vec<(u64, u64)>, Vec<Install>)> = Vec::new();
    // arena chunks nothing lives in now; dead ranges in chunks that hold live code (chunk lo, lo, hi)
    let mut closed: Vec<(u64, u64)> = Vec::new();
    let mut fill: Vec<(u64, u64, u64)> = Vec::new();
    {
        let mut map = u.code_groups.lock().unwrap_or_else(|e| e.into_inner());
        let _k = crate::zone_alloc::kept();
        let CodeGroups { groups, arenas } = &mut *map;
        for (g, e) in groups.iter_mut() {
            if e.invalidated.is_some() || !pick(*g) {
                continue;
            }
            e.invalidated = Some(reason.to_string());
            e.next = e.end;
            match *g {
                Group::Module { .. } => dead.push((*g, e.chunks.clone(), e.installs.clone())),
                Group::Patch { gen, .. } => {
                    if let Some(a) = arenas.get_mut(&gen) {
                        for &(lo, _) in &e.ranges {
                            let k = a.chunks.partition_point(|c| c.hi <= lo);
                            a.chunks[k].live -= 1;
                        }
                    }
                    dead.push((*g, e.ranges.clone(), e.installs.clone()));
                }
            }
        }
        // (once every picked group is out: a rollback's chunks hold nothing live, a layout
        // patch's mostly hold live code beside the dead)
        for (g, ranges, _) in &dead {
            let Group::Patch { gen, .. } = *g else { continue };
            let Some(a) = arenas.get_mut(&gen) else { continue };
            for &(lo, hi) in ranges {
                let k = a.chunks.partition_point(|c| c.hi <= lo);
                let c = &mut a.chunks[k];
                if c.live == 0 && !c.closed {
                    c.closed = true;
                    closed.push((c.lo, c.hi));
                    if a.end == c.hi {
                        a.next = a.end;
                    }
                } else if !c.closed {
                    fill.push((c.lo, lo, hi));
                }
            }
        }
    }
    let mut bytes = 0;
    for (g, ranges, installs) in &dead {
        let mut names: Vec<String> = Vec::new();
        for i in installs {
            let n = u.fns[i.slot as usize].name.to_string();
            if !names.contains(&n) {
                names.push(n);
            }
        }
        let name = match name {
            Some(n) => n.to_string(),
            None if names.len() > 3 => format!("{}, .. ({} fns)", names[..3].join(", "), names.len()),
            None => names.join(", "),
        };
        let version = installs.iter().map(|i| i.version).max().unwrap_or(0) as u64;
        // (a patch group's ranges one after the other, as one range)
        let mut runs: Vec<(u64, u64)> = Vec::new();
        for &(lo, hi) in ranges {
            match runs.last_mut() {
                Some(r) if r.1 == lo => r.1 = hi,
                _ => runs.push((lo, hi)),
            }
        }
        for &(lo, hi) in &runs {
            crate::zone_alloc::kept_with(|| WHYS.lock().unwrap_or_else(|e| e.into_inner()).push(Why { lo, hi, group: *g, by_gen, installs: installs.clone() }));
            crate::stale::register(lo, hi, kind, name.clone(), version, reason.to_string());
        }
        let n: u64 = ranges.iter().map(|r| r.1 - r.0).sum();
        if matches!(g, Group::Module { .. }) {
            for &(lo, hi) in ranges {
                crate::os::mem::invalidate_code(lo, (hi - lo) as usize);
            }
            PROT_BYTES.fetch_add(n, Ordering::Relaxed);
        }
        bytes += n;
        DEAD_GROUPS.fetch_add(1, Ordering::Relaxed);
        DEAD_BYTES.fetch_add(n, Ordering::Relaxed);
    }
    for &(lo, hi) in &closed {
        crate::os::mem::invalidate_code(lo, (hi - lo) as usize);
        PROT_BYTES.fetch_add(hi - lo, Ordering::Relaxed);
    }
    // the slots off the dead code before it is filled: calls go to their stubs from here on
    for (g, _, installs) in &dead {
        if matches!(g, Group::Patch { .. }) {
            for i in installs {
                u.stub_if_invalidated(i.slot);
            }
        }
    }
    trap_fill(&mut fill);
    if !dead.is_empty() {
        crate::inspect::flightrec::event("code.invalidate", &format!("{} group(s), {} KB ({} KB of chunks made inaccessible, {} ranges trap-filled): {}", dead.len(), bytes / 1024, closed.iter().map(|c| c.1 - c.0).sum::<u64>() / 1024, fill.len(), reason), by_gen as u64, bytes);
    }
    (dead.len(), bytes)
}

/// What a dead range is filled with: an instruction that faults (SIGILL) at every offset a fetch
/// can start at -- x86-64: 0x06, one byte, invalid in 64-bit mode; arm64: UDF #0xdead, a word
/// (fetches start at words only). Not a breakpoint: debuggers and tracers take those.
#[cfg(target_arch = "x86_64")]
pub(crate) const TRAP: [u8; 4] = [0x06; 4];
#[cfg(target_arch = "aarch64")]
pub(crate) const TRAP: [u8; 4] = 0x0000_dead_u32.to_le_bytes();

/// The code pages a trap fill has out of reach: [lo, hi) while TRANSIT_SEQ is odd (the last
/// fill's after it). The fault side reads them with atomic loads only (`transit_wait`).
static TRANSIT_SEQ: AtomicU64 = AtomicU64::new(0);
static TRANSIT_LO: AtomicU64 = AtomicU64::new(0);
static TRANSIT_HI: AtomicU64 = AtomicU64::new(0);
/// Fills, the faults that waited for one, the longest wait (ns).
static FILLS: AtomicU64 = AtomicU64::new(0);
pub(crate) static TRANSIT_WAITS: AtomicU64 = AtomicU64::new(0);
static TRANSIT_WAIT_MAX_NS: AtomicU64 = AtomicU64::new(0);
/// One fill at a time (the transit pages are one range).
static FILLING: relax_session::sync::Mutex<()> = relax_session::sync::Mutex::new(());
/// A test holds the pages out of reach this long, so a neighbour surely touches them.
#[cfg(test)]
pub(crate) static TRANSIT_HOLD_US: AtomicU64 = AtomicU64::new(0);

/// Overwrites dead ranges (chunk lo, lo, hi) with `TRAP`, per run of the pages they are on:
/// 1. the pages out of every thread's reach (os::mem::Transit::Out: the TLB shootdown makes it
///    so on every core), the fault side told first;
/// 2. the trap written through the RW alias;
/// 3. readable, the instruction cache told (arm64: nothing executes them meanwhile);
/// 4. executable again.
/// A thread that fetches or reads them meanwhile waits in its fault handler and runs its
/// instruction again: a live neighbour's runs on, a dead range's traps. No core can fetch the
/// dead bytes, or read a dead range's literal pool, while they change. Registration and the
/// slots' stubs come first (`invalidate`); nothing here takes a lock a faulting thread can hold.
fn trap_fill(ranges: &mut [(u64, u64, u64)]) {
    if ranges.is_empty() {
        return;
    }
    let page = crate::os::mem::page_size();
    ranges.sort_unstable();
    let _one = FILLING.lock().unwrap_or_else(|e| e.into_inner());
    let mut k = 0;
    while k < ranges.len() {
        // the ranges of one chunk whose pages touch: one transit
        let (chunk, lo, _) = ranges[k];
        let span_lo = lo & !(page - 1);
        let mut span_hi = (ranges[k].2 + page - 1) & !(page - 1);
        let mut j = k + 1;
        while j < ranges.len() && ranges[j].0 == chunk && ranges[j].1 & !(page - 1) <= span_hi {
            span_hi = span_hi.max((ranges[j].2 + page - 1) & !(page - 1));
            j += 1;
        }
        let len = (span_hi - span_lo) as usize;
        TRANSIT_LO.store(span_lo, Ordering::SeqCst);
        TRANSIT_HI.store(span_hi, Ordering::SeqCst);
        TRANSIT_SEQ.fetch_add(1, Ordering::SeqCst);
        crate::os::mem::code_transit(span_lo, len, crate::os::mem::Transit::Out);
        for &(_, lo, hi) in &ranges[k..j] {
            let n = (hi - lo) as usize;
            let w = crate::os::mem::code_alias(lo, n) as *mut u8;
            for q in (0..n).step_by(4) {
                unsafe { std::ptr::copy_nonoverlapping(TRAP.as_ptr(), w.add(q), 4) };
            }
            FILLED_BYTES.fetch_add(hi - lo, Ordering::Relaxed);
        }
        #[cfg(test)]
        {
            let hold = TRANSIT_HOLD_US.load(Ordering::Relaxed);
            if hold > 0 {
                std::thread::sleep(std::time::Duration::from_micros(hold));
            }
        }
        #[cfg(target_arch = "aarch64")]
        crate::os::mem::code_transit(span_lo, len, crate::os::mem::Transit::Read);
        for &(_, lo, hi) in &ranges[k..j] {
            unsafe { crate::os::mem::flush_code(lo, (hi - lo) as usize) };
        }
        crate::os::mem::code_transit(span_lo, len, crate::os::mem::Transit::Back);
        TRANSIT_SEQ.fetch_add(1, Ordering::SeqCst);
        FILLS.fetch_add(1, Ordering::Relaxed);
        k = j;
    }
}

/// A fault at data address `addr`, pc `pc` (the fault side, signal context: atomic loads and a
/// spin, no lock, no allocation): Some(the fill's number) when it touched pages a trap fill has
/// out of reach -- or just had: the fault may be seen after the fill ended, and is then that
/// fill's, never the next one's -- once no fill has them; the instruction then runs again (jit.rs
/// `transit_retry`). The wait is one fill's length (microseconds: a few KB written, three
/// protection changes).
pub(crate) fn transit_wait(addr: u64, pc: u64) -> Option<u64> {
    let s = TRANSIT_SEQ.load(Ordering::SeqCst);
    if s == 0 {
        return None;
    }
    let (lo, hi) = (TRANSIT_LO.load(Ordering::SeqCst), TRANSIT_HI.load(Ordering::SeqCst));
    if !(addr >= lo && addr < hi) && !(pc >= lo && pc < hi) {
        return None;
    }
    if s & 1 == 1 {
        let t = std::time::Instant::now();
        let mut spins = 0u32;
        while TRANSIT_SEQ.load(Ordering::SeqCst) == s {
            spins += 1;
            if spins < 64 {
                std::hint::spin_loop();
            } else {
                std::thread::yield_now();
            }
        }
        TRANSIT_WAITS.fetch_add(1, Ordering::Relaxed);
        TRANSIT_WAIT_MAX_NS.fetch_max(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
        return Some(s);
    }
    // (seen after its fill ended: that fill's. Named the next one's, the same instruction's
    // fault in the next fill would be taken for one after it ran again, and end the process)
    Some(s - 1)
}

/// The patch generations recorded in the flight recorder (`reload.patch`): gen -> (wall ns, text),
/// so `stale` names the patch event that invalidated the code and the one that made it.
static PATCH_EVENTS: relax_session::sync::Mutex<Vec<(u32, u64, String)>> = relax_session::sync::Mutex::new(Vec::new());

/// A patch generation was installed: a `reload.patch` flight-recorder event (and its copy here).
pub fn patch_event(gen: u32, text: String) {
    crate::inspect::flightrec::event("reload.patch", &text, gen as u64, 0);
    let t = crate::clock::wall_ns();
    let mut v = PATCH_EVENTS.lock().unwrap_or_else(|e| e.into_inner());
    crate::zone_alloc::kept_with(|| {
        v.push((gen, t, text));
        if v.len() > 4096 {
            v.remove(0);
        }
    });
}

/// The recorded `reload.patch` event of generation `gen`: (wall ns, text).
pub fn patch_event_of(gen: u32) -> Option<(u64, String)> {
    PATCH_EVENTS.lock().unwrap_or_else(|e| e.into_inner()).iter().rev().find(|e| e.0 == gen).map(|e| (e.1, e.2.clone()))
}

/// `makepad inspect <app> code`: the invalidatable groups' share of the code window.
pub fn stats_line() -> String {
    let kb = |a: &AtomicU64| a.load(Ordering::Relaxed) as f64 / 1024.0;
    let (c, b) = (CHUNK_BYTES.load(Ordering::Relaxed), GROUP_CODE_BYTES.load(Ordering::Relaxed));
    format!(
        "invalidatable code: {} groups ({} patch generations' arenas): {:.1} KB of chunks for {:.1} KB of code (+{:.1} KB rounding and arena tails); invalidated (never reused): {} groups, {:.1} KB -- {:.1} KB of chunks inaccessible (PROT_NONE), {:.1} KB trap-filled in {} fills ({} faults waited for a fill, the longest {:.1} us)",
        GROUPS.load(Ordering::Relaxed),
        ARENAS.load(Ordering::Relaxed),
        c as f64 / 1024.0,
        b as f64 / 1024.0,
        c.saturating_sub(b) as f64 / 1024.0,
        DEAD_GROUPS.load(Ordering::Relaxed),
        kb(&DEAD_BYTES),
        kb(&PROT_BYTES),
        kb(&FILLED_BYTES),
        FILLS.load(Ordering::Relaxed),
        TRANSIT_WAITS.load(Ordering::Relaxed),
        TRANSIT_WAIT_MAX_NS.load(Ordering::Relaxed) as f64 / 1000.0,
    )
}

/// A layout-changing patch (gen `gen`, `layout`: the types whose fields or variants changed),
/// with the program threads paused (inspect::instrument::pause_for_patch, before its install;
/// `paused`: that call paused them) and its code installed: the threads with a frame in the
/// replaced code of a slot the patch rebuilt are held for good and reported (stale::park_held);
/// every other thread resumes. Returns how many were held. The patch was asked first whether a
/// thread is in such code (`frame_in`, under the same pause: it then does not install), so this
/// finds only a thread the pause did not stop (Linux: one blocked in a read or a poll that came
/// back and called into it before the install).
///
/// Why frames only: the earlier patch versions of those slots are in groups (invalidated by the
/// patch, trap-filled or PROT_NONE: a return into them parks by its fault). A slot's first code shares its pages
/// with other code and cannot be made inaccessible; it is reached only through its slot (calls go
/// through the slot word, fn pointers and vtables hold thunks: the install has swapped the words)
/// or through a frame that was live at the patch. Those frames are what this finds. A thread
/// blocked in a system call is not held by the pause; its stack is walked where it blocks and,
/// with such a frame, it is held now (microseconds after the walk).
pub(crate) fn hold_old_frames(u: &Unit, gen: u32, layout: &str, paused: bool) -> usize {
    let affected: crate::fxhash::HashSet<u32> = u.patch_log.iter().filter(|(_, g)| *g == gen).map(|(id, _)| *id).collect();
    let caps = if affected.is_empty() { Ok(Vec::new()) } else { crate::inspect::capture_all(&crate::inspect::instrument::program_thread) };
    let mut held = Vec::new();
    match caps {
        Err(e) => eprintln!("relax: layout patch gen {}: the program's stacks could not be read ({}): a thread still in first-version code of a rebuilt fn is not held", gen, e),
        Ok(caps) => {
            for c in caps {
                // pc, return addresses; a leaf's caller (arm64 lr; x86-64 the word at sp: a
                // frameless leaf, a prologue)
                let mut at: Vec<(usize, u64)> = c.frames.iter().enumerate().map(|(k, &a)| (k, if k == 0 { a } else { a.wrapping_sub(1) })).collect();
                if c.lr != 0 {
                    at.push((usize::MAX, c.lr.wrapping_sub(1)));
                }
                #[cfg(target_arch = "x86_64")]
                if c.sp != 0 && c.err.is_none() {
                    if let Some(w) = crate::inspect::threads::read_mem(c.sp, 8) {
                        at.push((usize::MAX, u64::from_le_bytes(w[..8].try_into().unwrap()).wrapping_sub(1)));
                    }
                }
                // (the code tables' lock for the lookup alone: never across the thread's hold)
                let hit = {
                    let code = u.code.read().unwrap_or_else(|e| e.into_inner());
                    at.iter().find_map(|&(k, a)| {
                        let (start, len, slot) = code.entry_at(a)?;
                        (affected.contains(&slot) && start != u.fns[slot as usize].code && crate::stale::lookup(a).is_none()).then_some((k, start, len, slot))
                    })
                };
                if let Some((k, start, len, slot)) = hit {
                    let ok = crate::inspect::instrument::hold_for_good(&c.t);
                    let pc = at.iter().find(|x| x.0 == k).map_or(start, |x| x.1);
                    let path = if crate::inspect::codever::in_patched_path(u, slot, start, pc) { ", its patched path" } else { "" };
                    let what = format!("slot #{} `{}` first-version code [{:#x}, {:#x}) (+{:#x}{})", slot, u.fns[slot as usize].name, start, start + len as u64, pc - start, path);
                    held.push((c.t.os_id, c.frames.clone(), k, what, ok));
                }
            }
        }
    }
    if paused {
        crate::inspect::instrument::resume_program();
    }
    let n = held.len();
    let reason = format!("layout-changing patch gen {} (fields or variants of {}) replaced it: code built for the old layout never runs again", gen, layout);
    for (os_id, frames, k, what, ok) in held {
        crate::stale::park_held(os_id, frames, k, what, reason.clone(), ok);
    }
    n
}

/// A patch that changes a struct's layout in place (reload.rs fresh_layout_struct), about to
/// install with the program threads paused (inspect::instrument::pause_for_patch): the first frame
/// of a program thread in code of one of `slots` (any version: what the patch rebuilds was built
/// for the old layout; named with what of retired text it runs or reaches, below, when it does),
/// in a slot of text the program no longer has (reload::retired_text: a fn
/// an edit removed or replaced, a closure inside one, a closure a patch kept on its old code), in
/// a version that holds a copy of such code (an inlining tier's), in a slot that can still call
/// such code (reload::over_retired: an instance over a kept closure),
/// or in an earlier version of any slot (code
/// of a text no longer the program's, whose types are not asked), named for the restart's reason. Every thread's frames
/// are read, those of a walk that ends short too: a frame found is the answer. Err: none was
/// found, and a stack could not be read to its end (what it hides is not known).
pub(crate) fn frame_in(u: &Unit, slots: &crate::fxhash::HashSet<u32>) -> Result<Option<String>, String> {
    let caps = crate::inspect::capture_all(&crate::inspect::instrument::program_thread)?;
    // (a thread is named as reports name it: one nobody named is its program thread, or `unnamed`)
    let listed: Vec<crate::inspect::threads::Thread> = caps.iter().map(|c| c.t.clone()).collect();
    // the hosts of copies of a closure a patch kept on its old code, for linked code below O3 (the
    // image and disk-cache links note `inlined_into`, not a list per code body)
    let hosts: crate::fxhash::HashMap<u32, u32> = u.kept_old.keys().flat_map(|&k| u.fns[k as usize].history().inlined_into.iter().map(move |&h| (h, k))).collect();
    // a closure by the fn that makes it and its place, not by the JIT's number for it
    let shown = |slot: u32| crate::reload::slot_shown(u, slot);
    let mut unread = None;
    for c in caps {
        if let Some(e) = c.err {
            unread.get_or_insert_with(|| format!("thread {}: {}", c.t.os_id, e));
            continue;
        }
        if let Some(why) = c.stops() {
            unread.get_or_insert_with(|| format!("thread {}: {}", c.t.os_id, why));
        }
        let mut at: Vec<u64> = c.frames.iter().enumerate().map(|(k, &a)| if k == 0 { a } else { a.wrapping_sub(1) }).collect();
        if c.lr != 0 {
            at.push(c.lr.wrapping_sub(1));
        }
        #[cfg(target_arch = "x86_64")]
        if c.sp != 0 {
            if let Some(w) = crate::inspect::threads::read_mem(c.sp, 8) {
                at.push(u64::from_le_bytes(w[..8].try_into().unwrap()).wrapping_sub(1));
            }
        }
        let thread = format!("`{}` (tid {})", crate::inspect::shown_name(&c.t, &listed), c.t.os_id);
        for a in at {
            // (the code tables' lock for the lookups alone: naming a closure below asks type
            // queries, and one can run code whose install takes the lock to write)
            let found = {
                let code = u.code.read().unwrap_or_else(|e| e.into_inner());
                code.entry_at(a).map(|(start, _, slot)| (start, slot, code.inlined_at(start).map(|l| l.to_vec()), code.kind_of(start).origin))
            };
            let Some((start, slot, inlined, origin)) = found else { continue };
            let f = &u.fns[slot as usize];
            // what of retired text the frame runs or reaches; for a slot the patch rebuilds too, so
            // the reason names both
            let retired = || {
                if let Some(retired) = crate::reload::retired_text(u, slot) {
                    return Some(retired);
                }
                // a copy of such code inlined into this version
                // (code built in this run lists what it inlined when it inlined anything: none listed,
                // none taken in, whatever a later version of the slot took)
                let copy = match inlined.as_deref() {
                    Some(list) => list.iter().find_map(|&c| crate::reload::retired_text(u, c).map(|r| (c, r))),
                    None if matches!(origin, crate::jit::origin::CACHE | crate::jit::origin::IMAGE) => hosts.get(&slot).and_then(|&c| crate::reload::retired_text(u, c).map(|r| (c, r))),
                    None => None,
                };
                if let Some((c, retired)) = copy {
                    return Some(format!("in code that holds a copy of {}, {}", shown(c), retired));
                }
                crate::reload::over_retired(u, slot).map(|over| format!("and can still call {over}"))
            };
            if slots.contains(&slot) {
                let more = retired().map_or_else(String::new, |r| format!(" {r}"));
                return Ok(Some(format!("{} is running on thread {}{}", shown(slot), thread, more)));
            }
            if let Some(r) = retired() {
                return Ok(Some(format!("{} runs on thread {} {}", shown(slot), thread, r)));
            }
            if start != f.code {
                return Ok(Some(format!("{} runs code from before an earlier edit on thread {}", shown(slot), thread)));
            }
        }
    }
    match unread {
        Some(e) => Err(e),
        None => Ok(None),
    }
}

/// What this module holds of a run, as a fresh process has it (`run_end`). Nothing of the run is left that could see it.
pub(crate) unsafe fn reset_statics() {
    use relax_session::reset_static as reset;
    reset(&GROUPS, AtomicU64::new(0));
    reset(&CHUNK_BYTES, AtomicU64::new(0));
    reset(&GROUP_CODE_BYTES, AtomicU64::new(0));
    reset(&DEAD_GROUPS, AtomicU64::new(0));
    reset(&DEAD_BYTES, AtomicU64::new(0));
    reset(&PROT_BYTES, AtomicU64::new(0));
    reset(&FILLED_BYTES, AtomicU64::new(0));
    reset(&ARENAS, AtomicU64::new(0));
    reset(&FILLS, AtomicU64::new(0));
    reset(&TRANSIT_WAITS, AtomicU64::new(0));
    reset(&TRANSIT_WAIT_MAX_NS, AtomicU64::new(0));
    // (no fill runs at a run's end: they run in a layout patch or a rollback, on the owner)
    reset(&TRANSIT_SEQ, AtomicU64::new(0));
    reset(&TRANSIT_LO, AtomicU64::new(0));
    reset(&TRANSIT_HI, AtomicU64::new(0));
    reset(&FILLING, relax_session::sync::Mutex::new(()));
    reset(&WHYS, relax_session::sync::Mutex::new(Vec::new()));
    reset(&PATCH_EVENTS, relax_session::sync::Mutex::new(Vec::new()));
}
