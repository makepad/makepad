//! One process-wide CPU budget, shared by native and Relax callers.
//! A permit belongs to one worker; only `reclass` may be called by another thread.
//! Callers must use the permit returned by `checkpoint` and release it exactly once.
use makepad_relax_api::relax_api;
use std::cell::Cell;
use std::collections::{HashMap, VecDeque};
use std::fmt::Write;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicU8, Ordering::Relaxed};
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
}
struct Waiter { class: u8, ticket: u64, thread: Thread }
#[derive(Clone, Copy, Default)]
struct Span { label: Label, ns: u64 }
#[derive(Clone, Copy)]
struct Worst { class: u8, label: Label, ns: u64, when: u64, culprit: Option<(u8, Span)> }
struct State {
    slots: Box<[Option<Job>]>,
    waiters: Vec<Waiter>,
    ticket: u64,
    running: usize,
    background: usize,
    waiting: u8,
    parked: [u64; 3],
    admitted: [u64; 3],
    runtime: [u64; 3],
    spans: [Span; 3],
    warnings: HashMap<String, u64>,
    worst: VecDeque<Worst>,
    #[cfg(test)]
    changed: Option<std::sync::mpsc::Sender<()>>,
}
struct Budget {
    origin: Instant,
    cores: usize,
    limit: usize,
    state: Mutex<State>,
    classes: Box<[AtomicU8]>,
    // A precomputed bit per yielding class: should_yield needs just two relaxed loads.
    yield_classes: AtomicU8,
    warming: AtomicBool,
    queued: [[AtomicI64; 3]; 3],
    waits: [[AtomicU64; BUCKETS]; 3],
    max_wait: [AtomicU64; 3],
}
impl Budget {
    fn new(cores: usize) -> Self {
        assert!(cores > 0);
        Self {
            origin: Instant::now(), cores, limit: cores.saturating_sub(1).max(1),
            state: Mutex::new(State {
                slots: vec![None; cores].into_boxed_slice(), waiters: Vec::new(), ticket: 0,
                running: 0, background: 0, waiting: 0, parked: [0; 3], admitted: [0; 3],
                runtime: [0; 3], spans: [Span::default(); 3], warnings: HashMap::new(),
                worst: VecDeque::new(),
                #[cfg(test)]
                changed: None,
            }),
            classes: (0..cores).map(|_| AtomicU8::new(INTERACTIVE)).collect(),
            yield_classes: AtomicU8::new(0), warming: AtomicBool::new(false),
            queued: std::array::from_fn(|_| std::array::from_fn(|_| AtomicI64::new(0))),
            waits: std::array::from_fn(|_| std::array::from_fn(|_| AtomicU64::new(0))),
            max_wait: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }
    fn now(&self) -> u64 { self.origin.elapsed().as_nanos().min(u64::MAX as u128) as u64 }
    fn room(&self, s: &State, class: u8) -> bool {
        s.running < match class {
            INTERACTIVE => self.cores,
            VISIBLE => self.limit,
            BACKGROUND => self.limit.saturating_sub(2).max(1),
            _ => unreachable!(),
        } && (class != BACKGROUND || !self.warming.load(Relaxed) || s.background == 0)
    }
    fn first(s: &State) -> Option<usize> {
        s.waiters.iter().enumerate().min_by_key(|(_, w)| (w.class, w.ticket)).map(|(i, _)| i)
    }
    fn refresh(&self, s: &mut State) {
        s.waiting = s.waiters.iter().fold(0, |bits, w| bits | (1 << w.class));
        let mut bits = 0;
        for class in 0..3 {
            if s.waiting & ((1 << class) - 1) != 0 || s.running > self.cores
                || (class == BACKGROUND && self.warming.load(Relaxed) && s.background > 1)
            { bits |= 1 << class; }
        }
        self.yield_classes.store(bits, Relaxed);
        if let Some(i) = Self::first(s) {
            if self.room(s, s.waiters[i].class) { s.waiters[i].thread.unpark(); }
        }
        #[cfg(test)]
        if let Some(tx) = &s.changed { let _ = tx.send(()); }
    }
    fn acquire<'a>(&'a self, mut s: MutexGuard<'a, State>, mut job: Job) -> u32 {
        let key = (job.class, job.ticket);
        let first = Self::first(&s);
        let immediate = self.room(&s, job.class)
            && first.is_none_or(|i| key < (s.waiters[i].class, s.waiters[i].ticket));
        if !immediate {
            // Only the contended path grows the waiter list. No thread is created here.
            s.waiters.push(Waiter { class: job.class, ticket: job.ticket, thread: thread::current() });
            self.refresh(&mut s);
            loop {
                let i = Self::first(&s).unwrap();
                if s.waiters[i].ticket == job.ticket && self.room(&s, job.class) {
                    s.waiters.swap_remove(i);
                    break;
                }
                drop(s);
                thread::park();
                s = self.state.lock().unwrap();
            }
        }
        let index = s.slots.iter().position(Option::is_none).unwrap();
        let now = self.now();
        job.admitted_at = now;
        job.checkpoint_at = now;
        s.slots[index] = Some(job);
        s.running += 1;
        s.background += usize::from(job.class == BACKGROUND);
        s.admitted[job.class as usize] += 1;
        self.classes[index].store(job.class, Relaxed);
        self.refresh(&mut s);
        drop(s);
        set_thread_class(job.class);
        index as u32
    }
    fn admit(&self, class: u8, client: u8, label: &str) -> u32 {
        valid(class, client);
        let mut s = self.state.lock().unwrap();
        let ticket = s.ticket;
        s.ticket = s.ticket.checked_add(1).expect("CPU budget ticket overflow");
        self.acquire(s, Job { class, client, label: Label::new(label), ticket, admitted_at: 0, checkpoint_at: 0 })
    }
    fn should_yield(&self, permit: u32) -> bool {
        self.yield_classes.load(Relaxed) & (1 << self.classes[permit as usize].load(Relaxed)) != 0
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
        s.running -= 1;
        s.background -= usize::from(job.class == BACKGROUND);
        s.runtime[job.class as usize] += now.saturating_sub(job.admitted_at);
        Self::span(s, job.label, now.saturating_sub(job.checkpoint_at));
        job
    }
    fn checkpoint(&self, permit: u32, held_locks: u32, lock_site: &str) -> u32 {
        let mut s = self.state.lock().unwrap();
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
                return self.acquire(s, job);
            }
        }
        drop(s);
        set_thread_class(class);
        permit
    }
    fn release(&self, permit: u32) {
        let mut s = self.state.lock().unwrap();
        self.remove(&mut s, permit, self.now());
        self.refresh(&mut s);
    }
    fn reclass(&self, permit: u32, class: u8) {
        valid(class, APP);
        let mut s = self.state.lock().unwrap();
        let now = self.now();
        let job = s.slots[permit as usize].as_mut().expect("released CPU permit");
        let old = job.class;
        let elapsed = now.saturating_sub(job.admitted_at);
        job.class = class;
        job.admitted_at = now;
        s.runtime[old as usize] += elapsed;
        s.background -= usize::from(old == BACKGROUND);
        s.background += usize::from(class == BACKGROUND);
        self.classes[permit as usize].store(class, Relaxed);
        self.refresh(&mut s);
    }
    fn set_warming(&self, on: bool) {
        let mut s = self.state.lock().unwrap();
        self.warming.store(on, Relaxed);
        self.refresh(&mut s);
    }
    fn waited(&self, class: u8, client: u8, ns: u64, label: &str) {
        valid(class, client);
        let us = ns.div_ceil(1000).max(1);
        let bucket = (64 - (us - 1).leading_zeros() as usize).min(BUCKETS - 1);
        self.waits[class as usize][bucket].fetch_add(1, Relaxed);
        self.max_wait[class as usize].fetch_max(ns, Relaxed);
        if ns <= 100_000_000 { return; }
        let mut s = self.state.lock().unwrap();
        let now = self.now();
        let culprit = s.slots.iter().flatten()
            .max_by_key(|j| (j.class, now.saturating_sub(j.checkpoint_at)))
            .map(|j| (j.class, Span { label: j.label, ns: now.saturating_sub(j.checkpoint_at) }));
        // A monotone deque retains exactly the possible maxima of the rolling minute.
        while s.worst.front().is_some_and(|w| now.saturating_sub(w.when) > RECENT_NS) { s.worst.pop_front(); }
        while s.worst.back().is_some_and(|w| w.ns <= ns) { s.worst.pop_back(); }
        s.worst.push_back(Worst { class, label: Label::new(label), ns, when: now, culprit });
    }
    fn report(&self) -> String {
        let s = self.state.lock().unwrap();
        let now = self.now();
        let mut running = [[0u32; 3]; 3];
        let mut spans = s.spans;
        for job in s.slots.iter().flatten() {
            running[job.class as usize][job.client as usize] += 1;
            let ns = now.saturating_sub(job.checkpoint_at);
            if let Some(span) = spans.iter_mut().find(|span| span.label == job.label) { span.ns = span.ns.max(ns); }
            else {
                let span = spans.iter_mut().min_by_key(|span| span.ns).unwrap();
                if ns > span.ns { *span = Span { label: job.label, ns }; }
            }
        }
        let total = s.running;
        let warm = self.warming.load(Relaxed);
        let worst = s.worst.iter().find(|w| now.saturating_sub(w.when) <= RECENT_NS).copied();
        let parked = s.parked;
        let admitted = s.admitted;
        let runtime = s.runtime;
        let mut warnings: Vec<_> = s.warnings.iter().map(|(site, count)| (site.clone(), *count)).collect();
        drop(s);
        warnings.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        spans.sort_by_key(|span| std::cmp::Reverse(span.ns));
        let mut out = format!("cpu {total}/{} running", self.limit);
        if total > self.limit { let _ = write!(out, " (+{} over)", total - self.limit); }
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
        for span in spans { if span.ns > 0 { let _ = writeln!(out, "no checkpoint for: {} {}", span.label.text(), human(span.ns)); } }
        for (site, count) in warnings.into_iter().take(8) { let _ = writeln!(out, "checkpoint under lock: {site} x{count}"); }
        out
    }
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
fn global() -> &'static Budget {
    static BUDGET: OnceLock<Budget> = OnceLock::new();
    BUDGET.get_or_init(|| Budget::new(thread::available_parallelism().map_or(1, usize::from)))
}

