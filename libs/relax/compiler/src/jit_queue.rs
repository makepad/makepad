//! The JIT's single compile queue. Ordering and admission are decided together,
//! before an entry leaves the queue; a blocked admission never reserves stale work.
use super::*;
use makepad_cpu_budget as budget;
use crate::pool::{CurrentJob, JobClass, Source};
use std::sync::{Arc, OnceLock};
use std::time::Instant;
use std::sync::atomic::AtomicUsize;
use std::collections::BTreeSet;
// (Fx: the maps are looked up under the queue's lock at every pick and push; their keys are serials and slots)
use crate::fxhash::HashMap;

pub(super) type TierEntry = (u32, u8, u64, u64);
/// `Base`: a baseline slot's O2 build (it ran baseline code, or the startup sampled it); `TwinBase`:
/// the base of a twin, asked for by the twin's install or its callers' relowering (nothing calls
/// the base until its build links: `budget::WORK_BEHIND`).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum O2Kind { Normal, Base, TwinBase, Borrow, Verified }

pub(super) enum Job {
    Ahead(Arc<FrontShared>, AheadWork),
    O2(usize, u32, O2Kind),
    O3(TierEntry),
    Spec(usize, u32),
    /// the real-time reach's build of a slot it reached (jit.rs reach_build): installed at once,
    /// its references walked at the install. Spec's tier. True: a ticket waits for the reach (a
    /// device's start, a tier-up's install): Interactive, behind everything a program thread
    /// waits for (REACH_WAITED_KEY); else a build nobody waits for, before the rest of its class
    /// (REACH_KEY)
    Reach(usize, u32, bool),
    Tick,
    /// a slice of a check beside the program (check.rs CheckBatch)
    Check(Arc<crate::check::CheckBatch>),
}
impl Job {
    fn slot(&self) -> Option<(usize, u32)> {
        match self {
            Self::Ahead(f, AheadWork::Slot(id)) => Some((f.unit.load(Ordering::Acquire) as usize, *id)),
            Self::Ahead(_, AheadWork::Body(..) | AheadWork::Look(..)) | Self::Tick | Self::Check(..) => None,
            Self::O2(u, id, _) | Self::Spec(u, id) | Self::Reach(u, id, _) => Some((*u, *id)),
            Self::O3(e) => Some((TIER_UNIT.load(Ordering::Acquire) as usize, e.0)),
        }
    }
    fn label(&self) -> &'static str {
        match self { Self::Ahead(..) => "JIT ahead", Self::O2(..) => "JIT O2", Self::O3(..) => "JIT O3", Self::Spec(..) => "JIT spec", Self::Reach(..) => "JIT reach", Self::Tick => "JIT tick", Self::Check(..) => "JIT check" }
    }
    fn tier(&self) -> usize {
        match self { Self::Ahead(..) => 0, Self::O2(..) => 1, Self::O3(..) => 2, Self::Spec(..) | Self::Reach(..) => 3, Self::Tick => 4, Self::Check(..) => 5 }
    }
}
/// `slot`: the job's slot when it was queued (a wait for that slot promotes it). `since` (wall ns):
/// when it took its class, its queue time or its latest raise (`since_of`): its wait is reported at
/// its class from then, so a build that sat in the background and was then waited for reports the
/// waiter's wait, not its whole queue time, at the waiter's class.
struct Entry { serial: u64, job: Job, at: u64, since: u64, urgent: bool, class: u8, promoted: bool, key: i64, slot: Option<(usize, u32)> }
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
// Match the budget's DEMAND_SPAN_NS so a grant responds to outside work at the same rate.
const GRANT_SPAN_NS: u64 = 100_000_000;
/// A granted run (`MAKEPAD_CPU_GRANT=1`: devhub's test shards, told N cores of a shared box): the
/// cores in use outside the JIT's drains, per span the process's CPU time less the drains' running
/// time, so the JIT's jobs, waited ones too, fit what the run's own threads leave of the grant: the
/// cores the box was promised. A job asleep on other JIT work (`pool::suspend`) uses none and
/// leaves its place, so what it waits for can start, and one job always runs: slow, never
/// deadlocked. Section workers' CPU reads as outside: the JIT narrows itself, never the app.
/// Rolled under the queue lock at each pick, the only place `running` changes.
struct Grant {
    /// the span's start (wall ns) and the process's CPU time then
    span_at: u64,
    span_cpu: u64,
    /// awake running drain jobs times ns, summed; its value at the span's start; when it was
    /// advanced to
    area: u64,
    span_area: u64,
    area_at: u64,
    /// the last span's reading (thousandths of a core) and the larger of the last two, whole cores
    last: u64,
    outside: usize,
}
fn outside_reading(cpu_ns: u64, jobs_ns: u64, span: u64) -> u64 {
    cpu_ns.saturating_sub(jobs_ns).saturating_mul(1000) / span.max(1)
}
impl Grant {
    fn new(now: u64, cpu: u64) -> Self {
        Self { span_at: now, span_cpu: cpu, area: 0, span_area: 0, area_at: now, last: 0, outside: 0 }
    }
    fn roll(&mut self, now: u64, running: usize, cpu: impl FnOnce() -> u64) {
        let elapsed = now.saturating_sub(self.area_at);
        self.area = self.area.saturating_add((running as u64).saturating_mul(elapsed));
        self.area_at = now;
        let span = now.saturating_sub(self.span_at);
        if span < GRANT_SPAN_NS { return; }
        let cpu = cpu();
        self.observe(outside_reading(cpu.saturating_sub(self.span_cpu), self.area - self.span_area, span));
        self.span_at = now;
        self.span_cpu = cpu;
        self.span_area = self.area;
    }
    fn observe(&mut self, reading: u64) {
        self.outside = ((reading.max(self.last) + 500) / 1000) as usize;
        self.last = reading;
    }
}
/// The admission checks' view of the running jobs and the phase (`Queue::policy`).
struct Policy {
    /// the burst gate: one build at a time below Interactive while the startup runs or the UI is
    /// blocked on the JIT (postframe::ui_in_episode)
    burst: bool,
    /// running builds below Interactive (the burst gate's count: a check beside the program is none)
    builds: usize,
    noninteractive: usize,
    /// all running Background jobs, including ahead work and the tick
    background: usize,
    /// running Background O2, speculative, reach and check jobs
    breadth: usize,
    /// the running jobs per tier (the startup's tier widths)
    tiers: [usize; TIERS],
    /// through the startup the checks ahead of the main thread run on a few workers, as many as its
    /// helper threads were (STARTUP_AHEAD), and all of them do: a slot the main thread waits for is
    /// admitted beyond
    ahead_full: bool,
    limit: usize,
    background_limit: usize,
    /// the granted cores left for the JIT's jobs after outside work (usize::MAX: not a granted run)
    grant_width: usize,
    /// running jobs but the tick that are not asleep on other JIT work: what the grant counts
    awake: usize,
    /// an O3 job is queued or running
    o3_pending: bool,
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
    grant: Option<Grant>,
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
    /// when `ahead_class` last rose (wall ns): the unpromoted checks ahead took it then
    ahead_since: u64,
    ahead_len: usize,
    tick_at: Instant,
    tick_queued: bool,
    timer: Option<std::thread::ThreadId>,
    /// AHEAD_INBOX's other buffer (its entries move in without a new allocation per move)
    inbox: Vec<(Arc<FrontShared>, AheadWork, u64)>,
    /// the run quits (`quit`): no check or lowering ahead, O2 or speculative build nobody waits for
    /// is queued
    quitting: bool,
    /// the open interval of the hold ledger (`note_hold`)
    hold: Option<Hold>,
}
/// An open interval of the hold ledger: since `since` (wall ns) a free worker found the best queued
/// tier-up held for `HOLD_REASONS[reason]`, `queued` tier-ups in the queue, before or after the
/// first present.
struct Hold { reason: usize, after: bool, since: u64, queued: u64 }
impl Queue {
    fn new() -> Self {
        // (made once, by `queue()`: the run's grant is its environment's)
        let granted = std::env::var_os("MAKEPAD_CPU_GRANT").is_some_and(|v| v == "1");
        let grant = granted.then(|| Grant::new(wall_ns(), crate::first_frame::process_cpu_ns()));
        Self { entries: Vec::new(), running: Vec::new(), serial: 0, waiters: HashMap::default(), plan: std::array::from_fn(|_| std::array::from_fn(|_| BTreeSet::new())), indices: HashMap::default(), by_slot: HashMap::default(), refresh_cursor: 0, ahead_class: budget::BACKGROUND, ahead_since: 0, ahead_len: 0,
            tick_at: Instant::now(), tick_queued: false, timer: None, inbox: Vec::new(), quitting: false, hold: None, grant }
    }
    /// Queues `job` and identifies a drain for it if it can start now.
    pub(super) fn push(&mut self, job: Job, at: u64) -> Option<Kind> {
        // A producer can finish discovering work after the settle rebuild retired its front.
        if matches!(&job, Job::Ahead(front, _) if !front.alive.load(Ordering::Acquire)) { return None; }
        if !matches!(job, Job::Tick) && W0_IDLING.load(Ordering::Relaxed) { self.tick_at = Instant::now(); }
        self.sync_ahead();
        let now = HeatNow::take();
        let slot = job.slot();
        let wanted = slot.and_then(|slot| self.wanted(slot));
        // the run quits: a compile nobody waits for is speculative (`quit`)
        if wanted.is_none() && self.quitting && quit_drops(&job) { return None; }
        let promoted = wanted.is_some();
        let class = if job.tier() == 0 { budget::BACKGROUND } else { job_class(&job, &now) };
        let class = wanted.map_or(class, |w| w.min(self.bucket_class(job.tier(), class)));
        self.serial += 1;
        let urgent = at == O2_FRONT_AT && matches!(job, Job::O2(..));
        let at = if urgent { wall_ns() } else { at };
        let mut entry = Entry { serial: self.serial, job, at, since: at, urgent, class, promoted, key: 0, slot };
        entry.key = key(&entry, &now);
        self.plan[entry.job.tier()][class as usize].insert(order(entry.key, entry.serial));
        self.indices.insert(entry.serial, self.entries.len());
        if let Some(slot) = slot { self.by_slot.entry(slot).or_default().push(entry.serial); }
        let class = self.class_of(&entry);
        if entry.job.tier() == 0 && !entry.promoted { self.ahead_len += 1; }
        COUNTS[entry.job.tier()].fetch_add(1, Ordering::Relaxed);
        if matches!(entry.job, Job::Reach(..)) { REACH_QUEUED.fetch_add(1, Ordering::SeqCst); }
        self.entries.push(entry);
        let policy = self.policy();
        let entry = self.entries.last().unwrap();
        self.blocked(entry, &policy).is_none().then(|| DrainsNow::get().for_job(entry.job.tier(), class))
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
            if class < self.ahead_class { self.ahead_since = wall_ns(); }
            self.ahead_class = class;
        }
    }
    fn bucket_class(&self, tier: usize, stored: u8) -> u8 {
        if tier == 0 && stored == budget::BACKGROUND { self.ahead_class } else { stored }
    }
    fn class_of(&self, e: &Entry) -> u8 { self.bucket_class(e.job.tier(), e.class) }
    /// When `e` took the class it is reported at (`class_of`): an unpromoted check ahead took the
    /// ahead class with the others, at its raise.
    fn since_of(&self, e: &Entry) -> u64 {
        if e.job.tier() == 0 && !e.promoted && self.ahead_class != budget::BACKGROUND { e.since.max(self.ahead_since) } else { e.since }
    }
    // One heat sample per selection, O(log n). Age is a common epoch offset: it never
    // requires touching existing entries. New hotter work still gets sampled as we rotate.
    fn refresh(&mut self, now: &HeatNow) {
        self.sync_ahead();
        if self.entries.is_empty() { return; }
        self.refresh_cursor %= self.entries.len();
        let i = self.refresh_cursor;
        self.refresh_cursor += 1;
        self.rekey(i, now);
    }
    /// Entry `i`'s class and key sampled again, its place in the plan with them. True: either moved.
    fn rekey(&mut self, i: usize, now: &HeatNow) -> bool {
        let entry = &mut self.entries[i];
        if entry.job.tier() == 0 { return false; }
        let class = if entry.promoted { entry.class } else { job_class(&entry.job, now) };
        let key = key(entry, now);
        if class == entry.class && key == entry.key { return false; }
        self.plan[entry.job.tier()][entry.class as usize].remove(&order(entry.key, entry.serial));
        if class != entry.class {
            entry.since = if class < entry.class { now.wall } else if class == budget::BACKGROUND { entry.at } else { entry.since };
            entry.class = class;
        }
        entry.key = key;
        self.plan[entry.job.tier()][entry.class as usize].insert(order(entry.key, entry.serial));
        true
    }
    /// The entry to start now. An O3 entry's heat is live (its slot's record is charged until the
    /// build), so the one chosen has its key read again: moved since its last sample (code that
    /// stopped running, or got hotter), it goes to its place and the pick runs again. Each entry
    /// moves at most once per pick (its key is then current).
    fn head(&mut self, now: &HeatNow, takes: Takes) -> Option<usize> {
        self.refresh(now);
        // (a key read twice at one `now` is the same unless the program charged the record in
        // between: the walk ends after one pass over the entries whatever happens)
        for _ in 0..self.entries.len() {
            let i = self.first_for(takes)?;
            if !matches!(self.entries[i].job, Job::O3(_)) || !self.rekey(i, now) {
                return Some(i);
            }
        }
        self.first_for(takes)
    }
    /// The entry to start now, in rank order (head without its heat sample): the first class whose
    /// buckets hold one that can start, the best of their fronts.
    fn first(&self) -> Option<usize> {
        self.first_for(Takes::ALL)
    }
    /// `first` among the entries a worker that `takes` these may start.
    fn first_for(&self, takes: Takes) -> Option<usize> {
        let policy = self.policy();
        (0..3u8).find_map(|class| {
            self.plan.iter().enumerate().flat_map(|(tier, plan)| plan.iter().enumerate().map(move |(stored, bucket)| (tier, stored as u8, bucket)))
                .filter(|&(tier, stored, bucket)| !bucket.is_empty() && takes.takes(tier, class) && self.bucket_class(tier, stored) == class
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
    /// A worker to wake for the first runnable entry.
    fn runnable(&self) -> Option<Kind> {
        let e = &self.entries[self.first()?];
        Some(DrainsNow::get().for_job(e.job.tier(), self.class_of(e)))
    }
    /// What the admission checks read of the running jobs and the phase, once per decision. The
    /// burst gate holds one build at a time below Interactive while the startup runs or the UI thread
    /// is in a JIT block episode: its hold ends with the episode (a frame, or no wait for
    /// EPISODE_GAP_NS), not after a span of quiet. The tick is housekeeping beside the builds, not
    /// one of them: it neither counts nor waits (the O2 worker and the first O3 worker's polls ran
    /// side by side; a tick that waited for the owner held the one slot the startup's O2 builds had,
    /// and half as many ran).
    fn policy(&self) -> Policy {
        let mut policy = Policy {
            burst: !past_startup() || crate::postframe::ui_in_episode(),
            builds: 0,
            noninteractive: 0,
            background: 0,
            breadth: 0,
            tiers: [0; TIERS],
            ahead_full: false,
            limit: budget::limit() as usize,
            background_limit: budget::background_limit() as usize,
            grant_width: self.grant.as_ref().map_or(usize::MAX, |g| (budget::cores() as usize).saturating_sub(g.outside)),
            awake: 0,
            o3_pending: COUNTS[2].load(Ordering::Relaxed) + RUNNING[2].load(Ordering::Relaxed) > 0,
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
            if r.tier != 4 && !r.class.asleep() { policy.awake += 1; }
            if class == budget::BACKGROUND {
                policy.background += 1;
                if is_breadth(r.tier) { policy.breadth += 1; }
            }
            policy.tiers[r.tier] += 1;
            if r.tier == 0 && !r.promoted { ahead += 1; }
        }
        policy.ahead_full = startup_phase() && ahead >= startup_ahead();
        policy
    }
    fn class_blocked(&self, class: u8, tier: usize, policy: &Policy) -> Option<&'static str> {
        if tier < 4 && !eligible(class, policy.burst, policy.builds) { return Some("UI burst"); }
        if tier != 4 && policy.awake >= policy.grant_width.max(1) { return Some("grant"); }
        if class != budget::INTERACTIVE {
            if let Some(why) = drain_blocked(class, tier, policy) { return Some(why); }
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
    /// Why the best queued tier-up (O2, O3) does not start at this pick: its gate (`class_blocked`,
    /// `entry_blocked`), else "idle lane" when only the other kind of worker may start it
    /// (`takes`). None when no tier-up is queued or nothing holds it.
    fn tier_up_hold(&self, takes: Takes) -> Option<usize> {
        if (COUNTS[1].load(Ordering::Relaxed) + COUNTS[2].load(Ordering::Relaxed)) == 0 { return None; }
        let policy = self.policy();
        let (_, tier, class, serial) = [1usize, 2].into_iter()
            .flat_map(|tier| self.plan[tier].iter().enumerate().filter_map(move |(stored, bucket)| bucket.iter().next().map(|&(key, serial)| (tier, stored as u8, key, serial))))
            .map(|(tier, stored, key, serial)| {
                let class = self.bucket_class(tier, stored);
                (rank(class, key.0, serial), tier, class, serial)
            })
            .min_by_key(|r| r.0)?;
        let why = self.class_blocked(class, tier, &policy).or_else(|| self.entry_blocked(&self.entries[self.indices[&serial]]));
        let reason = match why {
            Some(why) => why,
            None if !takes.takes(tier, class) => "idle lane",
            None => return None,
        };
        Some(HOLD_REASONS.iter().position(|r| *r == reason).unwrap_or(HOLD_REASONS.len() - 1))
    }
    /// The hold ledger at a pick that parks or starts a tier-up: the open interval ends when its
    /// reason, the phase (the first present) or the queued tier-ups' count changed, or a tier-up
    /// starts (`reason` None); its wall goes to `HELD_NS` and that times the queued count to
    /// `HELD_QUEUED_NS`. Under the queue lock: a clock read and two atomics per interval.
    fn note_hold(&mut self, reason: Option<usize>, now: u64) {
        let after = past_startup();
        let queued = (COUNTS[1].load(Ordering::Relaxed) + COUNTS[2].load(Ordering::Relaxed)) as u64;
        if matches!(&self.hold, Some(h) if Some(h.reason) == reason && h.after == after && h.queued == queued) { return; }
        if let Some(h) = self.hold.take() { h.close(now); }
        self.hold = reason.map(|reason| Hold { reason, after, since: now, queued });
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
        if matches!(e.job, Job::Reach(..)) { REACH_QUEUED.fetch_sub(1, Ordering::SeqCst); }
        self.plan[e.job.tier()][e.class as usize].remove(&order(e.key, e.serial));
        if e.job.tier() == 0 && !e.promoted { self.ahead_len -= 1; }
        e.since = self.since_of(&e);
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
// Jobs keep their drain while asleep. Leave drains available for Visible work and first compiles.
fn drain_blocked(class: u8, tier: usize, policy: &Policy) -> Option<&'static str> {
    if class != budget::INTERACTIVE && policy.noninteractive >= policy.limit { return Some("first-compile reserve"); }
    if class == budget::BACKGROUND {
        return background_drains(tier, policy.breadth, policy.background, policy.background_limit, policy.o3_pending);
    }
    None
}
fn is_breadth(tier: usize) -> bool { matches!(tier, 1 | 3 | 5) }
fn background_drains(tier: usize, breadth: usize, background: usize, background_limit: usize, o3_pending: bool) -> Option<&'static str> {
    if background >= background_limit { return Some("visible reserve"); }
    if o3_pending && is_breadth(tier) && breadth >= background_limit.div_ceil(2) { return Some("O3 first"); }
    None
}
fn eligible(class: u8, burst: bool, noninteractive: usize) -> bool {
    !(burst && class != budget::INTERACTIVE && noninteractive != 0)
}
/// What a job's build is to the program while it runs (`budget::set_thread_work`, which places it
/// on the cores: on Apple silicon a tier-up takes a performance core the person's work leaves,
/// work behind runs on the efficiency cores while the person's work runs).
/// - Tier-up: the program runs slower code until it lands. An O3 build queued by its calls, its
///   loops or its counted window, an O2 build of a slot with heat, a reach build (code the program
///   reached runs baseline or not at all).
/// - Behind: it replaces code with code of the same tier and body, or nothing calls it yet. A
///   retier (O3 rebuilt for a verdict that came), a verified O1 rebuild, a borrow build, a twin's
///   base. (The callees an O3 install builds ahead of it: `prebuild_callees`, inside the O3 job.)
/// - Plain: nobody's measured demand. Speculation, look-ahead, checks, a marked slot's build
///   (`#[relax(O3)]`), a baseline O2 build of a slot without heat, the housekeeping tick.
fn work_kind(job: &Job, now: &HeatNow) -> u8 {
    match job {
        Job::O3(e) if e.1 == super::Q_RETIER => budget::WORK_BEHIND,
        Job::O3(e) if e.1 == super::Q_MARKED => budget::WORK_PLAIN,
        Job::O3(_) => budget::WORK_TIER_UP,
        Job::O2(_, _, O2Kind::Verified | O2Kind::Borrow | O2Kind::TwinBase) => budget::WORK_BEHIND,
        Job::O2(u, id, O2Kind::Normal | O2Kind::Base) if now.heat(*u, *id).0 > 0 => budget::WORK_TIER_UP,
        Job::Reach(..) => budget::WORK_TIER_UP,
        _ => budget::WORK_PLAIN,
    }
}

/// A job's class while nobody waits on it (a waited-for job is promoted: Interactive). The checks
/// ahead of the main thread's lazy compiles serve the startup and the UI's bursts; a tier-up (O2,
/// O3) or a speculative build only speeds code up, so it runs in the background class, below the
/// app's visible work, however hot its slot (its heat orders it among the background builds).
/// A check is Visible during a UI app's boot check, Background otherwise, and Interactive once waited for.
fn job_class(job: &Job, _now: &HeatNow) -> u8 {
    match job {
        Job::Ahead(..) if startup_phase() || ui_burst() => budget::INTERACTIVE,
        Job::Check(..) if crate::check::check_waited() => budget::INTERACTIVE,
        Job::Check(..) if crate::check::check_early() => budget::VISIBLE,
        Job::Reach(_, _, true) => budget::INTERACTIVE,
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
/// A reach build no ticket waits for (a late crossing) goes before every build of its class
/// (tier-ups, speculation): real-time code may call what it builds; the frontier grows only as
/// they install, which bounds how many are queued. Behind a waited check.
const REACH_KEY: i64 = i64::MAX - 2;
/// One a ticket waits for (Interactive) goes behind everything there a program thread waits for
/// or will (a slot made, a body's inline candidates: their age and heat keep them far above it) and before
/// the startup's look-ahead (-LOOK_BEHIND). Through the startup's UI burst a background build ran
/// one at a time: a cold launch's mark took 590-700 ms on workers against 280 ms on the owner.
const REACH_WAITED_KEY: i64 = -(1 << 39);
fn key(e: &Entry, now: &HeatNow) -> i64 {
    if matches!(e.job, Job::Tick) { return i64::MAX; }
    // (a check beside the program goes behind every build of its class, until it is waited for)
    if matches!(e.job, Job::Check(..)) { return if crate::check::check_waited() { CHECK_WAITED_KEY } else { i64::MIN }; }
    if let Job::Reach(_, _, waited) = e.job { return if waited { REACH_WAITED_KEY } else { REACH_KEY }; }
    let (heat, ui) = match &e.job {
        Job::O2(u, id, _) => now.heat(*u, *id),
        Job::O3(t) => now.tier_heat(t),
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
const TIER_NAMES: [&str; TIERS] = ["ahead", "O2", "O3", "spec", "tick", "check"];
/// What the jobs of each tier took so far, the CPU of the threads running them (makepad inspect `jit spent`).
static JOB_CPU: [AtomicU64; TIERS] = [const { AtomicU64::new(0) }; TIERS];
static JOB_RUNS: [AtomicU64; TIERS] = [const { AtomicU64::new(0) }; TIERS];
/// The hold ledger's reasons: what holds a runnable tier-up back while a worker is free. The gates'
/// own (`class_blocked`, `entry_blocked`), then "idle lane" (only a worker of the other kind may
/// start it: idle_tiers) and "other" (a gate this list does not name yet).
const HOLD_REASONS: [&str; 12] = ["UI burst", "first-compile reserve", "visible reserve", "startup tier width", "demand compiles", "O3 stopped", "startup", "same slot running", "O3 first", "grant", "idle lane", "other"];
/// Per reason, before the first present ([0]) and after it ([1]): the wall ns a free worker found the
/// best queued tier-up held for it, and those ns times the tier-ups queued then (build-ns waiting).
static HELD_NS: [[AtomicU64; HOLD_REASONS.len()]; 2] = [const { [const { AtomicU64::new(0) }; HOLD_REASONS.len()] }; 2];
static HELD_QUEUED_NS: [[AtomicU64; HOLD_REASONS.len()]; 2] = [const { [const { AtomicU64::new(0) }; HOLD_REASONS.len()] }; 2];
impl Hold {
    fn close(&self, now: u64) {
        let ns = now.saturating_sub(self.since);
        HELD_NS[self.after as usize][self.reason].fetch_add(ns, Ordering::Relaxed);
        HELD_QUEUED_NS[self.after as usize][self.reason].fetch_add(ns.saturating_mul(self.queued), Ordering::Relaxed);
    }
}
/// The hold ledger (makepad inspect `jit`): per reason the seconds a free worker found the best queued
/// tier-up held, and the build-seconds the queued tier-ups waited meanwhile, before the first present
/// and after it, the open interval counted to now.
pub(super) fn held_line() -> String {
    let open = { let q = lock(); q.hold.as_ref().map(|h| (h.reason, h.after, wall_ns().saturating_sub(h.since), h.queued)) };
    let phase = |after: bool| {
        let rows: Vec<String> = HOLD_REASONS.iter().enumerate().filter_map(|(i, name)| {
            let (mut ns, mut queued) = (HELD_NS[after as usize][i].load(Ordering::Relaxed), HELD_QUEUED_NS[after as usize][i].load(Ordering::Relaxed));
            if let Some((reason, a, extra, n)) = open { if reason == i && a == after { ns += extra; queued += extra.saturating_mul(n); } }
            (ns > 0).then(|| format!("{name} {:.2} s ({:.1} build-s)", ns as f64 / 1e9, queued as f64 / 1e9))
        }).collect();
        if rows.is_empty() { "none".to_string() } else { rows.join(", ") }
    };
    format!("tier-ups held while a worker was free: before the first present {}; after {}", phase(false), phase(true))
}
/// The same per slot for its O2 and O3 jobs: O2 ns, O2 jobs, O3 ns, O3 jobs, then its O3 jobs that
/// made a counted build (PGO) instead: ns, jobs.
static SLOT_CPU: relax_session::sync::Mutex<Option<HashMap<u32, [u64; 6]>>> = relax_session::sync::Mutex::new(None);
/// The O3 jobs that made a counted build instead (PGO's counted code, counted_build): ns, jobs.
static COUNTED_CPU: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];
thread_local! {
    /// The job this thread runs made a counted build (`job_counted`).
    static JOB_COUNTED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
/// The running job made a counted build: `jit spent` keeps its CPU apart from the O3 builds'.
pub(super) fn job_counted() {
    let _ = JOB_COUNTED.try_with(|c| c.set(true));
}
/// A worker's park state, on its own cache line (each worker writes its flag at every job).
#[repr(align(64))]
#[derive(Default)]
struct Sleeper {
    thread: OnceLock<budget::Wake>, waiting: AtomicBool, kind: std::sync::atomic::AtomicU8,
    #[cfg(test)]
    parked: AtomicBool,
    os_id: AtomicU64, jobs: [AtomicU64; 3], present_nice: OnceLock<Option<i32>>,
    id: OnceLock<std::thread::ThreadId>,
}
impl Sleeper {
    fn kind(&self) -> Kind {
        match self.kind.load(Ordering::Relaxed) { 0 => Kind::Back, 1 => Kind::Idle, _ => Kind::Fore }
    }
    fn nice(&self) -> Option<i32> {
        #[cfg(target_os = "linux")]
        if let tid @ 1.. = self.os_id.load(Ordering::Relaxed) {
            extern "C" { fn getpriority(which: i32, who: u32) -> i32; }
            let _errno = os::proc::KeepLastError::new();
            os::proc::set_last_error(0);
            let nice = unsafe { getpriority(0, tid as u32) };
            return (nice != -1 || os::proc::last_error() == 0).then_some(nice);
        }
        None
    }
}
pub(super) fn mark_present() {
    budget::mark_class_refusals();
    if crate::run::VERBOSE.load(Ordering::Relaxed) {
        for w in THREADS.get().into_iter().flatten() { let _ = w.present_nice.set(w.nice()); }
    }
}
impl super::Stats {
    pub(crate) fn drains_line() -> String {
        let threads = THREADS.get().map_or(&[][..], Vec::as_slice);
        let show = |n: Option<i32>| n.map_or_else(|| "-".into(), |n| n.to_string());
        let rows: Vec<_> = threads.iter().take(DRAINS.count.load(Ordering::Relaxed)).enumerate().map(|(i, w)| {
            let [a, v, b] = w.jobs.each_ref().map(|n| n.load(Ordering::Relaxed));
            format!("#{i} nice {} {a}/{v}/{b} jobs (first present {})", show(w.nice()), show(w.present_nice.get().copied().flatten()))
        }).collect();
        let [before, after] = budget::class_refusals();
        // each thread that met a refusal, once: a drain by its index, any other thread by its name
        let refused: Vec<_> = budget::refused_threads().into_iter().map(|r| {
            let who = threads.iter().position(|w| w.id.get() == Some(&r.id)).map_or(r.name, |i| format!("drain #{i}"));
            format!("{who} (class {} {} first present)", r.class, if r.after_mark { "after" } else { "before" })
        }).collect();
        format!("relax: {} drains, {} slots (I/V/B): {}; {} bounces; OS class refusals before first present {before:?}, after {after:?}; refused on: {}",
            DRAINS.count.load(Ordering::Relaxed), threads.len(), rows.join(", "), BOUNCES.load(Ordering::Relaxed), if refused.is_empty() { "none".to_string() } else { refused.join(", ") })
    }
}
/// While a real-time thread is live (jit::rt_live; Linux and Android, whose kernels have an idle
/// policy) the tier-ups run on workers at the idle policy, below every other thread of the machine:
/// a real-time thread sharing a core with one gets it back at once (6 copies of the rt_lock case on
/// a 16-thread box: its longest call 17 ms at the Background class, 4 ms with these). Nothing waits
/// for a tier-up (an O2 build marks no slot in flight: a program thread needing the slot builds it
/// itself; O3 replaces code that runs), so no thread that matters waits behind one; the builds a
/// lazy compile may wait for -- speculative and checked-ahead ones, checks -- stay at their class.
/// A worker at the idle policy takes tier-ups only, at most `background_limit` workers go there,
/// and none comes back (a thread without CAP_SYS_NICE cannot leave the idle policy; the pool ends
/// such a thread rather than reuse it).
fn idle_tiers() -> bool {
    cfg!(any(target_os = "linux", target_os = "android")) && super::rt_live()
}
fn is_tier_up(tier: usize) -> bool {
    tier == 1 || tier == 2
}
/// The calling thread may wait for a helper's check or lowering of a body it needs: not while
/// tier-ups run at the idle policy (idle_tiers), whose prefronts check and lower bodies too, unless
/// it runs there itself: the wait would last as long as every other thread keeps that helper's core.
pub(super) fn waits_on_helpers() -> bool {
    !idle_tiers() || budget::thread_idle()
}
/// The workers of this run at the idle policy (idle_tiers).
static IDLE_WORKERS: AtomicUsize = AtomicUsize::new(0);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub(super) enum Kind { Back, Idle, Fore }
/// What a worker may start at this pick.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Takes { kind: Kind, split: bool, back_tiers: bool }
impl Takes {
    const ALL: Self = Self { kind: Kind::Back, split: false, back_tiers: true };
    fn takes(self, tier: usize, class: u8) -> bool {
        match self.kind {
            Kind::Fore => class != budget::BACKGROUND,
            Kind::Back => (!self.split || class == budget::BACKGROUND) && (self.back_tiers || !is_tier_up(tier)),
            Kind::Idle => is_tier_up(tier),
        }
    }
}
#[derive(Clone, Copy)]
struct DrainsNow { cores: usize, started: usize, idle: usize, idle_limit: usize }
impl DrainsNow {
    fn get() -> Self {
        Self { cores: budget::cores() as usize, started: NEXT_WORKER.load(Ordering::Relaxed),
            idle: IDLE_WORKERS.load(Ordering::Relaxed), idle_limit: if idle_tiers() { budget::background_limit() as usize } else { 0 } }
    }
    fn takes(self, kind: Kind) -> Takes {
        Takes { kind, split: self.started == 2 * self.cores, back_tiers: self.idle_limit == 0 || self.idle < self.idle_limit }
    }
    fn for_job(self, tier: usize, class: u8) -> Kind {
        if class != budget::BACKGROUND && self.started > self.cores {
            // On a short start Back may help if every Fore worker is busy.
            if self.started == 2 * self.cores || THREADS.get().is_some_and(|w| w[self.cores..].iter().any(|s| s.waiting.load(Ordering::SeqCst))) {
                return Kind::Fore;
            }
        }
        if is_tier_up(tier) && self.idle > 0 && self.idle_limit > 0 && !self.go_idle(Kind::Back, tier) { Kind::Idle } else { Kind::Back }
    }
    fn go_idle(self, kind: Kind, tier: usize) -> bool {
        kind == Kind::Back && is_tier_up(tier) && self.idle < self.idle_limit
            && self.idle + 1 < self.started.min(self.cores)
    }
}
static BOUNCES: AtomicUsize = AtomicUsize::new(0);
static FLOOR_REFUSED: AtomicBool = AtomicBool::new(false);
static THREADS: OnceLock<Vec<Sleeper>> = OnceLock::new();
static NEXT_WORKER: AtomicUsize = AtomicUsize::new(0);
/// The workers leave at their next pick (`stop_workers`).
static WORKERS_STOP: AtomicBool = AtomicBool::new(false);
/// The workers' drains (`start`), and the drains that came back.
static DRAINS: Drains = Drains::new();
static DRAINS_BACK: AtomicUsize = AtomicUsize::new(0);
/// The drains the first start asks the pool for. One the pool cannot start (no idle
/// worker, and the OS refused a thread: a limit on the process's memory counts each worker's stack)
/// leaves the workers the start has, which take every job; with none the start fails, and the
/// next one asks again. A failed start leaves nothing a later one trips over: a Once its refused
/// thread poisoned once failed every later push ("Once instance has previously been poisoned",
/// on whichever thread pushed next).
struct Drains {
    /// the drains are started (the fast path of every push)
    started: AtomicBool,
    /// held by the start that starts them
    starting: relax_session::sync::Mutex<()>,
    /// drains started: `stop_workers` waits for as many to come back
    count: AtomicUsize,
}
#[derive(Debug, PartialEq)]
enum Started { Already, All, Short(usize, String) }
impl Drains {
    const fn new() -> Drains {
        Drains { started: AtomicBool::new(false), starting: relax_session::sync::Mutex::new(()), count: AtomicUsize::new(0) }
    }
    /// `init` once, then `drain` until `want` drains run. Err: not one started (the OS's answer).
    fn start(&self, want: usize, init: impl FnOnce(), mut drain: impl FnMut() -> std::io::Result<()>) -> std::io::Result<Started> {
        if self.started.load(Ordering::Acquire) { return Ok(Started::Already); }
        let _starting = self.starting.lock().unwrap_or_else(|e| e.into_inner());
        if self.started.load(Ordering::Acquire) { return Ok(Started::Already); }
        init();
        while self.count.load(Ordering::SeqCst) < want {
            // counted before it starts: stop_workers never sees a running drain uncounted
            self.count.fetch_add(1, Ordering::SeqCst);
            if let Err(e) = drain() {
                let have = self.count.fetch_sub(1, Ordering::SeqCst) - 1;
                if have == 0 { return Err(e); }
                self.started.store(true, Ordering::Release);
                return Ok(Started::Short(have, e.to_string()));
            }
        }
        self.started.store(true, Ordering::Release);
        Ok(Started::All)
    }
}
struct CompilerSource;
static SOURCE: CompilerSource = CompilerSource;
pub(super) fn queue() -> &'static relax_session::sync::Mutex<Queue> {
    QUEUE.get_or_init(|| crate::zone_alloc::kept_with(|| {
        let q = Queue::new();
        if q.grant.is_some() { crate::pool::on_job_sleep(granted_job_sleeps); }
        relax_session::sync::Mutex::new(q)
    }))
}
/// A granted run's job fell asleep on other JIT work: its place in the width is free, so a parked
/// drain that takes waited work wakes for what is queued. Lock-free: the sleeper may hold the lock
/// it waits under.
fn granted_job_sleeps() {
    if COUNTS.iter().any(|c| c.load(Ordering::Relaxed) != 0) {
        wake_kind(DrainsNow::get().for_job(0, budget::INTERACTIVE));
    }
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
    if FORKED.load(Ordering::Relaxed) || DRAINS.started.load(Ordering::Acquire) { return; }
    let _kept = crate::zone_alloc::kept();
    let cores = budget::cores() as usize;
    let want = if cfg!(any(target_os = "linux", target_os = "android")) { 2 * cores } else { cores };
    let init = || {
        crate::pool::thread_start(ensure_altstack);
        THREADS.get_or_init(|| (0..want).map(|i| Sleeper {
            kind: (if i < cores { Kind::Back } else { Kind::Fore } as u8).into(), ..Sleeper::default()
        }).collect());
    };
    match DRAINS.start(want, init, || crate::pool::drain(&SOURCE)) {
        Ok(Started::Already | Started::All) => {}
        Ok(Started::Short(have, e)) => eprintln!("relax: the JIT compiles on {have} of its {want} workers: the OS refused another thread ({e}); Back workers take every class"),
        Err(e) => panic!("failed to start JIT pool worker: {e}"),
    }
}
/// A lifecycle test measures retained process heap after its initial setup.
/// A pool thread allocates runtime state before entering the drain: merely spawning
/// the drains does not establish that those one-off allocations have happened.
#[cfg(all(test, target_os = "linux"))]
pub(super) fn warm_workers_for_test() {
    start();
    let count = DRAINS.count.load(Ordering::Acquire);
    while THREADS.get().unwrap().iter().take(count).any(|worker| worker.thread.get().is_none()) {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}
/// The workers leave at their next pick, a job in hand ending first (what compiles was told to stop
/// by then: `jit::stop_compiling`). Returns once every one has: a job reads the run's unit and the
/// statics the run's end puts back, so the end never goes on past one. While a job runs on, every
/// `report` says which.
pub(super) fn stop_workers(report: std::time::Duration) {
    WORKERS_STOP.store(true, Ordering::SeqCst);
    let t = Instant::now();
    let mut said = std::time::Duration::ZERO;
    while DRAINS_BACK.load(Ordering::SeqCst) < DRAINS.count.load(Ordering::SeqCst) {
        if t.elapsed() >= said + report {
            said = t.elapsed();
            let jobs: Vec<String> = {
                let _kept = crate::zone_alloc::kept();
                lock().running.iter().map(|r| match r.slot {
                    Some((_, slot)) => format!("{} of slot #{slot}", TIER_LABELS[r.tier]),
                    None => TIER_LABELS[r.tier].to_string(),
                }).collect()
            };
            eprintln!("relax: a run's end waits {:.0} s for its compile workers, {} still in a job: {}", said.as_secs_f64(), DRAINS.count.load(Ordering::SeqCst) - DRAINS_BACK.load(Ordering::SeqCst), jobs.join(", "));
        }
        wake_all();
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}
/// The jobs of each tier (`Job::tier`), as `Job::label` names them.
const TIER_LABELS: [&str; TIERS] = ["JIT ahead", "JIT O2", "JIT O3", "JIT spec", "JIT tick", "JIT check"];
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
/// Wake a parked drain that can start the entry.
fn wake_kind(kind: Kind) {
    if let Some(threads) = THREADS.get() {
        let (back, fore) = threads.split_at(budget::cores() as usize);
        for worker in if kind == Kind::Fore { fore } else { back } {
            if worker.kind() == kind && worker.waiting.load(Ordering::SeqCst) && worker.waiting.swap(false, Ordering::SeqCst) {
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
/// The run quits (`quit`): ahead work found from now on is not queued (Queue::quitting decides,
/// under the lock; this spares the inbox and the workers' wake).
static QUITTING: AtomicBool = AtomicBool::new(false);
/// A queued job the quit drops: checks and lowering ahead, O2 and speculative builds (not the
/// real-time reach's: the reach waits for each).
fn quit_drops(job: &Job) -> bool {
    matches!(job.tier(), 0 | 1 | 3) && !matches!(job, Job::Reach(..))
}
/// The run quits (run.rs quit_mark): the compiles queued and not started -- checks and lowering
/// ahead, O2 and speculative builds -- are dropped, unless a thread waits for one: they are
/// speculative, and the next launch asks again. None is queued from now on. The O3 tier-ups were
/// stopped (TIER_STOP) and stay queued, for the exit to record their verdicts
/// (record_queued_at_exit); the checks beside the program and the tick go on. In batches, the
/// queue's lock held for one at a time (a lazy compile of the app's shutdown may queue behind
/// it). Returns how many were dropped.
pub(super) fn quit() -> usize {
    QUITTING.store(true, Ordering::SeqCst);
    if FORKED.load(Ordering::Relaxed) { return 0; }
    let _kept = crate::zone_alloc::kept();
    let serials = lock().quit();
    let mut dropped = 0;
    for batch in serials.chunks(QUIT_DROP_BATCH) {
        // (the entries go after the lock is given back)
        let gone = lock().quit_drop(batch);
        dropped += gone.len();
    }
    dropped
}
/// `quit`'s entries dropped per hold of the queue's lock.
const QUIT_DROP_BATCH: usize = 1024;
#[cfg(unix)]
pub(super) fn fork_child() {
    FORKED.store(true, Ordering::SeqCst);
}
#[cfg(all(unix, test))]
pub(super) fn leave_child_for_test() { FORKED.store(false, Ordering::SeqCst); }
pub(super) fn push(job: Job, at: u64) {
    if FORKED.load(Ordering::Relaxed) { return; }
    let runnable = crate::zone_alloc::kept_with(|| { let mut q = lock(); q.push(job, at).or_else(|| q.runnable()) });
    start();
    if let Some(kind) = runnable { wake_kind(kind); }
}
pub(super) fn o2(unit: usize, id: u32, kind: O2Kind, at: u64) {
    push(Job::O2(unit, id, kind), at);
}
/// Speculative compile of unit `unit`'s slot `id` (a slot id means something only in its own unit).
pub(super) fn spec(unit: usize, id: u32) { push(Job::Spec(unit, id), wall_ns()); }
/// The real-time reach's build of unit `unit`'s slot `id` (jit.rs reach_build).
pub(super) fn reach(unit: usize, id: u32, waited: bool) { push(Job::Reach(unit, id, waited), wall_ns()); }
/// Reach builds queued (Queue::push, Queue::remove): a thread waiting for the reach takes them.
static REACH_QUEUED: AtomicUsize = AtomicUsize::new(0);
pub(super) fn reach_queued() -> bool { REACH_QUEUED.load(Ordering::SeqCst) != 0 }
/// Workers take what is queued here: not a plain fork's child (it has none, and nothing queues).
pub(super) fn takes_jobs() -> bool { !FORKED.load(Ordering::Relaxed) }
/// A queued reach build out of the queue, for a thread whose wait is the reach's to run it itself
/// (jit.rs realtime_before_install: a tier-up build of real-time code waits for the reach on a
/// worker; with every worker waiting so, the reach's builds would stay queued). The oldest one of
/// the most urgent class (a reach build's key is REACH_KEY or REACH_WAITED_KEY).
pub(super) fn take_reach() -> Option<(usize, u32)> {
    if FORKED.load(Ordering::Relaxed) || !reach_queued() { return None; }
    let mut q = lock();
    let serial = q.plan[3].iter().find_map(|bucket| [REACH_KEY, REACH_WAITED_KEY].into_iter().find_map(|key| bucket.range(order(key, 0)..=order(key, u64::MAX))
        .map(|&(_, serial)| serial).find(|serial| matches!(q.entries[q.indices[serial]].job, Job::Reach(..)))))?;
    let i = q.indices[&serial];
    match q.remove(i).job { Job::Reach(unit, id, _) => Some((unit, id)), _ => None }
}
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
/// met it on one push in seven (15k sleeps of the owner through a large app's cold start, the thread the
/// UI waits behind; 130k of the workers').
pub(super) static AHEAD_INBOX: relax_session::sync::Mutex<Vec<(Arc<FrontShared>, AheadWork, u64)>> = relax_session::sync::Mutex::new(Vec::new());
static AHEAD_PENDING: AtomicBool = AtomicBool::new(false);
/// A parked worker is woken while a check ahead can start: through the startup, below their
/// width (STARTUP_AHEAD; a running check takes the next itself when it ends). Ahead work blocked
/// otherwise starts when the startup's phases change (they wake the workers).
fn push_ahead(front: &Arc<FrontShared>, work: impl Iterator<Item = AheadWork>) {
    // (the run quits: nothing is checked or lowered ahead any more, `quit`)
    if FORKED.load(Ordering::Relaxed) || QUITTING.load(Ordering::Relaxed) { return; }
    // A producer can finish discovering work after the settle rebuild retired its front.
    if !front.alive.load(Ordering::Acquire) { return; }
    let at = wall_ns();
    crate::zone_alloc::kept_with(|| {
        let mut inbox = AHEAD_INBOX.lock().unwrap_or_else(|e| e.into_inner());
        inbox.extend(work.map(|w| (front.clone(), w, at)));
        AHEAD_PENDING.store(true, Ordering::Release);
    });
    start();
    let startup = startup_phase();
    if !startup || RUNNING[0].load(Ordering::Relaxed) < startup_ahead() {
        wake_kind(DrainsNow::get().for_job(0, if startup || ui_burst() { budget::INTERACTIVE } else { budget::BACKGROUND }));
    }
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
    if FORKED.load(Ordering::Relaxed) { return Vec::new(); }
    drain_tier_inbox();
    lock().entries.iter().filter_map(|e| if let Job::O3(t) = e.job { keep(&t).then_some(t) } else { None }).collect()
}
/// `counted`: an O3 job that made a counted build instead (job_counted).
fn note_job(tier: usize, slot: Option<u32>, ns: u64, counted: bool) {
    let counted = counted && tier == 2;
    if counted {
        COUNTED_CPU[0].fetch_add(ns, Ordering::Relaxed);
        COUNTED_CPU[1].fetch_add(1, Ordering::Relaxed);
    } else {
        JOB_CPU[tier].fetch_add(ns, Ordering::Relaxed);
        JOB_RUNS[tier].fetch_add(1, Ordering::Relaxed);
    }
    if let (true, Some(slot)) = (is_tier_up(tier), slot) {
        let at = if counted { 4 } else { (tier - 1) * 2 };
        crate::zone_alloc::kept_with(|| {
            let mut g = SLOT_CPU.lock().unwrap_or_else(|e| e.into_inner());
            let row = g.get_or_insert_with(Default::default).entry(slot).or_default();
            row[at] += ns;
            row[at + 1] += 1;
        });
    }
}
/// What the jobs took by tier (the counted builds apart from the O3 builds), then the `n` slots
/// whose O2, O3 and counted jobs took the most.
pub(super) fn spent(n: usize) -> Vec<String> {
    let mut tiers: Vec<String> = TIER_NAMES.iter().enumerate()
        .map(|(i, name)| format!("{name} {:.1} s x{}", JOB_CPU[i].load(Ordering::Relaxed) as f64 / 1e9, JOB_RUNS[i].load(Ordering::Relaxed)))
        .collect();
    tiers.insert(3, format!("counted {:.1} s x{}", COUNTED_CPU[0].load(Ordering::Relaxed) as f64 / 1e9, COUNTED_CPU[1].load(Ordering::Relaxed)));
    let mut rows = vec![tiers.join(", ")];
    let g = SLOT_CPU.lock().unwrap_or_else(|e| e.into_inner());
    let mut slots: Vec<(&u32, &[u64; 6])> = g.iter().flatten().collect();
    slots.sort_by_key(|(id, r)| (std::cmp::Reverse(r[0] + r[2] + r[4]), **id));
    rows.push(format!("{} slots had an O2, O3 or counted job", slots.len()));
    rows.extend(slots.into_iter().take(n).map(|(id, r)| {
        let name = slot_name(*id).unwrap_or_default();
        format!("S{id} O2 {:.1} ms x{} O3 {:.1} ms x{} counted {:.1} ms x{} {name}", r[0] as f64 / 1e6, r[1], r[2] as f64 / 1e6, r[3], r[4] as f64 / 1e6, r[5])
    }));
    rows
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
    /// The run quits (`quit`): nothing it drops is queued from now on; the queued entries it drops.
    fn quit(&mut self) -> Vec<u64> {
        self.quitting = true;
        self.entries.iter().filter(|e| quit_drops(&e.job) && !e.promoted).map(|e| e.serial).collect()
    }
    /// `serials`' entries out of the queue, but one a worker took since or a thread came to wait for.
    fn quit_drop(&mut self, serials: &[u64]) -> Vec<Entry> {
        let mut gone = Vec::with_capacity(serials.len());
        for serial in serials {
            let Some(&i) = self.indices.get(serial) else { continue };
            if !self.entries[i].promoted { gone.push(self.remove(i)); }
        }
        gone
    }
    /// Promotes the slot's jobs and identifies a drain that can start one now.
    fn promote(&mut self, unit: usize, id: u32) -> Option<Kind> {
        self.sync_ahead();
        let wanted = self.wanted((unit, id))?;
        let serials = self.by_slot.remove(&(unit, id)).unwrap_or_default();
        let now = wall_ns();
        for serial in serials.iter() {
            let i = self.indices[&serial];
            let old_since = self.since_of(&self.entries[i]);
            let e = &mut self.entries[i];
            self.plan[e.job.tier()][e.class as usize].remove(&order(e.key, e.serial));
            let old = if e.job.tier() == 0 && !e.promoted { self.ahead_len -= 1; self.ahead_class } else { e.class };
            e.promoted = true;
            e.class = wanted.min(old);
            e.since = if e.class < old { now } else { old_since };
            self.plan[e.job.tier()][e.class as usize].insert(order(e.key, e.serial));
        }
        for r in &self.running {
            if r.waits_for(unit, id) && wanted < r.class.class() { r.class.reclass(wanted); }
        }
        let policy = self.policy();
        let runnable = serials.iter().find_map(|serial| {
            let e = &self.entries[self.indices[&serial]];
            self.blocked(e, &policy).is_none().then(|| DrainsNow::get().for_job(e.job.tier(), self.class_of(e)))
        });
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
            e.promoted = false;
            let old = e.class;
            e.class = if tier == 0 { self.ahead_len += 1; budget::BACKGROUND } else { job_class(&e.job, &now) };
            // (back in the background it has waited there since it was queued; a check ahead
            // takes the ahead class's raise again: since_of)
            e.since = if e.class < old { now.wall } else if e.class == budget::BACKGROUND { e.at } else { e.since };
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
    if let Some(kind) = runnable { wake_kind(kind); }
    WaitPriority { unit, id, class }
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
        q.entries[i].class = budget::INTERACTIVE;
        q.entries[i].key = CHECK_WAITED_KEY;
        q.plan[5][budget::INTERACTIVE as usize].insert(order(CHECK_WAITED_KEY, serial));
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
        // the thread goes back to the shared pool, whose Background jobs must lower it again
        budget::set_thread_floor(false);
        DRAINS_BACK.fetch_add(1, Ordering::SeqCst);
    }
}
fn worker_loop(index: usize) {
        let worker = &THREADS.get().unwrap()[index];
        if worker.kind() == Kind::Fore {
            budget::set_thread_floor(true);
            if !budget::set_thread_class(budget::INTERACTIVE) && !FLOOR_REFUSED.swap(true, Ordering::Relaxed) {
                eprintln!("relax: the OS refused Interactive priority on a Fore worker; foreground builds run at inherited priority");
            }
        }
        worker.os_id.store(crate::inspect::threads::self_os_id(), Ordering::Relaxed);
        let _ = worker.id.set(std::thread::current().id());
        worker.thread.set(budget::Wake::current()).ok();
        // the job this worker ran last, (serial, tier): it leaves `running` under the lock that
        // takes the next one (one queue lock per job)
        let mut finished: Option<(u64, usize)> = None;
        let me = std::thread::current().id();
        // (a worker restarted after a panic keeps its thread, and so its policy)
        if budget::thread_idle() && worker.kind() == Kind::Back {
            worker.kind.store(Kind::Idle as u8, Ordering::Relaxed);
            IDLE_WORKERS.fetch_add(1, Ordering::Relaxed);
        }
        loop {
            if WORKERS_STOP.load(Ordering::Relaxed) {
                if let Some((serial, tier)) = finished.take() {
                    let _kept = crate::zone_alloc::kept();
                    lock().running.retain(|r| r.serial != serial);
                    RUNNING[tier].fetch_sub(1, Ordering::Relaxed);
                }
                return;
            }
            worker.waiting.store(true, Ordering::SeqCst);
            // Atomics-only producers publish to the existing tier-record inbox.
            drain_tier_inbox();
            prioritize_hot_verdicts();
            // one queue lock per pick, idle or not: the tick's timer is taken and given back
            // under it too
            let next = {
                let _kept = crate::zone_alloc::kept();
                let mut q = lock();
                // (a job asleep on other JIT work uses no CPU: the drains' area counts awake ones)
                let awake = if q.grant.is_some() { q.running.iter().filter(|r| !r.class.asleep()).count() } else { 0 };
                if let Some(grant) = &mut q.grant {
                    grant.roll(wall_ns(), awake, crate::first_frame::process_cpu_ns);
                }
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
                let drains = DrainsNow::get();
                let kind = worker.kind();
                let takes = drains.takes(kind);
                let head = q.head(&now, takes);
                // what only the other kind of worker may start: wake one of those
                // (never this worker's own kind: one of those would find what it found)
                let other = if head.is_none() && takes != Takes::ALL { q.runnable().filter(|&k| k != kind) } else { None };
                let picked = head.map(|i| {
                    // one entry per pick: the next free worker takes the next one (a batch held
                    // the frontier the main thread reached next behind the rest of its batch)
                    let e = q.remove(i);
                    let job = CurrentJob::new(e.class, e.job.label());
                    RUNNING[e.job.tier()].fetch_add(1, Ordering::Relaxed);
                    // a tier-up while a real-time thread is live: this worker goes to the idle
                    // policy for good (its place in the count reserved under the lock)
                    let go_idle = drains.go_idle(kind, e.job.tier());
                    if go_idle { IDLE_WORKERS.fetch_add(1, Ordering::Relaxed); }
                    q.running.push(Running { serial: e.serial, promoted: e.promoted, slot: e.slot, tier: e.job.tier(), base_class: job_class(&e.job, &now), class: job.control() });
                    // another parked worker only for work this one leaves runnable: a wake per job
                    // that finds the rest blocked (the startup's ahead width) costs the owner its
                    // queue lock and the cores their caches
                    let more = q.runnable();
                    let work = work_kind(&e.job, &now);
                    (e, job, more, go_idle, work)
                });
                // the hold ledger: a tier-up that starts ends the open interval; a worker that parks
                // finds why the best queued one does not start (other work started leaves it open)
                match &picked {
                    Some((e, ..)) if is_tier_up(e.job.tier()) => if q.hold.is_some() { q.note_hold(None, wall_ns()) },
                    Some(_) => {}
                    None => {
                        let hold = q.tier_up_hold(takes);
                        q.note_hold(hold, wall_ns());
                    }
                }
                // Park until a wake or, for the timer worker, until the tick is due.
                picked.ok_or_else(|| ((!q.tick_queued && q.timer.is_none()).then(|| {
                    q.timer = Some(me);
                    q.tick_at.saturating_duration_since(instant)
                }), other))
            };
            match next {
                Ok((entry, job, more, go_idle, work)) => {
                    worker.jobs[entry.class as usize].fetch_add(1, Ordering::Relaxed);
                    worker.waiting.store(false, Ordering::SeqCst);
                    crate::lifeline::beat();
                    if go_idle {
                        budget::set_thread_idle();
                        if budget::thread_idle() { worker.kind.store(Kind::Idle as u8, Ordering::Relaxed); }
                        else { IDLE_WORKERS.fetch_sub(1, Ordering::Relaxed); }
                    }
                    if let Some(kind) = more { wake_kind(kind); }
                    let (serial, tier) = (entry.serial, entry.job.tier());
                    if matches!(entry.job, Job::Check(..)) && crate::check::check_waited() {
                        crate::check::slice_queued(entry.since);
                    }
                    let active = if let Job::O3(tier) = &entry.job {
                        TIER_ACTIVE.lock().unwrap_or_else(|e| e.into_inner()).push(tier.0);
                        Some(TierActive(tier.0))
                    } else { None };
                    let (slot, cpu) = (entry.slot.map(|s| s.1), cpu_ns());
                    budget::set_thread_work(work);
                    let _ = JOB_COUNTED.try_with(|c| c.set(false));
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_job(entry.job)));
                    budget::set_thread_work(budget::WORK_PLAIN);
                    let counted = JOB_COUNTED.try_with(|c| c.replace(false)).unwrap_or(false);
                    note_job(tier, slot, cpu_ns().saturating_sub(cpu), counted);
                    drop(active);
                    if let Err(payload) = result {
                        let (message, bail) = panic_payload(&*payload);
                        if !bail { eprintln!("relax: JIT job panicked: {message}"); }
                    }
                    drop(job);
                    finished = Some((serial, tier));
                    // (this worker takes the next job itself: no wake; it keeps its arena chunks for
                    // it and hands them back when it goes idle)
                }
                Err((timeout, other)) => {
                    if let Some(kind) = other {
                        BOUNCES.fetch_add(1, Ordering::Relaxed);
                        wake_kind(kind);
                    }
                    crate::zone_alloc::compiler_hand_back_idle();
                    if worker.waiting.load(Ordering::SeqCst) {
                        #[cfg(test)]
                        worker.parked.store(true, Ordering::SeqCst);
                        if let Some(timeout) = timeout { budget::park_for(timeout.as_nanos().min(u64::MAX as u128) as u64); }
                        else { budget::park(); }
                        #[cfg(test)]
                        worker.parked.store(false, Ordering::SeqCst);
                    }
                    worker.waiting.store(false, Ordering::SeqCst);
                }
            }
        }
}

/// The JIT pool's line of `inspect cpu`: its drains and the section workers beside them on the
/// same thread cache, its jobs running per class (Interactive / Visible / Background, the tier-ups
/// among them), queued per class. One hold of the queue's lock, a few counts (a bucket's length).
pub(super) fn pool_line() -> String {
    let _kept = crate::zone_alloc::kept();
    let drains = DRAINS.count.load(Ordering::Relaxed).saturating_sub(DRAINS_BACK.load(Ordering::Relaxed));
    let sections = crate::pool::stats().1.saturating_sub(drains);
    let (running, tier_ups, queued, outside) = {
        let q = lock();
        let (mut running, mut tier_ups, mut queued) = ([0usize; 3], 0, [0usize; 3]);
        for r in &q.running {
            running[r.class.class() as usize] += 1;
            tier_ups += usize::from(is_tier_up(r.tier));
        }
        for (tier, plan) in q.plan.iter().enumerate() {
            for (stored, bucket) in plan.iter().enumerate() {
                queued[q.bucket_class(tier, stored as u8) as usize] += bucket.len();
            }
        }
        (running, tier_ups, queued, q.grant.as_ref().map(|g| g.outside))
    };
    let grant = outside.map(|outside| format!(", granted: {outside} cores in use outside the JIT")).unwrap_or_default();
    format!("{drains} drains, {sections} section workers  running {} / {} / {} ({tier_ups} tier-ups)  queued {} / {} / {}  settled: {}{grant}",
        running[0], running[1], running[2], queued[0], queued[1], queued[2], if super::jit_settled() { "yes" } else { "no" })
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
            Job::O3(t) => now.tier_heat(t),
            _ => (0, false),
        };
        let why = match &e.job {
            Job::O3(t) => Q_WHY[t.1 as usize],
            Job::O2(_, _, kind) => match kind { O2Kind::Normal => "optimize", O2Kind::Base => "baseline", O2Kind::TwinBase => "twin's base", O2Kind::Borrow => "borrow", O2Kind::Verified => "verified O1" },
            Job::Ahead(_, AheadWork::Slot(_)) => "prepare", Job::Ahead(_, AheadWork::Body(..)) => "inline candidate", Job::Ahead(_, AheadWork::Look(..)) => "look ahead", Job::Spec(..) => "speculate", Job::Reach(..) => "real-time reach", Job::Tick => "housekeeping", Job::Check(..) => "dependency check",
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
    fn a_granted_run_caps_its_awake_jobs_and_finishes_past_its_width() {
        assert_eq!(outside_reading(1_000_000_000, 400_000_000, 100_000_000), 6000, "drain time leaves six cores of outside work");
        assert_eq!(outside_reading(100_000_000, 400_000_000, 100_000_000), 0, "drain time above process CPU time leaves zero outside work");

        let mut grant = Grant::new(0, 0);
        grant.observe(3000);
        assert_eq!(grant.outside, 3, "the first span counts three cores");
        grant.observe(4000);
        assert_eq!(grant.outside, 4, "rising load counts in the current span");
        grant.observe(0);
        assert_eq!(grant.outside, 4, "the previous busy span still counts");
        grant.observe(0);
        assert_eq!(grant.outside, 0, "two idle spans release the grant");

        let mut grant = Grant::new(0, 0);
        grant.roll(50_000_000, 2, || panic!("a partial span must not read the CPU clock"));
        grant.roll(100_000_000, 2, || 500_000_000);
        assert_eq!(grant.outside, 3, "two drains across both half-spans leave three cores outside");

        // the width counts every job but the tick that is awake, waited ones too, and one job
        // always runs: with every granted core in use outside, one starts
        let mut q = Queue::new();
        let cores = budget::cores() as usize;
        grant.outside = cores;
        q.grant = Some(grant);
        q.push(Job::Spec(1, 40), 1);
        q.push(Job::Spec(1, 41), 2);
        q.waiters.insert((1, 41), [1, 0]);
        q.promote(1, 41);
        let now = HeatNow { hu: None, wall: 100, tier: 0 };
        let start = |q: &mut Queue, i: usize, label: &'static str| {
            let e = q.remove(i);
            // (made on a thread of its own, which CurrentJob gives the job's OS class; the guard
            // goes at once, the job's control lives on in its running row)
            let job_class_now = e.class;
            let class = std::thread::scope(|s| s.spawn(|| crate::pool::CurrentJob::new(job_class_now, label).control()).join().unwrap());
            q.running.push(Running { serial: e.serial, promoted: e.promoted, slot: e.slot, tier: e.job.tier(), base_class: job_class(&e.job, &now), class });
            e.job.slot()
        };
        let first = q.head(&now, Takes::ALL).expect("one job runs with no spare cores");
        assert_eq!(start(&mut q, first, "waited compile"), Some((1, 41)), "the waited compile goes first");
        assert!(q.head(&now, Takes::ALL).is_none(), "a second job waits while one runs awake at the grant's floor");
        let policy = q.policy();
        assert_eq!(q.blocked(&q.entries[0], &policy), Some("grant"), "the grant accounts for the hold");
        assert_eq!(q.class_blocked(budget::VISIBLE, 3, &policy), Some("grant"), "waited work counts against the grant too");
        assert_eq!(q.class_blocked(budget::BACKGROUND, 4, &policy), None, "the tick runs with no spare cores");
        // the running job falls asleep on other JIT work (pool::suspend, on its thread): its place
        // is free, so what it waits for starts, and when it wakes it runs on
        let sleeper = q.running[0].class.clone();
        let asleep = { let _on = crate::pool::enter_job(Some(sleeper.clone())); crate::pool::suspend() };
        assert!(sleeper.asleep(), "suspend marks the job asleep");
        let next = q.head(&now, Takes::ALL).expect("a job asleep on other JIT work leaves its place");
        assert_eq!(start(&mut q, next, "the build it waits for"), Some((1, 40)), "what it waits for starts");
        drop(asleep);
        assert!(!sleeper.asleep(), "the job wakes when its guard drops");
        q.running.clear();

        // more waited jobs than the width all run, never more of them awake at once than the width
        // (waited: only the grant holds them, whatever the process's other queues hold)
        let width = 2;
        q.grant.as_mut().unwrap().outside = cores - width;
        for id in 50..56 {
            q.push(Job::Spec(1, id), id as u64);
            q.waiters.insert((1, id), [1, 0]);
            q.promote(1, id);
        }
        let (mut started, mut widest) = (0, 0);
        loop {
            while let Some(i) = q.head(&now, Takes::ALL) {
                start(&mut q, i, "build");
                started += 1;
            }
            widest = widest.max(q.running.len());
            if q.running.is_empty() { break; }
            q.running.remove(0);
        }
        assert_eq!(started, 6, "every job past the width runs");
        assert_eq!(widest, width, "the run never had more awake jobs than its grant leaves");
        assert!(q.entries.is_empty(), "nothing is left queued");
    }

    #[test]
    fn a_jobs_cpu_goes_to_its_tier_and_slot() {
        // (the counts are process-wide and other tests add to theirs: it checks what its own jobs
        // added, so it holds beside them and when a warm test process runs it again)
        let slot = u32::MAX - 11;
        let of_slot = || SLOT_CPU.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|m| m.get(&slot).copied()).unwrap_or_default();
        let tiers = || [1, 2].map(|t| (JOB_CPU[t].load(Ordering::Relaxed), JOB_RUNS[t].load(Ordering::Relaxed)));
        let counted = || (COUNTED_CPU[0].load(Ordering::Relaxed), COUNTED_CPU[1].load(Ordering::Relaxed));
        let (slot0, tiers0, counted0) = (of_slot(), tiers(), counted());
        note_job(2, Some(slot), 5_000_000, false);
        note_job(2, Some(slot), 5_000_000, false);
        note_job(1, Some(slot), 1_000_000, false);
        // an O3 job that made a counted build instead: counted, not O3
        note_job(2, Some(slot), 3_000_000, true);
        let (slot1, tiers1, counted1) = (of_slot(), tiers(), counted());
        assert_eq!([0, 1, 2, 3, 4, 5].map(|i| slot1[i] - slot0[i]), [1_000_000, 1, 10_000_000, 2, 3_000_000, 1], "the slot's O2, O3 and counted time and runs");
        for (t, (ns, runs)) in [(0, (1_000_000, 1)), (1, (10_000_000, 2))] {
            assert!(tiers1[t].0 - tiers0[t].0 >= ns && tiers1[t].1 - tiers0[t].1 >= runs, "tier {} totals hold the jobs", t + 1);
        }
        assert!(counted1.0 - counted0.0 >= 3_000_000 && counted1.1 - counted0.1 >= 1, "the counted total holds the counted job");
        let rows = spent(usize::MAX);
        assert!(rows[0].contains("O3 ") && rows[0].contains("counted "), "by tier first, counted builds apart: {}", rows[0]);
        let row = format!("S{slot} O2 {:.1} ms x{} O3 {:.1} ms x{} counted {:.1} ms x{}", slot1[0] as f64 / 1e6, slot1[1], slot1[2] as f64 / 1e6, slot1[3], slot1[4] as f64 / 1e6, slot1[5]);
        assert!(rows.iter().any(|r| r.starts_with(&row)), "{row} in {rows:?}");
    }

    #[test]
    fn admission_order_and_waiter_promotion() {
        isolated_test!(admission_order_and_waiter_promotion);
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
        let mut q = Queue::new();
        let mut policy = Policy { noninteractive: 1, background: 1, breadth: 1, limit: 2, background_limit: 1, ..q.policy() };
        assert_eq!(drain_blocked(budget::BACKGROUND, 1, &policy), Some("visible reserve"), "parked Background builds cannot fill the Visible drain");
        assert_eq!(drain_blocked(budget::VISIBLE, 1, &policy), None, "Visible work passes a parked Background build");
        policy.noninteractive = 2;
        assert_eq!(drain_blocked(budget::INTERACTIVE, 1, &policy), None, "first compiles retain their drain above both lower classes");
        assert_eq!(background_drains(2, 2, 2, 3, true), None, "O3 can take the last drain beside running breadth");
        assert_eq!(background_drains(2, 2, 3, 3, true), Some("visible reserve"), "O3 and breadth share the Background ceiling");
        for tier in [1, 3, 5] {
            assert_eq!(background_drains(tier, 1, 2, 3, true), None, "tier {tier} can take the rounded-up half beside O3");
            assert_eq!(background_drains(tier, 2, 2, 3, true), Some("O3 first"), "tier {tier} leaves the last drain for O3");
            assert_eq!(background_drains(tier, 2, 2, 3, false), None, "tier {tier} can fill the Background width once O3 finishes");
            assert_eq!(background_drains(tier, 3, 3, 3, false), Some("visible reserve"), "tier {tier} stops at the Background ceiling");
        }
        for tier in [0, 4] {
            assert_eq!(background_drains(tier, 2, 2, 3, true), None, "O3 first leaves ahead work and the tick eligible");
        }
        q.push(Job::Spec(1, 10), 1);
        q.push(Job::Spec(1, 11), 2);
        q.waiters.insert((1, 11), [0, 2]);
        let raised_from = wall_ns();
        q.promote(1, 11);
        assert_eq!(q.entries[1].class, budget::VISIBLE, "a job takes its waiters' class, not more");
        assert!(q.entries[1].since >= raised_from, "a raised job's wait at its waiters' class starts at the raise");
        assert_eq!(q.entries[0].since, 1, "a job nobody waits for waits from its queue time");
        q.waiters.get_mut(&(1, 11)).unwrap()[0] += 1;
        q.promote(1, 11);
        assert_eq!(q.entries[1].class, budget::INTERACTIVE, "the highest waiter's class");
        q.end_wait(1, 11, 0);
        q.end_wait(1, 11, 1);
        assert_eq!(q.entries[1].class, budget::INTERACTIVE, "another waiter retains promotion");
        q.end_wait(1, 11, 1);
        assert_eq!(q.entries[1].class, budget::BACKGROUND, "last waiter restores the queued class");
        assert_eq!(q.entries[1].since, 2, "back in the background a job has waited since it was queued");
        q.waiters.insert((1, 11), [1, 0]);
        q.promote(1, 11);
        let now = HeatNow { hu: None, wall: 100, tier: 0 };
        assert_eq!(job_class(&Job::O2(1, 7, O2Kind::Normal), &now), budget::BACKGROUND, "a tier-up runs below the app's visible work");
        assert_eq!(job_class(&Job::O3((7, 0, 0, 0)), &now), budget::BACKGROUND, "an O3 build runs below the app's visible work");
        let first = q.head(&now, Takes::ALL).expect("queued waiter can run");
        assert_eq!(q.entries[first].class, budget::INTERACTIVE, "heat refresh preserves waiter promotion");
        assert_eq!(q.remove(first).job.slot(), Some((1, 11)), "queued waiter moves to the front");
        let next = q.head(&now, Takes::ALL).expect("cold work can run after the waiter");
        assert_eq!(q.remove(next).job.slot(), Some((1, 10)), "cold work remains queued after promotion");
        assert!(q.head(&now, Takes::ALL).is_none(), "both jobs were removed exactly once");
        let front = Arc::new(FrontShared::new(&Arc::new(crate::Session::default())));
        front.alive.store(true, Ordering::Release);
        q.push(Job::Spec(1, 12), 1);
        q.push(Job::Ahead(front.clone(), AheadWork::Slot(13)), 100);
        q.set_ahead_class(budget::BACKGROUND);
        assert_eq!(q.entries[q.ordered(1)[0].0].job.tier(), 3, "older cold work leads outside a UI burst");
        q.set_ahead_class(budget::INTERACTIVE);
        let first = q.ordered(1)[0].0;
        assert_eq!(q.entries[first].job.tier(), 0, "a UI burst promotes the whole ahead bucket immediately");
        assert_eq!(q.remove(first).class, budget::INTERACTIVE, "admission uses the bucket's current class");
        q.remove(0);
        assert_eq!(q.ahead_len, 0, "removals balance bucket accounting");

        crate::check::with_early_batch_for_test(front, |batch, wait| {
            let check = Job::Check(batch.clone());
            let old_headless = HEADLESS.swap(true, Ordering::Relaxed);
            let headless_class = job_class(&check, &now);
            HEADLESS.store(false, Ordering::Relaxed);
            // Insert directly so no worker drains the entry before the frame waits.
            lock().push(Job::Check(batch), 1);
            let early = { let q = lock(); (q.entries[0].class, q.entries[0].key) };
            wait();
            let promoted = lock().remove(0);
            let waited_class = job_class(&check, &now);
            HEADLESS.store(old_headless, Ordering::Relaxed);
            assert_eq!(headless_class, budget::BACKGROUND, "a headless boot check runs in Background");
            assert_eq!(early, (budget::VISIBLE, i64::MIN), "an early UI check runs last in Visible");
            assert_eq!((promoted.class, promoted.key), (budget::INTERACTIVE, CHECK_WAITED_KEY), "the wait promotes the queued Visible check");
            assert_eq!(waited_class, budget::INTERACTIVE, "new check slices are Interactive after the wait");
        });
    }

    /// A drain the OS refuses a thread (a memory limit on the process: a tests/ui check under the
    /// sweep's data limit) leaves the workers started, and no later start fails for it; a start
    /// that got none, or panicked, is asked again by the next.
    #[test]
    fn a_refused_worker_thread_poisons_no_later_start() {
        let refused = || std::io::Error::from_raw_os_error(11);
        let d = Drains::new();
        let mut asked = 0;
        let got = d.start(4, || {}, || { asked += 1; if asked <= 2 { Ok(()) } else { Err(refused()) } }).unwrap();
        assert!(matches!(got, Started::Short(2, _)), "the two drains that started work: {got:?}");
        assert_eq!(d.count.load(Ordering::SeqCst), 2, "stop_workers waits for the two, not four");
        assert_eq!(d.start(4, || panic!("init again"), || panic!("asked again")).unwrap(), Started::Already, "the next push finds them started");

        let d = Drains::new();
        assert!(d.start(4, || {}, || Err(refused())).is_err(), "no drain at all: the start fails");
        assert_eq!(d.count.load(Ordering::SeqCst), 0);
        assert_eq!(d.start(4, || {}, || Ok(())).unwrap(), Started::All, "the next start asks again, and gets them");
        assert_eq!(d.count.load(Ordering::SeqCst), 4);

        let d = Drains::new();
        let first = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| d.start(4, || panic!("a panic in the first start"), || Ok(()))));
        assert!(first.is_err());
        assert_eq!(d.start(4, || {}, || Ok(())).unwrap(), Started::All, "a start after a panicked one runs, poisoned by nothing");
        assert_eq!(d.count.load(Ordering::SeqCst), 4);
    }

    /// A run's end waits out a worker still in a job however long the job takes: the job reads
    /// the unit and the statics the end puts back next (a prefront still lowering after a 10 s
    /// give-up read a Tcx the reset had freed: a fault outside JIT code, the app gone, 11-08).
    #[test]
    fn a_runs_end_waits_out_a_worker_in_a_job() {
        isolated_test!(a_runs_end_waits_out_a_worker_in_a_job);
        // a drain that is in its job until `back` comes
        DRAINS.count.fetch_add(1, Ordering::SeqCst);
        let t = Instant::now();
        let back = std::thread::spawn(|| {
            std::thread::sleep(std::time::Duration::from_millis(400));
            DRAINS_BACK.fetch_add(1, Ordering::SeqCst);
        });
        stop_workers(std::time::Duration::from_millis(50));
        assert!(t.elapsed() >= std::time::Duration::from_millis(400), "the end went on while the job ran ({:?})", t.elapsed());
        back.join().unwrap();
    }

    /// While a real-time thread is live, a worker at the idle policy starts tier-ups only, and one
    /// that may not go there any more everything else (idle_tiers).
    #[test]
    fn idle_workers_start_tier_ups_only() {
        let mut q = Queue::new();
        let now = HeatNow { hu: None, wall: 100, tier: 0 };
        q.push(Job::Spec(1, 20), 1);
        q.push(Job::O2(1, 21, O2Kind::Normal), 2);
        let drains = DrainsNow { cores: 2, started: 2, idle: 1, idle_limit: 1 };
        let tier_up = q.head(&now, drains.takes(Kind::Idle)).expect("an idle worker has a tier-up to start");
        assert_eq!(q.entries[tier_up].job.tier(), 1, "an idle worker starts the O2 build");
        let other = q.head(&now, drains.takes(Kind::Back)).expect("a worker kept out of the idle policy has work");
        assert_eq!(q.entries[other].job.tier(), 3, "it starts the speculative build, not the tier-up");
        q.remove(other);
        assert!(q.head(&now, drains.takes(Kind::Back)).is_none(), "the tier-up waits for an idle worker");
        assert!(q.first().is_some(), "which some worker may start: one of the other kind is woken");
    }

    #[test]
    fn a_push_starts_each_class_with_every_worker_parked() {
        isolated_test!(a_push_starts_each_class_with_every_worker_parked, cores = 2);
        use std::time::Duration;
        lock().tick_queued = true; // No periodic tick can rescue a missed wake.
        start();
        let workers = THREADS.get().unwrap();
        let split = cfg!(any(target_os = "linux", target_os = "android"));
        assert_eq!(workers.len(), budget::cores() as usize * if split { 2 } else { 1 });
        assert_eq!(DRAINS.count.load(Ordering::Acquire), workers.len());
        let until = |condition: &dyn Fn() -> bool, why: &str| {
            let start = Instant::now();
            while !condition() {
                assert!(start.elapsed() < Duration::from_secs(5), "{why}");
                std::thread::sleep(Duration::from_millis(1));
            }
        };
        for class in [budget::INTERACTIVE, budget::VISIBLE, budget::BACKGROUND] {
            let id = u32::from(class);
            let _wait = (class != budget::BACKGROUND).then(|| promote(0, id, class));
            until(&|| workers.iter().all(|w| w.parked.load(Ordering::SeqCst)), "workers did not park");
            spec(0, id); // No live unit. The job starts and finishes without a build.
            let kind = if split && class != budget::BACKGROUND { Kind::Fore } else { Kind::Back };
            until(&|| workers.iter().any(|w| w.kind() == kind && w.jobs[class as usize].load(Ordering::Acquire) == 1),
                "the drain for the pushed class did not start");
        }
        stop_workers(Duration::from_secs(1));
    }

    #[test]
    fn every_entry_has_a_started_taker() {
        for started in [1, 2, 3, 4] {
            for idle_limit in [0, 1, 2] {
                let mut d = DrainsNow { cores: 2, started, idle: 0, idle_limit };
                assert!(!d.go_idle(Kind::Fore, 1) && !d.go_idle(Kind::Back, 0));
                loop {
                    assert!(d.idle < started.min(d.cores), "one Back drain stays out of idle");
                    for class in [budget::INTERACTIVE, budget::VISIBLE, budget::BACKGROUND] {
                        for tier in [0, 1] {
                            let takes = |kind| d.takes(kind).takes(tier, class);
                            assert_eq!(takes(Kind::Fore), class != budget::BACKGROUND);
                            assert_eq!(takes(Kind::Idle), is_tier_up(tier));
                            assert_eq!(takes(Kind::Back), (started < 4 || class == budget::BACKGROUND)
                                && (!is_tier_up(tier) || idle_limit == 0 || d.idle < idle_limit));
                            assert!(takes(Kind::Back) || (started > 2 && takes(Kind::Fore)) || (d.idle > 0 && takes(Kind::Idle)));
                        }
                    }
                    if !d.go_idle(Kind::Back, 1) { break; }
                    d.idle += 1;
                }
            }
        }
    }

    /// At the quit the compiles queued and not started go, but one a thread waits for; the O3
    /// tier-ups stay queued (stopped: the exit records their verdicts), and nothing new nobody
    /// waits for is queued after it.
    #[test]
    fn a_quit_drops_the_queued_compiles_nobody_waits_for() {
        let mut q = Queue::new();
        let front = Arc::new(FrontShared::new(&Arc::new(crate::Session::default())));
        front.alive.store(true, Ordering::Release);
        q.push(Job::Ahead(front.clone(), AheadWork::Slot(30)), 1);
        q.push(Job::O2(1, 31, O2Kind::Normal), 2);
        q.push(Job::Spec(1, 32), 3);
        q.push(Job::O2(1, 33, O2Kind::Normal), 4);
        q.push(Job::O3((34, 0, 0, 0)), 5);
        q.push(Job::Tick, 6);
        q.waiters.insert((1, 33), [1, 0]);
        q.promote(1, 33);
        let serials = q.quit();
        // (a worker takes one of them before its batch is dropped)
        let taken = q.indices[&serials[0]];
        q.remove(taken);
        let gone = q.quit_drop(&serials);
        assert_eq!(gone.len(), 2, "the queued checks ahead, O2 and speculative builds nobody waits for are dropped, once");
        let left: Vec<usize> = q.entries.iter().map(|e| e.job.tier()).collect();
        assert_eq!(left.len(), 3, "the waited-for O2 build, the O3 tier-up and the tick stay: {left:?}");
        assert!(q.entries.iter().any(|e| e.job.slot() == Some((1, 33))), "a compile a thread waits for stays queued");
        assert!(q.entries.iter().any(|e| matches!(e.job, Job::O3(..))), "a queued O3 tier-up stays for the exit's verdicts");
        assert_eq!(q.ahead_len, 0, "the dropped check ahead leaves the bucket accounting balanced");
        q.push(Job::O2(1, 35, O2Kind::Normal), 7);
        q.push(Job::Spec(1, 36), 8);
        q.push(Job::Ahead(front, AheadWork::Slot(37)), 9);
        assert_eq!(q.entries.len(), 3, "after the quit nothing nobody waits for is queued");
        q.waiters.insert((1, 38), [1, 0]);
        q.push(Job::O2(1, 38, O2Kind::Normal), 10);
        assert_eq!(q.entries.len(), 4, "a compile a thread waits for is still queued");
    }
}

static LOOKS_GONE: AtomicBool = AtomicBool::new(false);

/// Release unused burst results once no ahead job can still publish into them. The startup's
/// look-ahead still queued goes when the startup ends.
pub(super) fn cleanup_ahead() {
    if startup_phase() { return; }
    if !LOOKS_GONE.swap(true, Ordering::Relaxed) && LOOK_QUEUED.load(Ordering::Relaxed) != 0 {
        retain(|job| !matches!(job, Job::Ahead(_, AheadWork::Look(..))));
    }
    // (between the UI's bursts, or past the startup of a program that never presents)
    if ahead_phase() && ahead_burst() { return; }
    let front = LAST_FRONT.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let Some(front) = front else { return };
    let q = lock();
    if q.entries.iter().any(|e| e.job.tier() == 0) || q.running.iter().any(|r| r.tier == 0) { return; }
    // (not what a background build checked and lowered for its own take: its bodies are published
    // under the claim lock, its lowerings under the map's)
    let (keys, unused) = {
        let _claim = front.inprog.lock().unwrap_or_else(|e| e.into_inner());
        if fronts_held() { return; }
        let keys = std::mem::take(&mut *front.ahead_post.lock().unwrap_or_else(|e| e.into_inner()));
        let unused: Vec<_> = keys.iter().filter_map(|key| front.ahead.remove(key)).collect();
        (keys, unused)
    };
    let lowered = {
        let mut lowered = front.lowered.lock().unwrap_or_else(|e| e.into_inner());
        if fronts_held() { Default::default() } else { std::mem::take(&mut *lowered) }
    };
    drop(q);
    lowered_end();
    if !keys.is_empty() {
        AHEAD_DROPPED.fetch_add(unused.len() as u64, Ordering::Relaxed);
        crate::postframe::ahead_over();
    }
    drop((keys, unused, lowered));
}

/// What this module holds of a run, as a fresh process has it (`run_end`). Nothing of the run is left that could see it.
pub(crate) unsafe fn reset_statics() {
    use relax_session::reset_static as reset;
    reset(&QUEUE, OnceLock::new());
    reset(&COUNTS, [const { AtomicUsize::new(0) }; TIERS]);
    reset(&REACH_QUEUED, AtomicUsize::new(0));
    reset(&RUNNING, [const { AtomicUsize::new(0) }; TIERS]);
    reset(&JOB_CPU, [const { AtomicU64::new(0) }; TIERS]);
    reset(&JOB_RUNS, [const { AtomicU64::new(0) }; TIERS]);
    reset(&COUNTED_CPU, [const { AtomicU64::new(0) }; 2]);
    reset(&HELD_NS, [const { [const { AtomicU64::new(0) }; HOLD_REASONS.len()] }; 2]);
    reset(&HELD_QUEUED_NS, [const { [const { AtomicU64::new(0) }; HOLD_REASONS.len()] }; 2]);
    reset(&SLOT_CPU, relax_session::sync::Mutex::new(None));
    reset(&THREADS, OnceLock::new());
    reset(&NEXT_WORKER, AtomicUsize::new(0));
    reset(&IDLE_WORKERS, AtomicUsize::new(0));
    reset(&BOUNCES, AtomicUsize::new(0));
    reset(&FLOOR_REFUSED, AtomicBool::new(false));
    reset(&WORKERS_STOP, AtomicBool::new(false));
    reset(&DRAINS, Drains::new());
    reset(&DRAINS_BACK, AtomicUsize::new(0));
    reset(&LOCK_MAX_NS, AtomicU64::new(0));
    reset(&AHEAD_INBOX, relax_session::sync::Mutex::new(Vec::new()));
    reset(&AHEAD_PENDING, AtomicBool::new(false));
    reset(&LOOKS_GONE, AtomicBool::new(false));
    reset(&QUITTING, AtomicBool::new(false));
}
