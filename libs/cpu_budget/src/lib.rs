//! One process-wide CPU budget for the app and the HAL.
//! A permit belongs to one worker; only `reclass` may be called by another thread.
//! Callers must use the permit returned by `checkpoint` and release it exactly once.
//! A permit is a row of the table, a token is one count of `running`: a thread asleep lends its
//! token (`lend`) and keeps its row, so the permit its caller holds stays good.
//! On a machine with two kinds of core (Apple silicon) the budget also places Background work on
//! them, by what it is to the program (`set_thread_work`): a build of hot code takes a performance
//! core only where the person's work leaves one (`fast_room`: the cores less the Interactive and
//! Visible jobs that run, averaged, and the threads that hold a core against a deadline,
//! `set_thread_deadline`), and work that only replaces something in use runs on the efficiency
//! cores while any of the person's work runs. The OS's priorities cannot do it there: QoS DEFAULT
//! is the class of the person's Visible work itself, and below it the kernel still runs a thread on
//! a performance core the moment one is free for an instant, so a realtime pipeline that sleeps and
//! wakes every round finds its core taken. Only QoS BACKGROUND keeps a thread off them.

// thread-locals take no pthread key each on Android (libs/thread_local)
#[macro_use(thread_local)]
extern crate makepad_thread_local;

use relax_api::relax_api;
use std::cell::Cell;
use std::collections::{HashMap, VecDeque};
use std::fmt::Write;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicU8, AtomicUsize, Ordering::Relaxed};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::thread::{self, Thread};

relax_api::option_layouts! {
    (u8, Span),
}

pub const INTERACTIVE: u8 = 0;
pub const VISIBLE: u8 = 1;
pub const BACKGROUND: u8 = 2;
pub const APP: u8 = 0;
pub const HAL: u8 = 2;
const CLASSES: [&str; 3] = ["interactive", "visible", "background"];
const BUCKETS: usize = 33;
/// Rows of the permit table: one per thread holding a permit, more than a process has threads.
const ROWS: usize = 1 << 16;
const RECENT_NS: u64 = 60_000_000_000;

