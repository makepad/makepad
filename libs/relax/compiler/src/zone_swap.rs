//! The compiler-generation swap at settle (memlane step 1b-B, DESIGN-zones.md F): once a run has
//! settled, the compiler's heap (generation g of the compiler arena) is destroyed and the compile
//! state is rebuilt, warm-sized, in generation g+1. What the running program needs to go on --
//! slot identities, code, symbols, statics, sites, pc maps (the kept heap, `Unit` doc in jit.rs)
//! -- is never touched. On by default; `MAKEPAD_RUN_ZONE_SWAP=0` turns it off.
//!
//! Protocol (driver: the run monitor, after the settle; abortable until step 6):
//! 1. the watcher and the borrow-check worker end (they hold generation-g state for their whole
//!    life; both restart after the swap);
//! 2. quiesce: DRAIN (no new compile job starts, the owner serves the jobs under way, and a job
//!    that waits for the owner is let back in, so it finishes), then HARD (the owner serves only
//!    this thread, the 2 s escape of the job gate is off). The drained state is: no job running,
//!    none waiting for the owner, the owner idle, no thread alive that took `compiler_thread()`
//!    except the long-lived workers that hold nothing across their idle wait (`ALLOWED`);
//! 3. one owner request records the front images the store lacks; this thread writes them;
//! 4. one owner request encodes the collected image of the program AS IT IS NOW (the first
//!    encode of the run is older: types, definitions and files came after it); this thread
//!    commits it to the store;
//! 5. generation g+1 begins; the Z statics are taken for rollback; the image is restored
//!    (`warm_front`), the cache is opened as a warm start opens it and the front state is rebuilt,
//!    all in g+1; the counts and sampled rows are compared with the running program. An abort
//!    restores the old statics and leaves compiler TLS valid;
//! 6. one owner request overwrites the compiler fields of the kept `Unit` box and of every slot in
//!    place (`ptr::write` of the new value, `mem::forget` of the old one, never a drop) and
//!    invalidates compiler TLS and destroys every older generation, including those retained by
//!    aborted swaps;
//! 7. the gate opens; the threads that ended restart.

use super::*;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// On unless MAKEPAD_RUN_ZONE_SWAP=0.
pub(crate) fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("MAKEPAD_RUN_ZONE_SWAP").map_or(true, |v| v != "0"))
}

/// MAKEPAD_RUN_ZONE_VERIFY=1: the restored Program is compared table by table with the running one.
fn verify_on() -> bool {
    std::env::var("MAKEPAD_RUN_ZONE_VERIFY").is_ok_and(|v| v == "1")
}

/// The crate list of this launch (kept: the swap restores against it).
static BOOT: relax_session::sync::Mutex<Option<Vec<crate::preload::PreCrate>>> = relax_session::sync::Mutex::new(None);
/// Appended crates, in Program order. Their recipes must outlive every compiler generation.
static RUNTIME: relax_session::sync::Mutex<Vec<crate::preload::PreCrate>> = relax_session::sync::Mutex::new(Vec::new());

