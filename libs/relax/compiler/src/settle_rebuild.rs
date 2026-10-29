//! The settle rebuild (MEMSAFE.DESIGN.md R2): once a run has settled, the compile state its cold
//! start built (the program and type context with every interned inference type, the query tables,
//! the bodies, the RIR kept for inlining, the caches) is replaced by the warm-sized state a warm
//! start would map, and the old state is dropped by value. The compiler arena then gets back whole
//! chunks: what the dropped values held and the free slots that sat between them.
//!
//! Nothing is freed in bulk and no thread is quiesced. Safety rests on two rules:
//! - every block is freed by its owner's Drop (the old values are moved out of the Unit and
//!   dropped off the owner; whatever still holds an `Arc` into them keeps it alive);
//! - off the owner, the program and type context are read only through a front read
//!   (`FrontRead::unit` / `FrontRead::of`, whose references cannot outlive it), and the replace
//!   runs on the owner under a front write: no reader holds a reference into the old values
//!   while they are moved out. The owner's own readers are its request frames: the replace is a
//!   Program request, which no pump serves (owner.rs `keeps_job`), so it never runs nested in one.
//!
//! Steps (driver: the run monitor, after a settle; retried while preconditions fail):
//! 1. the entries the cache recorded are written to its stores (off the owner); the collected
//!    image of the program as it is now, encoded under a front read (compiles go on) and committed
//!    to the store;
//! 2. the image restored into a new program and type context, on the monitor thread (it reads
//!    only the image and the stores);
//! 3. one owner request under a front write: the counts must be those of the encode (only the
//!    interners may have grown: the new interner is topped up from the live one, so type ids stay
//!    identical), what the old cache recorded since step 1 is written to its stores, the cache is
//!    opened as a warm start opens it and inherits what the old one learned, and the Unit's
//!    compile fields are replaced; the old ones are returned;
//! 4. the old values are dropped on the monitor's thread, then the heap is trimmed.
//! `MAKEPAD_RUN_SETTLE_REBUILD=0` turns it off.

use super::*;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Instant;

/// On unless MAKEPAD_RUN_SETTLE_REBUILD=0.
pub(crate) fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("MAKEPAD_RUN_SETTLE_REBUILD").map_or(true, |v| v != "0"))
}

/// The crate list of this launch (kept: the rebuild restores against it).
static BOOT: relax_session::sync::Mutex<Option<Vec<crate::preload::PreCrate>>> = relax_session::sync::Mutex::new(None);

pub(crate) fn remember_boot(pre: &[crate::preload::PreCrate]) {
    *BOOT.lock().unwrap_or_else(|e| e.into_inner()) = Some(crate::zone_alloc::kept_with(|| pre.to_vec()));
}

static REBUILDS: AtomicU32 = AtomicU32::new(0);

/// Settle rebuilds done so far.
pub(crate) fn rebuilds() -> u32 {
    REBUILDS.load(Ordering::Relaxed)
}

/// A rebuild is running now (its preconditions held; postframe::activity).
static RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub(crate) fn running() -> bool {
    RUNNING.load(Ordering::Relaxed)
}

struct Running;
impl Drop for Running {
    fn drop(&mut self) {
        RUNNING.store(false, Ordering::Relaxed);
    }
}

/// How many monitor tries (one a second) a rebuild waits for the tier-up tail before it runs anyway.
pub(crate) const TAIL_WAIT: u32 = 60;

pub(crate) enum Outcome {
    Done,
    /// not now: the monitor tries again
    Retry(String),
    /// never in this run
    Refused(String),
}

/// What must be equal between the encode and the replace (the interners may only grow).
#[derive(PartialEq, Debug, Clone, Copy)]
struct Counts {
    defs: usize,
    files: usize,
    crates: usize,
    mods: usize,
    tys: usize,
    regions: (usize, usize),
    syms: usize,
    late_syms: usize,
    lists: (usize, usize),
}