#[derive(Clone, Copy, PartialEq, Eq)]
struct Label {
    bytes: [u8; 47],
    len: u8,
}
impl Default for Label {
    fn default() -> Self { Self { bytes: [0; 47], len: 0 } }
}
impl Label {
    fn new(s: &str) -> Self {
        let mut len = s.len().min(47);
        while !s.is_char_boundary(len) { len -= 1; }
        let mut label = Self { bytes: [0; 47], len: len as u8 };
        label.bytes[..len].copy_from_slice(&s.as_bytes()[..len]);
        label
    }
    fn text(&self) -> &str { std::str::from_utf8(&self.bytes[..self.len as usize]).unwrap() }
}
#[derive(Clone, Copy)]
struct Job {
    class: u8,
    client: u8,
    label: Label,
    ticket: u64,
    admitted_at: u64,
    checkpoint_at: u64,
    /// the thread it was last admitted on (`me`)
    thread: u64,
    /// its token is back in the budget while its thread sleeps (`lend`): not counted in `running`
    lent: bool,
}
impl Job {
    fn new(class: u8, client: u8, label: &str, ticket: u64) -> Self {
        Self { class, client, label: Label::new(label), ticket, admitted_at: 0, checkpoint_at: 0, thread: 0, lent: false }
    }
}
struct Waiter { class: u8, client: u8, ticket: u64, thread: Thread, id: u64 }
#[derive(Clone, Copy, Default)]
struct Span { label: Label, ns: u64 }
#[derive(Clone, Copy)]
struct Worst { class: u8, label: Label, ns: u64, when: u64, culprit: Option<(u8, Span)> }
struct State {
    /// The permit table, grown when lent rows leave no free one (`ROWS`).
    slots: Vec<Option<Job>>,
    waiters: Vec<Waiter>,
    ticket: u64,
    running: usize,
    waiting: u8,
    parked: [u64; 3],
    admitted: [u64; 3],
    runtime: [u64; 3],
    spans: [Span; 3],
    warnings: HashMap<String, u64>,
    worst: VecDeque<Worst>,
    /// The longest recent waits behind an earlier job on the same strand (`strand_waited`).
    strand_worst: VecDeque<Worst>,
    /// How many times the table changed (`refresh`): `Budget::changed` wakes the waits for it.
    changes: u64,
    /// Threads in `wait_for_change`: a change with none costs no wake.
    watchers: usize,
    /// The tokens of the person's work that run (Interactive and Visible rows that count), since
    /// `urgent_at`; `urgent_area` sums tokens times nanoseconds, and the span since `window_at`
    /// (its area then `window_area`) makes the next reading of `Budget::demand` (`Budget::roll`).
    urgent: usize,
    urgent_at: u64,
    urgent_area: u64,
    window_at: u64,
    window_area: u64,
    /// The last span's reading, in thousandths of a token.
    last_window: u64,
}
/// The span the person's demand on the cores is averaged over (`Budget::roll`). A live run's packs
/// start and end at its round rate (about 1.7 thousand rounds a second in a live audio simulation),
/// so an instant reads 0 or 3 tokens by chance; a tenth of a second holds a few hundred rounds. The
/// reading is the larger of the last two spans: work that starts counts within one span, work that
/// ended leaves after two. A wrong reading only places a build on the other kind of core.
const DEMAND_SPAN_NS: u64 = 100_000_000;
struct Budget {
    /// `monotonic_ns` at its start: `now` counts from here (wrapping, so an origin set back past the
    /// clock's own start still counts forward).
    origin: u64,
    cores: usize,
    limit: usize,
    /// Background's ceiling: below Visible's, and at most the physical cores but one. Background
    /// work on an SMT sibling of a busy core takes far more CPU for the same work (a cold plate on
    /// 8 cores / 16 threads: the same 990 O3 builds took 31.6 CPU-s 8 wide, 58.5 CPU-s 13 wide)
    /// and gets the work done little sooner. The JIT's queue sizes its own Background width from it.
    background_limit: usize,
    /// The performance cores of a machine with two kinds of core (Apple silicon), within the
    /// budget's; 0 on a machine with one kind: placement by kind of work is then the OS's alone.
    fast: usize,
    /// The person's work on the cores, in thousandths of a token: the larger of the last two
    /// spans' averages (`roll`).
    demand: AtomicU64,
    /// Threads that hold a core against a deadline outside admission (`set_thread_deadline`).
    deadlines: AtomicUsize,
    /// Threads whose tier-up builds hold a performance core now (`set_thread_work`).
    fast_tier_ups: AtomicUsize,
    state: Mutex<State>,
    /// Woken on each change of the table (`State::changes`, `wait_for_change`).
    changed: Condvar,
    classes: Box<[AtomicU8]>,
    // A precomputed bit per yielding class: should_yield needs just two relaxed loads.
    yield_classes: AtomicU8,
    queued: [[AtomicI64; 3]; 3],
    waits: [[AtomicU64; BUCKETS]; 3],
    max_wait: [AtomicU64; 3],
    /// Waits behind an earlier job on the same strand, kept apart from the waits for a worker.
    strand_waits: [AtomicU64; BUCKETS],
    max_strand_wait: AtomicU64,
}
impl Budget {
    fn new(cores: usize, physical: usize) -> Self {
        assert!(cores > 0);
        let limit = cores.saturating_sub(1).max(1);
        Self {
            origin: monotonic_ns(), cores, limit,
            background_limit: limit.saturating_sub(2).min(physical.saturating_sub(1)).max(1),
            fast: 0,
            demand: AtomicU64::new(0),
            deadlines: AtomicUsize::new(0),
            fast_tier_ups: AtomicUsize::new(0),
            state: Mutex::new(State {
                slots: vec![None; cores], waiters: Vec::new(), ticket: 0,
                running: 0, waiting: 0, parked: [0; 3], admitted: [0; 3],
                runtime: [0; 3], spans: [Span::default(); 3], warnings: HashMap::new(),
                worst: VecDeque::new(),
                strand_worst: VecDeque::new(),
                changes: 0,
                watchers: 0,
                urgent: 0, urgent_at: 0, urgent_area: 0, window_at: 0, window_area: 0, last_window: 0,
            }),
            changed: Condvar::new(),
            classes: (0..ROWS).map(|_| AtomicU8::new(INTERACTIVE)).collect(),
            yield_classes: AtomicU8::new(0),
            queued: std::array::from_fn(|_| std::array::from_fn(|_| AtomicI64::new(0))),
            waits: std::array::from_fn(|_| std::array::from_fn(|_| AtomicU64::new(0))),
            max_wait: std::array::from_fn(|_| AtomicU64::new(0)),
            strand_waits: std::array::from_fn(|_| AtomicU64::new(0)),
            max_strand_wait: AtomicU64::new(0),
        }
    }
    /// The budget of a machine with `fast` performance cores (two kinds of core; 0: one kind).
    fn with_fast(mut self, fast: usize) -> Self {
        self.fast = fast.min(self.cores);
        self
    }
    fn now(&self) -> u64 { monotonic_ns().wrapping_sub(self.origin) }
    /// The person's work's tokens counted up to `now`, and a new reading of `demand` once a span
    /// passed. A few integer updates on most calls; under the lock, with the caller's clock.
    fn roll(&self, s: &mut State, now: u64) {
        let dt = now.saturating_sub(s.urgent_at);
        s.urgent_area = s.urgent_area.saturating_add(s.urgent as u64 * dt);
        s.urgent_at = s.urgent_at.max(now);
        let span = now.saturating_sub(s.window_at);
        if span < DEMAND_SPAN_NS { return; }
        let reading = (s.urgent_area - s.window_area).saturating_mul(1000) / span;
        self.demand.store(reading.max(s.last_window), Relaxed);
        s.last_window = reading;
        s.window_at = now;
        s.window_area = s.urgent_area;
    }
    /// The performance cores the person's work leaves now: 0 on a machine with one kind of core.
    fn fast_room(&self) -> usize {
        let demand = (self.demand.load(Relaxed) as usize).div_ceil(1000);
        self.fast.saturating_sub(demand + self.deadlines.load(Relaxed))
    }
    /// Some of the person's work runs, or ran within the last two spans, or a thread holds a core
    /// against a deadline. (Placement reads it on macOS only.)
    #[cfg(any(target_os = "macos", test))]
    fn urgent(&self) -> bool { self.deadlines.load(Relaxed) > 0 || self.demand.load(Relaxed) > 0 }
    fn class_limit(&self, class: u8) -> usize {
        match class {
            INTERACTIVE => self.cores,
            VISIBLE => self.limit,
            BACKGROUND => self.background_limit,
            _ => unreachable!(),
        }
    }
    fn over_cores(&self, s: &State) -> bool {
        s.running > self.cores
    }
    fn room(&self, s: &State, class: u8) -> bool {
        s.running < self.class_limit(class)
    }
    fn first(&self, s: &State) -> Option<usize> {
        s.waiters.iter().enumerate().filter(|(_, w)| self.room(s, w.class)).min_by_key(|(_, w)| (w.class, w.ticket)).map(|(i, _)| i)
    }
    fn refresh(&self, s: &mut State) {
        s.waiting = s.waiters.iter().fold(0, |bits, w| bits | (1 << w.class));
        let mut bits = 0;
        for class in 0..3 {
            if s.waiting & ((1 << class) - 1) != 0 || self.over_cores(s) {
                bits |= 1 << class;
            }
        }
        self.yield_classes.store(bits, Relaxed);
        for (index, job) in s.slots.iter().enumerate() {
            if let Some(job) = job {
                self.classes[index].store(job.class, Relaxed);
            }
        }
        if let Some(i) = self.first(s) {
            s.waiters[i].thread.unpark();
        }
        s.changes = s.changes.wrapping_add(1);
        if s.watchers > 0 { self.changed.notify_all(); }
    }
    /// Blocks until the table changed after it showed `seen` changes; the count then.
    fn wait_for_change(&self, seen: u64) -> u64 {
        let mut s = self.lock();
        s.watchers += 1;
        while s.changes == seen {
            s = self.changed.wait(s).unwrap_or_else(|e| e.into_inner());
        }
        s.watchers -= 1;
        s.changes
    }
    fn acquire<'a>(&'a self, mut s: MutexGuard<'a, State>, mut job: Job, desired: Option<&AtomicU8>) -> u32 {
        let key = (job.class, job.ticket);
        let first = self.first(&s);
        let immediate = self.room(&s, job.class)
            && first.is_none_or(|i| key < (s.waiters[i].class, s.waiters[i].ticket));
        if !immediate {
            // Only the contended path grows the waiter list. No thread is created here.
            s.waiters.push(Waiter { class: job.class, client: job.client, ticket: job.ticket, thread: thread::current(), id: me() });
            self.refresh(&mut s);
            loop {
                if let Some(i) = self.first(&s).filter(|&i| s.waiters[i].ticket == job.ticket) {
                    s.waiters.swap_remove(i);
                    break;
                }
                drop(s);
                match WAIT_HOOK.get() {
                    Some(hook) => hook(job.class),
                    None => thread::park(),
                }
                s = self.lock();
                if let Some(desired) = desired {
                    job.class = desired.load(Relaxed);
                    if let Some(w) = s.waiters.iter_mut().find(|w| w.ticket == job.ticket) { w.class = job.class; }
                    self.refresh(&mut s);
                }
            }
        }
        self.install(s, job)
    }
    /// A panic under this lock (a permit that names no row) comes before any change to the table, and
    /// stays that thread's: every other thread's tokens go on.
    fn lock(&self) -> MutexGuard<'_, State> { self.state.lock().unwrap_or_else(|e| e.into_inner()) }
    fn install(&self, mut s: MutexGuard<'_, State>, mut job: Job) -> u32 {
        let index = s.slots.iter().position(Option::is_none).unwrap_or_else(|| {
            assert!(s.slots.len() < ROWS, "more threads hold a CPU permit than the table has rows");
            s.slots.push(None);
            s.slots.len() - 1
        });
        let now = self.now();
        job.admitted_at = now;
        job.checkpoint_at = now;
        job.thread = me();
        ADMITTED.with(|admitted| admitted.set(true));
        s.slots[index] = Some(job);
        self.start_counting(&mut s, &job, now);
        s.admitted[job.class as usize] += 1;
        self.classes[index].store(job.class, Relaxed);
        self.refresh(&mut s);
        drop(s);
        set_thread_class(job.class);
        index as u32
    }
    fn admit(&self, class: u8, client: u8, label: &str) -> u32 {
        valid(class, client);
        let mut s = self.lock();
        let ticket = s.ticket;
        s.ticket = s.ticket.checked_add(1).expect("CPU budget ticket overflow");
        self.acquire(s, Job::new(class, client, label, ticket), None)
    }
    /// `admit` at the class `desired` holds, read again each time the waiting thread wakes: whoever
    /// raises it unparks the thread.
    fn admit_following(&self, desired: &AtomicU8, client: u8, label: &str) -> u32 {
        let class = desired.load(Relaxed);
        valid(class, client);
        let mut s = self.lock();
        let ticket = s.ticket;
        s.ticket = s.ticket.checked_add(1).expect("CPU budget ticket overflow");
        self.acquire(s, Job::new(class, client, label, ticket), Some(desired))
    }
    fn should_yield(&self, permit: u32) -> bool {
        let class = self.classes[permit as usize].load(Relaxed);
        self.yield_classes.load(Relaxed) & (1 << class) != 0
    }
    fn span(s: &mut State, label: Label, ns: u64) {
        if let Some(span) = s.spans.iter_mut().find(|span| span.label == label) {
            span.ns = span.ns.max(ns);
        } else {
            let span = s.spans.iter_mut().min_by_key(|span| span.ns).unwrap();
            if ns > span.ns { *span = Span { label, ns }; }
        }
    }
    fn remove(&self, s: &mut State, permit: u32, now: u64) -> Job {
        let job = s.slots[permit as usize].take().expect("released CPU permit");
        if !job.lent { self.stop_counting(s, &job, now); }
        job
    }
    fn start_counting(&self, s: &mut State, job: &Job, now: u64) {
        self.roll(s, now);
        s.urgent += usize::from(job.class != BACKGROUND);
        s.running += 1;
    }
    fn stop_counting(&self, s: &mut State, job: &Job, now: u64) {
        self.roll(s, now);
        s.urgent -= usize::from(job.class != BACKGROUND);
        s.running -= 1;
        s.runtime[job.class as usize] += now.saturating_sub(job.admitted_at);
        Self::span(s, job.label, now.saturating_sub(job.checkpoint_at));
    }
    /// The calling thread goes to sleep on other work: the tokens of its permits go back to the
    /// budget, its rows stay.
    /// True: it lent one, and `take_back` ends that.
    fn lend(&self) -> bool {
        let me = me();
        let mut s = self.lock();
        let now = self.now();
        let mut lent = false;
        for i in 0..s.slots.len() {
            let Some(job) = s.slots[i].as_mut().filter(|j| j.thread == me && !j.lent) else { continue };
            job.lent = true;
            let job = *job;
            self.stop_counting(&mut s, &job, now);
            lent = true;
        }
        if lent { self.refresh(&mut s); }
        lent
    }
    /// Lend this job's token while its worker runs a nested job. Its permit keeps the same row.
    fn lend_permit(&self, permit: u32) {
        let mut s = self.lock();
        let now = self.now();
        let job = s.slots[permit as usize].as_mut().expect("lent a released CPU permit");
        assert!(!job.lent, "lent a CPU permit twice");
        job.lent = true;
        let job = *job;
        self.stop_counting(&mut s, &job, now);
        self.refresh(&mut s);
    }
    /// The calling thread is awake: its lent tokens are its own again, at once, whatever is running
    /// by now. It never waits here: it may hold locks that what runs now needs (a program thread
    /// sleeps anywhere). Over the limit no one new is admitted and its next checkpoint yields.
    fn take_back(&self) {
        let me = me();
        let mut s = self.lock();
        let now = self.now();
        let mut back = false;
        for i in 0..s.slots.len() {
            if s.slots[i].as_ref().is_some_and(|j| j.thread == me && j.lent) {
                self.count_again(&mut s, i, now);
                back = true;
            }
        }
        if back { self.refresh(&mut s); }
    }
    /// `take_back` of `lend_permit`: its job has not run meanwhile, and the same goes.
    fn take_back_permit(&self, permit: u32) {
        let mut s = self.lock();
        let now = self.now();
        self.count_again(&mut s, permit as usize, now);
        self.refresh(&mut s);
    }
    fn count_again(&self, s: &mut State, index: usize, now: u64) {
        let job = s.slots[index].as_mut().expect("took back a released CPU permit");
        job.lent = false;
        job.admitted_at = now;
        job.checkpoint_at = now;
        let job = *job;
        self.start_counting(s, &job, now);
    }
    fn tokens(&self) -> Tokens {
        let s = self.lock();
        let now = self.now();
        let holders = s.slots.iter().flatten().map(|j| Holder {
            thread: j.thread, class: j.class, client: j.client, label: j.label.text().to_owned(), lent: j.lent,
            held_ns: now.saturating_sub(j.admitted_at),
        }).collect();
        let waiting = s.waiters.iter().map(|w| Waiting { thread: w.id, class: w.class, client: w.client }).collect();
        Tokens { running: s.running, limits: [self.cores, self.limit, self.background_limit], holders, waiting }
    }
    fn checkpoint(&self, permit: u32, held_locks: u32, lock_site: &str) -> u32 {
        let mut s = self.lock();
        match self.yields(&mut s, permit, held_locks, lock_site) {
            // Release and enqueue under the same lock; retain the original admission ticket.
            Some(job) => self.acquire(s, job, None),
            None => {
                let class = s.slots[permit as usize].as_ref().expect("released CPU permit").class;
                drop(s);
                set_thread_class(class);
                permit
            }
        }
    }
    /// A checkpoint's account and verdict: the job it took out of its slot to yield, or None (it goes
    /// on: no reason to yield, or it holds locks).
    fn yields(&self, s: &mut State, permit: u32, held_locks: u32, lock_site: &str) -> Option<Job> {
        let now = self.now();
        self.roll(s, now);
        let job = s.slots[permit as usize].as_mut().expect("released CPU permit");
        let span = Span { label: job.label, ns: now.saturating_sub(job.checkpoint_at) };
        job.checkpoint_at = now;
        let class = job.class;
        Self::span(s, span.label, span.ns);
        if !self.should_yield(permit) {
            return None;
        }
        if held_locks != 0 {
            if let Some(count) = s.warnings.get_mut(lock_site) { *count += 1; }
            else { s.warnings.insert(lock_site.to_owned(), 1); }
            return None;
        }
        let job = self.remove(s, permit, now);
        s.parked[class as usize] += 1;
        Some(job)
    }
    /// A run of the app ended in place (`makepad run`): the threads of the app's rows are gone and
    /// never release them. The HAL's stay, its threads with them.
    fn end_run(&self) {
        for count in CLASS_REFUSALS.iter().flatten() { count.store(0, Relaxed); }
        REFUSALS_MARKED.store(false, Relaxed);
        REFUSAL_GEN.fetch_add(1, Relaxed);
        REFUSED_THREADS.lock().unwrap_or_else(|e| e.into_inner()).clear();
        let mut s = self.lock();
        let now = self.now();
        for i in 0..s.slots.len() {
            if s.slots[i].is_some_and(|j| j.client != HAL) {
                self.remove(&mut s, i as u32, now);
            }
        }
        s.waiters.retain(|w| w.client == HAL);
        for class in &self.queued {
            class[APP as usize].store(0, Relaxed);
        }
        self.refresh(&mut s);
    }
    fn release(&self, permit: u32) {
        let mut s = self.lock();
        self.remove(&mut s, permit, self.now());
        self.refresh(&mut s);
    }
    fn reclass(&self, permit: u32, class: u8) {
        valid(class, APP);
        let mut s = self.lock();
        let now = self.now();
        let job = s.slots[permit as usize].as_mut().expect("released CPU permit");
        let old = job.class;
        let elapsed = now.saturating_sub(job.admitted_at);
        job.class = class;
        job.admitted_at = now;
        if !job.lent && (old == BACKGROUND) != (class == BACKGROUND) {
            self.roll(&mut s, now);
            if class == BACKGROUND { s.urgent -= 1 } else { s.urgent += 1 }
        }
        s.runtime[old as usize] += elapsed;
        self.classes[permit as usize].store(class, Relaxed);
        self.refresh(&mut s);
    }
    fn waited(&self, class: u8, client: u8, ns: u64, label: &str) {
        valid(class, client);
        self.waits[class as usize][bucket(ns)].fetch_add(1, Relaxed);
        self.max_wait[class as usize].fetch_max(ns, Relaxed);
        if ns <= 100_000_000 { return; }
        let mut s = self.lock();
        let now = self.now();
        let culprit = s.slots.iter().flatten().filter(|j| !j.lent)
            .max_by_key(|j| (j.class, now.saturating_sub(j.checkpoint_at)))
            .map(|j| (j.class, Span { label: j.label, ns: now.saturating_sub(j.checkpoint_at) }));
        // A monotone deque retains exactly the possible maxima of the rolling minute.
        while s.worst.front().is_some_and(|w| now.saturating_sub(w.when) > RECENT_NS) { s.worst.pop_front(); }
        while s.worst.back().is_some_and(|w| w.ns <= ns) { s.worst.pop_back(); }
        s.worst.push_back(Worst { class, label: Label::new(label), ns, when: now, culprit });
    }
    /// A strand's job waited `ns` behind the strand's earlier jobs before its turn came: the
    /// strand's own order, not a worker or a permit (its wait for those is `waited`).
    fn strand_waited(&self, class: u8, ns: u64, label: &str) {
        valid(class, APP);
        self.strand_waits[bucket(ns)].fetch_add(1, Relaxed);
        self.max_strand_wait.fetch_max(ns, Relaxed);
        if ns <= 100_000_000 { return; }
        let mut s = self.lock();
        let now = self.now();
        while s.strand_worst.front().is_some_and(|w| now.saturating_sub(w.when) > RECENT_NS) { s.strand_worst.pop_front(); }
        while s.strand_worst.back().is_some_and(|w| w.ns <= ns) { s.strand_worst.pop_back(); }
        s.strand_worst.push_back(Worst { class, label: Label::new(label), ns, when: now, culprit: None });
    }
    fn report(&self) -> String {
        let s = self.lock();
        let now = self.now();
        let mut running = [[0u32; 3]; 3];
        let mut spans = s.spans;
        let lent = s.slots.iter().flatten().filter(|j| j.lent).count();
        for job in s.slots.iter().flatten().filter(|j| !j.lent) {
            running[job.class as usize][job.client as usize] += 1;
            let ns = now.saturating_sub(job.checkpoint_at);
            if let Some(span) = spans.iter_mut().find(|span| span.label == job.label) { span.ns = span.ns.max(ns); }
            else {
                let span = spans.iter_mut().min_by_key(|span| span.ns).unwrap();
                if ns > span.ns { *span = Span { label: job.label, ns }; }
            }
        }
        let total = s.running;
        let worst = s.worst.iter().find(|w| now.saturating_sub(w.when) <= RECENT_NS).copied();
        let strand_worst = s.strand_worst.iter().find(|w| now.saturating_sub(w.when) <= RECENT_NS).copied();
        let parked = s.parked;
        let admitted = s.admitted;
        let runtime = s.runtime;
        let mut warnings: Vec<_> = s.warnings.iter().map(|(site, count)| (site.clone(), *count)).collect();
        drop(s);
        warnings.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        spans.sort_by_key(|span| std::cmp::Reverse(span.ns));
        let mut out = format!("cpu {total}/{} running", self.limit);
        if total > self.limit { let _ = write!(out, " (+{} over)", total - self.limit); }
        if lent != 0 { let _ = write!(out, ", {lent} lent by sleeping threads"); }
        if self.fast > 0 {
            let _ = write!(out, ", fast room {} of {} (the person's work {:.1}, deadline threads {}, tier-ups on them {})", self.fast_room(), self.fast,
                self.demand.load(Relaxed) as f64 / 1000.0, self.deadlines.load(Relaxed), self.fast_tier_ups.load(Relaxed));
        }
        if let Some(w) = worst {
            let _ = write!(out, ", worst: {} {:?} waited {}", CLASSES[w.class as usize], w.label.text(), human(w.ns));
            if let Some((class, span)) = w.culprit {
                let _ = write!(out, " -- {} {:?} ran {} without a checkpoint", CLASSES[class as usize], span.label.text(), human(span.ns));
            }
        }
        out.push_str("\nclass        running app/hal  queued  wait p50 / p95 / max  parked  admitted  run time\n");
        for class in 0..3 {
            let hist = std::array::from_fn::<_, BUCKETS, _>(|i| self.waits[class][i].load(Relaxed));
            let queued: i64 = self.queued[class].iter().map(|q| q.load(Relaxed)).sum();
            let _ = writeln!(out, "{:<12} {} / {}  {}  {} / {} / {}  {}  {}  {}", CLASSES[class],
                running[class][APP as usize], running[class][HAL as usize], queued,
                human(percentile(&hist, 50)), human(percentile(&hist, 95)), human(self.max_wait[class].load(Relaxed)),
                parked[class], admitted[class], human(runtime[class]));
        }
        let hist = std::array::from_fn::<_, BUCKETS, _>(|i| self.strand_waits[i].load(Relaxed));
        if hist.iter().any(|&n| n != 0) {
            let _ = write!(out, "behind an earlier job on its strand: wait p50 / p95 / max  {} / {} / {}",
                human(percentile(&hist, 50)), human(percentile(&hist, 95)), human(self.max_strand_wait.load(Relaxed)));
            if let Some(w) = strand_worst {
                let _ = write!(out, ", worst: {} {:?} waited {}", CLASSES[w.class as usize], w.label.text(), human(w.ns));
            }
            out.push('\n');
        }
        for span in spans { if span.ns > 0 { let _ = writeln!(out, "no checkpoint for: {} {}", span.label.text(), human(span.ns)); } }
        for (site, count) in warnings.into_iter().take(8) { let _ = writeln!(out, "checkpoint under lock: {site} x{count}"); }
        out
    }
}
/// A wait's histogram bucket: powers of two of microseconds.
fn bucket(ns: u64) -> usize {
    let us = ns.div_ceil(1000).max(1);
    (64 - (us - 1).leading_zeros() as usize).min(BUCKETS - 1)
}
fn valid(class: u8, client: u8) { assert!(class <= BACKGROUND && matches!(client, APP | HAL), "invalid CPU budget class/client"); }
fn percentile(hist: &[u64; BUCKETS], percent: u64) -> u64 {
    let total: u64 = hist.iter().sum();
    if total == 0 { return 0; }
    let target = (total * percent).div_ceil(100);
    let mut count = 0;
    for (i, n) in hist.iter().enumerate() {
        count += n;
        if count >= target { return (1u64 << i) * 1000; }
    }
    0
}
fn human(ns: u64) -> String {
    if ns < 1000 { format!("{ns} ns") }
    else if ns < 1_000_000 { format!("{:.1} us", ns as f64 / 1000.0) }
    else if ns < 1_000_000_000 { format!("{:.1} ms", ns as f64 / 1_000_000.0) }
    else { format!("{:.1} s", ns as f64 / 1_000_000_000.0) }
}
/// The calling thread, as the JIT's wait registry names it (`relax_base::waits::me`).
#[cfg(unix)]
fn me() -> u64 {
    unsafe extern "C" { fn pthread_self() -> usize; }
    unsafe { pthread_self() as u64 }
}
#[cfg(windows)]
fn me() -> u64 {
    #[link(name = "kernel32")]
    unsafe extern "system" { fn GetCurrentThreadId() -> u32; }
    unsafe { GetCurrentThreadId() as u64 }
}
#[cfg(not(any(unix, windows)))]
fn me() -> u64 { 0 }
/// Nanoseconds on the platform's monotonic clock, from an arbitrary origin (as
/// `makepad_platform::monotonic_seconds`). This crate sits below the platform, so on the web it reads
/// the host clock the platform does: `Instant::now` panics on wasm32-unknown-unknown ("time not
/// implemented on this platform"), and a page's first pool aborted it.
#[cfg(not(target_arch = "wasm32"))]
fn monotonic_ns() -> u64 {
    static START: OnceLock<std::time::Instant> = OnceLock::new();
    START.get_or_init(std::time::Instant::now).elapsed().as_nanos().min(u64::MAX as u128) as u64
}
#[cfg(target_arch = "wasm32")]
fn monotonic_ns() -> u64 {
    #[link(wasm_import_module = "env")]
    extern "C" {
        fn js_monotonic_now() -> f64;
    }
    (unsafe { js_monotonic_now() }.max(0.0) * 1_000_000_000.0) as u64
}
static WAIT_HOOK: OnceLock<extern "C" fn(u8)> = OnceLock::new();
fn global() -> &'static Budget {
    static BUDGET: OnceLock<Budget> = OnceLock::new();
    BUDGET.get_or_init(|| {
        let cores = granted_cores(machine_cores(), std::env::var("MAKEPAD_CPU_CORES").ok().as_deref());
        // (a machine whose topology is not known: every CPU a core)
        Budget::new(cores, match physical_cores() { 0 => cores, n => n.min(cores) }).with_fast(fast_cores())
    })
}