pub(crate) fn remember_boot(pre: &[crate::preload::PreCrate]) {
    *BOOT.lock().unwrap_or_else(|e| e.into_inner()) = Some(crate::zone_alloc::kept_with(|| pre.to_vec()));
    RUNTIME.lock().unwrap_or_else(|e| e.into_inner()).clear();
    *IMAGE.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// The owner records even a failed load's published crate: its ids remain in the Program.
pub(crate) fn remember_runtime(prog: &Program, krate: u32) {
    let _k = crate::zone_alloc::kept();
    let c = &prog.crates[krate as usize];
    let deps: Vec<_> = c.deps.iter().map(|&(name, k)|
        (prog.syms.str(name).to_owned(), prog.syms.str(prog.crates[k as usize].name).to_owned())).collect();
    let mut cfg = c.cfg.clone();
    cfg.session = prog.session.clone();
    RUNTIME.lock().unwrap_or_else(|e| e.into_inner()).push(crate::preload::PreCrate {
        name: prog.syms.str(c.name).to_owned(), root: c.root_path().to_owned(), edition: c.edition,
        cfg, deps: deps.clone(), proc_macro: c.proc_macro,
        image_deps: c.deps.iter().map(|&(_, k)| k as usize).collect(), image_dep_names: deps,
    });
}

/// Threads that took `compiler_thread()`, may be alive at the swap and are known to hold no
/// compiler-generation state across their wait: the owner (a request in flight is the only
/// time it holds some; the swap runs as one) and the workers that wait on a queue, popping an id
/// before they pass the job gate (spec workers, O3, O2) or reading everything through the unit
/// pointer (the post-boot loader).
const ALLOWED: [&str; 5] = ["run-owner", "run-spec", "run-o3-", "run-O2", "run-complete"];

fn unexpected_threads() -> Vec<String> {
    // the front's fork/join workers idle with the arena chunks and thread-local caches of the
    // generation they last ran in: they end before the swap (and start again on demand)
    crate::pool::retire_idle();
    crate::zone_alloc::kept_with(|| {
        let mut v = crate::zone_alloc::live_compiler_threads();
        v.retain(|n| !ALLOWED.iter().any(|a| n.starts_with(a)));
        v
    })
}

/// How the swap ended.
pub(crate) enum Outcome {
    Done,
    /// not now (a precondition, a busy owner, a thread still running): the monitor tries again
    Retry(String),
    /// never in this run
    Refused(String),
}

/// Holds the job gate shut (drain, then hard); opens it when dropped.
struct Gate;

impl Drop for Gate {
    fn drop(&mut self) {
        {
            let _g = BG_MX.lock().unwrap_or_else(|e| e.into_inner());
            BG_PAUSE.store(false, Ordering::SeqCst);
            BG_PAUSER.store(0, Ordering::SeqCst);
            SWAP_HARD.store(false, Ordering::SeqCst);
            SWAP_DRAIN.store(false, Ordering::SeqCst);
        }
        BG_CV.notify_all();
        owner::resume();
        end_pause_turn();
    }
}

fn drained(mine: u32) -> Result<(), String> {
    let active = BG_ACTIVE.load(Ordering::SeqCst);
    if active > mine {
        return Err(format!("{} compile jobs running", active - mine));
    }
    let idle = BG_IDLE.load(Ordering::SeqCst);
    if idle > 0 {
        return Err(format!("{idle} compile jobs wait for the owner"));
    }
    if !owner::idle() {
        return Err(format!("the owner serves ({})", owner::serving_note()));
    }
    let t = unexpected_threads();
    if !t.is_empty() {
        return Err(format!("threads still alive: {}", t.join(",")));
    }
    Ok(())
}

fn quiesce(deadline: Duration) -> Result<Gate, String> {
    if !try_pause_turn() {
        return Err("another pause is active".into());
    }
    let t0 = Instant::now();
    let mine = if JOB_DEPTH.with(|d| d.get()) > 0 { 1 } else { 0 };
    SWAP_DRAIN.store(true, Ordering::SeqCst);
    let gate = Gate;
    let wait = |what: &str| -> Result<(), String> {
        loop {
            match drained(mine) {
                Ok(()) => return Ok(()),
                Err(e) if t0.elapsed() > deadline => return Err(format!("{what}: {e}")),
                Err(_) => {}
            }
            // the main thread waiting at the gate: no swap is worth a frame
            let since = GATED_MAIN.load(Ordering::SeqCst);
            if since != 0 && wall_ns().saturating_sub(since) > 30_000_000 {
                return Err(format!("{what}: the main thread waits at the job gate"));
            }
            std::thread::sleep(Duration::from_micros(200));
        }
    };
    wait("drain")?;
    BG_PAUSER.store(me(), Ordering::SeqCst);
    SWAP_HARD.store(true, Ordering::SeqCst);
    BG_PAUSE.store(true, Ordering::SeqCst);
    SWAP_DRAIN.store(false, Ordering::SeqCst);
    wait("hard pause")?;
    Ok(gate)
}

/// What must be equal before and after the restore.
#[derive(PartialEq, Debug, Clone, Copy)]
struct Counts {
    defs: usize,
    files: usize,
    crates: usize,
    mods: usize,
    tys: usize,
    regions: (usize, usize),
    syms: usize,
}

/// The counts of the program the store's collected image describes (set when this run restored
/// or recorded it, and by each swap): equal counts later mean nothing was added since, and the
/// swap restores without encoding again. The optional entry names a kept, swap-only image;
/// None names the boot image, whose counts are recorded only before runtime crates arrive.
static IMAGE: relax_session::sync::Mutex<Option<(Counts, Option<crate::cache::CollectedWritten>)>> = relax_session::sync::Mutex::new(None);

pub(crate) fn note_image(prog: &Program, tcx: &Tcx) {
    // A mid-run writer may publish the frozen boot after runtime crates were appended.
    // Its image then describes the boot snapshot, not these live counts.
    if RUNTIME.lock().unwrap_or_else(|e| e.into_inner()).is_empty() {
        *IMAGE.lock().unwrap_or_else(|e| e.into_inner()) = Some((counts(prog, tcx), None));
    }
}

fn counts(prog: &Program, tcx: &Tcx) -> Counts {
    Counts { defs: prog.defs.len(), files: prog.files.len(), crates: prog.crates.len(), mods: prog.mods.len(), tys: tcx.tys.len(), regions: tcx.tys.region_counts(), syms: prog.syms.len() }
}

/// Counts equal, and rows sampled across the tables: a restore that renumbered anything shows
/// here, before anything is destroyed.
fn compare(old: (&Program, &Tcx), new: (&Program, &Tcx)) -> Result<(), String> {
    let (a, b) = (counts(old.0, old.1), counts(new.0, new.1));
    if a != b {
        return Err(format!("counts differ: running {a:?}, restored {b:?}"));
    }
    if old.0.file_paths != new.0.file_paths {
        return Err("file paths differ".into());
    }
    let step = |n: usize| (n / 96).max(1);
    for i in (0..a.defs).step_by(step(a.defs)) {
        let d = DefId(i as u32);
        let (x, y) = (old.0.def(d), new.0.def(d));
        if (x.kind, x.name, x.parent, x.file, x.krate, x.scope) != (y.kind, y.name, y.parent, y.file, y.krate, y.scope) || old.0.def_path(d) != new.0.def_path(d) {
            return Err(format!("definition {i} differs: {} / {}", old.0.def_path(d), new.0.def_path(d)));
        }
    }
    for i in (0..a.syms).step_by(step(a.syms)) {
        if old.0.syms.str(i as u32) != new.0.syms.str(i as u32) {
            return Err(format!("symbol {i} differs"));
        }
    }
    for i in (0..a.tys).step_by(step(a.tys)) {
        let t = TyId(i as u32);
        if old.1.tys.kind(t) != new.1.tys.kind(t) {
            return Err(format!("type {i} differs"));
        }
    }
    for (x, y) in old.0.crates.iter().zip(&new.0.crates) {
        if (x.name, x.root_mod, x.edition, &x.deps, &x.cfg.set, x.proc_macro, &x.root_path()) != (y.name, y.root_mod, y.edition, &y.deps, &y.cfg.set, y.proc_macro, &y.root_path()) {
            return Err("a crate differs".into());
        }
    }
    Ok(())
}

/// The state built in the new generation, ready to be moved into the kept `Unit`.
struct Fresh {
    prog: Program,
    tcx: Tcx,
    cache: Option<Box<crate::cache::CacheCell>>,
    front: std::sync::Arc<FrontShared>,
    lay: crate::layout::Layouts,
    const_failed: std::collections::HashSet<(DefId, Vec<TyId>)>,
}

/// A swap's numbers (the report line).
#[derive(Default)]
struct Timeline {
    quiesce: f64,
    front_images: f64,
    encode: f64,
    store: f64,
    restore: f64,
    cache: f64,
    commit: f64,
    chunks: usize,
    touched: usize,
    specs: (usize, usize),
}

static SWAPS: AtomicU32 = AtomicU32::new(0);
static ATTEMPTS: AtomicU32 = AtomicU32::new(0);
/// A swap has been done: generation 0 is gone.
pub(crate) fn swapped() -> bool {
    SWAPS.load(Ordering::Relaxed) > 0
}

/// Loader symbol `relax_zone_swaps`: the swaps done so far. A test program (diffpkg `zone_swap`)
/// waits on it to first use statics after the swap; a rustc build has no such symbol.
pub(crate) extern "C" fn relax_zone_swaps() -> u32 {
    SWAPS.load(Ordering::Acquire)
}

pub(crate) static LAST_SWAP_MS: AtomicU64 = AtomicU64::new(0);

/// Runs the swap. Called by the monitor thread once the settle is done and this run's collected
/// image is current; `up` is the run's unit.
pub(crate) fn run(up: *mut Unit) -> Outcome {
    let n = ATTEMPTS.fetch_add(1, Ordering::Relaxed);
    if let Some(why) = refuse(up) {
        return Outcome::Refused(why);
    }
    if let Some(why) = not_now(up) {
        return Outcome::Retry(why);
    }
    // 1. the threads that hold generation state for their whole life
    let watcher = match crate::reload::swap_stop_watcher(100) {
        Some(was) => was,
        None => return Outcome::Retry("the live watcher did not end".into()),
    };
    if !crate::borrowck::lazy::swap_stop(150) {
        if watcher { crate::reload::swap_restart_watcher(up); }
        return Outcome::Retry("the borrow-check worker is busy".into());
    }
    let r = swap(up, n);
    if watcher {
        crate::reload::swap_restart_watcher(up);
    }
    r
}

/// Reasons the swap can never run in this process.
fn refuse(up: *mut Unit) -> Option<String> {
    let u = unsafe { &*up };
    if !crate::reload::edited_crates().is_empty() {
        return Some("a live edit was applied (the collected image no longer describes the program)".into());
    }
    if crate::sandbox::any_live() {
        return Some("a sandbox module exists".into());
    }
    if BOOT.lock().unwrap_or_else(|e| e.into_inner()).is_none() {
        return Some("no boot record".into());
    }
    if u.retired {
        return Some("a retired unit".into());
    }
    None
}

/// Reasons not to start now (the monitor tries again at the next quiet).
fn not_now(up: *mut Unit) -> Option<String> {
    let _ = up;
    if unsafe { (*up).cache.is_none() } {
        // (off, or taken by a cache write on the owner just now)
        return Some("the cache is not in the unit".into());
    }
    if owner::main_waiting() {
        return Some("the main thread waits for the owner".into());
    }
    if !FIRST_PRESENT.load(Ordering::Acquire) || startup_phase() {
        return Some("still starting".into());
    }
    if RELOADING.load(Ordering::Acquire) > 0 {
        return Some("a reload is in progress".into());
    }
    if IDS_PENDING.load(Ordering::Acquire) {
        return Some("startup identities are still encoded".into());
    }
    if crate::pmhost::world_alive() {
        return Some("the proc-macro world is alive".into());
    }
    None
}

fn trace(what: &str) {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ON.get_or_init(|| std::env::var_os("MAKEPAD_RUN_ZONE_TRACE").is_some()) {
        eprintln!("makepad-run zone: trace: {what} (t={:.0} ms)", crate::clock::ms_since_start());
    }
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e3
}

fn swap(up: *mut Unit, n: u32) -> Outcome {
    // Recipes and image descriptors on this stack remain live across arena destruction.
    let _k = crate::zone_alloc::kept();
    let t_all = Instant::now();
    let rss0 = crate::memstat::resident();
    let mut tl = Timeline::default();
    let options = unsafe { (*std::ptr::addr_of!((*up).prog.session)).clone() };
    let Some(mode) = crate::cache::mode_from_env(&options.options) else { return Outcome::Refused("the cache mode is off".into()) };
    let abort = |why: String| Outcome::Retry(why);

    // 2. quiesce
    let gate = match quiesce(Duration::from_millis(2000)) {
        Ok(g) => g,
        Err(e) => return Outcome::Retry(format!("quiesce: {e}")),
    };
    tl.quiesce = ms(t_all);
    // the main thread waiting at the gate ends the swap at the next step, as in the quiesce (30 ms):
    // 250 ms here let a swap hold the UI for its encode + restore (the user's Sandbox: 2.1 s swap)
    let hurry = || ms(t_all) > 2500.0 || { let s = GATED_MAIN.load(Ordering::SeqCst); s != 0 && wall_ns().saturating_sub(s) > 30_000_000 };

    // 3. the front images the store does not have yet
    let t = Instant::now();
    let prepared = owner::call(owner::Class::Program, || unsafe { prepare(up) });
    let (spill, now) = match prepared {
        Ok(s) => s,
        Err(e) => return abort(format!("prepare: {e}")),
    };
    if let Some(s) = spill {
        if let Err(e) = s.write() {
            return abort(format!("store write: {e}"));
        }
    }
    if crate::cache::Cache::flush_front_graphs().is_err() {
        return abort("the front graph could not be written".into());
    }
    tl.front_images = ms(t);

    // 4. the collected image of the program as it is now (unless the store's is that already)
    let before = now;
    let pre = BOOT.lock().unwrap_or_else(|e| e.into_inner()).clone().unwrap();
    let extra = RUNTIME.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if pre.len() + extra.len() != now.crates {
        return abort(format!("recipe has {} crates, program has {}", pre.len() + extra.len(), now.crates));
    }
    let saved = IMAGE.lock().unwrap_or_else(|e| e.into_inner()).clone().filter(|(c, _)| *c == now);
    let mut swap_image = saved.as_ref().and_then(|(_, image)| image.clone());
    if saved.is_some() {
        trace("the collected image is current: not encoded again");
    } else {
        let t = Instant::now();
        let encoded = owner::call(owner::Class::Program, || unsafe { encode(up, &pre, &extra) });
        let (dest, payload, _) = match encoded {
            Ok(x) => x,
            Err(e) => return abort(format!("encode: {e}")),
        };
        tl.encode = ms(t);
        let t = Instant::now();
        let Some(written) = crate::cache::Cache::commit_collected(None, dest, payload) else {
            return abort("the collected image could not be written".into());
        };
        if !extra.is_empty() { swap_image = Some(written); }
        tl.store = ms(t);
    }
    if hurry() {
        return abort("too slow before the restore".into());
    }

    // Runtime inputs have no crate-store ledger. Carry the current union even when the
    // collected tables' counts still match an older image (a body can add only a late input).
    let late = if extra.is_empty() { Vec::new() } else { crate::sip::late_inputs() };
    trace("encoded and stored");
    // 5. the next generation
    let old_gen = crate::zone_alloc::compiler_generation();
    let t = Instant::now();
    crate::zone_alloc::compiler_begin_generation();
    trace("generation begun");
    let Some(taken) = zone_scan::zone_take_all() else {
        return abort("an owner request arrived after the quiesce".into());
    };
    trace("statics taken");
    zone_scan::stale_after_take(crate::zone_alloc::compiler_generation());
    let fresh = {
        let _c = crate::zone_alloc::compiler();
        // live edits after the swap expand the program's derives and macro calls as before it
        if let Err(e) = taken.reinstall_macro_registry(&options) {
            taken.put_back();
            return abort(format!("macro registry: {e}"));
        }
        let built = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| build(up, &options, mode, &pre, &extra, swap_image.as_ref(), &late, before, &mut tl)));
        match built {
            Ok(Ok(f)) => f,
            Ok(Err(e)) => {
                taken.put_back();
                return abort(format!("restore: {e}"));
            }
            Err(p) => {
                taken.put_back();
                let why = p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_default();
                return abort(format!("restore panicked: {why}"));
            }
        }
    };
    tl.restore = ms(t);
    trace("restored and compared");
    if hurry() {
        // Both generations stay live: the old compile state resumes allocating in g+1.
        drop(fresh);
        taken.put_back();
        return abort("too slow after the restore".into());
    }

    // 6. the commit: no abort from here
    trace("commit begins");
    let t = Instant::now();
    // The snapshot owns old-arena pointers: forget it on this thread before commit destroys them.
    taken.forget();
    let (chunks, touched, specs) = owner::call(owner::Class::Program, || unsafe { commit(up, fresh) });
    tl.commit = ms(t);
    trace("commit done");
    crate::zone_alloc::kept_with(|| {
        *IMAGE.lock().unwrap_or_else(|e| e.into_inner()) = Some((before, swap_image.clone()));
    });
    tl.chunks = chunks;
    tl.touched = touched;
    tl.specs = specs;
    drop(gate);
    trace("gate open");

    // 7. the freed pages go back, then the numbers
    crate::memstat::trim();
    crate::pack::release_resident();
    let rss1 = crate::memstat::resident();
    SWAPS.fetch_add(1, Ordering::Release);
    LAST_SWAP_MS.store(wall_ns() / 1_000_000, Ordering::Relaxed);
    crate::boot_stats::swap_done(tl.chunks, rss0, rss1);
    eprintln!("makepad-run zone: swap {}->{} at t={:.0} ms (attempt {}): quiesce {:.1} ms, front images {:.1} ms, encode {:.1} ms, store {:.1} ms, restore {:.1} ms (cache {:.1} ms), commit {:.1} ms, total {:.1} ms; destroyed {} chunks ({:.1} MB touched); {} speculative codes moved to the kept heap ({:.1} MB); rss {:.0} -> {:.0} MB",
        old_gen, old_gen + 1, crate::clock::ms_since_start(), n + 1, tl.quiesce, tl.front_images, tl.encode, tl.store, tl.restore, tl.cache, tl.commit, ms(t_all),
        tl.chunks, tl.touched as f64 / 1048576.0, tl.specs.0, tl.specs.1 as f64 / 1048576.0, rss0 as f64 / 1048576.0, rss1 as f64 / 1048576.0);
    crate::inspect::flightrec::event("zone.swap", &format!("gen {}->{}: total {:.0} ms (quiesce {:.0}, encode {:.0}, restore {:.0}, commit {:.0}); rss {:.0} -> {:.0} MB", old_gen, old_gen + 1, ms(t_all), tl.quiesce, tl.encode, tl.restore, tl.commit, rss0 as f64 / 1048576.0, rss1 as f64 / 1048576.0), (ms(t_all) * 1e6) as u64, 0);
    Outcome::Done
}