impl Counts {
    fn of(prog: &Program, tcx: &Tcx) -> Counts {
        Counts { defs: prog.defs.len(), files: prog.files.len(), crates: prog.crates.len(), mods: prog.mods.len(), tys: tcx.tys.len(),
            regions: tcx.tys.region_counts(), syms: prog.syms.len(), late_syms: prog.syms.late_len(), lists: tcx.tys.list_const_counts() }
    }

    /// Equal but for the interners, which only grew.
    fn same_program(&self, o: &Counts) -> bool {
        (self.defs, self.files, self.crates, self.mods, self.syms, self.late_syms) == (o.defs, o.files, o.crates, o.mods, o.syms, o.late_syms)
            && self.tys <= o.tys && self.regions.0 <= o.regions.0 && self.regions.1 <= o.regions.1 && self.lists.0 <= o.lists.0 && self.lists.1 <= o.lists.1
    }
}

/// Counts equal, and rows sampled across the tables: a restore that renumbered anything shows here.
fn compare(old: (&Program, &Tcx), new: (&Program, &Tcx)) -> Result<(), String> {
    let (a, b) = (Counts::of(old.0, old.1), Counts::of(new.0, new.1));
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
            return Err(format!("definition {i} differs"));
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
    Ok(())
}

/// Reasons the rebuild can never run in this process.
fn refuse(up: *mut Unit) -> Option<String> {
    if !crate::reload::edited_crates().is_empty() {
        return Some("a live edit was applied (the collected image no longer describes the program)".into());
    }
    if crate::sandbox::any_live() {
        return Some("a sandbox module exists".into());
    }
    let Some(boot) = BOOT.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|b| b.len()) else {
        return Some("no boot record".into());
    };
    let r = front_read("settle rebuild check");
    let (prog, _) = r.unit(up);
    if prog.crates.len() != boot {
        return Some("crates were added at run time (the boot image does not describe them)".into());
    }
    if unsafe { (*up).retired } {
        return Some("a retired unit".into());
    }
    None
}

/// Reasons not to start now.
fn not_now() -> Option<String> {
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
    if *ON.get_or_init(|| std::env::var_os("MAKEPAD_RUN_SETTLE_TRACE").is_some()) {
        eprintln!("makepad-run settle rebuild: {what} (t={:.0} ms)", crate::clock::ms_since_start());
    }
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e3
}

/// The replaced compile state of the Unit, dropped by value off the owner. (The slot fields
/// `calls`, `rir_refs` and `spec` are ids and code: they stay.)
struct Old {
    _prog: Program,
    _tcx: Tcx,
    _lay: crate::layout::Layouts,
    _cache: Option<Box<crate::cache::CacheCell>>,
    _front: std::sync::Arc<FrontShared>,
    _const_failed: std::collections::HashSet<(DefId, Vec<TyId>)>,
    _bodies: crate::fxhash::HashMap<u32, std::sync::Arc<Body>>,
    _old_bodies: crate::fxhash::HashMap<(u32, u32), std::sync::Arc<Body>>,
    _coro_rir: crate::conc::MemoMap<u32, relax_backend::rir::Func>,
    _spec_cached: crate::conc::MemoMap<u32, ()>,
    _unwind_drop_cache: crate::conc::MemoMap<TyId, bool>,
    _unpin_cache: crate::conc::MemoMap<TyId, bool>,
    _core_path_cache: crate::conc::MemoMap<String, Option<DefId>>,
    _ctfe_fn_bodies: relax_session::sync::Mutex<crate::fxhash::HashMap<(DefId, Vec<TyId>), (u64, std::sync::Arc<Body>)>>,
    _query_errors: relax_types::infer::diagnostics::DiagCtxt,
    _rir: Vec<std::sync::Arc<relax_backend::rir::Func>>,
}
// SAFETY: every value in `Old` was moved out of the Unit under a front write on the owner; no
// thread holds a reference into it (off-owner readers go through a front read, the owner's own
// request frames are not nested under this one). What is shared with other threads is shared
// through Arcs, whose counts are atomic.
unsafe impl Send for Old {}

