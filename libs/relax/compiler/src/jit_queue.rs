//! The JIT's single compile queue. Ordering and admission are decided together,
//! before an entry leaves the queue; a blocked admission never reserves stale work.
use super::*;
use makepad_cpu_budget as budget;
use crate::pool::{BudgetJob, JobClass, Source};
use std::sync::{Arc, OnceLock};
use std::time::Instant;
use std::sync::atomic::AtomicUsize;
use std::collections::BTreeSet;
// (Fx: the maps are looked up under the queue's lock at every pick and push; their keys are serials and slots)
use crate::fxhash::HashMap;

pub(super) type TierEntry = (u32, u8, u64, u64);
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum O2Kind { Normal, Base, Borrow, Verified }

pub(super) enum Job {
    Ahead(Arc<FrontShared>, AheadWork),
    O2(usize, u32, O2Kind),
    O3(TierEntry),
    Spec(usize, u32),
    Tick,
    /// a slice of a check beside the program (check.rs CheckBatch): below every build until
    /// something waits for it
    Check(Arc<crate::check::CheckBatch>),
}
impl Job {
    fn slot(&self) -> Option<(usize, u32)> {
        match self {
            Self::Ahead(f, AheadWork::Slot(id)) => Some((f.unit.load(Ordering::Acquire) as usize, *id)),
            Self::Ahead(_, AheadWork::Body(..) | AheadWork::Look(..)) | Self::Tick | Self::Check(..) => None,
            Self::O2(u, id, _) | Self::Spec(u, id) => Some((*u, *id)),
            Self::O3(e) => Some((TIER_UNIT.load(Ordering::Acquire) as usize, e.0)),
        }
    }
    fn label(&self) -> &'static str {
        match self { Self::Ahead(..) => "JIT ahead", Self::O2(..) => "JIT O2", Self::O3(..) => "JIT O3", Self::Spec(..) => "JIT spec", Self::Tick => "JIT tick", Self::Check(..) => "JIT check" }
    }
    fn tier(&self) -> usize {
        match self { Self::Ahead(..) => 0, Self::O2(..) => 1, Self::O3(..) => 2, Self::Spec(..) => 3, Self::Tick => 4, Self::Check(..) => 5 }
    }
}
/// `slot`: the job's slot when it was queued (a wait for that slot promotes it).
struct Entry { serial: u64, job: Job, at: u64, urgent: bool, class: u8, promoted: bool, key: i64, slot: Option<(usize, u32)> }
/// A slot's queued entries (their serials): one almost always, held without an allocation.
#[derive(Default)]
struct Serials { first: Option<u64>, more: Vec<u64> }
impl Serials {
    fn push(&mut self, serial: u64) {
        if self.first.is_none() { self.first = Some(serial) } else { self.more.push(serial) }
    }
    fn remove(&mut self, serial: u64) {
        if self.first == Some(serial) { self.first = self.more.pop() } else { self.more.retain(|&s| s != serial) }
    }
    fn is_empty(&self) -> bool { self.first.is_none() }
    fn iter(&self) -> impl Iterator<Item = u64> + '_ { self.first.into_iter().chain(self.more.iter().copied()) }
}
/// The admission checks' view of the running jobs and the phase (`Queue::policy`).
struct Policy {
    /// the burst gate: one build at a time below Interactive while the startup or the UI runs
    burst: bool,
    /// running builds below Interactive (the burst gate's count: a check beside the program is none)
    builds: usize,
    noninteractive: usize,
    background: usize,
    /// the running jobs per tier (the startup's tier widths)
    tiers: [usize; TIERS],
    /// through the startup the checks ahead of the main thread run on a few workers, as many as its
    /// helper threads were (STARTUP_AHEAD), and all of them do: a slot the main thread waits for is
    /// admitted beyond
    ahead_full: bool,
    limit: usize,
    background_limit: usize,
    /// before the first present: the startup's tier widths apply
    widths: bool,
    o3_stopped: bool,
    o3_startup: bool,
    /// a program thread compiles on demand (or did within TIER1_POLL): one O3 build at a time, as
    /// the first O3 worker alone ran then (mini, Amp: first present +23% with three O3 builds
    /// beside the startup's compiles; a cold test that opens a window, headless to the JIT, ran 2k
    /// O3 builds for 9.5 CPU-s before its first frame where the O3 workers ran 1.2, and drew 2 s later)
    demand: bool,
}
struct Running { serial: u64, promoted: bool, slot: Option<(usize, u32)>, tier: usize, base_class: u8, class: Arc<JobClass> }
pub(super) struct Queue {
    entries: Vec<Entry>,
    running: Vec<Running>,
    serial: u64,
    /// the threads waiting for a slot's job, per class they wait at (Interactive, Visible:
    /// owner::waiter_class)
    waiters: HashMap<(usize, u32), [u32; 2]>,
    /// the queued entries per tier and stored class, hottest first: a pick reads the buckets that
    /// hold any, each from its front
    plan: [[BTreeSet<(std::cmp::Reverse<i64>, u64)>; 3]; TIERS],
    indices: HashMap<u64, usize>,
    /// the queued entries (serials) per slot: a wait promotes and restores them without a scan of
    /// the queue (the main thread's waits hold the queue lock for those)
    by_slot: HashMap<(usize, u32), Serials>,
    refresh_cursor: usize,
    ahead_class: u8,
    ahead_len: usize,
    tick_at: Instant,
    tick_queued: bool,
    timer: Option<std::thread::ThreadId>,
    /// AHEAD_INBOX's other buffer (its entries move in without a new allocation per move)
    inbox: Vec<(Arc<FrontShared>, AheadWork, u64)>,
}
impl Queue {
    fn new() -> Self {
        Self { entries: Vec::new(), running: Vec::new(), serial: 0, waiters: HashMap::default(), plan: std::array::from_fn(|_| std::array::from_fn(|_| BTreeSet::new())), indices: HashMap::default(), by_slot: HashMap::default(), refresh_cursor: 0, ahead_class: budget::BACKGROUND, ahead_len: 0,
            tick_at: Instant::now(), tick_queued: false, timer: None, inbox: Vec::new() }
    }
    /// Queues `job`; returns whether it can start now.
    pub(super) fn push(&mut self, job: Job, at: u64) -> bool {
        // A producer can finish discovering work after the settle rebuild retired its front.
        if matches!(&job, Job::Ahead(front, _) if !front.alive.load(Ordering::Acquire)) { return false; }
        if !matches!(job, Job::Tick) && W0_IDLING.load(Ordering::Relaxed) { self.tick_at = Instant::now(); }
        self.sync_ahead();
        let now = HeatNow::take();
        let slot = job.slot();
        let wanted = slot.and_then(|slot| self.wanted(slot));
        let promoted = wanted.is_some();
        let class = if job.tier() == 0 { budget::BACKGROUND } else { job_class(&job, &now) };
        let class = wanted.map_or(class, |w| w.min(self.bucket_class(job.tier(), class)));
        self.serial += 1;
        let urgent = at == O2_FRONT_AT && matches!(job, Job::O2(..));
        let at = if urgent { wall_ns() } else { at };
        let mut entry = Entry { serial: self.serial, job, at, urgent, class, promoted, key: 0, slot };
        entry.key = key(&entry, &now);
        self.plan[entry.job.tier()][class as usize].insert(order(entry.key, entry.serial));
        self.indices.insert(entry.serial, self.entries.len());
        if let Some(slot) = slot { self.by_slot.entry(slot).or_default().push(entry.serial); }
        let class = self.class_of(&entry);
        if entry.job.tier() == 0 && !entry.promoted { self.ahead_len += 1; }
        COUNTS[entry.job.tier()].fetch_add(1, Ordering::Relaxed);
        self.entries.push(entry);
        budget::queued(class, budget::JIT, 1);
        let policy = self.policy();
        self.blocked(&self.entries[self.entries.len() - 1], &policy).is_none()
    }
    /// The class the waiters of `slot` ask for.
    fn wanted(&self, slot: (usize, u32)) -> Option<u8> {
        self.waiters.get(&slot)?.iter().position(|&n| n != 0).map(|class| class as u8)
    }
    fn sync_ahead(&mut self) {
        self.set_ahead_class(if startup_phase() || ui_burst() { budget::INTERACTIVE } else { budget::BACKGROUND });
    }
    fn set_ahead_class(&mut self, class: u8) {
        if class != self.ahead_class {
            let mut left = self.ahead_len;
            while left != 0 {
                let count = left.min(i32::MAX as usize) as i32;
                budget::queued(self.ahead_class, budget::JIT, -count);
                budget::queued(class, budget::JIT, count);
                left -= count as usize;
            }
            self.ahead_class = class;
        }
    }
    fn bucket_class(&self, tier: usize, stored: u8) -> u8 {
        if tier == 0 && stored == budget::BACKGROUND { self.ahead_class } else { stored }
    }
    fn class_of(&self, e: &Entry) -> u8 { self.bucket_class(e.job.tier(), e.class) }
    // One heat sample per selection, O(log n). Age is a common epoch offset: it never
    // requires touching existing entries. New hotter work still gets sampled as we rotate.
    fn refresh(&mut self, now: &HeatNow) {
        self.sync_ahead();
        if self.entries.is_empty() { return; }
        self.refresh_cursor %= self.entries.len();
        let entry = &mut self.entries[self.refresh_cursor];
        self.refresh_cursor += 1;
        if entry.job.tier() == 0 { return; }
        self.plan[entry.job.tier()][entry.class as usize].remove(&order(entry.key, entry.serial));
        let class = if entry.promoted { entry.class } else { job_class(&entry.job, now) };
        if class != entry.class {
            budget::queued(entry.class, budget::JIT, -1);
            budget::queued(class, budget::JIT, 1);
            entry.class = class;
        }
        entry.key = key(entry, now);
        self.plan[entry.job.tier()][entry.class as usize].insert(order(entry.key, entry.serial));
    }
    fn head(&mut self, now: &HeatNow) -> Option<usize> {
        self.refresh(now);
        self.first()
    }
    /// The entry to start now, in rank order (head without its heat sample): the first class whose
    /// buckets hold one that can start, the best of their fronts.
    fn first(&self) -> Option<usize> {
        let policy = self.policy();
        (0..3u8).find_map(|class| {
            self.plan.iter().enumerate().flat_map(|(tier, plan)| plan.iter().enumerate().map(move |(stored, bucket)| (tier, stored as u8, bucket)))
                .filter(|&(tier, stored, bucket)| !bucket.is_empty() && self.bucket_class(tier, stored) == class
                    // (an unpromoted check ahead is stored Background: the promoted ones are Interactive)
                    && !(tier == 0 && stored != budget::INTERACTIVE && policy.ahead_full)
                    && self.class_blocked(class, tier, &policy).is_none())
                .filter_map(|(_, _, bucket)| bucket.iter().find_map(|&(key, serial)| {
                    let i = self.indices[&serial];
                    self.entry_blocked(&self.entries[i]).is_none().then_some(((key, serial), i))
                }))
                .min_by_key(|&(r, _)| r).map(|(_, i)| i)
        })
    }
    /// Some job could start now (a worker woken for it would take it).
    fn runnable(&self) -> bool {
        self.first().is_some()
    }
    /// What the admission checks read of the running jobs and the phase, once per decision. The
    /// burst gate holds one build at a time below Interactive while the startup or the UI runs. The
    /// tick is housekeeping beside the builds, not one of them: it neither counts nor waits (the O2
    /// worker and the first O3 worker's polls ran side by side; a tick that waited for the owner held
    /// the one slot the startup's O2 builds had, and half as many ran).
    fn policy(&self) -> Policy {
        let mut policy = Policy {
            burst: !past_startup() || crate::postframe::ui_idle_ns() < BURST_UI_QUIET_NS,
            builds: 0,
            noninteractive: 0,
            background: 0,
            tiers: [0; TIERS],
            ahead_full: false,
            limit: budget::limit() as usize,
            background_limit: budget::background_limit() as usize,
            widths: !FIRST_PRESENT.load(Ordering::Acquire) && !HEADLESS.load(Ordering::Acquire),
            o3_stopped: TIER_STOP.load(Ordering::Acquire),
            o3_startup: startup_holds_tier1(),
            demand: !tier1_workers_ready(),
        };
        let mut ahead = 0;
        for r in &self.running {
            let class = r.class.class();
            if r.tier != 4 && class != budget::INTERACTIVE { policy.noninteractive += 1; }
            if r.tier < 4 && class != budget::INTERACTIVE { policy.builds += 1; }
            if class == budget::BACKGROUND { policy.background += 1; }
            policy.tiers[r.tier] += 1;
            if r.tier == 0 && !r.promoted { ahead += 1; }
        }
        policy.ahead_full = startup_phase() && ahead >= startup_ahead();
        policy
    }
    /// (a check beside the program is no build: the budget alone decides how much of it runs beside them)
    fn class_blocked(&self, class: u8, tier: usize, policy: &Policy) -> Option<&'static str> {
        if tier < 4 && !eligible(class, policy.burst, policy.builds) { return Some("UI burst"); }
        if class != budget::INTERACTIVE {
            if let Some(why) = drain_blocked(class, policy.noninteractive, policy.background, policy.limit, policy.background_limit) { return Some(why); }
            if policy.widths {
                let width = match tier { 1 => 1, 2 | 3 => 2, _ => usize::MAX };
                if policy.tiers[tier] >= width { return Some("startup tier width"); }
            }
            if tier == 2 && policy.demand && policy.tiers[2] != 0 { return Some("demand compiles"); }
        }
        if tier == 2 && policy.o3_stopped { return Some("O3 stopped"); }
        if tier == 2 && policy.o3_startup { return Some("startup"); }
        None
    }
    fn blocked(&self, e: &Entry, policy: &Policy) -> Option<&'static str> {
        if let Some(why) = self.class_blocked(self.class_of(e), e.job.tier(), policy) { return Some(why); }
        if e.job.tier() == 0 && !e.promoted && policy.ahead_full { return Some("startup ahead width"); }
        self.entry_blocked(e)
    }
    /// What holds `e` back beside its bucket's class and width.
    fn entry_blocked(&self, e: &Entry) -> Option<&'static str> {
        if let Some(slot) = e.slot {
            if self.running.iter().any(|r| r.tier == e.job.tier() && r.slot == Some(slot)) { return Some("same slot running"); }
        }
        match &e.job {
            Job::Ahead(f, _) if !f.alive.load(Ordering::Acquire) => Some("front retired"),
            Job::O2(_, _, O2Kind::Verified) if !past_startup() => Some("startup"),
            _ => None,
        }
    }
    fn ordered(&self, limit: usize) -> Vec<(usize, Option<&'static str>)> {
        let policy = self.policy();
        let mut rows = Vec::new();
        // Merge existing class buckets. Ahead's ordinary bucket changes class in O(1),
        // so a UI burst never waits for per-entry refresh to promote its frontier.
        for blocked_pass in [false, true] {
            let mut cursors: Vec<_> = self.plan.iter().flat_map(|p| p.iter().map(|bucket| bucket.iter().peekable())).collect();
            loop {
                let next = cursors.iter_mut().enumerate().filter_map(|(i, c)| c.peek().map(|&&(key, serial)| (i, rank(self.bucket_class(i / 3, (i % 3) as u8), key.0, serial))))
                    .min_by_key(|&(_, r)| r);
                let Some((lane, (_, _, serial))) = next else { break };
                cursors[lane].next();
                let i = self.indices[&serial];
                let blocked = self.blocked(&self.entries[i], &policy);
                if blocked.is_some() == blocked_pass { rows.push((i, blocked)); }
                if rows.len() >= limit { return rows; }
            }
        }
        rows
    }
    fn remove(&mut self, i: usize) -> Entry {
        let mut e = self.entries.swap_remove(i);
        COUNTS[e.job.tier()].fetch_sub(1, Ordering::Relaxed);
        budget::queued(self.class_of(&e), budget::JIT, -1);
        self.plan[e.job.tier()][e.class as usize].remove(&order(e.key, e.serial));
        if e.job.tier() == 0 && !e.promoted { self.ahead_len -= 1; }
        e.class = self.class_of(&e);
        self.indices.remove(&e.serial);
        if let Some(moved) = self.entries.get(i) { self.indices.insert(moved.serial, i); }
        if let Some(slot) = e.slot {
            let serials = self.by_slot.get_mut(&slot).expect("a queued slot's entry is indexed");
            serials.remove(e.serial);
            if serials.is_empty() { self.by_slot.remove(&slot); }
        }
        e
    }
}
/// The startup's workers checking ahead of the main thread: the cores but the main thread's and the
/// owner's, at most four (more check further ahead than its lazy compiles reach, and take the CPU
/// they need).
fn startup_ahead() -> usize {
    (budget::cores() as usize).saturating_sub(2).clamp(1, 4)
}
fn rank(class: u8, key: i64, serial: u64) -> (u8, std::cmp::Reverse<i64>, u64) {
    (class, std::cmp::Reverse(key), serial)
}
/// An entry's place in its bucket (the bucket holds one class: rank without it).
fn order(key: i64, serial: u64) -> (std::cmp::Reverse<i64>, u64) {
    (std::cmp::Reverse(key), serial)
}
// Parked jobs retain their drain thread even after returning their CPU permit. Mirror
// the budget's class ceilings so Background cannot occupy the drains Visible needs,
// just as non-Interactive work must leave a drain for first compiles.
fn drain_blocked(class: u8, noninteractive: usize, background: usize, limit: usize, background_limit: usize) -> Option<&'static str> {
    if class != budget::INTERACTIVE && noninteractive >= limit { return Some("first-compile reserve"); }
    if class == budget::BACKGROUND && background >= background_limit { return Some("visible reserve"); }
    None
}
fn eligible(class: u8, burst: bool, noninteractive: usize) -> bool {
    !(burst && class != budget::INTERACTIVE && noninteractive != 0)
}
/// A job's class while nobody waits on it (a waited-for job is promoted: Interactive). The checks
/// ahead of the main thread's lazy compiles serve the startup and the UI's bursts; a tier-up (O2,
/// O3) or a speculative build only speeds code up, so it runs in the background class, below the
/// app's visible work, however hot its slot (its heat orders it among the background builds).
/// A check beside the program runs in the background class until something waits for it.
fn job_class(job: &Job, _now: &HeatNow) -> u8 {
    match job {
        Job::Ahead(..) if startup_phase() || ui_burst() => budget::INTERACTIVE,
        Job::Check(..) if crate::check::check_waited() => budget::INTERACTIVE,
        _ => budget::BACKGROUND,
    }
}
pub(super) fn heat_key(heat: u64, ui: bool, waited_ns: u64) -> u64 {
    let lg = if heat == 0 { 0 } else { ((heat as f64).log2() * 1024.0) as u64 };
    lg.saturating_add(waited_ns.saturating_mul(1024) / 4_000_000_000).saturating_add(if ui { 1024 } else { 0 })
}
/// A check that something waits for goes before every build of its class: the first frame and the
/// run's exit wait for nothing else.
const CHECK_WAITED_KEY: i64 = i64::MAX - 1;
fn key(e: &Entry, now: &HeatNow) -> i64 {
    if matches!(e.job, Job::Tick) { return i64::MAX; }
    // (a check beside the program goes behind every build of its class, until it is waited for)
    if matches!(e.job, Job::Check(..)) { return if crate::check::check_waited() { CHECK_WAITED_KEY } else { i64::MIN }; }
    let (heat, ui) = match &e.job {
        Job::O2(u, id, _) => now.heat(*u, *id),
        Job::O3(t) => now.heat(0, t.0),
        _ => (0, false),
    };
    // log2 heat buckets, with one doubling per four seconds of age. Adding the
    // current epoch to every key would leave their order unchanged. The startup's look-ahead
    // goes behind every slot made (seconds of age are a few thousand); a checked body's inline
    // candidates go before every slot, the newest first (the lowering of the body they came from
    // takes them next).
    let ahead = match e.job {
        Job::Ahead(_, AheadWork::Look(..)) => -LOOK_BEHIND,
        Job::Ahead(_, AheadWork::Body(..)) => BODY_FIRST + 2 * age_key(e.at),
        _ => 0,
    };
    epoch_key(heat, ui, e.at) + if e.urgent { 64 * 1024 } else { 0 } + ahead
}
const LOOK_BEHIND: i64 = 1 << 40;
const BODY_FIRST: i64 = 1 << 40;
fn age_key(at: u64) -> i64 { (at / 3_906_250).min(i64::MAX as u64) as i64 }
fn epoch_key(heat: u64, ui: bool, at: u64) -> i64 {
    let bucket = if heat == 0 { 0 } else { heat.ilog2() as i64 * 1024 };
    bucket + if ui { 1024 } else { 0 } - age_key(at)
}
static QUEUE: OnceLock<relax_session::sync::Mutex<Queue>> = OnceLock::new();
/// The queue's tiers: ahead, O2, O3, spec, tick, check beside the program.
const TIERS: usize = 6;
static COUNTS: [AtomicUsize; TIERS] = [const { AtomicUsize::new(0) }; TIERS];
static RUNNING: [AtomicUsize; TIERS] = [const { AtomicUsize::new(0) }; TIERS];
/// A worker's park state, on its own cache line (each worker writes its flag at every job).
#[repr(align(64))]
struct Sleeper { thread: OnceLock<budget::Wake>, waiting: AtomicBool }
static THREADS: OnceLock<Vec<Sleeper>> = OnceLock::new();
static START: std::sync::Once = std::sync::Once::new();
static NEXT_WORKER: AtomicUsize = AtomicUsize::new(0);
struct CompilerSource;
static SOURCE: CompilerSource = CompilerSource;
pub(super) fn queue() -> &'static relax_session::sync::Mutex<Queue> {
    QUEUE.get_or_init(|| crate::zone_alloc::kept_with(|| relax_session::sync::Mutex::new(Queue::new())))
}
static LOCK_MAX_NS: AtomicU64 = AtomicU64::new(0);
struct QueueGuard {
    guard: relax_session::sync::MutexGuard<'static, Queue>,
    acquired: Instant,
}
impl std::ops::Deref for QueueGuard {
    type Target = Queue;
    fn deref(&self) -> &Queue { &self.guard }
}
impl std::ops::DerefMut for QueueGuard {
    fn deref_mut(&mut self) -> &mut Queue { &mut self.guard }
}
impl Drop for QueueGuard {
    fn drop(&mut self) {
        LOCK_MAX_NS.fetch_max(self.acquired.elapsed().as_nanos().min(u64::MAX as u128) as u64, Ordering::Relaxed);
    }
}
fn lock() -> QueueGuard {
    let mut guard = queue().lock().unwrap_or_else(|e| e.into_inner());
    crate::zone_alloc::kept_with(|| drain_ahead_inbox(&mut guard));
    QueueGuard { guard, acquired: Instant::now() }
}
pub(super) fn start() {
    START.call_once(|| {
        crate::pool::thread_start(ensure_altstack);
        let _kept = crate::zone_alloc::kept();
        THREADS.get_or_init(|| (0..budget::cores()).map(|_| Sleeper { thread: OnceLock::new(), waiting: AtomicBool::new(false) }).collect());
        for _ in 0..budget::cores() { crate::pool::drain(&SOURCE); }
    });
}
/// Callable from a hot stub: no queue lock, allocation or compiler work. Wakes the first parked
/// worker: the work stays on the few workers it needs (a startup's checks ahead on as many as its
/// width), each on its core with its caches warm, not spread over every worker in turn.
pub(super) fn wake() {
    if let Some(threads) = THREADS.get() {
        for worker in threads {
            if worker.waiting.load(Ordering::SeqCst) && worker.waiting.swap(false, Ordering::SeqCst) {
                if let Some(thread) = worker.thread.get() { thread.unpark(); return; }
            }
        }
    }
}
pub(super) fn wake_all() {
    if let Some(threads) = THREADS.get() {
        for worker in threads {
            worker.waiting.store(false, Ordering::SeqCst);
            if let Some(thread) = worker.thread.get() { thread.unpark(); }
        }
    }
}
/// This process is a plain fork's child (jit::fork_child): it has no workers, and a parent's worker
/// may have held the queue's lock or inbox at the fork, held for good there: nothing is queued.
static FORKED: AtomicBool = AtomicBool::new(false);
#[cfg(unix)]
pub(super) fn fork_child() {
    FORKED.store(true, Ordering::SeqCst);
}
pub(super) fn push(job: Job, at: u64) {
    if FORKED.load(Ordering::Relaxed) { return; }
    let runnable = crate::zone_alloc::kept_with(|| { let mut q = lock(); q.push(job, at) || q.runnable() });
    start();
    if runnable { wake(); }
}
pub(super) fn o2(unit: usize, id: u32, kind: O2Kind, at: u64) {
    push(Job::O2(unit, id, kind), at);
}
/// Speculative compile of unit `unit`'s slot `id` (a slot id means something only in its own unit).
pub(super) fn spec(unit: usize, id: u32) { push(Job::Spec(unit, id), wall_ns()); }
/// Slot `id` to check and lower ahead. Every slot made goes: one whose body a check ahead already
/// checked (an inline candidate, the look-ahead) is still lowered here, off the owner.
pub(super) fn ahead(front: Arc<FrontShared>, id: u32) { push_ahead(&front, std::iter::once(AheadWork::Slot(id))); }
/// Ahead work a check found (a checked body's inline candidates, the startup's look-ahead below
/// it), queued at once.
pub(super) fn ahead_found(front: &Arc<FrontShared>, work: Vec<AheadWork>) {
    if !work.is_empty() { push_ahead(front, work.into_iter()); }
}
/// Ahead work made since the queue's lock was last taken: whoever takes it next moves it in
/// (`lock`). The owner makes a slot for each callee of a lazy compile, and the checks ahead find
/// work in every body: each queued under the queue's lock, which a worker holds for every pick,
/// met it on one push in seven (15k sleeps of the owner through a cold Fab start, the thread the
/// UI waits behind; 130k of the workers').
static AHEAD_INBOX: relax_session::sync::Mutex<Vec<(Arc<FrontShared>, AheadWork, u64)>> = relax_session::sync::Mutex::new(Vec::new());
static AHEAD_PENDING: AtomicBool = AtomicBool::new(false);
/// A parked worker is woken while a check ahead can start: through the startup, below their
/// width (STARTUP_AHEAD; a running check takes the next itself when it ends). Ahead work blocked
/// otherwise starts when the startup's phases change (they wake the workers).
fn push_ahead(front: &Arc<FrontShared>, work: impl Iterator<Item = AheadWork>) {
    if FORKED.load(Ordering::Relaxed) { return; }
    // A producer can finish discovering work after the settle rebuild retired its front.
    if !front.alive.load(Ordering::Acquire) { return; }
    let at = wall_ns();
    crate::zone_alloc::kept_with(|| {
        let mut inbox = AHEAD_INBOX.lock().unwrap_or_else(|e| e.into_inner());
        inbox.extend(work.map(|w| (front.clone(), w, at)));
        AHEAD_PENDING.store(true, Ordering::Release);
    });
    start();
    if !startup_phase() || RUNNING[0].load(Ordering::Relaxed) < startup_ahead() { wake(); }
}
/// AHEAD_INBOX into the queue (its lock held).
fn drain_ahead_inbox(q: &mut Queue) {
    if !AHEAD_PENDING.load(Ordering::Acquire) { return; }
    {
        let mut inbox = AHEAD_INBOX.lock().unwrap_or_else(|e| e.into_inner());
        AHEAD_PENDING.store(false, Ordering::Relaxed);
        std::mem::swap(&mut *inbox, &mut q.inbox);
    }
    let mut items = std::mem::take(&mut q.inbox);
    for (front, work, at) in items.drain(..) { q.push(Job::Ahead(front, work), at); }
    q.inbox = items;
}
pub(super) fn counts() -> [usize; TIERS] {
    std::array::from_fn(|i| COUNTS[i].load(Ordering::Relaxed))
}
pub(super) fn running() -> [usize; TIERS] {
    std::array::from_fn(|i| RUNNING[i].load(Ordering::Relaxed))
}
pub(super) fn tiers() -> Vec<TierEntry> {
    tiers_where(|_| true)
}
/// The queued O3 entries `keep` takes (it runs under the queue's lock).
pub(super) fn tiers_where(keep: impl Fn(&TierEntry) -> bool) -> Vec<TierEntry> {
    drain_tier_inbox();
    lock().entries.iter().filter_map(|e| if let Job::O3(t) = e.job { keep(&t).then_some(t) } else { None }).collect()
}
pub(super) fn retire_front(front: &Arc<FrontShared>) {
    front.alive.store(false, Ordering::SeqCst);
    retain(|job| !matches!(job, Job::Ahead(f, _) if Arc::ptr_eq(f, front)));
    wake_all();
}
pub(super) fn retain(mut keep: impl FnMut(&Job) -> bool) {
    let mut q = lock();
    let mut i = 0;
    while i < q.entries.len() {
        if keep(&q.entries[i].job) { i += 1; } else { q.remove(i); }
    }
}
impl Queue {
    /// Promotes slot `id`'s queued and running jobs to their waiters' class; returns whether a
    /// queued one can start now.
    fn promote(&mut self, unit: usize, id: u32) -> bool {
        self.sync_ahead();
        let Some(wanted) = self.wanted((unit, id)) else { return false };
        let serials = self.by_slot.remove(&(unit, id)).unwrap_or_default();
        for serial in serials.iter() {
            let e = &mut self.entries[self.indices[&serial]];
            self.plan[e.job.tier()][e.class as usize].remove(&order(e.key, e.serial));
            let old = if e.job.tier() == 0 && !e.promoted { self.ahead_len -= 1; self.ahead_class } else { e.class };
            budget::queued(old, budget::JIT, -1);
            e.promoted = true;
            e.class = wanted.min(old);
            self.plan[e.job.tier()][e.class as usize].insert(order(e.key, e.serial));
            budget::queued(e.class, budget::JIT, 1);
        }
        for r in &self.running {
            if r.waits_for(unit, id) && wanted < r.class.class() { r.class.reclass(wanted); }
        }
        let policy = self.policy();
        let runnable = serials.iter().any(|serial| self.blocked(&self.entries[self.indices[&serial]], &policy).is_none());
        if !serials.is_empty() { self.by_slot.insert((unit, id), serials); }
        runnable
    }
}
/// Promotion lives exactly as long as its waiter waits, including unwinding/cancellation (a lower
/// class than the job's asks for nothing, and its last waiter ends it).
pub(super) struct WaitPriority { unit: usize, id: u32, class: u8 }
impl Drop for WaitPriority {
    fn drop(&mut self) {
        let _kept = crate::zone_alloc::kept();
        // (a wait's end only lowers classes: nothing becomes runnable)
        lock().end_wait(self.unit, self.id, self.class);
    }
}
impl Queue {
    fn end_wait(&mut self, unit: usize, id: u32, class: u8) {
        self.sync_ahead();
        let key = (unit, id);
        let counts = self.waiters.get_mut(&key).unwrap();
        counts[class as usize] -= 1;
        if counts.iter().any(|&n| n != 0) { return; }
        self.waiters.remove(&key);
        let now = HeatNow::take();
        let serials = self.by_slot.remove(&key).unwrap_or_default();
        for serial in serials.iter() {
            let e = &mut self.entries[self.indices[&serial]];
            let tier = e.job.tier();
            self.plan[tier][e.class as usize].remove(&order(e.key, e.serial));
            budget::queued(e.class, budget::JIT, -1);
            e.promoted = false;
            e.class = if tier == 0 { self.ahead_len += 1; budget::BACKGROUND } else { job_class(&e.job, &now) };
            budget::queued(if tier == 0 { self.ahead_class } else { e.class }, budget::JIT, 1);
            self.plan[tier][e.class as usize].insert(order(e.key, e.serial));
        }
        if !serials.is_empty() { self.by_slot.insert(key, serials); }
        let mut restore: Vec<(Arc<JobClass>, u8)> = Vec::new();
        for r in &self.running {
            if !r.waits_for(unit, id) || restore.iter().any(|(c, _)| Arc::ptr_eq(c, &r.class)) { continue; }
            let wanted = self.running.iter().filter(|other| Arc::ptr_eq(&r.class, &other.class))
                .filter_map(|other| other.slot.and_then(|slot| self.wanted(slot))).min();
            restore.push((r.class.clone(), wanted.unwrap_or(r.base_class)));
        }
        // Serialize restoration with a new waiter's promotion of the same job.
        for (class, priority) in restore { class.reclass(priority); }
    }
}
impl Running {
    fn waits_for(&self, unit: usize, id: u32) -> bool { self.slot == Some((unit, id)) }
}
/// A thread of `class` waits for slot `id`'s job (owner::waiter_class) until this is dropped.
pub(super) fn promote(unit: usize, id: u32, class: u8) -> WaitPriority {
    let _kept = crate::zone_alloc::kept();
    let mut q = lock();
    q.waiters.entry((unit, id)).or_default()[class as usize] += 1;
    let runnable = q.promote(unit, id);
    drop(q);
    if runnable { wake(); }
    WaitPriority { unit, id, class }
}