/// Owner request: the slots are settled, the front images of the program's files and the crate
/// records are queued for the store. The entries are returned to be written off the owner.
unsafe fn prepare(up: *mut Unit) -> Result<(Option<crate::cache::Spill>, Counts), String> {
    let u = &mut *up;
    for (i, f) in u.fns.iter().enumerate() {
        if f.building || f.inflight {
            return Err(format!("slot {i} is being built"));
        }
    }
    if !crate::run::front_recordable() {
        return Err("the program is not recordable".into());
    }
    u.flush_cache_records();
    let c = u.cache.as_mut().ok_or("no cache")?;
    for (k, v) in crate::borrowck::lazy::take_verdict_keys() {
        c.record_borrowck(k, v);
    }
    crate::run::push_front_images(&u.prog, c);
    c.record_front_crates(&u.prog);
    let spill = c.spill_take();
    Ok((spill, counts(&u.prog, &u.tcx)))
}

/// Owner request: the collected image of the program now, and its counts.
unsafe fn encode(up: *mut Unit, pre: &[crate::preload::PreCrate], extra: &[crate::preload::PreCrate]) -> Result<((std::path::PathBuf, u128), crate::image::Writer<'static>, Counts), String> {
    let _front = super::front_write_within(200).ok_or("the front end is busy")?;
    let u = &*up;
    let c = u.cache.as_ref().ok_or("no cache")?;
    let stop = || GATED_MAIN.load(Ordering::SeqCst) != 0;
    match c.encode_collected_now(&u.prog, &u.tcx, pre, extra, &stop) {
        crate::cache::CollectedRecord::Ready(dest, payload) => Ok((dest, payload, counts(&u.prog, &u.tcx))),
        crate::cache::CollectedRecord::Current => Err("the store already holds the image (nothing to restore from)".into()),
        crate::cache::CollectedRecord::Yielded => Err("yielded to the main thread".into()),
        crate::cache::CollectedRecord::Failed => Err("the program is not recordable (see MAKEPAD_RUN_BOOT_TRACE)".into()),
    }
}

