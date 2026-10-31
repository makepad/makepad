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
//! module's code from one generation. A group's code lies in page-aligned chunks of the unit's
//! code window that hold nothing else, so invalidating it is one `mprotect(PROT_NONE)` per chunk
//! (the kernel's TLB shootdown makes it immediate on every core). Every chunk is registered with
//! `stale::register` first. Addresses are never reused. Code of a run's first generation (every
//! slot's first code, std's twins, glue) stays bump-shared: it is never invalidated.
//!
//! Code built after a patch from that patch's source is the patch's code too (`group_of`): a fn
//! first compiled after a patch of its file, and an unpatched caller whose lazy first compile or
//! O3 tier-up inlined patched bodies (the latest generation over what it took in). A rollback of
//! the generation (or a later layout patch) invalidates it with the rest.
//!
//! First code cannot be made inaccessible (it shares pages). A layout-changing patch therefore
//! pauses the program while it installs and holds every thread with a frame in the first code of
//! a fn it rebuilt (`hold_old_frames`, stale.rs `park_held`): such a thread never returns there.

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

/// A group's chunks and installs.
#[derive(Default)]
pub struct GroupCode {
    /// [lo, hi), page-aligned, never shared with other code
    pub chunks: Vec<(u64, u64)>,
    next: u64,
    end: u64,
    pub installs: Vec<Install>,
    pub code_bytes: u64,
    /// why it was invalidated (None: live)
    pub invalidated: Option<String>,
}

/// Totals for `makepad inspect <app> code`: groups, chunk bytes (what the groups took of the code
/// window: page rounding included), code bytes in them, invalidated groups and their chunk bytes.
pub static GROUPS: AtomicU64 = AtomicU64::new(0);
pub static CHUNK_BYTES: AtomicU64 = AtomicU64::new(0);
pub static GROUP_CODE_BYTES: AtomicU64 = AtomicU64::new(0);
pub static DEAD_GROUPS: AtomicU64 = AtomicU64::new(0);
pub static DEAD_BYTES: AtomicU64 = AtomicU64::new(0);