/// (tests) The class of slot `id`'s running job.
#[cfg(test)]
pub(super) fn running_class(unit: usize, id: u32) -> Option<u8> {
    let _kept = crate::zone_alloc::kept();
    lock().running.iter().find(|r| r.waits_for(unit, id)).map(|r| r.class.class())
}

/// Something waits for the check beside the program: its queued and running jobs take the waiter's
/// class and go first in it (job_class and key give the jobs it queues from now on the same).
pub(super) fn promote_checks() {
    if FORKED.load(Ordering::Relaxed) { return; }
    let _kept = crate::zone_alloc::kept();
    let mut q = lock();
    let checks: Vec<u64> = q.entries.iter().filter(|e| matches!(e.job, Job::Check(..)) && e.class != budget::INTERACTIVE).map(|e| e.serial).collect();
    for serial in checks {
        let i = q.indices[&serial];
        let (key, class) = (q.entries[i].key, q.entries[i].class);
        q.plan[5][class as usize].remove(&order(key, serial));
        budget::queued(class, budget::JIT, -1);
        q.entries[i].class = budget::INTERACTIVE;
        q.entries[i].key = CHECK_WAITED_KEY;
        q.plan[5][budget::INTERACTIVE as usize].insert(order(CHECK_WAITED_KEY, serial));
        budget::queued(budget::INTERACTIVE, budget::JIT, 1);
    }
    for r in q.running.iter().filter(|r| r.tier == 5) { r.class.reclass(budget::INTERACTIVE); }
    drop(q);
    wake_all();
}

