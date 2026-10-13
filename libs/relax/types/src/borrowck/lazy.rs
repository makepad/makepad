//! Gandalf, Relax's lazy borrow checker. Named by Kevin Boos: never early, nor late, but precisely when he means to.

use super::event::{admit, BorrowError, GandalfEvent, Origin, APP_CRATES_ONLY};
use crate::fxhash::HashMap;
use crate::program::{DefId, Program};
use crate::tcx::Tcx;
use crate::types::TyId;
use std::collections::{BTreeSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, LazyLock};
use relax_session::sync::{Condvar, Mutex};

pub static WITHHELD: AtomicU64 = AtomicU64::new(0);
pub fn reject_trap_allowed() -> bool { !APP_CRATES_ONLY }

static SUBS: Mutex<Vec<(u64, Sender<GandalfEvent>)>> = Mutex::new(Vec::new());
static NEXT_SUB: AtomicU64 = AtomicU64::new(1);
static SEQ: AtomicU64 = AtomicU64::new(1);
pub fn subscribe() -> (u64, Receiver<GandalfEvent>) {
    let _k = crate::zone_alloc::kept();
    let (tx, rx) = mpsc::channel();
    let id = NEXT_SUB.fetch_add(1, Ordering::Relaxed);
    SUBS.lock().unwrap_or_else(|e| e.into_inner()).push((id, tx));
    (id, rx)
}
pub fn unsubscribe(id: u64) { SUBS.lock().unwrap_or_else(|e| e.into_inner()).retain(|(i, _)| *i != id); }
fn publish(ev: &mut GandalfEvent) {
    // Subscribers can retain queued events after the compiler arena is destroyed.
    let _k = crate::zone_alloc::kept();
    let mut subs = SUBS.lock().unwrap_or_else(|e| e.into_inner());
    ev.seq = SEQ.fetch_add(1, Ordering::Relaxed);
    subs.retain(|(_, tx)| tx.send(ev.clone()).is_ok());
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FnVerdict { Clean, Errors(Vec<GandalfEvent>), Unchecked(String), Pending }
// rustc mir_borrowck is per LocalDefId, independent of codegen substitutions.
type ItemKey = (usize, DefId, u64);
// Group by owner so live edits invalidate only that definition.
static VERDICTS: LazyLock<Mutex<HashMap<DefId, HashMap<ItemKey, FnVerdict>>>> = LazyLock::new(|| Mutex::new(HashMap::default()));
pub fn verdict(src: usize, def: DefId, _args: &[TyId], epoch: u64) -> FnVerdict {
    let queue = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
    let epoch = queue.as_ref().map_or(epoch, |q| q.canon(src, epoch));
    if queue.as_ref().and_then(|q| q.epoch.get(&src)).is_some_and(|e| epoch < *e) {
        return FnVerdict::Unchecked("stale".into());
    }
    VERDICTS.lock().unwrap_or_else(|e| e.into_inner()).get(&def).and_then(|instances| instances.get(&(src, def, epoch))).cloned()
        .unwrap_or_else(|| FnVerdict::Unchecked("not queued".into()))
}
fn record(key: ItemKey, value: FnVerdict) { VERDICTS.lock().unwrap_or_else(|e| e.into_inner()).entry(key.1).or_default().insert(key, value); }

pub fn report_checked(prog: &Program, src: usize, epoch: u64, def: DefId, args: &[TyId], errors: Vec<BorrowError>, origin: Origin) -> Vec<GandalfEvent> {
    let mut admitted = Vec::new();
    let mut all = Vec::new();
    VIOLATIONS.fetch_add(errors.len() as u64, Ordering::Relaxed);
    for error in errors {
        let allowed = admit(prog, error.krate);
        let mut ev = GandalfEvent { seq: 0, origin, src, epoch, args: args.to_vec(), error };
        if allowed { publish(&mut ev); admitted.push(ev.clone()); }
        else { WITHHELD.fetch_add(1, Ordering::Relaxed); }
        all.push(ev);
    }
    record((src, def, epoch), if all.is_empty() { FnVerdict::Clean } else { FnVerdict::Errors(all) });
    admitted
}

/// MAKEPAD_RUN_FRONT_TRACE=1: background checks that hold the front end's read lock over 200 ms
/// (a live patch's commit waits for them), with the phase they ended in.
fn front_trace() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("MAKEPAD_RUN_FRONT_TRACE").is_some())
}

/// A live patch moved the program from collect epoch `from` to `to` without changing what
/// owners outside the edited crates and their dependents read: `to` becomes an alias of
/// `from`'s epoch for `src`, so the verdicts, seen marks and queued checks of owners `keep`
/// accepts stay valid (they are not checked again); the other owners' entries and the
/// discovery pass are dropped (discovery runs again and queues those owners). Without this
/// every live edit re-checked the whole program in the background, and the patch's commit
/// waited for whichever long check (seconds) happened to hold the front end's read lock.
/// Call with the front end's write lock held (no check is running).
pub fn carry_epoch(src: usize, from: u64, to: u64, keep: &dyn Fn(DefId) -> bool) {
    let mut guard = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
    let Some(q) = guard.as_mut() else { return };
    let base = q.canon(src, from);
    // entries older than `from` (or none at all): the next advance drops them as before
    if q.epoch.get(&src) != Some(&base) { return; }
    q.alias.insert((src, to), base);
    // every queued, seen or checked key of an owner has a verdict entry (Pending at least), so
    // the dropped keys are found from the verdicts of the dropped owners: no pass over the
    // queue or the seen set (they hold the whole program)
    let mut verdicts = VERDICTS.lock().unwrap_or_else(|e| e.into_inner());
    let dropped: Vec<DefId> = verdicts.keys().copied().filter(|d| *d == crate::program::NO_DEF || !keep(*d)).collect();
    let mut gone: Vec<ItemKey> = vec![(src, crate::program::NO_DEF, base)];
    for d in dropped {
        let Some(instances) = verdicts.get_mut(&d) else { continue };
        let keys: Vec<ItemKey> = instances.keys().filter(|k| k.0 == src).cloned().collect();
        for k in keys {
            instances.remove(&k);
            gone.push(k);
        }
        if instances.is_empty() { verdicts.remove(&d); }
    }
    drop(verdicts);
    for k in gone {
        q.seen.remove(&k);
        if let Some(at) = q.rank_of.remove(&k) {
            q.order.remove(&at);
            q.items.remove(&at);
            STALE.fetch_add(1, Ordering::Relaxed);
        }
    }
    q.pending_ranks.retain(|k, _| k.0 != src || keep(k.1));
    q.pending_order.retain(|k| k.0 != src || keep(k.1));
}