/// The cores the process may use: its machine's `machine`, or fewer where its run was given
/// fewer (`MAKEPAD_CPU_CORES`: a gate's share of its lease for each of the units it runs side by
/// side): every pool and the JIT's workers then size from that share, not the box. A tests shard
/// ran three units at once, each unit's pools and JIT workers sized from the whole box, and its
/// box ran well over its cores (11-08).
///
/// Which variable sizes what:
/// - `MAKEPAD_CPU_CORES`: the budget itself ([`cores`], [`limit`]): every admission through it, the
///   JIT queue's own widths (from [`background_limit`]), and the long-lived pools when no
///   `MAKEPAD_POOL_CORES` is set.
/// - `MAKEPAD_POOL_CORES`: the long-lived pools alone ([`pool_cores`]): the platform's app pool,
///   the CSG pool and the script compute pool. Nothing else reads it.
/// - `MAKEPAD_MACHINE_CORES`: the machine itself ([`machine_cores`]), which the two above are at
///   most.
/// - `RELAX_JOBS`: the JIT's parse jobs (relax/session options), not this budget.
fn granted_cores(machine: usize, jobs: Option<&str>) -> usize {
    jobs.and_then(|v| v.parse::<usize>().ok()).filter(|n| *n > 0).map_or(machine, |n| n.min(machine))
}