impl Source for CompilerSource {
    fn run(&'static self) {
        let index = NEXT_WORKER.fetch_add(1, Ordering::Relaxed);
        let name = crate::zone_alloc::kept_with(|| std::thread::current().name().unwrap_or("run-jit").to_owned());
        crate::lifeline::run(&name, crate::lifeline::OnPanic::Restart, || worker_loop(index));
    }
}
fn worker_loop(index: usize) {
        let worker = &THREADS.get().unwrap()[index];
        worker.thread.set(budget::Wake::current()).ok();
        let mut admission = budget::Admission::default();
        // the job this worker ran last, (serial, tier): it leaves `running` under the lock that
        // takes the next one (one queue lock per job)
        let mut finished: Option<(u64, usize)> = None;
        // the last job's permit, held for the next job while that is of its class and label and
        // nobody of the class or above waits for a slot: an admission and a release per job were
        // two budget locks for each of a cold start's ~300k checks ahead
        let mut held: Option<BudgetJob> = None;
        let me = std::thread::current().id();
        loop {
            worker.waiting.store(true, Ordering::SeqCst);
            // Atomics-only producers publish to the existing tier-record inbox.
            drain_tier_inbox();
            // one queue lock per pick, idle or not: the tick's timer is taken and given back
            // under it too
            let next = {
                let _kept = crate::zone_alloc::kept();
                let mut q = lock();
                // (this worker parked as the tick's timer: the next idle one takes it again)
                if q.timer == Some(me) { q.timer = None; }
                let instant = Instant::now();
                if let Some((serial, tier)) = finished.take() {
                    q.running.retain(|r| r.serial != serial);
                    RUNNING[tier].fetch_sub(1, Ordering::Relaxed);
                    if tier == 4 { q.tick_queued = false; q.tick_at = instant + tick_delay(); }
                }
                if !q.tick_queued && instant >= q.tick_at {
                    q.tick_queued = true;
                    q.push(Job::Tick, wall_ns());
                }
                let now = HeatNow::take();
                let head = q.head(&now);
                if head.is_none() { admission = budget::Admission::default(); }
                let picked = head.and_then(|i| {
                    let e = &q.entries[i];
                    let class = q.class_of(e);
                    let job = match held.take() {
                        Some(job) if job.keeps(class, e.job.label()) => job,
                        other => {
                            drop(other);
                            let p = admission.poll(class, budget::JIT, e.job.label())?;
                            BudgetJob::from_permit(p, class, e.job.label())
                        }
                    };
                    // one entry per admission: the next free worker takes the next one (a batch held
                    // the frontier the main thread reached next behind the rest of its batch)
                    let e = q.remove(i);
                    RUNNING[e.job.tier()].fetch_add(1, Ordering::Relaxed);
                    q.running.push(Running { serial: e.serial, promoted: e.promoted, slot: e.slot, tier: e.job.tier(), base_class: job_class(&e.job, &now), class: job.control() });
                    // another parked worker only for work this one leaves runnable: a wake per job
                    // that finds the rest blocked (the startup's ahead width) costs the owner its
                    // queue lock and the cores their caches
                    let more = q.runnable();
                    Some((e, job, more))
                });
                // idle, or held back by the budget: parked until a wake, the timer worker until
                // the tick is due
                picked.ok_or_else(|| (!q.tick_queued && q.timer.is_none()).then(|| {
                    q.timer = Some(me);
                    q.tick_at.saturating_duration_since(instant)
                }))
            };
            match next {
                Ok((entry, permit, more)) => {
                    worker.waiting.store(false, Ordering::SeqCst);
                    crate::lifeline::beat();
                    if more { wake(); }
                    let (serial, tier) = (entry.serial, entry.job.tier());
                    crate::zone_alloc::kept_with(|| budget::waited(entry.class, budget::JIT, wall_ns().saturating_sub(entry.at), entry.job.label()));
                    let active = if let Job::O3(tier) = &entry.job {
                        TIER_ACTIVE.lock().unwrap_or_else(|e| e.into_inner()).push(tier.0);
                        Some(TierActive(tier.0))
                    } else { None };
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_job(entry.job)));
                    drop(active);
                    if let Err(payload) = result {
                        let (message, bail) = panic_payload(&*payload);
                        if !bail { eprintln!("makepad: JIT job panicked: {message}"); }
                    }
                    held = Some(permit);
                    finished = Some((serial, tier));
                    // (this worker takes the next job itself: no wake; it keeps its arena chunks for
                    // it and hands them back when it goes idle)
                }
                Err(timeout) => {
                    // (an idle worker, or one the budget holds back, has no permit)
                    drop(held.take());
                    crate::zone_alloc::compiler_hand_back_idle();
                    if worker.waiting.load(Ordering::SeqCst) {
                        if let Some(timeout) = timeout { budget::park_for(timeout.as_nanos().min(u64::MAX as u128) as u64); }
                        else { budget::park(); }
                    }
                    worker.waiting.store(false, Ordering::SeqCst);
                }
            }
        }
}