/// Builds the next generation's compile state from the committed image (this thread, in the
/// compiler phase of generation g+1). The running state is only read.
fn build(up: *mut Unit, sess: &std::sync::Arc<crate::Session>, mode: crate::cache::Mode,
    pre: &[crate::preload::PreCrate], extra: &[crate::preload::PreCrate], swap_image: Option<&crate::cache::CollectedWritten>,
    late: &[(String, u8, String, u128)], before: Counts, tl: &mut Timeline) -> Result<Fresh, String> {
    let u = unsafe { &*up };
    trace("build: opening the images");
    let images = crate::cache::FrontImages::open(&sess.options).ok_or("the front images cannot be opened")?;
    trace("build: restoring");
    let (prog, tcx, collected) = crate::run::warm_front(&images, sess, pre, extra, swap_image).ok_or("the collected image was not restored")?;
    for (c, kind, name, hash) in late {
        let content = if *kind == crate::sip::LATE_FILE { crate::vfs::read(name).ok() }
            else { crate::crates::crate_env_named(c, name).map(String::into_bytes) };
        if crate::sip::late_hash(content.as_deref()) != *hash { return Err(format!("a late input changed: {name}")); }
        crate::sip::note_late_input(c, *kind, name, content.as_deref());
    }
    trace("build: restored");
    crate::program::share_std_end();
    let collected = collected.ok_or("only a prefix of the program was restored")?;
    let after = counts(&prog, &tcx);
    if after != before {
        return Err(format!("counts differ: running {before:?}, restored {after:?}"));
    }
    compare((&u.prog, &u.tcx), (&prog, &tcx))?;
    trace("build: compared");
    if verify_on() {
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| u.prog.verify_collected(&prog)));
        if r.is_err() {
            return Err("MAKEPAD_RUN_ZONE_VERIFY: the restored Program differs".into());
        }
    }
    // the cache, as the warm start opens it, and what the old one learned
    let t = Instant::now();
    let root = crate::layout::root_crate();
    let mut cache = match crate::cache::Cache::open_collected(u, mode, Some(&collected)) {
        Ok(c) => c,
        Err(e) => return Err(format!("the cache did not open: {e}")),
    };
    cache.load_manifest(root);
    let old_cache = u.cache.as_ref().ok_or("no cache")?;
    let (p_entries, p_verdicts) = old_cache.swap_pending();
    if p_entries > 0 || p_verdicts > 0 {
        return Err(format!("the old cache has {p_entries} entries and {p_verdicts} verdicts pending"));
    }
    trace("build: cache opened");
    cache.inherit_run_state(old_cache);
    tl.cache = ms(t);
    // the front state: slot keys for every slot, the bits the old one had
    let mut front = FrontShared::new(sess);
    front.census_id = u.front.census_id;
    front.warm_cache.store(u.front.warm_cache.load(Ordering::Relaxed), Ordering::Relaxed);
    for (i, f) in u.fns.iter().enumerate() {
        front.slot_keys.insert(i as u32, f.key.clone());
    }
    trace("build: front rebuilt");
    let mut lay = crate::layout::Layouts::new();
    lay.opaque = crate::zone_alloc::kept_clone(&u.lay.opaque);
    trace("build: layouts");
    Ok(Fresh {
        prog,
        tcx,
        cache: Some(Box::new(crate::cache::CacheCell::new(cache))),
        front: std::sync::Arc::new(front),
        lay,
        const_failed: u.const_failed.clone(),
    })
}