#[relax_api]
pub fn cores() -> u32 { global().cores as u32 }
#[relax_api]
pub fn limit() -> u32 { global().limit as u32 }
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
#[relax_api]
pub fn reclass(permit: u32, class: u8) { global().reclass(permit, class); }
#[relax_api]
pub fn queued(class: u8, client: u8, delta: i32) {
    valid(class, client);
    global().queued[class as usize][client as usize].fetch_add(delta as i64, Relaxed);
}
#[relax_api]
pub fn waited(class: u8, client: u8, ns: u64, label: &str) { global().waited(class, client, ns, label); }
#[relax_api]
pub fn set_warming(on: bool) { global().set_warming(on); }
#[relax_api]
pub fn warming() -> bool { global().warming.load(Relaxed) }
#[relax_api]
pub fn report(out: &mut [u8]) -> usize {
    let text = global().report();
    let mut len = text.len().min(out.len());
    while !text.is_char_boundary(len) { len -= 1; }
    out[..len].copy_from_slice(&text.as_bytes()[..len]);
    len
}
/// Native convenience wrapper; strings do not cross the Relax op boundary.
pub fn report_string() -> String {
    let mut out = vec![0; 4096];
    loop {
        let len = report(&mut out);
        // Up to three bytes may be omitted to preserve a UTF-8 boundary.
        if len + 4 < out.len() { out.truncate(len); return String::from_utf8(out).unwrap(); }
        out.resize(out.len() * 2, 0);
    }
}