/// The machine's cores as the process counts them: `MAKEPAD_MACHINE_CORES` where its run was told
/// them, else what the OS shows it (`available_parallelism`, which on Linux is also its cgroup's
/// CPU quota). A process sizes from this once, at its start: one started under a quota that lifts
/// later is told its machine's cores, or it keeps its quota's size for its whole life. A live
/// session launched under a 10-core quota beside a tests shard would have kept a 10-core JIT on a
/// 16-core box after its launch ended (11-26). Read once on each side of a JIT run ([`MACHINE`]).
pub fn machine_cores() -> usize {
    *MACHINE.get_or_init(|| machine_from(std::env::var("MAKEPAD_MACHINE_CORES").ok().as_deref(), || thread::available_parallelism().map_or(1, usize::from)))
}

/// [`machine_cores`]'s memo. Each side of a JIT run keeps its own: the loader's std and Relax's lay
/// out a `OnceLock` each their own way, and JIT code reaching an unmarked provided static is refused
/// (a live app panicked at its start, 11-27). Both sides read the same variable and OS count.
#[relax_api(per_side)]
static MACHINE: OnceLock<usize> = OnceLock::new();

/// `given` when it is a count above zero, else what `seen` counts.
fn machine_from(given: Option<&str>, seen: impl FnOnce() -> usize) -> usize {
    given.and_then(|v| v.parse::<usize>().ok()).filter(|n| *n > 0).unwrap_or_else(seen)
}