/// The state built from the restored image, ready to be moved into the Unit.
struct Fresh {
    prog: Program,
    tcx: Tcx,
    collected: crate::cache::CollectedCache,
    /// its cache, opened before the replace (off the owner thread)
    cache: Option<crate::cache::Cache>,
}

/// Runs the rebuild. Called by the monitor thread once the run has settled and its collected
/// image is current; `up` is the run's unit.
/// `patient`: the rebuild may still wait for the tier-up tail (`TAIL_WAIT`).
pub(crate) fn run(up: *mut Unit, patient: bool) -> Outcome {
    if let Some(why) = refuse(up) {
        return Outcome::Refused(why);
    }
    if let Some(why) = not_now() {
        return Outcome::Retry(why);
    }
    // the tier-up tail first: a rebuild drops every slot's RIR, and each O3 build still queued
    // then lowers its callees again, which stay pinned while they tier up (Sandbox race rebuilt
    // during the tail: 18-22k RIR bodies and 190k query nodes regrown, 400 MB live, against 2k
    // and 250 MB after a rebuild behind it)
    if patient {
        let queued = tier_queue().map_or(0, |q| q.len()) + TIER_ACTIVE.lock().map_or(0, |a| a.len());
        if queued > 0 {
            return Outcome::Retry("O3 builds are queued".into());
        }
    }
    // the recipe, the session, the images: process state
    let _k = crate::zone_alloc::kept();
    RUNNING.store(true, Ordering::Relaxed);
    let _running = Running;
    let t_all = Instant::now();
    let rss0 = crate::memstat::resident();
    let pre = BOOT.lock().unwrap_or_else(|e| e.into_inner()).clone().unwrap();
    let sess = unsafe { unit_session(up) };
    let Some(mode) = crate::cache::mode_from_env(&sess.options) else { return Outcome::Refused("the cache mode is off".into()) };

    // a slot being built would defer the replace: ask before paying for the encode and restore
    if let Some(i) = owner::publish(|| unsafe { (*up).fns.iter().position(|f| f.building || f.inflight) }) {
        return Outcome::Retry(format!("slot {i} is being built"));
    }
    // the recorded entries go to the stores now, off the owner: the new cache opens the stores
    // as they are, and an entry recorded but not yet written would be unknown to it (every slot
    // such entries hold compiled again: Sandbox's settled state grew back to 18k RIR bodies)
    let spill = owner::publish(|| unsafe {
        (*up).flush_cache_records();
        (*up).cache.as_mut().and_then(|c| c.spill_take())
    });
    if let Some(s) = spill {
        if let Err(e) = s.write() {
            return Outcome::Retry(format!("the cache spill failed: {e}"));
        }
    }
    // 1. the collected image of the program now, under a front read (compiles go on)
    let t = Instant::now();
    let Some(inputs) = owner::publish(|| unsafe { (*up).cache.as_ref().map(|c| c.collected_inputs()) }) else {
        return Outcome::Retry("the cache is not in the unit".into());
    };
    let (before, encoded) = {
        let _c = crate::zone_alloc::compiler();
        let r = front_read("settle rebuild encode");
        let (prog, tcx) = r.unit(up);
        let before = Counts::of(prog, tcx);
        let stop = || crate::preempt::writer_waiting() || crate::preempt::is_stopped();
        (before, inputs.encode_now(prog, tcx, &stop))
    };
    let (dest, payload) = match encoded {
        crate::cache::CollectedRecord::Ready(dest, payload) => (dest, payload),
        crate::cache::CollectedRecord::Current => return Outcome::Retry("the store already holds the image".into()),
        crate::cache::CollectedRecord::Yielded => return Outcome::Retry("the encode yielded to a front writer".into()),
        crate::cache::CollectedRecord::Failed => return Outcome::Refused("the program is not recordable".into()),
    };
    if crate::cache::Cache::commit_collected(None, dest, payload).is_none() {
        return Outcome::Retry("the collected image could not be written".into());
    }
    let encode_ms = ms(t);
    trace(&format!("encoded and stored in {encode_ms:.1} ms"));

    // 2. the restore (the image and the stores only)
    let t = Instant::now();
    // what the old state's chunks still hold after the rebuild is counted apart (`memory` arena
    // states: "old"); the new state goes into chunks carved from here on
    crate::zone_alloc::compiler_mark_old();
    let fresh = {
        let _c = crate::zone_alloc::compiler();
        // the new state in fresh chunks: compact, not in the old state's partly free ones
        let _f = crate::zone_alloc::compiler_fresh();
        let restored = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let images = crate::cache::FrontImages::open(&sess.options)?;
            images.restore_collected(&sess, &pre)
        }));
        match restored {
            Ok(Some((prog, tcx, Some(collected)))) => Fresh { prog, tcx, collected, cache: None },
            Ok(Some(_)) => return Outcome::Retry("only a prefix of the program was restored".into()),
            Ok(None) => return Outcome::Retry("the collected image was not restored".into()),
            Err(_) => return Outcome::Retry("the restore panicked".into()),
        }
    };
    crate::program::share_std_end();
    let restore_ms = ms(t);
    trace(&format!("restored in {restore_ms:.1} ms"));

    // 3. the replace, on the owner under a front write; what the cache recorded so far is
    // written first, off the owner (the replace writes only what comes after this)
    // ... and the new state's cache opened, off the owner too: the new program is this thread's
    // alone until the replace (what the old cache records meanwhile is written by the replace
    // and found by the next start)
    let (spill, settings) = owner::call(owner::Class::Program, || unsafe {
        let u = &mut *up;
        u.flush_cache_records();
        (u.cache.as_mut().and_then(|c| c.spill_take()), (u.opt.clone(), u.tier1.is_some(), u.provided.iter().map(|(k, pc)| (*k, pc.binding)).collect::<Vec<_>>()))
    });
    if let Some(s) = spill {
        if let Err(e) = s.write() { return Outcome::Retry(format!("the cache spill failed: {e}")); }
    }
    let mut fresh = fresh;
    let t_open = Instant::now();
    {
        let (opt, tier1, provided) = settings;
        let from = crate::cache::OpenFrom { prog: &fresh.prog, tcx: &fresh.tcx, opt, tier1, global_allocator: crate::jit::global_allocator_static_of(&fresh.prog), provided };
        let mut cache = match crate::cache::Cache::open_from(&from, mode, crate::dirs::relax_dir(), Some(&fresh.collected)) {
            Ok(c) => c,
            Err(e) => return Outcome::Retry(format!("the cache did not open: {e}")),
        };
        cache.load_manifest(crate::layout::root_crate());
        fresh.cache = Some(cache);
    }
    trace(&format!("cache opened in {:.1} ms", ms(t_open)));
    let t = Instant::now();
    let replaced = owner::call(owner::Class::Program, || unsafe { replace(up, &sess, mode, fresh, before) });
    let old = match replaced {
        Ok(old) => old,
        Err(why) => return Outcome::Retry(why),
    };
    let replace_ms = ms(t);

    // 4. the old state goes by value, off the owner (on this, the monitor's, thread)
    let t = Instant::now();
    drop(old);
    // the process-wide memos that only grow go too (made again on demand)
    drop_compile_memos();
    relax_types::tcx::drop_process_memos();
    crate::program::drop_shared_std();
    let drop_ms = ms(t);
    crate::memstat::trim();
    crate::pack::release_resident();
    let rss1 = crate::memstat::resident();
    REBUILDS.fetch_add(1, Ordering::Relaxed);
    if crate::run::VERBOSE.load(Ordering::Relaxed) || crate::memstat::enabled() { eprintln!("makepad-run settle rebuild at t={:.0} ms: encode {:.1} ms, restore {:.1} ms, replace on the owner {:.1} ms, old state dropped in {:.1} ms; total {:.1} ms; rss {:.0} -> {:.0} MB",
        crate::clock::ms_since_start(), encode_ms, restore_ms, replace_ms, drop_ms, ms(t_all), rss0 as f64 / 1048576.0, rss1 as f64 / 1048576.0); }
    Outcome::Done
}