/// Owner request, everything quiescent: the compile state of the kept `Unit` is overwritten by
/// the new generation's, in place, and all older generations are destroyed. Returns (chunks, touched
/// bytes) of the destroyed arena and (count, bytes) of the speculative codes moved to the kept heap.
unsafe fn commit(up: *mut Unit, f: Fresh) -> (usize, usize, (usize, usize)) {
    let _front = super::front_write_within(1000);
    trace("commit: entered");
    let u = &mut *up;
    // overwrite and forget: the old value is never dropped (its blocks are generation g's)
    fn put<T>(slot: &mut T, new: T) {
        std::mem::forget(std::mem::replace(slot, new));
    }
    trace("commit: overwriting");
    put(&mut u.prog, f.prog);
    put(&mut u.tcx, f.tcx);
    put(&mut u.lay, f.lay);
    put(&mut u.cache, f.cache);
    put(&mut u.front, f.front);
    put(&mut u.const_failed, f.const_failed);
    put(&mut u.bodies, Default::default());
    put(&mut u.old_bodies, Default::default());
    put(&mut u.coro_rir, Default::default());
    put(&mut u.coro_busy, Default::default());
    put(&mut u.coro_selfref, Default::default());
    put(&mut u.coro_relower, Default::default());
    put(&mut u.spec_cached, Default::default());
    put(&mut u.unwind_drop_cache, Default::default());
    put(&mut u.cycle_inprog, Default::default());
    put(&mut u.unpin_cache, Default::default());
    put(&mut u.core_path_cache, crate::conc::MemoMap::with_bits(8));
    put(&mut u.ctfe_fn_bodies, Default::default());
    put(&mut u.reload_slots, Default::default());
    put(&mut u.reload_rir, None);
    put(&mut u.reload_errors, Default::default());
    put(&mut u.reload_specs, Default::default());
    put(&mut u.query_errors, u.tcx.dcx.clone());
    // the slots: the compile half of each is replaced; the reference accounting of the last
    // build (rir_refs) is identity-like (a rebuild subtracts it), so it is copied to the kept heap
    let (mut specs, mut spec_bytes) = (0usize, 0usize);
    for e in u.fns.iter_mut() {
        put(&mut e.rir, None);
        put(&mut e.build_err, None);
        put(&mut e.calls, Box::default());
        let refs = crate::zone_alloc::kept_clone(&e.rir_refs);
        put(&mut e.rir_refs, refs);
        if e.spec.is_some() {
            // code built ahead and not yet called (teardown): kept until it is
            let spec = crate::zone_alloc::kept_clone(&e.spec);
            if let Some(s) = &spec {
                specs += 1;
                spec_bytes += std::mem::size_of::<SpecCode>() + s.code.len() + s.relocs.len() * std::mem::size_of::<relax_backend::rir::RelocAt>();
            }
            put(&mut e.spec, spec);
        }
        e.building = false;
        e.inflight = false;
    }
    // what Unit::new does with a restored program, again for the new one
    u.tcx.const_eval_provider = Some(relax_types::const_eval::ConstEvalProvider {
        context: u.self_ptr.0 as usize, evaluate: const_eval_to_valtree,
    });
    u.prog.body_memos.store(true, Ordering::Relaxed);
    u.tcx.publish_collected();
    attach_front(u);
    crate::run::set_diagnostics(u.tcx.dcx.clone());
    // Only commit invalidates TLS; an aborted restore must keep the running state's TLS valid.
    relax_session::gen_local::bump();
    zone_scan::stale_roots(crate::zone_alloc::compiler_generation());
    trace("commit: destroying");
    let (chunks, touched) = crate::zone_alloc::compiler_destroy_older();
    trace("commit: destroyed");
    (chunks, touched, (specs, spec_bytes))
}

/// `set_shared_front` for a unit that already runs: the new FrontShared points at the unit's
/// (same-address) program and type context.
fn attach_front(u: &mut Unit) {
    let options = &u.prog.session.clone().options;
    let core_crate = u.core_crate;
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
}