/// The machine's cores, an SMT core's hardware threads counted once (Linux: one per distinct
/// sibling set of the online CPUs).
#[cfg(any(target_os = "linux", target_os = "android"))]
fn physical_cores() -> usize {
    let mut sets = Vec::new();
    for entry in std::fs::read_dir("/sys/devices/system/cpu").into_iter().flatten().flatten() {
        let name = entry.file_name();
        let Some(n) = name.to_str().and_then(|n| n.strip_prefix("cpu")) else { continue };
        if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) { continue; }
        // (an offline CPU has no topology)
        if let Ok(set) = std::fs::read_to_string(entry.path().join("topology/thread_siblings_list")) {
            sets.push(set.trim().to_owned());
        }
    }
    sets.sort_unstable();
    sets.dedup();
    sets.len()
}
#[cfg(target_vendor = "apple")]
fn physical_cores() -> usize {
    unsafe extern "C" {
        fn sysctlbyname(name: *const u8, old: *mut u8, oldlen: *mut usize, new: *const u8, newlen: usize) -> i32;
    }
    let mut n: i32 = 0;
    let mut len = std::mem::size_of::<i32>();
    let r = unsafe { sysctlbyname(b"hw.physicalcpu\0".as_ptr(), &mut n as *mut i32 as *mut u8, &mut len, std::ptr::null(), 0) };
    if r == 0 { n.max(0) as usize } else { 0 }
}
#[cfg(windows)]
fn physical_cores() -> usize {
    // SYSTEM_LOGICAL_PROCESSOR_INFORMATION: one RelationProcessorCore (0) entry per core
    #[repr(C)]
    struct Info { mask: usize, relationship: u32, union: [u64; 2] }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetLogicalProcessorInformation(buffer: *mut Info, length: *mut u32) -> i32;
    }
    let mut len = 0u32;
    unsafe { GetLogicalProcessorInformation(std::ptr::null_mut(), &mut len) };
    let mut infos: Vec<Info> = Vec::with_capacity(len as usize / std::mem::size_of::<Info>());
    if unsafe { GetLogicalProcessorInformation(infos.as_mut_ptr(), &mut len) } == 0 { return 0; }
    unsafe { infos.set_len(len as usize / std::mem::size_of::<Info>()) };
    infos.iter().filter(|info| info.relationship == 0).count()
}
#[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple", windows)))]
fn physical_cores() -> usize { 0 }
/// The performance cores of a machine with two kinds of core (`hw.nperflevels` 2 on Apple
/// silicon); 0 on one with one kind, and where the OS does not say.
#[cfg(target_os = "macos")]
fn fast_cores() -> usize {
    unsafe extern "C" {
        fn sysctlbyname(name: *const u8, old: *mut u8, oldlen: *mut usize, new: *const u8, newlen: usize) -> i32;
    }
    let read = |name: &[u8]| {
        let mut n: i32 = 0;
        let mut len = std::mem::size_of::<i32>();
        let r = unsafe { sysctlbyname(name.as_ptr(), &mut n as *mut i32 as *mut u8, &mut len, std::ptr::null(), 0) };
        if r == 0 { n.max(0) as usize } else { 0 }
    };
    if read(b"hw.nperflevels\0") < 2 { return 0; }
    read(b"hw.perflevel0.physicalcpu\0")
}
#[cfg(not(target_os = "macos"))]
fn fast_cores() -> usize { 0 }

#[relax_api]
pub fn cores() -> u32 { global().cores as u32 }
#[relax_api]
pub fn limit() -> u32 { global().limit as u32 }
/// The cores a long-lived pool sizes its threads from: `MAKEPAD_POOL_CORES` where the run was given
/// that share for its steady work, at most [`cores`]; else [`cores`] ([`granted_cores`] says which
/// variable sizes what). A pool's threads stay for the whole run, while a compile burst (the JIT's,
/// at a launch) is short and is what an interactive launch waits on: devhub gives a live test
/// session's pools its call's charge and leaves the budget, and so the JIT, the box. A live app
/// test's app pool and geometry pool each took the box's 16 cores beside the JIT's workers while the
/// budget counted it a small ask, and its box stood at load 57 (11-18). Read on each call: a
/// pool reads it once, at its start.
pub fn pool_cores() -> u32 {
    granted_cores(cores() as usize, std::env::var("MAKEPAD_POOL_CORES").ok().as_deref()) as u32
}
/// How many Background jobs run at once.
#[relax_api]
pub fn background_limit() -> u32 { global().background_limit as u32 }

#[derive(Clone)]
enum Which { Process, Own(Arc<Budget>) }
impl Which {
    fn get(&self) -> &Budget {
        match self { Self::Process => global(), Self::Own(budget) => budget }
    }
}

/// A CPU budget shared by its workers. Each permit belongs to the budget that issued it.
#[relax_api(handle, clone)]
#[derive(Clone)]
pub struct BudgetHandle(Which);

#[relax_api]
impl BudgetHandle {
    /// The budget shared by the process.
    pub fn process() -> Self { Self(Which::Process) }
    /// A separate budget with the given positive number of cores.
    pub fn of_cores(cores: u32) -> Self {
        let cores = cores as usize;
        Self(Which::Own(Arc::new(Budget::new(cores, cores))))
    }
    /// Identifies a live budget. The process budget has ID zero.
    pub fn id(&self) -> u64 {
        match &self.0 { Which::Process => 0, Which::Own(budget) => Arc::as_ptr(budget) as u64 }
    }
    pub fn cores(&self) -> u32 { self.0.get().cores as u32 }
    pub fn limit(&self) -> u32 { self.0.get().limit as u32 }
    pub fn background_limit(&self) -> u32 { self.0.get().background_limit as u32 }
    pub fn pool_cores(&self) -> u32 {
        match &self.0 { Which::Process => pool_cores(), Which::Own(budget) => budget.cores as u32 }
    }
    pub fn admit(&self, class: u8, client: u8, label: &str) -> u32 { self.0.get().admit(class, client, label) }
    /// `desired` must point to the job's live `AtomicU8` for the duration of the call.
    pub fn admit_following(&self, desired: *const u8, client: u8, label: &str) -> u32 {
        let desired = unsafe { &*(desired as *const AtomicU8) };
        self.0.get().admit_following(desired, client, label)
    }
    pub fn checkpoint(&self, permit: u32, held_locks: u32, lock_site: &str) -> u32 {
        self.0.get().checkpoint(permit, held_locks, lock_site)
    }
    pub fn should_yield(&self, permit: u32) -> bool { self.0.get().should_yield(permit) }
    pub fn release(&self, permit: u32) { self.0.get().release(permit); }
    pub fn reclass(&self, permit: u32, class: u8) { self.0.get().reclass(permit, class); }
    pub fn lend_permit(&self, permit: u32) { self.0.get().lend_permit(permit); }
    pub fn take_back_permit(&self, permit: u32) { self.0.get().take_back_permit(permit); }
    pub fn queued(&self, class: u8, client: u8, delta: i32) {
        valid(class, client);
        self.0.get().queued[class as usize][client as usize].fetch_add(delta as i64, Relaxed);
    }
    pub fn waited(&self, class: u8, client: u8, ns: u64, label: &str) { self.0.get().waited(class, client, ns, label); }
    pub fn strand_waited(&self, class: u8, ns: u64, label: &str) { self.0.get().strand_waited(class, ns, label); }
    pub fn changes(&self) -> u64 { self.0.get().lock().changes }
    pub fn wait_for_change(&self, seen: u64) -> u64 {
        match &self.0 { Which::Process => wait_for_change(seen), Which::Own(budget) => budget.wait_for_change(seen) }
    }
    pub fn tokens_text(&self, out: &mut [u8]) -> usize { write_tokens(self.0.get(), out) }
    pub fn report(&self, out: &mut [u8]) -> usize { write_report(self.0.get(), out) }
    /// A request for a permit from this budget.
    pub fn admission(&self) -> Admission { Admission { budget: self.0.clone(), ticket: None } }
}

impl BudgetHandle {
    pub fn tokens(&self) -> Tokens { read_tokens(|out| self.tokens_text(out)) }
    pub fn report_string(&self) -> String { read_text(|out| self.report(out)) }
}

/// The budget's native park token. JIT and native std have different thread-local
/// parkers; queue notifications and admission notifications must wake the same one.
#[relax_api(handle, clone)]
#[derive(Clone)]
pub struct Wake(thread::Thread);

#[relax_api]
impl Wake {
    pub fn current() -> Self { Self(thread::current()) }
    pub fn unpark(&self) { self.0.unpark(); }
}

#[relax_api]
pub fn park() { thread::park(); }

#[relax_api]
pub fn park_for(ns: u64) { thread::park_timeout(std::time::Duration::from_nanos(ns)); }

/// A wait for an [`Admission`] of `class` (its poll gave no permit): the budget's native park,
/// shown as a token wait where the host traces waits, as a wait in [`admit`] is.
#[relax_api]
pub fn park_admission(class: u8) {
    match WAIT_HOOK.get() {
        Some(hook) => hook(class),
        None => thread::park(),
    }
}

/// Admission whose class follows a native job while its worker waits. `desired` is the job's
/// `AtomicU8::as_ptr`, alive for the call: the byte crosses the Relax op boundary, std's type doesn't.
#[relax_api]
pub fn admit_following(desired: *const u8, client: u8, label: &str) -> u32 {
    BudgetHandle::process().admit_following(desired, client, label)
}