/// Owner request under a front write: checks the program against the encode, finishes the new
/// state (interner top-up, cache, front state) and moves it into the Unit. The old values come
/// back to be dropped elsewhere.
unsafe fn replace(up: *mut Unit, sess: &std::sync::Arc<crate::Session>, mode: crate::cache::Mode, fresh: Fresh, before: Counts) -> Result<Old, String> {
    let Some(_front) = super::front_write_within(1000) else {
        return Err("a front reader held on for 1 s".into());
    };
    let _f = crate::zone_alloc::compiler_fresh();
    let u = &mut *up;
    let now = Counts::of(&u.prog, &u.tcx);
    if !before.same_program(&now) {
        return Err("the program changed after the encode".into());
    }
    for (i, f) in u.fns.iter().enumerate() {
        if f.building || f.inflight {
            return Err(format!("slot {i} is being built"));
        }
    }
    let t_r = Instant::now();
    let mut laps: Vec<(&str, f64)> = Vec::new();
    let lap = |w: &'static str, laps: &mut Vec<(&'static str, f64)>| laps.push((w, t_r.elapsed().as_secs_f64() * 1e3));
    let Fresh { prog, mut tcx, collected: _, cache: fresh_cache } = fresh;
    // the types interned since the encode, copied from the live interner: ids stay identical
    tcx.tys.top_up_from(&u.tcx.tys).ok_or("the image holds more types than the program")?;
    compare((&u.prog, &u.tcx), (&prog, &tcx))?;
    lap("compare", &mut laps);
    // (the steps up to the swap give way to the main thread: the rebuild is tried again later)
    let yielded = || owner::main_waiting() && !crate::jit::QUIT_SEEN.load(Ordering::Relaxed);
    if yielded() { return Err("yielded to the main thread".into()); }
    // what was recorded since the spill before the encode (a few entries) is written before the
    // new cache opens the stores: a rebuild never waits for spills
    u.flush_cache_records();
    if let Some(s) = u.cache.as_mut().and_then(|c| c.spill_take()) {
        s.write().map_err(|e| format!("the cache spill failed: {e}"))?;
    }
    lap("spill", &mut laps);
    // the cache, as the warm start opens it, and what the old one learned
    let _ = mode;
    let mut cache = fresh_cache.ok_or("no cache")?;
    lap("cache open", &mut laps);
    if yielded() { return Err("yielded to the main thread".into()); }
    if let Some(old) = u.cache.as_ref() {
        cache.inherit_run_state(old);
    }
    let mut front = FrontShared::new(sess);
    front.census_id = u.front.census_id;
    front.checker_id = u.front.checker_id;
    front.linked_start.store(u.front.linked_start.load(Ordering::Relaxed), Ordering::Relaxed);
    for (i, f) in u.fns.iter().enumerate() {
        front.slot_keys.insert(i as u32, f.key.clone());
    }
    let mut lay = crate::layout::Layouts::new();
    lay.opaque = u.lay.opaque.clone();
    let const_failed = u.const_failed.clone();
    let query_errors = tcx.dcx.clone();
    let checked_epoch = u.tcx.collect_epoch.load(Ordering::SeqCst);
    lap("front state", &mut laps);
    if yielded() { return Err("yielded to the main thread".into()); }
    // the replace: nothing can fail from here
    u.front.retire_memos();
    // the old front state names the old program: no reader may reach it after the replace (a
    // reader checks `alive` under its front read, and this runs under the front write)
    u.front.alive.store(false, Ordering::SeqCst);
    let mut rir = Vec::new();
    for e in u.fns.iter_mut() {
        if let Some(r) = e.rir.take() {
            rir.push(r);
        }
        e.build_err = None;
        // the slots' call lists stay; made during the cold start, they are scattered over the
        // old state's chunks (a few bytes each, 200k slots): copied into the fresh chunks, the old
        // ones go with the rest (only the owner reads a slot's fields: this runs on it)
        e.rehome_lists();
    }
    let old = Old {
        _prog: std::mem::replace(&mut u.prog, prog),
        _tcx: std::mem::replace(&mut u.tcx, tcx),
        _lay: std::mem::replace(&mut u.lay, lay),
        _cache: std::mem::replace(&mut u.cache, Some(Box::new(crate::cache::CacheCell::new(cache)))),
        _front: std::mem::replace(&mut u.front, std::sync::Arc::new(front)),
        _const_failed: std::mem::replace(&mut u.const_failed, const_failed),
        _bodies: std::mem::replace(&mut u.bodies, Default::default()),
        _old_bodies: std::mem::replace(&mut u.old_bodies, Default::default()),
        _coro_rir: std::mem::replace(&mut u.coro_rir, Default::default()),
        _spec_cached: std::mem::replace(&mut u.spec_cached, Default::default()),
        _unwind_drop_cache: std::mem::replace(&mut u.unwind_drop_cache, Default::default()),
        _unpin_cache: std::mem::replace(&mut u.unpin_cache, Default::default()),
        _core_path_cache: std::mem::replace(&mut u.core_path_cache, crate::conc::MemoMap::with_bits(8)),
        _ctfe_fn_bodies: std::mem::replace(&mut u.ctfe_fn_bodies, Default::default()),
        _query_errors: std::mem::replace(&mut u.query_errors, query_errors),
        _rir: rir,
    };
    // what Unit::new does with a restored program, again for the new one
    u.tcx.const_eval_provider = Some(relax_types::const_eval::ConstEvalProvider {
        context: u.self_ptr.0 as usize, evaluate: const_eval_to_valtree,
    });
    u.prog.body_memos.store(true, Ordering::Relaxed);
    // the same program: its collect epochs go on from the old one's (the checker's epochs only
    // advance: a restored context counting from its image would put every later epoch behind)
    u.tcx.collect_epoch.fetch_max(checked_epoch, Ordering::SeqCst);
    u.tcx.publish_collected();
    // the background borrow checker keeps every verdict and queued check (the same program)
    let epoch_to = u.tcx.collect_epoch.load(Ordering::SeqCst);
    crate::borrowck::lazy::carry_whole(u.front.checker_id, checked_epoch, epoch_to);
    set_shared_front(u, u.core_crate);
    // ... and reads them from the new front state: the old one (dead now) is not kept alive by
    // the queue or the worker (its memos pinned the cold state's chunks)
    let src: std::sync::Arc<dyn crate::borrowck::lazy::FrontSource> = std::sync::Arc::new(BorrowckFront(u.front.clone()));
    let moved = crate::borrowck::lazy::rehome(&src);
    trace(&format!("{moved} queued borrow checks moved to the new front state (epoch {checked_epoch} -> {epoch_to})"));
    crate::run::set_diagnostics(u.tcx.dcx.clone());
    // every thread's compiler memos go at its next access (dropped there, by value)
    relax_session::memo_local::retire_all();
    lap("swap", &mut laps);
    if crate::run::VERBOSE.load(Ordering::Relaxed) {
        trace(&format!("replace steps (ms to): {}", laps.iter().map(|(w, t)| format!("{w} {t:.1}")).collect::<Vec<_>>().join(", ")));
        eprintln!("makepad-run settle rebuild replace (ms to): {}", laps.iter().map(|(w, t)| format!("{w} {t:.1}")).collect::<Vec<_>>().join(", "));
    }
    Ok(old)
}