/// A source of the program / type context for the worker (the unit's shared front end).
pub trait FrontSource: Send + Sync {
    /// Wait for the runner to admit background checking (once, before any work).
    fn wait_start(&self) {}
    /// Runs `f` with the program and type context if the unit is alive (under the front
    /// end's read lock); None when it is gone or not checkable right now.
    fn with_front(&self, f: &mut dyn FnMut(&Program, &Tcx, &crate::tcx::EvalCx<'_>)) -> bool;
    /// ns since the last compile started or ended (the quiet gate)
    fn quiet_ns(&self) -> u64;
    /// The unit still exists (a retired unit's queued fns are dropped, not retried)
    fn alive(&self) -> bool {
        true
    }
    fn lower_priority(&self);
    /// Stable identity shared by handles to the same frontend.
    fn identity(&self) -> usize;
    /// Driver-owned persistent verdict for item `def`'s identity body: its persistent key
    /// (None: not storable) and the verdict an earlier run stored under it. Called under
    /// with_front; implementations must not acquire the compiler's unit lock.
    fn owner_verdict(&self, _prog: &Program, _def: DefId, _epoch: u64) -> (Option<u128>, Option<StoredVerdict>) {
        (None, None)
    }
}


/// A completed verdict the driver persists by body key (sources, dependencies, compiler build,
/// BORROWCK_VERSION), so an unchanged body is never checked again: Clean, or Unchecked -- the
/// check completed but could not certify the body (its identity body has type errors, or a
/// trait property stays unresolved). Both are deterministic for one build and source; errors
/// and checker failures are never stored (they are reported again).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoredVerdict { Clean, Unchecked(String) }
impl StoredVerdict {
    fn reuse(self, key: ItemKey) {
        match self {
            StoredVerdict::Clean => { record(key, FnVerdict::Clean); CACHED_CLEAN.fetch_add(1, Ordering::Relaxed); }
            StoredVerdict::Unchecked(why) => { record(key, FnVerdict::Unchecked(why)); CACHED_UNCHECKED.fetch_add(1, Ordering::Relaxed); }
        }
    }
}

struct Item { key: ItemKey, args: Option<Vec<TyId>>, cache_key: Option<u128>, src: Arc<dyn FrontSource>, discovery: usize }
#[derive(Default)]
struct Queue {
    order: BTreeSet<(u32, u64)>, items: HashMap<(u32, u64), Item>,
    rank_of: HashMap<ItemKey, (u32, u64)>, seen: HashMap<ItemKey, ()>,
    pending_ranks: HashMap<(usize, DefId), u32>,
    pending_order: VecDeque<(usize, DefId)>,
    epoch: HashMap<usize, u64>, seq: u64,
    /// (src, epoch) -> the earlier epoch it stands for (carry_epoch): keys hold that one
    alias: HashMap<(usize, u64), u64>,
}
pub const RANK_DEFAULT: u32 = 1 << 31;
const RANK_DISCOVER: u32 = u32::MAX - 1;
const RANK_OWNER: u32 = u32::MAX;
pub struct NoteFn {
    pub def: DefId, pub args: Vec<TyId>, pub cache_key: Option<u128>, pub epoch: u64,
    pub rank: Option<u32>, pub cached: Option<StoredVerdict>,
}
impl Queue {
    /// The epoch `src`'s keys use for collect epoch `epoch`.
    fn canon(&self, src: usize, epoch: u64) -> u64 { self.alias.get(&(src, epoch)).copied().unwrap_or(epoch) }
    fn insert(&mut self, item: Item, rank: u32) {
        let at = (rank, self.seq); self.seq += 1;
        self.rank_of.insert(item.key.clone(), at);
        self.order.insert(at); self.items.insert(at, item);
    }
    fn pop(&mut self) -> Option<(u32, Item)> {
        let at = self.order.pop_first()?;
        let item = self.items.remove(&at).unwrap();
        self.rank_of.remove(&item.key);
        Some((at.0, item))
    }
    fn advance(&mut self, src: usize, epoch: u64) -> bool {
        let previous = self.epoch.get(&src).copied();
        if previous.is_some_and(|e| e > epoch) { return false; }
        if previous == Some(epoch) { return true; }
        self.epoch.insert(src, epoch);
        let stale: Vec<_> = self.rank_of.iter().filter(|(k, _)| k.0 == src && k.2 < epoch).map(|(k, at)| (k.clone(), *at)).collect();
        STALE.fetch_add(stale.len() as u64, Ordering::Relaxed);
        for (key, at) in stale { self.rank_of.remove(&key); self.order.remove(&at); self.items.remove(&at); }
        self.seen.retain(|k, _| k.0 != src || k.2 >= epoch);
        self.alias.retain(|k, base| k.0 != src || *base >= epoch);
        self.pending_ranks.retain(|k, _| k.0 != src);
        self.pending_order.retain(|k| k.0 != src);
        VERDICTS.lock().unwrap_or_else(|e| e.into_inner()).retain(|_, instances| {
            instances.retain(|k, _| k.0 != src || k.2 >= epoch);
            !instances.is_empty()
        });
        true
    }
}
pub fn set_ranks(src: usize, epoch: u64, ranks: &[(DefId, Vec<TyId>, u32)]) {
    let mut guard = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
    let q = guard.get_or_insert_with(Queue::default);
    let epoch = q.canon(src, epoch);
    if !q.advance(src, epoch) { return; }
    for (def, _, rank) in ranks {
        let key = (src, *def, epoch);
        if let Some(at) = q.rank_of.remove(&key) {
            q.order.remove(&at);
            let item = q.items.remove(&at).unwrap();
            let next = ((*rank).min(at.0), at.1);
            q.order.insert(next); q.items.insert(next, item); q.rank_of.insert(key, next);
        } else if !q.seen.contains_key(&key) {
            let key = (src, *def);
            if !q.pending_ranks.contains_key(&key) { q.pending_order.push_back(key.clone()); }
            q.pending_ranks.entry(key).and_modify(|r| *r = (*r).min(*rank)).or_insert(*rank);
            while q.pending_ranks.len() > 65_536 {
                if let Some(key) = q.pending_order.pop_front() { q.pending_ranks.remove(&key); }
            }
        }
    }
    WAKE.notify_one();
}
static VERDICT_KEYS: Mutex<Vec<(u128, StoredVerdict)>> = Mutex::new(Vec::new());
/// fns whose clean verdict came from the disk cache this run
pub static CACHED_CLEAN: AtomicU64 = AtomicU64::new(0);
/// fns whose stored Unchecked verdict was reused this run (not checked again)
pub static CACHED_UNCHECKED: AtomicU64 = AtomicU64::new(0);

/// A verdict found outside the lazy checker (the coherence check) for the run to write to the cache.
pub fn push_verdict_key(key: u128, v: StoredVerdict) {
    VERDICT_KEYS.lock().unwrap_or_else(|e| e.into_inner()).push((key, v));
}

/// Storable verdicts' cache keys found since the last call (the run writes them to the cache).
pub fn take_verdict_keys() -> Vec<(u128, StoredVerdict)> {
    std::mem::take(&mut *VERDICT_KEYS.lock().unwrap_or_else(|e| e.into_inner()))
}

static QUEUE: Mutex<Option<Queue>> = Mutex::new(None);

pub(crate) struct ZoneTaken {
    verdicts: HashMap<DefId, HashMap<ItemKey, FnVerdict>>,
    keys: Vec<(u128, StoredVerdict)>,
    queue: Option<Queue>,
}

pub(crate) fn zone_take() -> ZoneTaken {
    // VERDICTS' LazyLock stays initialised; its mutex payload can be reset normally.
    let mut verdicts = VERDICTS.lock().unwrap_or_else(|e| e.into_inner());
    let mut keys = VERDICT_KEYS.lock().unwrap_or_else(|e| e.into_inner());
    let mut queue = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
    let taken = ZoneTaken {
        verdicts: std::mem::take(&mut *verdicts),
        keys: std::mem::take(&mut *keys),
        queue: std::mem::take(&mut *queue),
    };
    #[cfg(test)]
    { assert!(verdicts.is_empty()); assert!(keys.is_empty()); assert!(queue.is_none()); }
    taken
}

pub(crate) fn zone_put_back(taken: ZoneTaken) {
    let mut verdicts = VERDICTS.lock().unwrap_or_else(|e| e.into_inner());
    std::mem::forget(std::mem::replace(&mut *verdicts, taken.verdicts));
    let mut keys = VERDICT_KEYS.lock().unwrap_or_else(|e| e.into_inner());
    std::mem::forget(std::mem::replace(&mut *keys, taken.keys));
    let mut queue = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
    std::mem::forget(std::mem::replace(&mut *queue, taken.queue));
}

static WAKE: Condvar = Condvar::new();
static STARTED: AtomicBool = AtomicBool::new(false);
static STOP: AtomicBool = AtomicBool::new(false);
/// The settle swap asks the worker to end (it holds the old front for its whole life), and
/// whether it runs.
static SWAP_STOP: AtomicBool = AtomicBool::new(false);
static RUNNING: AtomicBool = AtomicBool::new(false);

/// Ends the worker for the settle swap and waits up to `ms` for it (the check in flight
/// finishes first). It starts again at the next enqueue, from the new front. false: it did not
/// end in time (it keeps running).
pub fn swap_stop(ms: u64) -> bool {
    if !STARTED.load(Ordering::SeqCst) { return true; }
    SWAP_STOP.store(true, Ordering::SeqCst);
    let t = std::time::Instant::now();
    loop {
        {
            let _q = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
            WAKE.notify_all();
        }
        if !RUNNING.load(Ordering::SeqCst) {
            // nothing queues between here and the swap's end: enqueue spawns a new worker
            STARTED.store(false, Ordering::SeqCst);
            SWAP_STOP.store(false, Ordering::SeqCst);
            return true;
        }
        if t.elapsed().as_millis() as u64 > ms {
            SWAP_STOP.store(false, Ordering::SeqCst);
            return false;
        }
        std::thread::sleep(std::time::Duration::from_micros(500));
    }
}

fn stopping() -> bool { STOP.load(Ordering::SeqCst) || SWAP_STOP.load(Ordering::SeqCst) || crate::preempt::is_stopped() }
struct RunningGuard;
impl Drop for RunningGuard { fn drop(&mut self) { RUNNING.store(false, Ordering::SeqCst); } }
pub static CHECKED: AtomicU64 = AtomicU64::new(0);
/// checks that failed inside the checker (a panic): those fns are NOT checked
pub static PANICS: AtomicU64 = AtomicU64::new(0);
/// queued before the program was collected again (DefIds / types renumbered): dropped
pub static STALE: AtomicU64 = AtomicU64::new(0);
/// the first few panicking fns (path: message), for the run summary
pub static PANIC_NOTES: Mutex<Vec<String>> = Mutex::new(Vec::new());
pub static CHECK_NS: AtomicU64 = AtomicU64::new(0);
/// checks a front-end writer (a live patch) preempted; each was queued again
pub static PREEMPTED: AtomicU64 = AtomicU64::new(0);
pub static VIOLATIONS: AtomicU64 = AtomicU64::new(0);
static IDLE_REPORTED: AtomicBool = AtomicBool::new(false);

/// The quiet gate of O3: ms without compile activity before background checks start.
const QUIET_NS: u64 = 500_000_000;
/// Definitions discovery looks at per item (it yields the front read lock between items).
const DISCOVERY_BATCH: usize = 4096;

/// Background mode: MAKEPAD_RUN_GANDALF=0 turns it off (default on when the checker is on).
pub fn gandalf_enabled(options: &crate::options::Options) -> bool {
    !DISABLED.load(Ordering::Relaxed) && (super::enabled(options) && options.gandalf)
}

static DISABLED: AtomicBool = AtomicBool::new(false);

/// `makepad check` checks synchronously: no background worker.
pub fn disable_gandalf() {
    DISABLED.store(true, Ordering::Relaxed);
}


pub fn note_compiled(options: &crate::options::Options, src: &Arc<dyn FrontSource>, n: NoteFn) {
    enqueue(options, src, n.def, Some(n.args), n.cache_key, n.epoch, n.rank, n.cached);
}
fn enqueue(options: &crate::options::Options, src: &Arc<dyn FrontSource>, def: DefId, args: Option<Vec<TyId>>, cache_key: Option<u128>, epoch: u64, rank: Option<u32>, cached: Option<StoredVerdict>) {
    if !gandalf_enabled(options) { return; }
    {
        let mut guard = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
        let q = guard.get_or_insert_with(Queue::default);
        let epoch = q.canon(src.identity(), epoch);
        if !q.advance(src.identity(), epoch) { return; }
        let key = (src.identity(), def, epoch);
        if q.seen.contains_key(&key) {
            // Discovery may have queued this owner first. A compiled instance
            // promotes that same check instead of adding an instance check.
            if args.is_some() {
                if let Some(at) = q.rank_of.remove(&key) {
                    let requested = rank.or_else(|| q.pending_ranks.get(&(key.0, def)).copied()).unwrap_or(RANK_DEFAULT);
                    q.order.remove(&at);
                    let item = q.items.remove(&at).unwrap();
                    let next = (at.0.min(requested), at.1);
                    q.order.insert(next); q.items.insert(next, item); q.rank_of.insert(key, next);
                }
            }
            return;
        }
        q.seen.insert(key, ());
        if let Some(v) = cached {
            v.reuse(key);
            return;
        }
        let k = (key.0, def);
        let pending = q.pending_ranks.remove(&k);
        if pending.is_some() { q.pending_order.retain(|x| *x != k); }
        let rank = if def == crate::program::NO_DEF { RANK_DISCOVER } else if args.is_none() { RANK_OWNER } else { rank.or(pending).unwrap_or(RANK_DEFAULT) };
        record(key, FnVerdict::Pending);
        q.insert(Item { key, args, cache_key, src: src.clone(), discovery: 0 }, rank);
    }
    if !STARTED.swap(true, Ordering::SeqCst) {
        RUNNING.store(true, Ordering::SeqCst);
        let s = src.clone(); let verbose = options.verbose;
        { let _k = crate::zone_alloc::kept();
        let _ = std::thread::Builder::new().name("gandalf".to_string()).spawn(move || { let _z = crate::zone_alloc::compiler_thread(); worker(s, verbose) });
        }
    }
    WAKE.notify_one();
}
pub fn note_program(options: &crate::options::Options, src: &Arc<dyn FrontSource>, epoch: u64) {
    enqueue(options, src, crate::program::NO_DEF, None, None, epoch, None, None);
}

/// The run summary line (and the checker's internal failures, if any).
pub fn print_summary(options: &crate::options::Options) {
    let checked = CHECKED.load(Ordering::Relaxed);
    let cached = CACHED_CLEAN.load(Ordering::Relaxed);
    let cached_unchecked = CACHED_UNCHECKED.load(Ordering::Relaxed);
    let panics = PANICS.load(Ordering::Relaxed);
    let stale = STALE.load(Ordering::Relaxed);
    if checked + cached + panics + stale == 0 || (!options.verbose && panics == 0) {
        return;
    }
    let mut s = format!(
        "gandalf: {} fns checked ({} violations, cpu {:.1} ms), {} clean from the cache",
        checked,
        VIOLATIONS.load(Ordering::Relaxed),
        CHECK_NS.load(Ordering::Relaxed) as f64 / 1e6,
        cached
    );
    if cached_unchecked > 0 {
        s.push_str(&format!(", {} unchecked from the cache", cached_unchecked));
    }
    let preempted = PREEMPTED.load(Ordering::Relaxed);
    if preempted > 0 {
        s.push_str(&format!(", {} preempted by live patches (queued again)", preempted));
    }
    if stale > 0 {
        s.push_str(&format!(", {} dropped (queued before the program was collected again, or by a retired unit)", stale));
    }
    if panics > 0 {
        s.push_str(&format!(", {} NOT CHECKED: the checker failed on them (makepad run bug, please report)", panics));
    }
    eprintln!("{}", s);
    for n in PANIC_NOTES.lock().unwrap_or_else(|e| e.into_inner()).iter() {
        eprintln!("gandalf:   checker failure: {}", n);
    }
}

/// Stop the worker (the unit is going away, or the run is ending): a type check in flight
/// unwinds at its next preemption point, nothing is queued again, nobody waits for it.
pub fn stop() {
    STOP.store(true, Ordering::SeqCst);
    crate::preempt::stop_all();
    WAKE.notify_all();
}

// Delivered under QUEUE so a wake cannot be lost between the idle check and wait.
static SETTLE: Mutex<Vec<std::sync::mpsc::Sender<u64>>> = Mutex::new(Vec::new());

pub(super) fn settle() -> std::sync::mpsc::Receiver<u64> {
    let (tx, rx) = std::sync::mpsc::channel();
    let _q = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
    if STARTED.load(Ordering::SeqCst) && !STOP.load(Ordering::SeqCst) {
        SETTLE.lock().unwrap_or_else(|e| e.into_inner()).push(tx);
        WAKE.notify_one();
    } else {
        let _ = tx.send(0);
    }
    rx
}

fn settle_finished_check() {
    let requests = std::mem::take(&mut *SETTLE.lock().unwrap_or_else(|e| e.into_inner()));
    if requests.is_empty() { return; }
    // Body-owned query caches have already dropped at the completed-check boundary.
    for tx in requests { let _ = tx.send(0); }
}

fn panic_message(e: &Box<dyn std::any::Any + Send>) -> String {
    e.downcast_ref::<String>().cloned().or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default()
}
fn finish_verdict(prog: &Program, src: usize, epoch: u64, def: DefId, args: &[TyId], result: Result<Option<super::Verdict>, String>, origin: Origin) -> Vec<GandalfEvent> {
    let key = (src, def, epoch);
    match result {
        Ok(Some(v)) => {
            let incomplete = v.errors.is_empty().then(|| v.unchecked.first().cloned()).flatten();
            let events = report_checked(prog, src, epoch, def, args, v.errors, origin);
            if let Some(reason) = incomplete { record(key, FnVerdict::Unchecked(reason)); }
            events
        }
        Ok(None) => { record(key, FnVerdict::Unchecked("typeck".into())); Vec::new() }
        Err(message) => { record(key, FnVerdict::Unchecked(format!("checker failure: {}", message))); Vec::new() }
    }
}
fn worker(src: Arc<dyn FrontSource>, verbose: bool) {
    let _running = RunningGuard;
    let _mem = crate::memstat::tag(crate::memstat::Tag::Borrowck);
    src.lower_priority(); src.wait_start();
    loop {
        if stopping() { return; }
        let quiet = src.quiet_ns();
        if quiet < QUIET_NS {
            std::thread::sleep(std::time::Duration::from_millis(((QUIET_NS - quiet) / 1_000_000).clamp(20, 500)));
            continue;
        }
        let (rank, item) = {
            let mut guard = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                if stopping() { return; }
                settle_finished_check();
                if let Some(item) = guard.as_mut().and_then(Queue::pop) { break item; }
                if verbose && std::env::var_os("MAKEPAD_RUN_GANDALF_STATS").is_some() && CHECKED.load(Ordering::Relaxed) > 0 && !IDLE_REPORTED.swap(true, Ordering::Relaxed) {
                    eprintln!("gandalf: idle at t={:.0} ms: {} fns checked, {} violations, cpu {:.1} ms ({:.1} us/fn), {} clean from the cache",
                        crate::clock::wall_ns() as f64 / 1e6, CHECKED.load(Ordering::Relaxed), VIOLATIONS.load(Ordering::Relaxed),
                        CHECK_NS.load(Ordering::Relaxed) as f64 / 1e6, CHECK_NS.load(Ordering::Relaxed) as f64 / 1e3 / CHECKED.load(Ordering::Relaxed).max(1) as f64, CACHED_CLEAN.load(Ordering::Relaxed));
                }
                guard = WAKE.wait(guard).unwrap_or_else(|e| e.into_inner());
                IDLE_REPORTED.store(false, Ordering::Relaxed);
            }
        };
        if !item.src.alive() {
            STALE.fetch_add(1, Ordering::Relaxed);
            record(item.key, FnVerdict::Unchecked("stale".into()));
            continue;
        }
        let _cost = crate::fncost::Record::new(u32::MAX, 0, "gandalf");
        let t0 = crate::clock::cpu_ns();
        let mut stale = false;
        let mut discovery_next = None;
        let mut reused_owner = false;
        let mut preempted = false;
        let (identity, def, epoch) = &item.key;
        let phase = std::cell::Cell::new("start");
        let ok = item.src.with_front(&mut |prog, tcx, cx| {
            struct Slow<'a>(std::time::Instant, &'a std::cell::Cell<&'static str>, &'a Program, DefId, usize);
            impl Drop for Slow<'_> {
                fn drop(&mut self) {
                    let ms = self.0.elapsed().as_secs_f64() * 1e3;
                    if ms > 200.0 && front_trace() {
                        let name = if self.3 == crate::program::NO_DEF { format!("discovery@{}", self.4) } else { self.2.def_path(self.3) };
                        eprintln!("gandalf-slow: {:.0} ms in {} ({})", ms, name, self.1.get());
                    }
                }
            }
            let _slow = Slow(std::time::Instant::now(), &phase, prog, *def, item.discovery);
            let now = tcx.collect_epoch.load(Ordering::SeqCst);
            if QUEUE.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map_or(now != *epoch, |q| q.canon(*identity, now) != *epoch || q.epoch.get(identity).is_some_and(|e| *e > *epoch)) {
                stale = true; return;
            }
            if *def == crate::program::NO_DEF {
                // Discovery is resumable at definition boundaries. Never hold the front
                // read lock while walking the whole program: a live patch needs its writer.
                // Only the app's own crates are checked (event::admit, the one app-crate rule):
                // framework, dependency and std bodies are not queued. Definitions outside the
                // app are skipped in runs (a crate's definitions are contiguous) without
                // holding the read lock for more than one batch.
                let mut d = item.discovery;
                let mut admitted: Option<(u32, bool)> = None;
                let end = (d + DISCOVERY_BATCH).min(prog.defs.len());
                while d < end {
                    let krate = prog.def(DefId(d as u32)).krate;
                    let ok = match admitted { Some((k, ok)) if k == krate => ok, _ => { let ok = super::event::admit(prog, krate); admitted = Some((krate, ok)); ok } };
                    if ok {
                        if let Some(owner) = super::body::CheckedBody::item(prog, DefId(d as u32)) {
                            enqueue(&prog.session.options, &item.src, owner.owner, None, None, *epoch, None, None);
                            d += 1;
                            break;
                        }
                    }
                    d += 1;
                }
                if d < prog.defs.len() { discovery_next = Some(d); }
                return;
            }
            let identity_args;
            let args = match &item.args {
                Some(args) => args,
                None => {
                    let Some(owner) = super::body::CheckedBody::item(prog, *def) else { return; };
                    identity_args = owner.identity_args(prog, tcx);
                    &identity_args
                }
            };
            // Every instance checks the same generic body and can persist its owner verdict.
            let (owner_key, stored) = item.src.owner_verdict(prog, *def, *epoch);
            phase.set("stored");
            if let Some(v) = stored {
                reused_owner = true;
                v.reuse(item.key);
                return;
            }
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _p = crate::preempt::Preemptible::on();
                crate::preempt::point();
                let _origin = crate::clock::origin("gandalf");
                phase.set("typeck");
                // a live patch waiting for the front end's write lock stops this check
                // (preempt): it is queued again below
                let _p = crate::preempt::Preemptible::on();
                let body = tcx.typeck(prog, *def);
                if !body.errors.is_empty() { phase.set("typeck (errors)"); return None; }
                phase.set("borrowck");
                static COUNTS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
                if *COUNTS.get_or_init(|| std::env::var_os("MAKEPAD_RUN_BODY_COUNTS").is_some()) { eprintln!("body-count borrowck"); }
                Some(super::check_body(cx, prog, tcx, &body))
            }));
            if result.as_ref().is_err_and(|p| crate::preempt::is_preempted(&**p)) {
                phase.set("preempted");
                preempted = true;
                return;
            }
            let result = result.map_err(|e| panic_message(&e));
            if let Err(msg) = &result {
                PANICS.fetch_add(1, Ordering::Relaxed);
                let mut notes = PANIC_NOTES.lock().unwrap_or_else(|e| e.into_inner());
                if notes.len() < 5 { notes.push(format!("{}: {}", prog.def_path(*def), msg)); }
            }
            // Re-ranking to a new epoch may have happened during the check.
            let queue = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
            if queue.as_ref().and_then(|q| q.epoch.get(identity)).is_some_and(|e| *e > *epoch) {
                stale = true; return;
            }
            phase.set("record");
            let storable = match &result {
                Ok(Some(v)) if v.errors.is_empty() => Some(v.unchecked.first().map_or(StoredVerdict::Clean, |why| StoredVerdict::Unchecked(why.clone()))),
                Ok(None) => Some(StoredVerdict::Unchecked("typeck".into())),
                _ => None,
            };
            if let Some(v) = storable {
                let mut keys = VERDICT_KEYS.lock().unwrap_or_else(|e| e.into_inner());
                keys.extend(item.cache_key.into_iter().chain(owner_key).map(|k| (k, v.clone())));
            }
            phase.set("finish");
            finish_verdict(prog, *identity, *epoch, *def, args, result, Origin::Background);
        });
        CHECK_NS.fetch_add(crate::clock::cpu_ns() - t0, Ordering::Relaxed);
        // An exit cancels the whole attempt, including constant deferrals/discovery.
        // Only a front writer's temporary preemption is queued again below.
        if crate::preempt::is_stopped() { return; }
        if stale {
            STALE.fetch_add(1, Ordering::Relaxed);
            // Epoch-purged entries stay purged; verdict() supplies the stale reason.
            let queue = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
            if !queue.as_ref().and_then(|q| q.epoch.get(identity)).is_some_and(|e| *e > *epoch) {
                record(item.key.clone(), FnVerdict::Unchecked("stale".into()));
            }
            continue;
        }
        if preempted {
            if stopping() { return; }
            PREEMPTED.fetch_add(1, Ordering::Relaxed);
            let mut guard = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
            if crate::preempt::is_stopped() { return; }
            if let Some(q) = guard.as_mut() {
                if !q.epoch.get(identity).is_some_and(|e| *e > *epoch) { q.insert(item, rank); }
            }
            drop(guard);
            // the writer runs; checking again at once would only meet it again
            std::thread::sleep(std::time::Duration::from_millis(50));
            continue;
        }
        if !ok {
            let mut guard = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(q) = guard.as_mut() {
                if q.epoch.get(identity).is_some_and(|e| *e > *epoch) { STALE.fetch_add(1, Ordering::Relaxed); }
                else { q.insert(item, rank); }
            }
            drop(guard);
            std::thread::sleep(std::time::Duration::from_millis(200));
            continue;
        }
        if let Some(next) = discovery_next {
            let mut guard = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(q) = guard.as_mut() {
                if q.epoch.get(identity) == Some(epoch) {
                    q.insert(Item { key: item.key, args: None, cache_key: None, src: item.src.clone(), discovery: next }, RANK_DISCOVER);
                }
            }
        }
        if *def != crate::program::NO_DEF && !reused_owner { CHECKED.fetch_add(1, Ordering::Relaxed); }
    }
}