/// A native pool's admission request. The pool may change its head class while
/// waiting, without occupying a worker with a stale lower-priority admission.
/// `poll` never parks: budget changes unpark the requesting thread.
/// The request keeps its FIFO ticket across class changes and cancels on drop.
#[relax_api(handle)]
pub struct Admission { budget: Which, ticket: Option<u64> }
#[relax_api]
impl Default for Admission {
    fn default() -> Self { Self { budget: Which::Process, ticket: None } }
}
#[relax_api]
impl Admission {
    pub fn poll(&mut self, class: u8, client: u8, label: &str) -> Option<u32> {
        valid(class, client);
        let b = self.budget.get();
        let mut s = b.lock();
        let ticket = *self.ticket.get_or_insert_with(|| {
            let ticket = s.ticket;
            s.ticket = s.ticket.checked_add(1).expect("CPU budget ticket overflow");
            s.waiters.push(Waiter { class, client, ticket, thread: thread::current(), id: me() });
            ticket
        });
        let w = s.waiters.iter_mut().find(|w| w.ticket == ticket).unwrap();
        w.class = class;
        w.client = client;
        if let Some(i) = b.first(&s).filter(|&i| s.waiters[i].ticket == ticket) {
            s.waiters.swap_remove(i);
            self.ticket = None;
            Some(b.install(s, Job::new(class, client, label, ticket)))
        } else {
            b.refresh(&mut s);
            None
        }
    }
    /// `checkpoint` that never parks, on a new admission: Some(the permit to go on with), or None:
    /// the job yielded its slot, and this admission asks for it back with the job's own ticket
    /// (poll it with the job's client and label, with `park_admission` between polls).
    pub fn checkpoint(&mut self, permit: u32, held_locks: u32, lock_site: &str) -> Option<u32> {
        assert!(self.ticket.is_none(), "a checkpoint on an admission that waits already");
        let b = self.budget.get();
        let mut s = b.lock();
        let Some(job) = b.yields(&mut s, permit, held_locks, lock_site) else {
            let class = s.slots[permit as usize].as_ref().expect("released CPU permit").class;
            drop(s);
            set_thread_class(class);
            return Some(permit);
        };
        s.waiters.push(Waiter { class: job.class, client: job.client, ticket: job.ticket, thread: thread::current(), id: me() });
        self.ticket = Some(job.ticket);
        b.refresh(&mut s);
        None
    }
}
impl Drop for Admission {
    fn drop(&mut self) {
        if let Some(ticket) = self.ticket.take() {
            let b = self.budget.get();
            let mut s = b.lock();
            s.waiters.retain(|w| w.ticket != ticket);
            b.refresh(&mut s);
        }
    }
}

#[relax_api]
pub fn admit(class: u8, client: u8, label: &str) -> u32 { global().admit(class, client, label) }
#[relax_api]
pub fn should_yield(permit: u32) -> bool { global().should_yield(permit) }
/// Returns the permit to use from now on. A yielding, unlocked caller releases its
/// slot and parks with its original FIFO ticket; readmission may use a different slot.
/// With held locks it records a warning and keeps running with the existing permit.
#[relax_api]
pub fn checkpoint(permit: u32, held_locks: u32, lock_site: &str) -> u32 { global().checkpoint(permit, held_locks, lock_site) }
#[relax_api]
pub fn release(permit: u32) { global().release(permit); }
/// `Budget::end_run`
#[relax_api]
pub fn end_run() { global().end_run(); }
#[relax_api]
pub fn reclass(permit: u32, class: u8) { global().reclass(permit, class); }
/// `Budget::lend`, for the JIT: a thread asleep on JIT work or a program lock keeps no token.
/// Waits nest (a thread in a futex wait's entry compiles the futex call and waits for that build, a
/// job back from a wait holds for a pause under the wait's own guard): the outermost lends, the
/// inner ones count. True: a `take_back` ends this one.
#[relax_api]
pub fn lend() -> bool {
    if !ADMITTED.with(Cell::get) { return false; }
    let lends = LENDS.with(Cell::get);
    let lent = lends > 0 || global().lend();
    LENDS.with(|count| count.set(lends + u32::from(lent)));
    lent
}
/// `Budget::take_back`, when the outermost `lend` ends.
#[relax_api]
pub fn take_back() {
    let lends = LENDS.with(Cell::get);
    if lends == 0 { return; }
    LENDS.with(|count| count.set(lends - 1));
    if lends == 1 { global().take_back(); }
}
/// `Budget::lend_permit`.
#[relax_api]
pub fn lend_permit(permit: u32) { global().lend_permit(permit); }
/// `Budget::take_back_permit`.
#[relax_api]
pub fn take_back_permit(permit: u32) { global().take_back_permit(permit); }
/// How many times the budget changed so far: an admission, a release, a wait for a token begun or
/// ended, a lend, a class's change (whatever `tokens` and `report` show moves with it).
#[relax_api]
pub fn changes() -> u64 { global().lock().changes }
/// Blocks until the budget changed after `changes` said `seen`, and returns the count then: a wait
/// for a state of the budget looks again at each change, never by the clock. A thread with a
/// permit lends its token meanwhile (`lend`).
#[relax_api]
pub fn wait_for_change(seen: u64) -> u64 {
    let lent = lend();
    let now = global().wait_for_change(seen);
    if lent { take_back(); }
    now
}
/// One permit, as `tokens` reports it.
pub struct Holder {
    pub thread: u64,
    pub class: u8,
    pub client: u8,
    pub label: String,
    /// its thread sleeps: the token is back in the budget
    pub lent: bool,
    pub held_ns: u64,
}
/// One thread queued for a token.
pub struct Waiting { pub thread: u64, pub class: u8, pub client: u8 }
/// Who has the budget now, for the inspector (the thread ids are `relax_base::waits::me`'s).
pub struct Tokens {
    pub running: usize,
    /// what each class may reach: it is admitted below this
    pub limits: [usize; 3],
    pub holders: Vec<Holder>,
    pub waiting: Vec<Waiting>,
}
/// `tokens`, one row a line: its strings and vectors do not cross the Relax op boundary.
#[relax_api]
pub fn tokens_text(out: &mut [u8]) -> usize { write_tokens(global(), out) }
fn write_tokens(budget: &Budget, out: &mut [u8]) -> usize {
    let t = budget.tokens();
    let mut text = format!("{} {} {} {}\n", t.running, t.limits[0], t.limits[1], t.limits[2]);
    for h in &t.holders {
        text += &format!("h {} {} {} {} {} {}\n", h.thread, h.class, h.client, u8::from(h.lent), h.held_ns, h.label.replace('\n', " "));
    }
    for w in &t.waiting {
        text += &format!("w {} {} {}\n", w.thread, w.class, w.client);
    }
    fill(out, &text)
}
pub fn tokens() -> Tokens { read_tokens(tokens_text) }
fn read_tokens(op: impl Fn(&mut [u8]) -> usize) -> Tokens {
    let text = read_text(op);
    let mut lines = text.lines();
    let mut head = lines.next().unwrap().split(' ').map(|n| n.parse().unwrap());
    let mut next = || head.next().unwrap();
    let mut tokens = Tokens { running: next(), limits: [next(), next(), next()], holders: Vec::new(), waiting: Vec::new() };
    for line in lines {
        let mut fields = line.splitn(7, ' ');
        let row = fields.next().unwrap();
        let mut number = || fields.next().unwrap().parse::<u64>().unwrap();
        let (thread, class, client) = (number(), number() as u8, number() as u8);
        if row == "w" {
            tokens.waiting.push(Waiting { thread, class, client });
        } else {
            let (lent, held_ns) = (number() != 0, number());
            tokens.holders.push(Holder { thread, class, client, label: fields.next().unwrap().to_owned(), lent, held_ns });
        }
    }
    tokens
}
/// Sets what every thread's wait for a token runs: `hook(class)` calls `park`. The budget sits below
/// the JIT's wait registry, which registers those waits through this.
#[relax_api]
pub fn set_wait_hook(hook: extern "C" fn(u8)) { let _ = WAIT_HOOK.set(hook); }
/// The logical class last requested on this thread (Interactive on the UI thread). A job the
/// thread waits for inherits this class even if the thread's OS priority could not change.
#[relax_api]
pub fn thread_class() -> Option<u8> { THREAD_CLASS.with(Cell::get).map(budget_class) }
/// The class the OS was last set to on this thread, or None when a change failed or none was made.
#[relax_api]
pub fn applied_thread_class() -> Option<u8> { APPLIED_CLASS.with(Cell::get).map(budget_class) }
fn budget_class(class: u8) -> u8 {
    match class { 3 => INTERACTIVE, IDLE | BACKGROUND_HOT | BACKGROUND_BEHIND => BACKGROUND, class => class }
}
#[relax_api]
pub fn queued(class: u8, client: u8, delta: i32) {
    BudgetHandle::process().queued(class, client, delta);
}
#[relax_api]
pub fn waited(class: u8, client: u8, ns: u64, label: &str) { global().waited(class, client, ns, label); }
/// A strand's job waited `ns` behind the strand's earlier jobs: reported apart from waits for a worker.
#[relax_api]
pub fn strand_waited(class: u8, ns: u64, label: &str) { global().strand_waited(class, ns, label); }
#[relax_api]
pub fn report(out: &mut [u8]) -> usize { write_report(global(), out) }
fn write_report(budget: &Budget, out: &mut [u8]) -> usize { fill(out, &budget.report()) }
/// Native convenience wrapper; strings do not cross the Relax op boundary.
pub fn report_string() -> String { read_text(report) }
fn fill(out: &mut [u8], text: &str) -> usize {
    let mut len = text.len().min(out.len());
    while !text.is_char_boundary(len) { len -= 1; }
    out[..len].copy_from_slice(&text.as_bytes()[..len]);
    len
}
fn read_text(op: impl Fn(&mut [u8]) -> usize) -> String {
    let mut out = vec![0; 4096];
    loop {
        let len = op(&mut out);
        // Up to three bytes may be omitted to preserve a UTF-8 boundary.
        if len + 4 < out.len() { out.truncate(len); return String::from_utf8(out).unwrap(); }
        out.resize(out.len() * 2, 0);
    }
}

