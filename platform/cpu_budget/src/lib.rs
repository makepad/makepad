//! One process-wide CPU budget, shared by native and Relax callers.
//! A permit belongs to one worker; only `reclass` may be called by another thread.
//! Callers must use the permit returned by `checkpoint` and release it exactly once.
//! A permit is a row of the table, a token is one count of `running`: a thread asleep lends its
//! token (`lend`) and keeps its row, so the permit its caller holds stays good.
//! The JIT always has a job able to run: with no job of its running one is admitted over a full
//! budget, on a row that counts no token (`above`) and never yields. A long slice would run on slow
//! code the whole way otherwise, and settled work keeps its cores: nothing is reserved.
use makepad_relax_api::relax_api;
use std::cell::Cell;
use std::collections::{HashMap, VecDeque};
use std::fmt::Write;
use std::sync::atomic::{AtomicI64, AtomicU64, AtomicU8, Ordering::Relaxed};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::thread::{self, Thread};
use std::time::Instant;

pub const INTERACTIVE: u8 = 0;
pub const VISIBLE: u8 = 1;
pub const BACKGROUND: u8 = 2;
pub const APP: u8 = 0;
pub const JIT: u8 = 1;
pub const HAL: u8 = 2;
const CLASSES: [&str; 3] = ["interactive", "visible", "background"];
const BUCKETS: usize = 33;
/// Rows of the permit table: one per thread holding a permit, more than a process has threads.
const ROWS: usize = 1 << 16;
const RECENT_NS: u64 = 60_000_000_000;
/// A row's word in `Budget::classes`: its class and these.
const CLASS_BITS: u8 = 3;
const NEVER_YIELDS: u8 = 0x40;
const FORCED_YIELD: u8 = 0x80;

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
    /// the JIT's job admitted over a full budget: it holds no token
    above: bool,
}
impl Job {
    fn new(class: u8, client: u8, label: &str, ticket: u64) -> Self {
        Self { class, client, label: Label::new(label), ticket, admitted_at: 0, checkpoint_at: 0, thread: 0, lent: false, above: false }
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
    /// the JIT's jobs among the rows that run, `above` ones too
    jit_running: usize,
    background: usize,
    visible: usize,
    /// The JIT's Background jobs: running, and waiting for a permit.
    jit_background: usize,
    jit_waiting: usize,
    waiting: u8,
    parked: [u64; 3],
    admitted: [u64; 3],
    runtime: [u64; 3],
    spans: [Span; 3],
    warnings: HashMap<String, u64>,
    worst: VecDeque<Worst>,
    /// The longest recent waits behind an earlier job on the same strand (`strand_waited`).
    strand_worst: VecDeque<Worst>,
    #[cfg(test)]
    changed: Option<std::sync::mpsc::Sender<()>>,
}
/// The JIT's Background jobs Visible work makes room for while they wait, counting the one JIT job that always runs:
/// two permits. A cold slice runs on the code those jobs compile: starved of them it stays on slow code, and takes
/// longer at more CPU.
const JIT_FLOOR: usize = 3;
struct Budget {
    origin: Instant,
    cores: usize,
    limit: usize,
    /// Background's ceiling: below Visible's, and at most the physical cores but one. A background
    /// build on an SMT sibling of a busy core takes far more CPU for the same work (a cold plate on
    /// 8 cores / 16 threads: the same 990 O3 builds took 31.6 CPU-s 8 wide, 58.5 CPU-s 13 wide)
    /// and gets the work done little sooner.
    background_limit: usize,
    /// JIT_FLOOR, within a quarter of the limit: on a small machine it would leave Visible nothing.
    jit_floor: usize,
    state: Mutex<State>,
    classes: Box<[AtomicU8]>,
    // A precomputed bit per yielding class: should_yield needs just two relaxed loads.
    yield_classes: AtomicU8,
    // ... and per class that a waiter of its own class or above (or a budget over its cores) wants
    // a slot of: `contended`.
    contended_classes: AtomicU8,
    // 0: not begun, 1: startup warm-up, 2: permanently ended.
    warmup: AtomicU8,
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
            origin: Instant::now(), cores, limit,
            background_limit: limit.saturating_sub(2).min(physical.saturating_sub(1)).max(1),
            jit_floor: JIT_FLOOR.min(limit / 4),
            state: Mutex::new(State {
                slots: vec![None; cores], waiters: Vec::new(), ticket: 0,
                running: 0, jit_running: 0, background: 0, visible: 0, jit_background: 0, jit_waiting: 0, waiting: 0, parked: [0; 3], admitted: [0; 3],
                runtime: [0; 3], spans: [Span::default(); 3], warnings: HashMap::new(),
                worst: VecDeque::new(),
                strand_worst: VecDeque::new(),
                #[cfg(test)]
                changed: None,
            }),
            classes: (0..ROWS).map(|_| AtomicU8::new(INTERACTIVE)).collect(),
            yield_classes: AtomicU8::new(0), contended_classes: AtomicU8::new(0), warmup: AtomicU8::new(0),
            queued: std::array::from_fn(|_| std::array::from_fn(|_| AtomicI64::new(0))),
            waits: std::array::from_fn(|_| std::array::from_fn(|_| AtomicU64::new(0))),
            max_wait: std::array::from_fn(|_| AtomicU64::new(0)),
            strand_waits: std::array::from_fn(|_| AtomicU64::new(0)),
            max_strand_wait: AtomicU64::new(0),
        }
    }
    fn now(&self) -> u64 { self.origin.elapsed().as_nanos().min(u64::MAX as u128) as u64 }
    /// How far the JIT's Background jobs are below their floor: it holds only against Visible work.
    fn jit_short(&self, s: &State) -> usize {
        if s.visible == 0 && s.waiting & (1 << VISIBLE) == 0 { return 0; }
        self.jit_floor.saturating_sub(s.jit_background)
    }
    fn class_limit(&self, s: &State, class: u8, client: u8) -> usize {
        match class {
            INTERACTIVE => self.cores,
            VISIBLE => self.limit - self.jit_short(s).min(s.jit_waiting),
            BACKGROUND if client == JIT && self.jit_short(s) > 0 => self.limit,
            BACKGROUND => self.background_limit,
            _ => unreachable!(),
        }
    }
    fn room(&self, s: &State, class: u8, client: u8) -> bool {
        client == JIT && s.jit_running == 0
            || s.running < self.class_limit(s, class, client) && (class != BACKGROUND || client != APP || !self.warming() || s.background == 0)
    }
    fn first(&self, s: &State) -> Option<usize> {
        s.waiters.iter().enumerate().filter(|(_, w)| self.room(s, w.class, w.client)).min_by_key(|(_, w)| (w.class, w.ticket)).map(|(i, _)| i)
    }
    fn refresh(&self, s: &mut State) {
        s.waiting = s.waiters.iter().fold(0, |bits, w| bits | (1 << w.class));
        s.jit_waiting = s.waiters.iter().filter(|w| w.class == BACKGROUND && w.client == JIT).count();
        let (mut bits, mut contended) = (0, 0);
        for class in 0..3 {
            if s.waiting & ((1 << class) - 1) != 0 || s.running > self.cores
            { bits |= 1 << class; }
            if s.waiting & ((2 << class) - 1) != 0 || s.running > self.cores
            { contended |= 1 << class; }
        }
        if s.running >= self.limit && self.jit_short(s).min(s.jit_waiting) > 0 { bits |= 1 << VISIBLE; }
        self.yield_classes.store(bits, Relaxed);
        self.contended_classes.store(contended, Relaxed);
        for (index, job) in s.slots.iter().enumerate() {
            if let Some(job) = job {
                self.classes[index].store(job.class | if job.class == BACKGROUND && job.client == APP
                    && self.warming() && s.background > 1 { FORCED_YIELD } else { 0 } | if job.above { NEVER_YIELDS } else { 0 }, Relaxed);
            }
        }
        if let Some(i) = self.first(s) {
            if self.room(s, s.waiters[i].class, s.waiters[i].client) { s.waiters[i].thread.unpark(); }
        }
        #[cfg(test)]
        if let Some(tx) = &s.changed { let _ = tx.send(()); }
    }
    fn acquire<'a>(&'a self, mut s: MutexGuard<'a, State>, mut job: Job, desired: Option<&AtomicU8>) -> u32 {
        let key = (job.class, job.ticket);
        let first = self.first(&s);
        let immediate = self.room(&s, job.class, job.client)
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
        job.above = job.client == JIT && s.running >= self.class_limit(&s, job.class, job.client);
        ADMITTED.with(|admitted| admitted.set(true));
        s.slots[index] = Some(job);
        Self::start_counting(&mut s, &job);
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
    fn should_yield(&self, permit: u32) -> bool {
        let class = self.classes[permit as usize].load(Relaxed);
        class & NEVER_YIELDS == 0 && (class & FORCED_YIELD != 0 || self.yield_classes.load(Relaxed) & (1 << (class & CLASS_BITS)) != 0)
    }
    fn contended(&self, permit: u32) -> bool {
        let class = self.classes[permit as usize].load(Relaxed);
        class & FORCED_YIELD != 0 || self.contended_classes.load(Relaxed) & (1 << (class & CLASS_BITS)) != 0
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
        if !job.lent { Self::stop_counting(s, &job, now); }
        job
    }
    fn start_counting(s: &mut State, job: &Job) {
        s.running += usize::from(!job.above);
        s.jit_running += usize::from(job.client == JIT);
        s.background += usize::from(job.class == BACKGROUND && job.client == APP);
        s.visible += usize::from(job.class == VISIBLE);
        s.jit_background += usize::from(job.class == BACKGROUND && job.client == JIT);
    }
    fn stop_counting(s: &mut State, job: &Job, now: u64) {
        s.running -= usize::from(!job.above);
        s.jit_running -= usize::from(job.client == JIT);
        s.background -= usize::from(job.class == BACKGROUND && job.client == APP);
        s.visible -= usize::from(job.class == VISIBLE);
        s.jit_background -= usize::from(job.class == BACKGROUND && job.client == JIT);
        s.runtime[job.class as usize] += now.saturating_sub(job.admitted_at);
        Self::span(s, job.label, now.saturating_sub(job.checkpoint_at));
    }
    /// The calling thread goes to sleep on other work: the tokens of its permits go back to the
    /// budget, its rows stay. A compile job's permit is not lent: it moves between threads
    /// (the owner borrows it), and its job gives it up and is admitted again.
    /// True: it lent one, and `take_back` ends that.
    fn lend(&self) -> bool {
        let me = me();
        let mut s = self.lock();
        let now = self.now();
        let mut lent = false;
        for i in 0..s.slots.len() {
            let Some(job) = s.slots[i].as_mut().filter(|j| j.thread == me && j.client != JIT && !j.lent) else { continue };
            job.lent = true;
            let job = *job;
            Self::stop_counting(&mut s, &job, now);
            lent = true;
        }
        if lent { self.refresh(&mut s); }
        lent
    }
    /// A compile job's token goes back while its request waits for another build on the owner: on
    /// a machine with one token that build gets no other. Its row and permit stay.
    fn lend_permit(&self, permit: u32) {
        let mut s = self.lock();
        let now = self.now();
        let job = s.slots[permit as usize].as_mut().expect("lent a released CPU permit");
        assert!(!job.lent, "lent a CPU permit twice");
        job.lent = true;
        let job = *job;
        Self::stop_counting(&mut s, &job, now);
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
                Self::count_again(&mut s, i, now);
                back = true;
            }
        }
        if back { self.refresh(&mut s); }
    }
    /// `take_back` of `lend_permit`: its job has not run meanwhile, and the same goes.
    fn take_back_permit(&self, permit: u32) {
        let mut s = self.lock();
        let now = self.now();
        Self::count_again(&mut s, permit as usize, now);
        self.refresh(&mut s);
    }
    fn count_again(s: &mut State, index: usize, now: u64) {
        let job = s.slots[index].as_mut().expect("took back a released CPU permit");
        job.lent = false;
        job.admitted_at = now;
        job.checkpoint_at = now;
        let job = *job;
        Self::start_counting(s, &job);
    }
    fn tokens(&self) -> Tokens {
        let s = self.lock();
        let now = self.now();
        let holders = s.slots.iter().flatten().map(|j| Holder {
            thread: j.thread, class: j.class, client: j.client, label: j.label.text().to_owned(), lent: j.lent, above: j.above,
            held_ns: now.saturating_sub(j.admitted_at),
        }).collect();
        let waiting = s.waiters.iter().map(|w| Waiting { thread: w.id, class: w.class, client: w.client }).collect();
        Tokens { running: s.running, limits: [self.cores, self.limit, self.background_limit], holders, waiting }
    }
    fn checkpoint(&self, permit: u32, held_locks: u32, lock_site: &str) -> u32 {
        self.checkpoint_following(permit, held_locks, lock_site, None)
    }
    fn checkpoint_following(&self, permit: u32, held_locks: u32, lock_site: &str, desired: Option<&AtomicU8>) -> u32 {
        let mut s = self.lock();
        let now = self.now();
        let job = s.slots[permit as usize].as_mut().expect("released CPU permit");
        let span = Span { label: job.label, ns: now.saturating_sub(job.checkpoint_at) };
        job.checkpoint_at = now;
        let class = job.class;
        Self::span(&mut s, span.label, span.ns);
        if self.should_yield(permit) {
            if held_locks != 0 {
                if let Some(count) = s.warnings.get_mut(lock_site) { *count += 1; }
                else { s.warnings.insert(lock_site.to_owned(), 1); }
            } else {
                let job = self.remove(&mut s, permit, now);
                s.parked[class as usize] += 1;
                // Release and enqueue under the same lock; retain the original admission ticket.
                return self.acquire(s, job, desired);
            }
        }
        drop(s);
        set_thread_class(class);
        permit
    }
    /// A run of the app ended in place (`makepad run`): the threads of the app's and the JIT's rows
    /// are gone and never release them. The HAL's stay, its threads with them.
    fn end_run(&self) {
        let mut s = self.lock();
        let now = self.now();
        for i in 0..s.slots.len() {
            if s.slots[i].is_some_and(|j| j.client != HAL) {
                self.remove(&mut s, i as u32, now);
            }
        }
        s.waiters.retain(|w| w.client == HAL);
        for class in &self.queued {
            for (client, queued) in class.iter().enumerate() {
                if client != HAL as usize {
                    queued.store(0, Relaxed);
                }
            }
        }
        self.warmup.store(0, Relaxed);
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
        let client = job.client;
        let app = client == APP;
        job.class = class;
        job.admitted_at = now;
        s.runtime[old as usize] += elapsed;
        s.background -= usize::from(old == BACKGROUND && app);
        s.background += usize::from(class == BACKGROUND && app);
        s.visible -= usize::from(old == VISIBLE);
        s.visible += usize::from(class == VISIBLE);
        s.jit_background -= usize::from(old == BACKGROUND && client == JIT);
        s.jit_background += usize::from(class == BACKGROUND && client == JIT);
        self.classes[permit as usize].store(class, Relaxed);
        self.refresh(&mut s);
    }
    fn warming(&self) -> bool { self.warmup.load(Relaxed) == 1 }
    fn begin_warmup(&self) {
        let mut s = self.lock();
        // Only JIT startup may begin it. Input before startup also closes the latch.
        if self.warmup.compare_exchange(0, 1, Relaxed, Relaxed).is_ok() {
            self.refresh(&mut s);
        }
    }
    fn end_warmup(&self) {
        // Input dispatch stays atomic-only after the first interaction/settle.
        if self.warmup.load(Relaxed) == 2 { return; }
        let mut s = self.lock();
        self.warmup.store(2, Relaxed);
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
        let above = s.slots.iter().flatten().filter(|j| j.above && !j.lent).count();
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
        let warm = self.warming();
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
        if above != 0 { let _ = write!(out, ", {above} JIT job above the budget"); }
        let _ = write!(out, ", warming: {}", if warm { "yes" } else { "no" });
        if let Some(w) = worst {
            let _ = write!(out, ", worst: {} {:?} waited {}", CLASSES[w.class as usize], w.label.text(), human(w.ns));
            if let Some((class, span)) = w.culprit {
                let _ = write!(out, " -- {} {:?} ran {} without a checkpoint", CLASSES[class as usize], span.label.text(), human(span.ns));
            }
        }
        out.push_str("\nclass        running app/jit/hal  queued  wait p50 / p95 / max  parked  admitted  run time\n");
        for class in 0..3 {
            let hist = std::array::from_fn::<_, BUCKETS, _>(|i| self.waits[class][i].load(Relaxed));
            let queued: i64 = self.queued[class].iter().map(|q| q.load(Relaxed)).sum();
            let _ = writeln!(out, "{:<12} {} / {} / {}  {}  {} / {} / {}  {}  {}  {}", CLASSES[class],
                running[class][0], running[class][1], running[class][2], queued,
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
fn valid(class: u8, client: u8) { assert!(class <= BACKGROUND && client <= HAL, "invalid CPU budget class/client"); }
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
static WAIT_HOOK: OnceLock<extern "C" fn(u8)> = OnceLock::new();
fn global() -> &'static Budget {
    static BUDGET: OnceLock<Budget> = OnceLock::new();
    BUDGET.get_or_init(|| {
        let cores = thread::available_parallelism().map_or(1, usize::from);
        // (a machine whose topology is not known: every CPU a core)
        Budget::new(cores, match physical_cores() { 0 => cores, n => n.min(cores) })
    })
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

#[relax_api]
pub fn cores() -> u32 { global().cores as u32 }
#[relax_api]
pub fn limit() -> u32 { global().limit as u32 }
/// How many Background jobs run at once.
#[relax_api]
pub fn background_limit() -> u32 { global().background_limit as u32 }
/// How many of the JIT's Background jobs Visible work leaves room for: `JIT_FLOOR`, on this machine.
#[relax_api]
pub fn jit_floor() -> u32 { global().jit_floor as u32 }

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

/// Admission whose class follows a native job while its worker waits. `desired` is the job's
/// `AtomicU8::as_ptr`, alive for the call: the byte crosses the Relax op boundary, std's type doesn't.
#[relax_api]
pub fn admit_following(desired: *const u8, client: u8, label: &str) -> u32 {
    let desired = unsafe { &*(desired as *const AtomicU8) };
    let b = global();
    let class = desired.load(Relaxed);
    valid(class, client);
    let mut s = b.lock();
    let ticket = s.ticket;
    s.ticket = s.ticket.checked_add(1).expect("CPU budget ticket overflow");
    b.acquire(s, Job::new(class, client, label, ticket), Some(desired))
}

/// Native workers whose waiters may promote them while they are parked. Like
/// `checkpoint`, with a live class and an unpark from the promoting waiter.
#[relax_api]
pub fn checkpoint_following(permit: u32, held_locks: u32, lock_site: &str, desired: *const u8) -> u32 {
    let desired = unsafe { &*(desired as *const AtomicU8) };
    global().checkpoint_following(permit, held_locks, lock_site, Some(desired))
}

/// A native pool's admission request. The pool may change its head class while
/// waiting, without occupying a worker with a stale lower-priority admission.
/// `poll` never parks: budget changes unpark the requesting thread.
/// The request keeps its FIFO ticket across class changes and cancels on drop.
#[relax_api(handle)]
pub struct Admission { ticket: Option<u64> }
#[relax_api]
impl Default for Admission {
    fn default() -> Self { Self { ticket: None } }
}
#[relax_api]
impl Admission {
    pub fn poll(&mut self, class: u8, client: u8, label: &str) -> Option<u32> {
        valid(class, client);
        let b = global();
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
}
impl Drop for Admission {
    fn drop(&mut self) {
        if let Some(ticket) = self.ticket.take() {
            let b = global();
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
/// A finished job's permit goes back rather than on to the worker's next job of its class: a
/// waiter of that class or above wants a slot, or it should yield (lock-free).
#[relax_api]
pub fn contended(permit: u32) -> bool { global().contended(permit) }
/// Returns the permit to use from now on. A yielding, unlocked caller releases its
/// slot and parks with its original FIFO ticket; readmission may use a different slot.
/// With held locks it records a warning and keeps running with the existing permit.
#[relax_api]
pub fn checkpoint(permit: u32, held_locks: u32, lock_site: &str) -> u32 { global().checkpoint(permit, held_locks, lock_site) }
#[relax_api]
pub fn release(permit: u32) { global().release(permit); }
/// `Budget::end_run`
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
/// One permit, as `tokens` reports it.
pub struct Holder {
    pub thread: u64,
    pub class: u8,
    pub client: u8,
    pub label: String,
    /// its thread sleeps: the token is back in the budget
    pub lent: bool,
    /// the JIT's one job over a full budget: it holds no token
    pub above: bool,
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
pub fn tokens_text(out: &mut [u8]) -> usize {
    let t = global().tokens();
    let mut text = format!("{} {} {} {}\n", t.running, t.limits[0], t.limits[1], t.limits[2]);
    for h in &t.holders {
        text += &format!("h {} {} {} {} {} {} {}\n", h.thread, h.class, h.client, u8::from(h.lent), u8::from(h.above), h.held_ns, h.label.replace('\n', " "));
    }
    for w in &t.waiting {
        text += &format!("w {} {} {}\n", w.thread, w.class, w.client);
    }
    fill(out, &text)
}
pub fn tokens() -> Tokens {
    let text = read_text(tokens_text);
    let mut lines = text.lines();
    let mut head = lines.next().unwrap().split(' ').map(|n| n.parse().unwrap());
    let mut next = || head.next().unwrap();
    let mut tokens = Tokens { running: next(), limits: [next(), next(), next()], holders: Vec::new(), waiting: Vec::new() };
    for line in lines {
        let mut fields = line.splitn(8, ' ');
        let row = fields.next().unwrap();
        let mut number = || fields.next().unwrap().parse::<u64>().unwrap();
        let (thread, class, client) = (number(), number() as u8, number() as u8);
        if row == "w" {
            tokens.waiting.push(Waiting { thread, class, client });
        } else {
            let (lent, above, held_ns) = (number() != 0, number() != 0, number());
            tokens.holders.push(Holder { thread, class, client, label: fields.next().unwrap().to_owned(), lent, above, held_ns });
        }
    }
    tokens
}
/// Sets what every thread's wait for a token runs: `hook(class)` calls `park`. The budget sits below
/// the JIT's wait registry, which registers those waits through this.
#[relax_api]
pub fn set_wait_hook(hook: extern "C" fn(u8)) { let _ = WAIT_HOOK.set(hook); }
/// The class `set_thread_class` last gave the calling thread (the UI thread: Interactive); None: it
/// set none, or a promotion failed. A job the thread waits for runs at its class meanwhile.
#[relax_api]
pub fn thread_class() -> Option<u8> { THREAD_CLASS.with(Cell::get).map(|class| match class { 3 => INTERACTIVE, IDLE => BACKGROUND, class => class }) }
#[relax_api]
pub fn queued(class: u8, client: u8, delta: i32) {
    valid(class, client);
    global().queued[class as usize][client as usize].fetch_add(delta as i64, Relaxed);
}
#[relax_api]
pub fn waited(class: u8, client: u8, ns: u64, label: &str) { global().waited(class, client, ns, label); }
/// A strand's job waited `ns` behind the strand's earlier jobs: reported apart from waits for a worker.
#[relax_api]
pub fn strand_waited(class: u8, ns: u64, label: &str) { global().strand_waited(class, ns, label); }
#[relax_api]
pub fn begin_warmup() { global().begin_warmup(); }
/// Permanently restore the normal budget at first settle or real user input.
#[relax_api]
pub fn end_warmup() { global().end_warmup(); }
#[relax_api]
pub fn warming() -> bool { global().warming() }
#[relax_api]
pub fn report(out: &mut [u8]) -> usize { fill(out, &global().report()) }
/// Native convenience wrapper; strings do not cross the Relax op boundary.
pub fn report_string() -> String { read_text(report) }
fn fill(out: &mut [u8], text: &str) -> usize {
    let mut len = text.len().min(out.len());
    while !text.is_char_boundary(len) { len -= 1; }
    out[..len].copy_from_slice(&text.as_bytes()[..len]);
    len
}
fn read_text(op: fn(&mut [u8]) -> usize) -> String {
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
    /// this thread was admitted once: a thread that never was skips the budget's lock when it sleeps
    static ADMITTED: Cell<bool> = const { Cell::new(false) };
    /// the lends in progress on this thread: above 0 its tokens are lent
    static LENDS: Cell<u32> = const { Cell::new(0) };
    /// set_ui_thread's thread: its OS class stays the UI's
    static UI_THREAD: Cell<bool> = const { Cell::new(false) };
    /// this thread runs at the OS's idle policy (Linux SCHED_IDLE): set_thread_idle
    static IDLE_NOW: Cell<bool> = const { Cell::new(false) };
}
/// Applies the class to the calling thread, caching successful applications only.
/// Linux may refuse a promotion without CAP_SYS_NICE; failure is returned, not hidden.
/// On a set_ui_thread thread it changes nothing.
#[relax_api]
pub fn set_thread_class(class: u8) -> bool {
    valid(class, APP);
    if UI_THREAD.with(Cell::get) { return true; }
    apply_thread_class(class)
}

/// The UI thread, and the thread it waits on for its work (the JIT owner), are outside admission
/// and keep the UI's OS priority, above pool jobs, for good: a job's class taken on later (a scope's
/// task run here, a pool job's admission borrowed for one request) leaves it. (A Linux thread
/// without CAP_SYS_NICE could not come back from a lower class: the owner, once it served a
/// background job's request, served the UI at that job's priority from then on.)
#[relax_api]
pub fn set_ui_thread() -> bool {
    let applied = apply_thread_class(3);
    UI_THREAD.with(|ui| ui.set(true));
    applied
}

/// The calling thread goes below every other thread: Linux SCHED_IDLE (a core runs it only when no
/// other thread wants that core), elsewhere the Background class's priority. A thread without
/// CAP_SYS_NICE (RLIMIT_NICE 0) cannot leave SCHED_IDLE again; Background work keeps it there.
pub fn set_thread_idle() -> bool {
    if UI_THREAD.with(Cell::get) { return false; }
    apply_thread_class(IDLE)
}
/// The calling thread runs at the OS's idle policy (`set_thread_idle` took effect, and nothing
/// raised it since).
pub fn thread_idle() -> bool { IDLE_NOW.with(Cell::get) }
/// The OS class code of `set_thread_idle` (below Background; 3 is the UI's).
const IDLE: u8 = 4;

fn apply_thread_class(class: u8) -> bool {
    THREAD_CLASS.with(|current| {
        if current.get() == Some(class) { return true; }
        // Background work on a thread at the idle policy: it stays there, below Background (a
        // thread without CAP_SYS_NICE could not leave it anyway)
        if class == BACKGROUND && IDLE_NOW.with(Cell::get) { return true; }
        let applied = os_class(class);
        // A failed call may have partially changed the OS state.
        current.set(if applied { Some(class) } else { None });
        applied
    })
}
#[cfg(target_os = "macos")]
fn os_class(class: u8) -> bool {
    unsafe extern "C" { fn pthread_set_qos_class_self_np(class: u32, relative: i32) -> i32; }
    unsafe { pthread_set_qos_class_self_np([0x19, 0x15, 0x11, 0x21, 0x11][class as usize], 0) == 0 }
}
#[cfg(any(target_os = "linux", target_os = "android"))]
fn os_class(class: u8) -> bool {
    #[repr(C)]
    struct SchedParam { priority: i32 }
    unsafe extern "C" {
        fn getpid() -> i32;
        fn getpriority(which: i32, who: u32) -> i32;
        fn setpriority(which: i32, who: u32, priority: i32) -> i32;
        fn sched_setscheduler(pid: i32, policy: i32, param: *const SchedParam) -> i32;
    }
    static BASE: OnceLock<i32> = OnceLock::new();
    let base = *BASE.get_or_init(|| unsafe { getpriority(0, getpid() as u32) }.clamp(0, 19));
    let nice = (base + [1, 3, 8, 0, 8][class as usize]).min(19);
    unsafe {
        let priority = setpriority(0, 0, nice) == 0;
        // SCHED_IDLE 5, SCHED_BATCH 3, SCHED_OTHER 0; a refused change leaves the policy as it was
        let policy = sched_setscheduler(0, match class { IDLE => 5, BACKGROUND => 3, _ => 0 }, &SchedParam { priority: 0 }) == 0;
        if policy { IDLE_NOW.with(|idle| idle.set(class == IDLE)); }
        priority && policy && getpriority(0, 0) == nice
    }
}
#[cfg(windows)]
fn os_class(class: u8) -> bool {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentThread() -> *mut std::ffi::c_void;
        fn SetThreadPriority(thread: *mut std::ffi::c_void, priority: i32) -> i32;
    }
    // Background at Normal, not below: Windows priority is strict, so below another program's busy threads
    // it gets almost no time (tier-up all but stopped, the program stayed on its slow code); the budget
    // decides how much of the machine Background takes
    unsafe { SetThreadPriority(GetCurrentThread(), [1, 0, 0, 2, 0][class as usize]) != 0 }
}
#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "android", windows)))]
fn os_class(_class: u8) -> bool { false }

#[cfg(test)]
mod tests;