/// `makepad run` never executes a body its checker rejects (check-before-run): the verdict of
/// `def`'s owner before its first code is installed -- from this run's verdicts, the disk cache
/// (`cached`: the instance's stored verdict), the driver's stored owner verdict, or a synchronous
/// check here, on the installing thread. A body still queued for the background is checked now
/// and taken off the queue. Returns the (admitted) errors that refuse running it; an Unchecked
/// verdict (the checker could not certify the body, or failed on it) does not refuse it.
/// The caller holds the program (`prog`, `tcx` are the running unit's), so no front lock is taken.
pub fn check_before_run(prog: &Program, tcx: &Tcx, src: &Arc<dyn FrontSource>, def: DefId, epoch: u64, cache_key: Option<u128>, cached: Option<StoredVerdict>) -> Vec<GandalfEvent> {
    let identity = src.identity();
    let key = {
        let mut guard = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
        let q = guard.get_or_insert_with(Queue::default);
        let epoch = q.canon(identity, epoch);
        // a newer program is being checked: this install is for a superseded epoch
        if !q.advance(identity, epoch) { return Vec::new(); }
        let key = (identity, def, epoch);
        match VERDICTS.lock().unwrap_or_else(|e| e.into_inner()).get(&def).and_then(|i| i.get(&key)) {
            Some(FnVerdict::Clean | FnVerdict::Unchecked(_)) => return Vec::new(),
            Some(FnVerdict::Errors(evs)) => return evs.iter().filter(|e| admit(prog, e.error.krate)).cloned().collect(),
            Some(FnVerdict::Pending) | None => {}
        }
        // queued for the background (Pending): checked here instead
        if let Some(at) = q.rank_of.remove(&key) { q.order.remove(&at); q.items.remove(&at); }
        q.seen.insert(key, ());
        if let Some(v) = cached { v.reuse(key); return Vec::new(); }
        key
    };
    let (owner_key, stored) = src.owner_verdict(prog, def, key.2);
    if let Some(v) = stored { v.reuse(key); return Vec::new(); }
    let Some(owner) = super::body::CheckedBody::item(prog, def) else { record(key, FnVerdict::Unchecked("no body".into())); return Vec::new(); };
    let args = owner.identity_args(prog, tcx);
    let t0 = crate::clock::cpu_ns();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let state = crate::tcx::EvalState::new();
        let cx = crate::tcx::EvalCx::new(prog, tcx, crate::tcx::TypingEnv::fully_monomorphized(prog), &state);
        let body = tcx.typeck(prog, def);
        if !body.errors.is_empty() { return None; }
        Some(super::check_body(&cx, prog, tcx, &body))
    })).map_err(|e| panic_message(&e));
    CHECK_NS.fetch_add(crate::clock::cpu_ns() - t0, Ordering::Relaxed);
    CHECKED.fetch_add(1, Ordering::Relaxed);
    if let Err(msg) = &result {
        PANICS.fetch_add(1, Ordering::Relaxed);
        let mut notes = PANIC_NOTES.lock().unwrap_or_else(|e| e.into_inner());
        if notes.len() < 5 { notes.push(format!("{}: {}", prog.def_path(def), msg)); }
    }
    let storable = match &result {
        Ok(Some(v)) if v.errors.is_empty() => Some(v.unchecked.first().map_or(StoredVerdict::Clean, |why| StoredVerdict::Unchecked(why.clone()))),
        Ok(None) => Some(StoredVerdict::Unchecked("typeck".into())),
        _ => None,
    };
    if let Some(v) = storable {
        let mut keys = VERDICT_KEYS.lock().unwrap_or_else(|e| e.into_inner());
        keys.extend(cache_key.into_iter().chain(owner_key).map(|k| (k, v.clone())));
    }
    finish_verdict(prog, identity, key.2, def, &args, result, Origin::Run)
}