/// a module group's chunks grow to this size (one module's code: many fns)
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
            FnKey::Plain(b) | FnKey::Sandbox(b) if (b as usize) < u.fns.len() => source_gen(u, id).max(source_gen(u, b)),
            _ => source_gen(u, id),
        }
    };
    // ... or the latest generation of code it inlined (code built after a patch that took in a
    // patched body: an unpatched caller's lazy first compile, its O3 tier-up): it is that
    // generation's code too, so a rollback of it (or a later layout patch) invalidates it
    let g = own.max(inlined_gen(u, id, inlined));
    if crate::modules::any_module_crate() {
        let base = match f.key {
            FnKey::Sandbox(b) => b,
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
        FnKey::Plain(b) | FnKey::Sandbox(b) if (b as usize) < u.fns.len() => Some(b),
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
    let e = map.entry(g).or_insert_with(|| {
        GROUPS.fetch_add(1, Ordering::Relaxed);
        GroupCode::default()
    });
    if let Some(r) = &e.invalidated {
        return Err(format!("its code group was invalidated ({})", r));
    }
    let need = (len.max(1) + 15) & !15;
    if e.next + need > e.end {
        let page = crate::os::mem::page_size();
        // a patch group mostly gets one fn (and maybe its O3 code later): no slack; a module's
        // chunks double up to 64 KB
        let grow = match g {
            Group::Module { .. } => e.chunks.iter().map(|c| c.1 - c.0).sum::<u64>().clamp(page, MODULE_CHUNK_MAX),
            Group::Patch { .. } => 0,
        };
        let size = (need.max(grow) + page - 1) / page * page;
        let at = chunk(size, page);
        CHUNK_BYTES.fetch_add(size, Ordering::Relaxed);
        e.chunks.push((at, at + size));
        e.next = at;
        e.end = at + size;
    }
    let at = e.next;
    e.next += need;
    e.code_bytes += len;
    GROUP_CODE_BYTES.fetch_add(len, Ordering::Relaxed);
    Ok(at)
}

/// Records an install into group `g` (after the slot took the code: its version is known).
pub(crate) fn note_install(u: &Unit, g: Group, slot: u32, at: u64, len: u32, version: u32) {
    let mut map = u.code_groups.lock().unwrap_or_else(|e| e.into_inner());
    let _k = crate::zone_alloc::kept();
    if let Some(e) = map.get_mut(&g) {
        e.installs.push(Install { slot, at, len, version, t_ns: crate::clock::wall_ns() });
    }
}

/// Whether group `g` was invalidated (an O3 build for it is dropped).
pub(crate) fn is_dead(u: &Unit, g: Group) -> bool {
    u.code_groups.lock().unwrap_or_else(|e| e.into_inner()).get(&g).is_some_and(|e| e.invalidated.is_some())
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

/// Invalidates every live group `pick` selects: each chunk is registered as a stale range, then
/// made inaccessible. A slot of a patch group whose current code is in it goes back to its lazy
/// stub (it builds again from the source it runs now); a module's slots keep their words (a call
/// of an unloaded module's code is a stale access). Returns (groups, chunk bytes).
/// `kind` / `name` are what a stale access into it says ("stale access into the <kind> `<name>`"):
/// "invalidated code" and the fns it held, or "unloaded module" and the module's name.
pub(crate) fn invalidate(u: &mut Unit, pick: impl Fn(Group) -> bool, by_gen: u32, kind: &'static str, name: Option<&str>, reason: &str) -> (usize, u64) {
    let mut dead: Vec<(Group, Vec<(u64, u64)>, Vec<Install>)> = Vec::new();
    {
        let mut map = u.code_groups.lock().unwrap_or_else(|e| e.into_inner());
        let _k = crate::zone_alloc::kept();
        for (g, e) in map.iter_mut() {
            if e.invalidated.is_some() || !pick(*g) {
                continue;
            }
            e.invalidated = Some(reason.to_string());
            e.next = e.end;
            dead.push((*g, e.chunks.clone(), e.installs.clone()));
        }
    }
    let mut bytes = 0;
    for (g, chunks, installs) in &dead {
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
        for &(lo, hi) in chunks {
            crate::zone_alloc::kept_with(|| WHYS.lock().unwrap_or_else(|e| e.into_inner()).push(Why { lo, hi, group: *g, by_gen, installs: installs.clone() }));
            crate::stale::register(lo, hi, kind, name.clone(), version, reason.to_string());
        }
        for &(lo, hi) in chunks {
            crate::os::mem::invalidate_code(lo, (hi - lo) as usize);
            bytes += hi - lo;
        }
        DEAD_GROUPS.fetch_add(1, Ordering::Relaxed);
        DEAD_BYTES.fetch_add(chunks.iter().map(|c| c.1 - c.0).sum::<u64>(), Ordering::Relaxed);
        if matches!(g, Group::Patch { .. }) {
            for i in installs {
                u.stub_if_invalidated(i.slot);
            }
        }
    }
    if !dead.is_empty() {
        crate::inspect::flightrec::event("code.invalidate", &format!("{} group(s), {} KB: {}", dead.len(), bytes / 1024, reason), by_gen as u64, bytes);
    }
    (dead.len(), bytes)
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
    let (g, c, b, dg, db) = (GROUPS.load(Ordering::Relaxed), CHUNK_BYTES.load(Ordering::Relaxed), GROUP_CODE_BYTES.load(Ordering::Relaxed), DEAD_GROUPS.load(Ordering::Relaxed), DEAD_BYTES.load(Ordering::Relaxed));
    format!(
        "invalidatable code: {} groups in own pages: {:.1} KB of chunks for {:.1} KB of code (+{:.1} KB page rounding); invalidated (PROT_NONE, never reused): {} groups, {:.1} KB",
        g,
        c as f64 / 1024.0,
        b as f64 / 1024.0,
        c.saturating_sub(b) as f64 / 1024.0,
        dg,
        db as f64 / 1024.0
    )
}

/// A layout-changing patch (gen `gen`, `layout`: the types whose fields or variants changed),
/// with the program threads paused (inspect::instrument::pause_for_patch, before its install;
/// `paused`: that call paused them) and its code installed: the threads with a frame in the
/// replaced code of a slot the patch rebuilt are held for good and reported (stale::park_held);
/// every other thread resumes. Returns how many were held.
///
/// Why frames only: the earlier patch versions of those slots are in groups (invalidated by the
/// patch, PROT_NONE: a return into them parks by its fault). A slot's first code shares its pages
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
        Err(e) => eprintln!("makepad: layout patch gen {}: the program's stacks could not be read ({}): a thread still in first-version code of a rebuilt fn is not held", gen, e),
        Ok(caps) => {
            let code = u.code.read().unwrap_or_else(|e| e.into_inner());
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
                let hit = at.iter().find_map(|&(k, a)| {
                    let (start, len, slot) = code.entry_at(a)?;
                    (affected.contains(&slot) && start != u.fns[slot as usize].code && crate::stale::lookup(a).is_none()).then_some((k, start, len, slot))
                });
                if let Some((k, start, len, slot)) = hit {
                    let ok = crate::inspect::instrument::hold_for_good(&c.t);
                    let what = format!("slot #{} `{}` first-version code [{:#x}, {:#x}) (+{:#x})", slot, u.fns[slot as usize].name, start, start + len as u64, at.iter().find(|x| x.0 == k).map_or(0, |x| x.1 - start));
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