thread_local! { static THREAD_CLASS: Cell<Option<u8>> = const { Cell::new(None) }; }
/// Applies the class to the calling thread, caching successful applications only.
/// Linux may refuse a promotion without CAP_SYS_NICE; failure is returned, not hidden.
#[relax_api]
pub fn set_thread_class(class: u8) -> bool {
    valid(class, APP);
    THREAD_CLASS.with(|current| {
        if current.get() == Some(class) { return true; }
        let applied = os_class(class);
        // A failed call may have partially changed the OS state.
        current.set(if applied { Some(class) } else { None });
        applied
    })
}
#[cfg(target_os = "macos")]
fn os_class(class: u8) -> bool {
    unsafe extern "C" { fn pthread_set_qos_class_self_np(class: u32, relative: i32) -> i32; }
    unsafe { pthread_set_qos_class_self_np([0x19, 0x15, 0x11][class as usize], 0) == 0 }
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
    let nice = (base + [1, 3, 8][class as usize]).min(19);
    unsafe {
        let priority = setpriority(0, 0, nice) == 0;
        let policy = sched_setscheduler(0, if class == BACKGROUND { 3 } else { 0 }, &SchedParam { priority: 0 }) == 0;
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
    unsafe { SetThreadPriority(GetCurrentThread(), [1, 0, -1][class as usize]) != 0 }
}
#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "android", windows)))]
fn os_class(_class: u8) -> bool { false }

#[cfg(test)]
mod tests;