thread_local! {
    static THREAD_CLASS: Cell<Option<u8>> = const { Cell::new(None) };
    static APPLIED_CLASS: Cell<Option<u8>> = const { Cell::new(None) };
    static THREAD_FLOOR: Cell<bool> = const { Cell::new(false) };
    /// this thread was admitted once: a thread that never was skips the budget's lock when it sleeps
    static ADMITTED: Cell<bool> = const { Cell::new(false) };
    /// the lends in progress on this thread: above 0 its tokens are lent
    static LENDS: Cell<u32> = const { Cell::new(0) };
    /// set_ui_thread's thread: its OS class stays the UI's
    static UI_THREAD: Cell<bool> = const { Cell::new(false) };
    /// set_thread_keeps_priority: this thread's OS class stays what it is
    static KEEPS_PRIORITY: Cell<bool> = const { Cell::new(false) };
    /// this thread runs at the OS's idle policy (Linux SCHED_IDLE): set_thread_idle
    static IDLE_NOW: Cell<bool> = const { Cell::new(false) };
    /// what the Background work this thread runs is to the program: set_thread_work
    static WORK: Cell<u8> = const { Cell::new(WORK_PLAIN) };
    /// this thread's counts in the budget, taken out when it ends
    static MARKS: Marks = const { Marks { deadline: Cell::new(false), fast: Cell::new(false) } };
}
/// A thread's counts in the budget: it holds a core against a deadline (`Budget::deadlines`), its
/// tier-up builds hold a performance core (`Budget::fast_tier_ups`). A thread that ends counts no more.
struct Marks {
    deadline: Cell<bool>,
    fast: Cell<bool>,
}
impl Drop for Marks {
    fn drop(&mut self) {
        if self.deadline.get() { global().deadlines.fetch_sub(1, Relaxed); }
        if self.fast.get() { global().fast_tier_ups.fetch_sub(1, Relaxed); }
    }
}
/// Applies the class to the calling thread, caching successful applications only.
/// Linux may refuse a promotion without CAP_SYS_NICE; failure is returned, not hidden.
/// On a set_ui_thread thread it changes nothing.
#[relax_api]
pub fn set_thread_class(class: u8) -> bool {
    valid(class, APP);
    if UI_THREAD.with(Cell::get) || KEEPS_PRIORITY.with(Cell::get) { return true; }
    apply_thread_class(class)
}

/// While `on`, the calling thread keeps the OS priority it has: admission, checkpoints and
/// `set_thread_class`, `set_thread_work` and `set_thread_idle` leave it, as on the UI thread.
/// Returns the previous setting. A pool sets it on a thread it does not own for the time that
/// thread takes part in its work (its share of a fan-out, a job it claimed), and restores it after:
/// nothing would bring that thread's priority back (a Linux thread without CAP_SYS_NICE cannot climb
/// back from Background).
#[relax_api]
pub fn set_thread_keeps_priority(on: bool) -> bool { KEEPS_PRIORITY.with(|keeps| keeps.replace(on)) }

/// The UI thread, and the thread it waits on for its work (the JIT owner), are outside admission
/// and keep the UI's OS priority, above pool jobs, for good: a job's class taken on later (a scope's
/// task run here) leaves it. (A Linux thread
/// without CAP_SYS_NICE could not come back from a lower class: the owner, once it served a
/// background job's request, served the UI at that job's priority from then on.)
#[relax_api]
pub fn set_ui_thread() -> bool {
    let applied = apply_thread_class(3);
    UI_THREAD.with(|ui| ui.set(true));
    applied
}

/// Whether the calling thread was marked by [`set_ui_thread`].
#[relax_api]
pub fn on_ui_thread() -> bool { UI_THREAD.with(Cell::get) }

/// `true`: the calling thread's OS priority stays at Interactive or above on later requests (a
/// Background or idle request changes only its logical class). `false`: it may go lower again, as a
/// thread that goes back to a shared pool must.
#[relax_api]
pub fn set_thread_floor(on: bool) { THREAD_FLOOR.with(|floor| floor.set(on)); }

/// The calling thread goes below every other thread: Linux SCHED_IDLE (a core runs it only when no
/// other thread wants that core), elsewhere the Background class's priority. A thread without
/// CAP_SYS_NICE (RLIMIT_NICE 0) cannot leave SCHED_IDLE again; Background work keeps it there.
#[relax_api]
pub fn set_thread_idle() -> bool {
    if UI_THREAD.with(Cell::get) || THREAD_FLOOR.with(Cell::get) || KEEPS_PRIORITY.with(Cell::get) { return false; }
    apply_thread_class(IDLE)
}
/// The calling thread runs at the OS's idle policy (`set_thread_idle` took effect, and nothing
/// raised it since).
#[relax_api]
pub fn thread_idle() -> bool { IDLE_NOW.with(Cell::get) }
/// The OS class code of `set_thread_idle` (below Background; 3 is the UI's).
const IDLE: u8 = 4;
/// The OS class code of a tier-up build on a performance core (`WORK_TIER_UP` with room there):
/// its job's class stays Background; only the OS's class of the thread differs. macOS QoS DEFAULT:
/// at UTILITY the kernel queues it on efficiency cores beside idle performance cores.
const BACKGROUND_HOT: u8 = 5;
/// The OS class code of Background work kept off the performance cores (`WORK_BEHIND` while the
/// person's work runs, `WORK_TIER_UP` without room): macOS QoS BACKGROUND, which Apple silicon runs
/// on the efficiency cores only. Its budget class stays Background.
const BACKGROUND_BEHIND: u8 = 6;

/// What a thread's Background work is to the program (`set_thread_work`). Nobody's measured
/// demand: speculation, checks ahead, encoding, the JIT's housekeeping. macOS QoS UTILITY.
pub const WORK_PLAIN: u8 = 0;
/// The program runs slower code until this lands: a tier-up of code it was measured running, the
/// first build of code it reached. On a machine with two kinds of core it takes a performance core
/// while the tier-ups on them are fewer than the person's work leaves (`fast_room`), else an
/// efficiency core; on one kind, QoS DEFAULT, below the program's own threads.
pub const WORK_TIER_UP: u8 = 1;
/// It replaces something in use with the same kind of thing, or nothing calls it yet: a plan made
/// behind the one a live run plays on, a rebuild of code at the tier it has, a build ahead of an
/// install. Nothing waits for it, and it never takes a core from the person's work: while any of
/// that runs (`fast_room`'s demand, a deadline thread) it runs at QoS BACKGROUND, on the
/// efficiency cores of Apple silicon; else as plain work. (At UTILITY a live simulation's re-plan
/// ran 95 % of its units on a base M-series Mac's 4 performance cores beside the realtime round,
/// which ran 13 % slower and gapped, 11-22.)
pub const WORK_BEHIND: u8 = 2;

/// The Background work the calling thread runs is `kind` from now on (`WORK_PLAIN`,
/// `WORK_TIER_UP`, `WORK_BEHIND`), until its next call. Its budget class stays Background; only
/// the OS's class of the thread differs, and only on macOS: Linux keeps Background's nice and
/// SCHED_BATCH (a thread without CAP_SYS_NICE could not come back from a lower class for the pool's
/// next job), Windows Background's normal priority (strict priority would starve it), and wasm has
/// no OS classes. A thread at the idle policy, or the UI's, stays where it is. The placement is
/// read again wherever the class is applied: at admission and at every checkpoint, so a build that
/// started in a quiet moment leaves the performance cores at its next checkpoint once the person's
/// work starts. Called at a job's start and end; O(1), an OS call only on a change. An op of the
/// crate: a JIT-compiled app's pool job calls it, and the class it keeps is the provided crate's,
/// with `set_thread_class`'s.
#[relax_api]
pub fn set_thread_work(kind: u8) -> bool {
    assert!(kind <= WORK_BEHIND, "invalid kind of Background work {kind}");
    WORK.with(|w| w.set(kind));
    if KEEPS_PRIORITY.with(Cell::get) { return true; }
    match THREAD_CLASS.with(Cell::get) {
        Some(BACKGROUND | BACKGROUND_HOT | BACKGROUND_BEHIND) => apply_thread_class(BACKGROUND),
        _ => true,
    }
}
/// What the calling thread's Background work is now (`set_thread_work`).
pub fn thread_work() -> u8 { WORK.with(Cell::get) }