fn class_name(class: u8) -> &'static str {
    match class { budget::INTERACTIVE => "Interactive", budget::VISIBLE => "Visible", _ => "Background" }
}
fn slot_label(slot: Option<(usize, u32)>) -> String {
    match slot {
        Some((unit, id)) => {
            let name = (unit == cur_unit() as usize).then(|| slot_name(id)).flatten();
            name.map_or_else(|| format!("slot={id}"), |name| format!("slot={id} {name}"))
        }
        None => String::new(),
    }
}
pub(super) fn tier_position(id: u32) -> Option<(TierEntry, usize, usize)> {
    let _kept = crate::zone_alloc::kept();
    drain_tier_inbox();
    let mut q = lock();
    q.refresh(&HeatNow::take());
    let order = q.ordered(usize::MAX);
    order.iter().enumerate().find_map(|(position, &(i, _))| match q.entries[i].job {
        Job::O3(t) if t.0 == id => Some((t, position, order.len())),
        _ => None,
    })
}
/// The slots with an O3 job in the queue: one walk of it, where `tier_position` walks all of it per slot.
pub(super) fn tier_queued() -> std::collections::HashSet<u32> {
    let _kept = crate::zone_alloc::kept();
    drain_tier_inbox();
    lock().entries.iter().filter_map(|e| match e.job {
        Job::O3(t) => Some(t.0),
        _ => None,
    }).collect()
}
pub(super) fn view(n: usize) -> Vec<String> {
    let _kept = crate::zone_alloc::kept();
    drain_tier_inbox();
    let now = HeatNow::take();
    let mut q = lock();
    q.refresh(&now);
    let mut rows = vec![format!("building: {} jobs; queued: {}; queue lock max: {:.3} ms", q.running.len(), q.entries.len(), LOCK_MAX_NS.load(Ordering::Relaxed) as f64 / 1e6)];
    for r in &q.running {
        rows.push(format!("running {} class={} {}", ["ahead", "O2", "O3", "spec", "tick", "check"][r.tier], class_name(r.class.class()), slot_label(r.slot)));
    }
    for (i, blocked) in q.ordered(n).into_iter() {
        let e = &q.entries[i];
        let (heat, ui) = match &e.job {
            Job::O2(unit, id, _) => now.heat(*unit, *id),
            Job::O3(t) => now.heat(0, t.0),
            _ => (0, false),
        };
        let why = match &e.job {
            Job::O3(t) => Q_WHY[t.1 as usize],
            Job::O2(_, _, kind) => match kind { O2Kind::Normal => "optimize", O2Kind::Base => "baseline", O2Kind::Borrow => "borrow", O2Kind::Verified => "verified O1" },
            Job::Ahead(_, AheadWork::Slot(_)) => "prepare", Job::Ahead(_, AheadWork::Body(..)) => "inline candidate", Job::Ahead(_, AheadWork::Look(..)) => "look ahead", Job::Spec(..) => "speculate", Job::Tick => "housekeeping", Job::Check(..) => "dependency check",
        };
        let name = slot_label(e.slot);
        rows.push(format!("queued {} class={} heat={}{} why={} waited={:.3}s {}{}", e.job.label(), class_name(q.class_of(e)), heat,
            if ui { " ui" } else { "" }, why, now.wall.saturating_sub(e.at) as f64 / 1e9, name,
            blocked.map_or_else(String::new, |why| format!(" [blocked: {why}]"))));
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admission_order_and_waiter_promotion() {
        let mut order = [
            (budget::BACKGROUND, heat_key(1024, false, 0), 0),
            (budget::VISIBLE, heat_key(2, false, 0), 1),
            (budget::INTERACTIVE, heat_key(0, false, 0), 2),
            (budget::VISIBLE, heat_key(64, false, 0), 3),
        ];
        order.sort_by_key(|&(class, key, serial)| rank(class, key as i64, serial));
        assert_eq!(order.map(|e| e.2), [2, 3, 1, 0], "class before heat; hot first within class");
        assert_eq!(heat_key(16, true, 0), heat_key(32, false, 0), "UI boost doubles heat");
        assert_eq!(heat_key(16, false, 4_000_000_000), heat_key(32, false, 0), "age prevents cold work starving");
        assert!(!eligible(budget::VISIBLE, true, 1), "burst admits only one noninteractive job");
        assert!(eligible(budget::INTERACTIVE, true, 1), "UI waiter passes the burst gate");
        assert!(eligible(budget::BACKGROUND, true, 0), "burst permits one background job");
        assert_eq!(epoch_key(16, false, 0), epoch_key(32, false, 4_000_000_000), "aging is the enqueue epoch, without a refresh");
        assert_eq!(drain_blocked(budget::BACKGROUND, 1, 1, 2, 1), Some("visible reserve"), "parked Background builds cannot fill the Visible drain");
        assert_eq!(drain_blocked(budget::VISIBLE, 1, 1, 2, 1), None, "Visible work passes a parked Background build");
        assert_eq!(drain_blocked(budget::INTERACTIVE, 2, 1, 2, 1), None, "first compiles retain their drain above both lower classes");
        let mut q = Queue::new();
        q.push(Job::Spec(1, 10), 1);
        q.push(Job::Spec(1, 11), 2);
        q.waiters.insert((1, 11), [0, 2]);
        q.promote(1, 11);
        assert_eq!(q.entries[1].class, budget::VISIBLE, "a job takes its waiters' class, not more");
        q.waiters.get_mut(&(1, 11)).unwrap()[0] += 1;
        q.promote(1, 11);
        assert_eq!(q.entries[1].class, budget::INTERACTIVE, "the highest waiter's class");
        q.end_wait(1, 11, 0);
        q.end_wait(1, 11, 1);
        assert_eq!(q.entries[1].class, budget::INTERACTIVE, "another waiter retains promotion");
        q.end_wait(1, 11, 1);
        assert_eq!(q.entries[1].class, budget::BACKGROUND, "last waiter restores the queued class");
        q.waiters.insert((1, 11), [1, 0]);
        q.promote(1, 11);
        let now = HeatNow { hu: None, wall: 100, tier: 0 };
        assert_eq!(job_class(&Job::O2(1, 7, O2Kind::Normal), &now), budget::BACKGROUND, "a tier-up runs below the app's visible work");
        assert_eq!(job_class(&Job::O3((7, 0, 0, 0)), &now), budget::BACKGROUND, "an O3 build runs below the app's visible work");
        let first = q.head(&now).expect("queued waiter can run");
        assert_eq!(q.entries[first].class, budget::INTERACTIVE, "heat refresh preserves waiter promotion");
        assert_eq!(q.remove(first).job.slot(), Some((1, 11)), "queued waiter moves to the front");
        let next = q.head(&now).expect("cold work can run after the waiter");
        assert_eq!(q.remove(next).job.slot(), Some((1, 10)), "cold work remains queued after promotion");
        assert!(q.head(&now).is_none(), "both jobs were removed exactly once");
        let front = Arc::new(FrontShared::new(&Arc::new(crate::Session::default())));
        front.alive.store(true, Ordering::Release);
        q.push(Job::Spec(1, 12), 1);
        q.push(Job::Ahead(front, AheadWork::Slot(13)), 100);
        q.set_ahead_class(budget::BACKGROUND);
        assert_eq!(q.entries[q.ordered(1)[0].0].job.tier(), 3, "older cold work leads outside a UI burst");
        q.set_ahead_class(budget::INTERACTIVE);
        let first = q.ordered(1)[0].0;
        assert_eq!(q.entries[first].job.tier(), 0, "a UI burst promotes the whole ahead bucket immediately");
        assert_eq!(q.remove(first).class, budget::INTERACTIVE, "admission uses the bucket's current class");
        q.remove(0);
        assert_eq!(q.ahead_len, 0, "removals balance bucket accounting");
    }
}

/// Release unused burst results once no ahead job can still publish into them. The startup's
/// look-ahead still queued goes when the startup ends.
pub(super) fn cleanup_ahead() {
    if startup_phase() { return; }
    static LOOKS_GONE: AtomicBool = AtomicBool::new(false);
    if !LOOKS_GONE.swap(true, Ordering::Relaxed) && LOOK_QUEUED.load(Ordering::Relaxed) != 0 {
        retain(|job| !matches!(job, Job::Ahead(_, AheadWork::Look(..))));
    }
    // (between the UI's bursts, or past the startup of a program that never presents)
    if ahead_phase() && ahead_burst() { return; }
    let front = LAST_FRONT.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let Some(front) = front else { return };
    let q = lock();
    if q.entries.iter().any(|e| e.job.tier() == 0) || q.running.iter().any(|r| r.tier == 0) { return; }
    let keys = std::mem::take(&mut *front.ahead_post.lock().unwrap_or_else(|e| e.into_inner()));
    let unused: Vec<_> = keys.iter().filter_map(|key| front.ahead.remove(key)).collect();
    let lowered = std::mem::take(&mut *front.lowered.lock().unwrap_or_else(|e| e.into_inner()));
    drop(q);
    lowered_end();
    if !keys.is_empty() {
        AHEAD_DROPPED.fetch_add(unused.len() as u64, Ordering::Relaxed);
        crate::postframe::ahead_over();
    }
    drop((keys, unused, lowered));
}