pub struct Growth<'a> {
    pub changed_insts: &'a [(DefId, Vec<TyId>)], pub before: &'a [Vec<super::fv::Summary>], pub visible_crates: &'a [bool],
}
pub struct CheckRequest<'a> {
    pub changed: &'a [DefId], pub instances: &'a [(DefId, Vec<TyId>)],
    /// Bodies the caller already type-checked against this program (a reload's check stage):
    /// used instead of checking the same instance again.
    pub checked: &'a [crate::typeck::Body],
    pub growth: Option<Growth<'a>>, pub origin: Origin, pub epoch: u64,
    /// A live edit's summary context: callee summaries computed once for the whole edit
    /// (the summaries before it, the changed bodies' checks, the summaries after it).
    pub fv: Option<&'a std::rc::Rc<super::fv::SummaryCx>>,
}
pub struct GateResult { pub events: Vec<GandalfEvent>, pub withheld: usize }
impl GateResult { pub fn refuses(&self) -> bool { !self.events.is_empty() } }

pub fn check_now(prog: &Program, tcx: &Tcx, req: &CheckRequest<'_>) -> GateResult {
    let mut out = GateResult { events: Vec::new(), withheld: 0 };
    let mut done = Vec::new();
    let mut todo: Vec<_> = req.instances.iter().filter(|(d, _)| req.changed.contains(d)).cloned().collect();
    for d in req.changed {
        if let Some(owner) = super::body::CheckedBody::item(prog, *d) { todo.push((*d, owner.identity_args(prog, tcx))); }
    }
    // Invalidate every previously checked instance of an edited definition, including
    // background identities. Each completed synchronous check replaces these below.
    {
        let mut verdicts = VERDICTS.lock().unwrap_or_else(|e| e.into_inner());
        for def in req.changed {
            if let Some(instances) = verdicts.get_mut(def) {
                for value in instances.values_mut() { *value = FnVerdict::Unchecked("changed".into()); }
            }
        }
    }
    for (def, args) in todo {
        if done.contains(&def) { continue; }
        done.push(def);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let state = crate::tcx::EvalState::new();
            let cx = crate::tcx::EvalCx::new(prog, &tcx, crate::tcx::TypingEnv::fully_monomorphized(&prog), &state);
            let body = tcx.typeck(prog, def);
            if !body.errors.is_empty() { return None; }
            Some(super::check_body_in(&cx, prog, tcx, &body, req.fv))
        })).map_err(|e| panic_message(&e));
        let errors = result.as_ref().ok().and_then(|v| v.as_ref()).map_or(0, |v| v.errors.len());
        let events = finish_verdict(prog, 0, req.epoch, def, &args, result, req.origin);
        out.withheld += errors - events.len();
        out.events.extend(events);
        let mut verdicts = VERDICTS.lock().unwrap_or_else(|e| e.into_inner());
        let instances = verdicts.get_mut(&def).expect("completed gate verdict");
        let value = instances[&(0, def, req.epoch)].clone();
        for old in instances.values_mut() { *old = value.clone(); }
        if out.refuses() { break; }
    }
    if out.refuses() { return out; }
    let Some(growth) = &req.growth else { return out; };
    let after = super::summaries_with(prog, tcx, growth.changed_insts, req.fv);
    let mut grown = Vec::new();
    for (i, (d, _)) in growth.changed_insts.iter().enumerate() {
        if let (Some(b), Some(a)) = (growth.before.get(i), after.get(i)) {
            for (k, (sb, sa)) in b.iter().zip(a.iter()).enumerate() {
                if grew(sb, sa) { grown.push((*d, super::describe_summary(prog, tcx, *d, k, sb), super::describe_summary(prog, tcx, *d, k, sa))); }
            }
        }
    }
    if grown.is_empty() { return out; }
    for i in 0..prog.defs.len() {
        let d = DefId(i as u32);
        let def = prog.def(d);
        if !growth.visible_crates[def.krate as usize] { continue; }
        if !matches!(def.kind, crate::program::DefKind::Fn | crate::program::DefKind::AssocFn) || !super::fv::relaxed(prog, d) { continue; }
        let it = prog.files[def.file as usize].ast.item(def.item);
        let text = String::from_utf8_lossy(&prog.files[def.file as usize].src[it.lo as usize..(it.hi as usize).min(prog.files[def.file as usize].src.len())]);
        if !grown.iter().any(|(g, _, _)| *g != d && text.contains(&format!("{}(", prog.name(*g)))) { continue; }
        let Some(args) = super::identity_args(tcx, prog, d) else { continue; };
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let state = crate::tcx::EvalState::new();
            let cx = crate::tcx::EvalCx::new(prog, &tcx, crate::tcx::TypingEnv::fully_monomorphized(&prog), &state);
            let body = tcx.typeck(prog, d);
            if !body.errors.is_empty() { return None; }
            let mut v = super::check_body(&cx, prog, tcx, &body);
            v.errors.retain_mut(|e| {
                let mut matched = false;
                for (g, b, a) in &grown {
                    let name = prog.name(*g);
                    if e.mentions(name) {
                        e.notes.push(format!("your edit grew the field view of `{}`: it used {}; now {}", name, b, a));
                        matched = true;
                    }
                }
                matched
            });
            Some(v)
        })).map_err(|e| panic_message(&e));
        let errors = result.as_ref().ok().and_then(|v| v.as_ref()).map_or(0, |v| v.errors.len());
        let events = finish_verdict(prog, 0, req.epoch, d, &args, result, req.origin);
        out.withheld += errors - events.len();
        out.events.extend(events);
    }
    out
}
fn grew(b: &super::fv::Summary, a: &super::fv::Summary) -> bool {
    use super::fv::Summary;
    match (b, a) {
        (Summary::Whole, _) => false,
        (Summary::Parts(_), Summary::Whole) => true,
        (Summary::Parts(bv), Summary::Parts(av)) => av.iter().any(|(p, m)| !bv.iter().any(|(q, qm)| q.len() <= p.len() && p[..q.len()] == q[..] && (*qm || !*m))),
    }
}

#[cfg(test)]
#[path = "lazy_tests.rs"]
mod lazy_tests;