/// The calling thread holds a core against a deadline from now on (`true`), outside admission: a
/// live simulation's worker, an audio render thread. It counts as one core of the person's work in
/// `fast_room` until `false` or its end. O(1), no OS call. An op of the crate, as
/// `set_thread_work`.
#[relax_api]
pub fn set_thread_deadline(on: bool) {
    MARKS.with(|m| if m.deadline.replace(on) != on {
        if on { global().deadlines.fetch_add(1, Relaxed); } else { global().deadlines.fetch_sub(1, Relaxed); }
    });
}

/// The performance cores the person's work leaves now (the cores of the faster kind less the
/// Interactive and Visible jobs that run, averaged over a span, and the deadline threads); 0 on a
/// machine with one kind of core. For the budget's report, the JIT's hold ledger and tests.
#[relax_api]
pub fn fast_room() -> u32 { global().fast_room() as u32 }

/// The OS class code of Background work of the calling thread's kind (`set_thread_work`), now.
#[cfg(target_os = "macos")]
fn background_class() -> u8 {
    let budget = global();
    match WORK.with(Cell::get) {
        WORK_TIER_UP if budget.fast == 0 => BACKGROUND_HOT,
        WORK_TIER_UP => {
            // a performance core while the tier-ups on them, this one apart, are fewer than the room
            let others = budget.fast_tier_ups.load(Relaxed).saturating_sub(usize::from(MARKS.with(|m| m.fast.get())));
            if others < budget.fast_room() { BACKGROUND_HOT } else { BACKGROUND_BEHIND }
        }
        WORK_BEHIND if budget.urgent() => BACKGROUND_BEHIND,
        _ => BACKGROUND,
    }
}
/// Elsewhere every kind of Background work has Background's OS class.
#[cfg(not(target_os = "macos"))]
fn background_class() -> u8 { BACKGROUND }

static CLASS_REFUSALS: [[AtomicU64; 7]; 2] = [const { [const { AtomicU64::new(0) }; 7] }; 2];
static REFUSALS_MARKED: AtomicBool = AtomicBool::new(false);
/// Split this run's refused OS class changes at the calling event.
#[relax_api]
pub fn mark_class_refusals() { REFUSALS_MARKED.store(true, Relaxed); }
/// Counts by requested OS class, before and after the mark. Without a mark all are before.
#[relax_api]
pub fn class_refusals() -> [[u64; 7]; 2] { CLASS_REFUSALS.each_ref().map(|row| row.each_ref().map(|n| n.load(Relaxed))) }
/// The threads a refused change was counted on in this run, each once (at its first): the class
/// asked, whether it came after the mark, the thread's id and name. A refused raise is not tried
/// again, so the list stays as short as the threads that met one.
static REFUSED_THREADS: Mutex<Vec<RefusedThread>> = Mutex::new(Vec::new());
/// The run whose refusals a thread has named itself in (`end_run` starts the next).
static REFUSAL_GEN: AtomicU64 = AtomicU64::new(1);
thread_local! { static REFUSAL_NAMED: Cell<u64> = const { Cell::new(0) }; }
/// A thread whose OS class change was refused ([`refused_threads`]).
#[derive(Clone, Debug)]
pub struct RefusedThread { pub class: u8, pub after_mark: bool, pub id: thread::ThreadId, pub name: String }
fn count_refusal(class: u8) {
    let after_mark = REFUSALS_MARKED.load(Relaxed);
    CLASS_REFUSALS[after_mark as usize][class as usize].fetch_add(1, Relaxed);
    let gen = REFUSAL_GEN.load(Relaxed);
    if REFUSAL_NAMED.with(|named| named.replace(gen)) != gen {
        let me = thread::current();
        let name = me.name().unwrap_or("unnamed").to_owned();
        REFUSED_THREADS.lock().unwrap_or_else(|e| e.into_inner()).push(RefusedThread { class, after_mark, id: me.id(), name });
    }
}
/// This run's threads that met a refused OS class change, each once.
#[relax_api::relax_api(loader = "the loader reports the refused class changes (the JIT's queue): JIT code has no use for it")]
pub fn refused_threads() -> Vec<RefusedThread> { REFUSED_THREADS.lock().unwrap_or_else(|e| e.into_inner()).clone() }

fn apply_thread_class(class: u8) -> bool {
    apply_thread_class_with(class, os_class)
}
fn apply_thread_class_with(class: u8, set: impl FnOnce(u8) -> Option<bool>) -> bool {
    THREAD_CLASS.with(|logical| logical.set(Some(class)));
    let class = if THREAD_FLOOR.with(Cell::get) {
        if class == BACKGROUND || class == IDLE { return true; }
        if class == VISIBLE { INTERACTIVE } else { class }
    } else { class };
    APPLIED_CLASS.with(|current| {
        // Background work on a thread at the idle policy: it stays there, below Background (a
        // thread without CAP_SYS_NICE could not leave it anyway)
        if class == BACKGROUND && IDLE_NOW.with(Cell::get) { return true; }
        let class = if class == BACKGROUND { background_class() } else { class };
        if current.get() == Some(class) { return true; }
        let Some(applied) = set(class) else { return false };
        if !applied { count_refusal(class); }
        // A failed call may have partially changed the OS state.
        current.set(if applied { Some(class) } else { None });
        // the tier-ups that hold a performance core: counted while this thread's class is theirs
        let fast = applied && class == BACKGROUND_HOT && global().fast > 0;
        MARKS.with(|m| if m.fast.replace(fast) != fast {
            if fast { global().fast_tier_ups.fetch_add(1, Relaxed); } else { global().fast_tier_ups.fetch_sub(1, Relaxed); }
        });
        applied
    })
}
#[cfg(target_os = "macos")]
fn os_class(class: u8) -> Option<bool> {
    unsafe extern "C" { fn pthread_set_qos_class_self_np(class: u32, relative: i32) -> i32; }
    Some(unsafe { pthread_set_qos_class_self_np([0x19, 0x15, 0x11, 0x21, 0x11, 0x15, 0x09][class as usize], 0) == 0 })
}
/// None means a previous refusal already ruled out this nice on the calling thread.
#[cfg(any(test, target_os = "linux", target_os = "android"))]
fn try_nice(nice: i32, set: impl FnOnce() -> (bool, bool)) -> Option<bool> {
    thread_local! { static REFUSED: Cell<Option<i32>> = const { Cell::new(None) }; }
    REFUSED.with(|refused| {
        if refused.get().is_some_and(|limit| nice <= limit) { return None; }
        let (applied, raise_refused) = set();
        if raise_refused { refused.set(Some(nice)); }
        Some(applied)
    })
}
#[cfg(any(target_os = "linux", target_os = "android"))]
fn os_class(class: u8) -> Option<bool> {
    #[repr(C)]
    struct SchedParam { priority: i32 }
    unsafe extern "C" {
        fn getpid() -> i32;
        fn getpriority(which: i32, who: u32) -> i32;
        fn setpriority(which: i32, who: u32, priority: i32) -> i32;
        fn sched_getscheduler(pid: i32) -> i32;
        fn sched_setscheduler(pid: i32, policy: i32, param: *const SchedParam) -> i32;
    }
    static BASE: OnceLock<i32> = OnceLock::new();
    let base = *BASE.get_or_init(|| unsafe { getpriority(0, getpid() as u32) }.clamp(0, 19));
    // Visible must not lower an Interactive thread to nice 3 for good.
    let nice = (base + [1, 1, 8, 0, 8, 8, 8][class as usize]).min(19);
    try_nice(nice, || unsafe {
        let priority = setpriority(0, 0, nice) == 0;
        // SCHED_IDLE 5, SCHED_BATCH 3, SCHED_OTHER 0; a refused change leaves the policy as it was
        let policy = sched_setscheduler(0, match class { IDLE => 5, BACKGROUND | BACKGROUND_HOT | BACKGROUND_BEHIND => 3, _ => 0 }, &SchedParam { priority: 0 }) == 0;
        let idle = if policy { class == IDLE } else { sched_getscheduler(0) == 5 };
        IDLE_NOW.with(|state| state.set(idle));
        let actual = getpriority(0, 0);
        (priority && policy && actual == nice, actual > nice || (!policy && idle && class != IDLE))
    })
}
#[cfg(windows)]
fn os_class(class: u8) -> Option<bool> {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentThread() -> *mut std::ffi::c_void;
        fn SetThreadPriority(thread: *mut std::ffi::c_void, priority: i32) -> i32;
    }
    // Background at Normal, not below: Windows priority is strict, so below another program's busy threads
    // it gets almost no time (tier-up all but stopped, the program stayed on its slow code); the budget
    // decides how much of the machine Background takes
    Some(unsafe { SetThreadPriority(GetCurrentThread(), [1, 0, 0, 2, 0, 0, 0][class as usize]) != 0 })
}
#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "android", windows)))]
fn os_class(_class: u8) -> Option<bool> { Some(false) }

#[cfg(test)]
mod tests;
