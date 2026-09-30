//! Kernel jobs on a worker pool: priorities, cancellation, a watchdog, the
//! async job API with "keep the last result" for realtime and a
//! synchronous run for locked-time export.
//!
//! The pool is the host's: Makepad's `TaskPool` (Heavy lane) through
//! [`FnExecutor`], or [`ThreadExecutor`] headless. This crate stays std
//! only; the host wires the pool in a few lines:
//!
//! ```ignore
//! let pool = cx.task_pool();
//! let (p1, p2) = (pool.clone(), pool.clone());
//! let exec = FnExecutor::new(
//!     pool.heavy_workers(),
//!     move |task| p1.submit_internal(Lane::Heavy, move || task()).is_ok(),
//!     move |n, f| p2.fan_out(Lane::Heavy, n, |w| f(w)),
//! );
//! let sched = Scheduler::new(Arc::new(exec), SchedulerConfig::default());
//! ```
//!
//! A job is a [`Job`]: a kernel, its params and owned buffers, and its
//! run state, all allocated once and reused: submit it, take it back when
//! it is done (outputs inside), change what changed, submit it again. No
//! thread is spawned per call and a job in steady state allocates nothing
//! (the host pool may box its task; [`ThreadExecutor`] does not).
//!
//! - **Priorities** ([`Priority`]) are served in order. At most
//!   `max_running` jobs run at once (each fans out over the workers); a
//!   MustComplete or Near job arriving while they are all taken pre-empts
//!   a running Far or Cosmetic job: it is cancelled through its cancel
//!   word (polled every element) and re-queued, keeping its place.
//! - **Admission** ([`crate::admission`]) happens before a job is queued;
//!   its ticket (the ledger share, the work limit, the deadline) is held
//!   until the job ends.
//! - **The watchdog** (one thread per scheduler, parked while nothing
//!   runs) cancels a running job at its deadline; the job fails with
//!   [`JobError::TimedOut`].
//! - **Realtime** uses a [`Stream`]: two jobs in rotation; while one runs
//!   the last completed one stays readable, and a request made while a
//!   job is still running is a dropped geometry frame (counted).
//! - **Locked time** uses [`Scheduler::run_sync`]: the job runs on the
//!   calling thread with the workers' help, under the same admission and
//!   watchdog, and returns only when it is done.
//!
//! Results never depend on the scheduling: chunks are fixed, chunk results
//! combine in chunk order, and four-wide code is bit-identical to scalar.
//!
//! GPU compute (later) dispatches the same AIR program; a job's buffers and
//! chunks map to a dispatch, so nothing here assumes CPU memory beyond the
//! bindings.

use crate::admission::{JobBudget, Ledger, Origin, Refused, Ticket};
use crate::kernel::{check, run_chunks, Access, ChunkCell, Kernel, KernelError, Mode, RunStats, CHUNK, K_COUNT, K_PARAMS, K_SEED, K_TIME};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, Weak};
use std::time::{Duration, Instant};

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

// =========================================================================
// Executors
// =========================================================================

/// Where kernel work runs.
pub trait Executor: Send + Sync {
    /// Workers that can help a fan-out (not counting the caller).
    fn workers(&self) -> usize;
    /// Starts `task` on a worker (never on the caller); false when the pool
    /// refuses it.
    fn spawn(&self, task: Arc<dyn Fn() + Send + Sync>) -> bool;
    /// Runs `f(w)` for every `w` in `0..n` on the caller and the workers,
    /// returning when every call has returned.
    fn fan_out(&self, n: usize, f: &(dyn Fn(usize) + Sync));
}

/// Runs everything on the caller (no workers; `spawn` is refused).
pub struct InlineExecutor;

impl Executor for InlineExecutor {
    fn workers(&self) -> usize {
        0
    }
    fn spawn(&self, _task: Arc<dyn Fn() + Send + Sync>) -> bool {
        false
    }
    fn fan_out(&self, n: usize, f: &(dyn Fn(usize) + Sync)) {
        for w in 0..n {
            f(w);
        }
    }
}

/// An executor from two host closures (Makepad's `TaskPool`: see the
/// module docs).
pub struct FnExecutor<S, F> {
    workers: usize,
    spawn: S,
    fan_out: F,
}

impl<S, F> FnExecutor<S, F>
where
    S: Fn(Arc<dyn Fn() + Send + Sync>) -> bool + Send + Sync,
    F: Fn(usize, &(dyn Fn(usize) + Sync)) + Send + Sync,
{
    pub fn new(workers: usize, spawn: S, fan_out: F) -> Self {
        FnExecutor { workers, spawn, fan_out }
    }
}

impl<S, F> Executor for FnExecutor<S, F>
where
    S: Fn(Arc<dyn Fn() + Send + Sync>) -> bool + Send + Sync,
    F: Fn(usize, &(dyn Fn(usize) + Sync)) + Send + Sync,
{
    fn workers(&self) -> usize {
        self.workers
    }
    fn spawn(&self, task: Arc<dyn Fn() + Send + Sync>) -> bool {
        (self.spawn)(task)
    }
    fn fan_out(&self, n: usize, f: &(dyn Fn(usize) + Sync)) {
        (self.fan_out)(n, f)
    }
}

/// Concurrent fan-outs a [`ThreadExecutor`] serves (more run on their
/// callers alone).
const BATCHES: usize = 8;

/// A `&dyn Fn(usize)` with its lifetime erased. Only dereferenced by
/// helpers that joined its batch while it was open; the caller waits for
/// all of them before its borrow ends.
#[derive(Clone, Copy)]
struct ErasedFn(*const (dyn Fn(usize) + Sync + 'static));
// SAFETY: it points at a `dyn Fn + Sync`, used only as `&` (see above).
unsafe impl Send for ErasedFn {}

struct Batch {
    n: AtomicUsize,
    next: AtomicUsize,
}

struct TeState {
    tasks: VecDeque<Arc<dyn Fn() + Send + Sync>>,
    open: [Option<ErasedFn>; BATCHES],
    used: [bool; BATCHES],
    helpers: [usize; BATCHES],
    shutdown: bool,
}

struct TeInner {
    state: Mutex<TeState>,
    work: Condvar,
    idle: Condvar,
    batches: [Batch; BATCHES],
    workers: usize,
}

/// A headless pool: worker threads started once, parked when idle.
/// Neither spawning a task nor a fan-out allocates.
pub struct ThreadExecutor {
    inner: Arc<TeInner>,
}

impl ThreadExecutor {
    pub fn new(workers: usize) -> ThreadExecutor {
        let inner = Arc::new(TeInner {
            state: Mutex::new(TeState { tasks: VecDeque::with_capacity(256), open: [None; BATCHES], used: [false; BATCHES], helpers: [0; BATCHES], shutdown: false }),
            work: Condvar::new(),
            idle: Condvar::new(),
            batches: std::array::from_fn(|_| Batch { n: AtomicUsize::new(0), next: AtomicUsize::new(0) }),
            workers,
        });
        for k in 0..workers {
            let inner = inner.clone();
            let _ = std::thread::Builder::new().name(format!("kernel-worker-{}", k)).spawn(move || worker(&inner));
        }
        ThreadExecutor { inner }
    }

    /// The process-wide pool for [`crate::kernel::Call::run_parallel`]:
    /// the machine's parallelism less one (the caller works too).
    pub fn shared() -> &'static ThreadExecutor {
        static SHARED: OnceLock<ThreadExecutor> = OnceLock::new();
        SHARED.get_or_init(|| ThreadExecutor::new(std::thread::available_parallelism().map_or(3, |n| n.get()).saturating_sub(1).max(1)))
    }
}

impl Drop for ThreadExecutor {
    fn drop(&mut self) {
        lock(&self.inner.state).shutdown = true;
        self.inner.work.notify_all();
    }
}

fn claim(b: &Batch, f: &(dyn Fn(usize) + Sync)) {
    loop {
        let w = b.next.fetch_add(1, Ordering::AcqRel);
        if w >= b.n.load(Ordering::Acquire) {
            return;
        }
        f(w);
    }
}

fn worker(inner: &TeInner) {
    let mut st = lock(&inner.state);
    loop {
        if st.shutdown {
            return;
        }
        if let Some(task) = st.tasks.pop_front() {
            drop(st);
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| task()));
            drop(task);
            st = lock(&inner.state);
            continue;
        }
        let open = (0..BATCHES).find(|k| st.open[*k].is_some() && inner.batches[*k].next.load(Ordering::Acquire) < inner.batches[*k].n.load(Ordering::Acquire));
        if let Some(k) = open {
            let f = st.open[k].unwrap();
            st.helpers[k] += 1;
            drop(st);
            // SAFETY: the batch is open, so its caller is inside fan_out and
            // waits for this helper before its borrow of f ends.
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| claim(&inner.batches[k], unsafe { &*f.0 })));
            st = lock(&inner.state);
            st.helpers[k] -= 1;
            if st.helpers[k] == 0 {
                inner.idle.notify_all();
            }
            continue;
        }
        st = inner.work.wait(st).unwrap_or_else(|e| e.into_inner());
    }
}

/// Closes a batch and waits for its helpers (also when the caller's own
/// work unwinds).
struct BatchGuard<'a> {
    inner: &'a TeInner,
    k: usize,
}

impl Drop for BatchGuard<'_> {
    fn drop(&mut self) {
        let mut st = lock(&self.inner.state);
        st.open[self.k] = None;
        while st.helpers[self.k] > 0 {
            st = self.inner.idle.wait(st).unwrap_or_else(|e| e.into_inner());
        }
        st.used[self.k] = false;
    }
}

impl Executor for ThreadExecutor {
    fn workers(&self) -> usize {
        self.inner.workers
    }

    fn spawn(&self, task: Arc<dyn Fn() + Send + Sync>) -> bool {
        let mut st = lock(&self.inner.state);
        if st.shutdown || self.inner.workers == 0 {
            return false;
        }
        st.tasks.push_back(task);
        drop(st);
        self.inner.work.notify_one();
        true
    }

    fn fan_out(&self, n: usize, f: &(dyn Fn(usize) + Sync)) {
        let inner = &*self.inner;
        if n <= 1 || inner.workers == 0 {
            for w in 0..n {
                f(w);
            }
            return;
        }
        let mut st = lock(&inner.state);
        let Some(k) = (0..BATCHES).find(|k| !st.used[*k]) else {
            drop(st);
            for w in 0..n {
                f(w);
            }
            return;
        };
        st.used[k] = true;
        inner.batches[k].n.store(n, Ordering::Release);
        inner.batches[k].next.store(0, Ordering::Release);
        // SAFETY: lifetime erasure; BatchGuard waits for every helper that
        // joined before `f`'s borrow ends (see ErasedFn).
        let erased: &'static (dyn Fn(usize) + Sync) = unsafe { std::mem::transmute::<&(dyn Fn(usize) + Sync), &'static (dyn Fn(usize) + Sync)>(f) };
        st.open[k] = Some(ErasedFn(erased as *const _));
        drop(st);
        let _guard = BatchGuard { inner, k };
        if n - 1 >= inner.workers {
            inner.work.notify_all();
        } else {
            for _ in 0..n - 1 {
                inner.work.notify_one();
            }
        }
        claim(&inner.batches[k], f);
    }
}

// =========================================================================
// Jobs
// =========================================================================

/// Priority classes, served in this order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Priority {
    /// Load gating (a world waiting to be ready), export.
    MustComplete = 0,
    /// Visible and near.
    Near = 1,
    /// Visible and far.
    Far = 2,
    /// Nice to have.
    Cosmetic = 3,
}

/// Why a job did not produce a result.
#[derive(Clone, Debug, PartialEq)]
pub enum JobError {
    /// Admission refused it (nothing ran).
    Refused(Refused),
    /// The kernel call failed (unbound or undersized buffers, ...).
    Kernel(KernelError),
    /// Cancelled by its owner.
    Cancelled,
    /// The watchdog stopped it at its deadline.
    TimedOut,
    /// The scheduler is gone or its pool refused the work.
    Closed,
}

impl std::fmt::Display for JobError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JobError::Refused(r) => write!(f, "refused: {}", r),
            JobError::Kernel(e) => write!(f, "{}", e),
            JobError::Cancelled => write!(f, "cancelled"),
            JobError::TimedOut => write!(f, "stopped by the watchdog at its deadline"),
            JobError::Closed => write!(f, "the scheduler is closed"),
        }
    }
}

enum Binding {
    None,
    InF32(Arc<[f32]>),
    InU32(Arc<[u32]>),
    OutF32(Vec<f32>),
    OutU32(Vec<u32>),
}

const IDLE: u8 = 0;
const QUEUED: u8 = 1;
const RUNNING: u8 = 2;
const DONE: u8 = 3;

const NO_REASON: u8 = 0;
const USER: u8 = 1;
const PREEMPT: u8 = 2;
const TIMEOUT: u8 = 3;

/// What the scheduler and the job's handle share.
struct Slot {
    /// The kernel's control word (host buffer 0): non-zero stops it at the
    /// next element.
    cancel: AtomicU32,
    reason: AtomicU8,
    state: AtomicU8,
    priority: AtomicU8,
    /// Nanoseconds since the scheduler's epoch; u64::MAX: none.
    deadline: AtomicU64,
    job: Mutex<Option<Job>>,
    result: Mutex<Option<Result<(), JobError>>>,
    done: Condvar,
    ticket: Mutex<Option<Ticket>>,
}

impl Slot {
    fn new() -> Arc<Slot> {
        Arc::new(Slot {
            cancel: AtomicU32::new(0),
            reason: AtomicU8::new(NO_REASON),
            state: AtomicU8::new(IDLE),
            priority: AtomicU8::new(Priority::Near as u8),
            deadline: AtomicU64::new(u64::MAX),
            job: Mutex::new(None),
            result: Mutex::new(None),
            done: Condvar::new(),
            ticket: Mutex::new(None),
        })
    }
}

/// A kernel invocation that owns its buffers and run state; reusable.
pub struct Job {
    kernel: Arc<Kernel>,
    ctx: Vec<u32>,
    bind: Vec<Binding>,
    count: usize,
    work_limit: u64,
    table: Vec<u64>,
    lens: Vec<usize>,
    cells: Vec<ChunkCell>,
    stats: RunStats,
    slot: Arc<Slot>,
    simd: bool,
}

impl Job {
    /// A job of `count` elements with every param at its default.
    pub fn new(kernel: Arc<Kernel>, count: usize) -> Job {
        let mut ctx = vec![0u32; kernel.ctx_words()];
        for (k, p) in kernel.params().iter().enumerate() {
            ctx[K_PARAMS as usize + k] = p.default.to_bits();
        }
        let nb = kernel.buffers().len();
        let chunks = count.div_ceil(CHUNK);
        Job {
            ctx,
            bind: (0..nb).map(|_| Binding::None).collect(),
            count,
            work_limit: crate::kernel::DEFAULT_WORK_LIMIT,
            table: Vec::with_capacity(2 * nb.max(2)),
            lens: Vec::with_capacity(nb),
            cells: (0..chunks).map(|_| ChunkCell::default()).collect(),
            stats: RunStats { reduced: Vec::with_capacity(16), ..Default::default() },
            slot: Slot::new(),
            kernel,
            simd: true,
        }
    }

    pub fn kernel(&self) -> &Arc<Kernel> {
        &self.kernel
    }

    pub fn count(&self) -> usize {
        self.count
    }

    pub fn set_count(&mut self, count: usize) {
        self.count = count;
    }

    pub fn set_param(&mut self, name: &str, value: f32) -> bool {
        match self.kernel.param_index(name) {
            Some(k) => {
                let p = &self.kernel.params()[k];
                let v = if value.is_finite() { value.clamp(p.min, p.max) } else { p.default };
                self.ctx[K_PARAMS as usize + k] = v.to_bits();
                true
            }
            None => false,
        }
    }

    pub fn set_time(&mut self, t: f32) {
        self.ctx[K_TIME as usize] = t.to_bits();
    }

    pub fn set_seed(&mut self, seed: u32) {
        self.ctx[K_SEED as usize] = seed;
    }

    /// Allows (default) or forbids the four-wide code (tests).
    pub fn set_simd(&mut self, on: bool) {
        self.simd = on;
    }

    fn index(&self, name: &str, write: bool) -> Result<usize, KernelError> {
        let k = self.kernel.buffer_index(name).filter(|k| *k > 0).ok_or_else(|| KernelError::NoSuchBuffer(name.into()))?;
        if !write && self.kernel.buffers()[k].access != Access::Read {
            return Err(KernelError::ReadOnly(name.into()));
        }
        Ok(k)
    }

    /// Binds a read-only input (shared, never written).
    pub fn input(&mut self, name: &str, data: Arc<[f32]>) -> Result<(), KernelError> {
        let k = self.index(name, false)?;
        self.bind[k] = Binding::InF32(data);
        Ok(())
    }

    pub fn input_u32(&mut self, name: &str, data: Arc<[u32]>) -> Result<(), KernelError> {
        let k = self.index(name, false)?;
        self.bind[k] = Binding::InU32(data);
        Ok(())
    }

    /// Binds an output the job owns (returned with the finished job).
    pub fn output(&mut self, name: &str, data: Vec<f32>) -> Result<(), KernelError> {
        let k = self.index(name, true)?;
        self.bind[k] = Binding::OutF32(data);
        Ok(())
    }

    pub fn output_u32(&mut self, name: &str, data: Vec<u32>) -> Result<(), KernelError> {
        let k = self.index(name, true)?;
        self.bind[k] = Binding::OutU32(data);
        Ok(())
    }

    /// An output's current contents.
    pub fn out(&self, name: &str) -> Option<&[f32]> {
        match self.bind.get(self.kernel.buffer_index(name)?)? {
            Binding::OutF32(v) => Some(v),
            _ => None,
        }
    }

    pub fn out_u32(&self, name: &str) -> Option<&[u32]> {
        match self.bind.get(self.kernel.buffer_index(name)?)? {
            Binding::OutU32(v) => Some(v),
            _ => None,
        }
    }

    /// Takes an output buffer out of the job (it becomes unbound).
    pub fn take_output(&mut self, name: &str) -> Option<Vec<f32>> {
        let k = self.kernel.buffer_index(name)?;
        match std::mem::replace(&mut self.bind[k], Binding::None) {
            Binding::OutF32(v) => Some(v),
            other => {
                self.bind[k] = other;
                None
            }
        }
    }

    /// The last run's stats.
    pub fn stats(&self) -> &RunStats {
        &self.stats
    }

    /// Fills the buffer table (buffer 0: this job's cancel word).
    fn table(&mut self) -> Result<(), KernelError> {
        self.table.clear();
        self.lens.clear();
        for (k, b) in self.bind.iter_mut().enumerate() {
            let (p, len) = match b {
                _ if k == 0 => (self.slot.cancel.as_ptr(), 1),
                Binding::None => return Err(KernelError::Unbound(self.kernel.buffers()[k].name.clone())),
                Binding::InF32(a) => (a.as_ptr() as *mut u32, a.len()),
                Binding::InU32(a) => (a.as_ptr() as *mut u32, a.len()),
                Binding::OutF32(v) => (v.as_mut_ptr() as *mut u32, v.len()),
                Binding::OutU32(v) => (v.as_mut_ptr(), v.len()),
            };
            if len == 0 {
                return Err(KernelError::Empty(self.kernel.buffers()[k].name.clone()));
            }
            if len > i32::MAX as usize {
                return Err(KernelError::TooLarge(self.kernel.buffers()[k].name.clone()));
            }
            self.table.push(p as u64);
            self.table.push(len as u64);
            self.lens.push(len);
        }
        while self.table.len() < 4 {
            self.table.push(0);
        }
        Ok(())
    }

    /// Runs the job now on the caller and up to `threads` workers of
    /// `exec` (no queue, no admission: [`Scheduler::run_sync`] adds them).
    pub fn run(&mut self, exec: &dyn Executor, threads: usize) -> Result<&RunStats, KernelError> {
        let t0 = Instant::now();
        self.table()?;
        let count = self.count;
        let split = threads > 1 && self.kernel.parallel_safe && count > CHUNK;
        let wide_ok = check(&self.kernel, &self.lens, count, true, self.work_limit).is_ok();
        check(&self.kernel, &self.lens, count, split, self.work_limit)?;
        let mode = if self.simd && self.kernel.simd() && self.kernel.parallel_safe && wide_ok { Mode::Vector } else { Mode::Scalar };
        let chunks = count.div_ceil(CHUNK);
        if self.cells.len() < chunks {
            self.cells.resize_with(chunks, ChunkCell::default);
        }
        self.ctx[K_COUNT as usize] = count as u32;
        let (overflowed, reduced) = run_chunks(&self.kernel, &self.ctx, &self.table, &self.lens, count, mode, &self.slot.cancel, &self.cells, exec, if split { threads } else { 1 })?;
        let lanes = self.kernel.reduce_parts().1;
        self.stats.elements = count;
        self.stats.overflowed = overflowed;
        self.stats.reduced.clear();
        self.stats.reduced.extend_from_slice(&reduced[..lanes]);
        self.stats.nanos = t0.elapsed().as_nanos() as u64;
        Ok(&self.stats)
    }
}

/// A submitted job.
pub struct JobHandle {
    slot: Arc<Slot>,
}

impl JobHandle {
    pub fn is_done(&self) -> bool {
        self.slot.state.load(Ordering::Acquire) == DONE
    }

    /// The finished job (its outputs inside), or None while it runs. Never
    /// blocks. The job comes back with its error when it failed.
    pub fn try_take(&self) -> Option<Result<Job, (Job, JobError)>> {
        if !self.is_done() {
            return None;
        }
        let job = lock(&self.slot.job).take()?;
        let r = lock(&self.slot.result).take().unwrap_or(Ok(()));
        self.slot.state.store(IDLE, Ordering::Release);
        Some(match r {
            Ok(()) => Ok(job),
            Err(e) => Err((job, e)),
        })
    }

    /// Blocks until the job is done (for synchronous and MustComplete
    /// callers off the UI thread).
    pub fn wait(self) -> Result<Job, (Job, JobError)> {
        let mut g = lock(&self.slot.job);
        while self.slot.state.load(Ordering::Acquire) != DONE {
            g = self.slot.done.wait(g).unwrap_or_else(|e| e.into_inner());
        }
        drop(g);
        self.try_take().expect("a done job is parked in its slot")
    }

    /// Stops the job at its next element (or before it starts).
    pub fn cancel(&self) {
        let _ = self.slot.reason.compare_exchange(NO_REASON, USER, Ordering::AcqRel, Ordering::Relaxed);
        self.slot.cancel.store(1, Ordering::Release);
    }
}

// =========================================================================
// The scheduler
// =========================================================================

#[derive(Clone)]
pub struct SchedulerConfig {
    /// Jobs running at once (each fans out over the workers).
    pub max_running: usize,
    /// Workers one job may use (the caller included).
    pub threads: usize,
    /// The device ledger admission charges (None: the process ledger).
    pub ledger: Option<Ledger>,
    /// Queued jobs per class before `submit` refuses (preallocated).
    pub queue_capacity: usize,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        SchedulerConfig { max_running: 2, threads: 8, ledger: None, queue_capacity: 256 }
    }
}

/// Counters for the stats overlay.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SchedStats {
    pub queued: [usize; 4],
    pub running: usize,
    pub completed: u64,
    pub failed: u64,
    pub cancelled: u64,
    pub timed_out: u64,
    pub preempted: u64,
    pub refused: u64,
    /// Wall time of the last finished job and the 95th percentile of the
    /// last 64, milliseconds.
    pub last_ms: f64,
    pub p95_ms: f64,
}

struct SState {
    queues: [VecDeque<Arc<Slot>>; 4],
    running: Vec<Arc<Slot>>,
    drivers: usize,
    closed: bool,
    stats: SchedStats,
    times: [u32; 64],
    times_at: usize,
}

struct SInner {
    exec: Arc<dyn Executor>,
    config: SchedulerConfig,
    state: Mutex<SState>,
    epoch: Instant,
    driver: OnceLock<Arc<dyn Fn() + Send + Sync>>,
    watchdog: Mutex<Option<std::thread::Thread>>,
    watchdog_alive: AtomicBool,
}

impl SInner {
    fn now_ns(&self) -> u64 {
        self.epoch.elapsed().as_nanos() as u64
    }

    fn ledger(&self) -> &Ledger {
        self.config.ledger.as_ref().unwrap_or_else(|| Ledger::device())
    }

    fn wake_watchdog(&self) {
        if let Some(t) = lock(&self.watchdog).as_ref() {
            t.unpark();
        }
    }
}

/// Runs kernel jobs on an executor (see the module docs).
pub struct Scheduler {
    inner: Arc<SInner>,
}

impl Scheduler {
    pub fn new(exec: Arc<dyn Executor>, config: SchedulerConfig) -> Scheduler {
        let cap = config.queue_capacity.max(1);
        let inner = Arc::new(SInner {
            exec,
            state: Mutex::new(SState {
                queues: std::array::from_fn(|_| VecDeque::with_capacity(cap)),
                running: Vec::with_capacity(config.max_running.max(1) + 1),
                drivers: 0,
                closed: false,
                stats: SchedStats::default(),
                times: [0; 64],
                times_at: 0,
            }),
            config,
            epoch: Instant::now(),
            driver: OnceLock::new(),
            watchdog: Mutex::new(None),
            watchdog_alive: AtomicBool::new(false),
        });
        let weak = Arc::downgrade(&inner);
        let _ = inner.driver.set(Arc::new(move || drive(&weak)));
        // One watchdog thread for the scheduler's lifetime (parked while
        // nothing runs). Without threads, deadlines are checked when a job
        // starts and cancel only through its owner.
        let weak = Arc::downgrade(&inner);
        if let Ok(h) = std::thread::Builder::new().name("kernel-watchdog".into()).spawn(move || watchdog(weak)) {
            *lock(&inner.watchdog) = Some(h.thread().clone());
            inner.watchdog_alive.store(true, Ordering::Release);
        }
        Scheduler { inner }
    }

    pub fn stats(&self) -> SchedStats {
        let st = lock(&self.inner.state);
        let mut s = st.stats.clone();
        for p in 0..4 {
            s.queued[p] = st.queues[p].len();
        }
        s.running = st.running.len();
        s
    }

    /// Admits and queues a job. On refusal the job comes back.
    pub fn submit(&self, mut job: Job, priority: Priority, origin: Origin, budget: JobBudget) -> Result<JobHandle, (Job, JobError)> {
        let inner = &self.inner;
        let threads = inner.config.threads.min(inner.exec.workers() + 1).max(1);
        let ticket = match inner.ledger().admit(&job.kernel, job.count, threads, origin, &budget) {
            Ok(t) => t,
            Err(r) => {
                lock(&inner.state).stats.refused += 1;
                return Err((job, JobError::Refused(r)));
            }
        };
        job.work_limit = ticket.work_limit();
        let slot = job.slot.clone();
        if slot.state.load(Ordering::Acquire) != IDLE {
            // Still owned by an earlier handle (not taken back).
            return Err((job, JobError::Closed));
        }
        slot.cancel.store(0, Ordering::Release);
        slot.reason.store(NO_REASON, Ordering::Release);
        slot.priority.store(priority as u8, Ordering::Release);
        let deadline = ticket.deadline().saturating_duration_since(inner.epoch).as_nanos() as u64;
        slot.deadline.store(deadline, Ordering::Release);
        *lock(&slot.ticket) = Some(ticket);
        *lock(&slot.result) = None;
        *lock(&slot.job) = Some(job);
        slot.state.store(QUEUED, Ordering::Release);
        let mut st = lock(&inner.state);
        if st.closed || st.queues[priority as usize].len() >= inner.config.queue_capacity {
            drop(st);
            let job = lock(&slot.job).take().unwrap();
            *lock(&slot.ticket) = None;
            slot.state.store(IDLE, Ordering::Release);
            return Err((job, JobError::Closed));
        }
        st.queues[priority as usize].push_back(slot.clone());
        let full = st.drivers >= inner.config.max_running.max(1);
        if full && priority <= Priority::Near {
            // Pre-empt one running far or cosmetic job.
            if let Some(victim) = st.running.iter().filter(|s| s.priority.load(Ordering::Acquire) >= Priority::Far as u8).max_by_key(|s| s.priority.load(Ordering::Acquire)) {
                if victim.reason.compare_exchange(NO_REASON, PREEMPT, Ordering::AcqRel, Ordering::Relaxed).is_ok() {
                    victim.cancel.store(1, Ordering::Release);
                }
            }
        }
        let start = !full;
        if start {
            st.drivers += 1;
        }
        drop(st);
        if start {
            let driver = inner.driver.get().unwrap().clone();
            if !inner.exec.spawn(driver) {
                // The pool refused: the job waits for the next driver.
                lock(&inner.state).drivers -= 1;
            }
        }
        Ok(JobHandle { slot })
    }

    /// Moves a job's deadline earlier (a frame deadline, a host timeout):
    /// the watchdog stops it at `now + within` if it is still running.
    pub fn tighten(&self, h: &JobHandle, within: Duration) {
        let d = self.inner.now_ns().saturating_add(within.as_nanos() as u64);
        h.slot.deadline.fetch_min(d, Ordering::AcqRel);
        self.inner.wake_watchdog();
    }

    /// Runs a job to completion on the calling thread (with the workers'
    /// help): admission, the watchdog and the result are the same as for a
    /// submitted job, and it is never queued behind others. For locked-time
    /// export; never on the UI thread.
    pub fn run_sync(&self, job: &mut Job, origin: Origin, budget: JobBudget) -> Result<(), JobError> {
        let inner = &self.inner;
        let threads = inner.config.threads.min(inner.exec.workers() + 1).max(1);
        let ticket = inner.ledger().admit(&job.kernel, job.count, threads, origin, &budget).map_err(|r| {
            lock(&inner.state).stats.refused += 1;
            JobError::Refused(r)
        })?;
        job.work_limit = ticket.work_limit();
        let slot = job.slot.clone();
        slot.cancel.store(0, Ordering::Release);
        slot.reason.store(NO_REASON, Ordering::Release);
        slot.priority.store(Priority::MustComplete as u8, Ordering::Release);
        slot.deadline.store(ticket.deadline().saturating_duration_since(inner.epoch).as_nanos() as u64, Ordering::Release);
        lock(&inner.state).running.push(slot.clone());
        inner.wake_watchdog();
        let r = job.run(&*inner.exec, threads).map(|_| ());
        let mut st = lock(&inner.state);
        st.running.retain(|s| !Arc::ptr_eq(s, &slot));
        let r = finish(&mut st, &slot, r, job.stats.nanos);
        drop(st);
        drop(ticket);
        r
    }
}

impl Drop for Scheduler {
    fn drop(&mut self) {
        let mut st = lock(&self.inner.state);
        st.closed = true;
        for q in &mut st.queues {
            for slot in q.drain(..) {
                *lock(&slot.ticket) = None;
                *lock(&slot.result) = Some(Err(JobError::Closed));
                slot.state.store(DONE, Ordering::Release);
                slot.done.notify_all();
            }
        }
        for slot in &st.running {
            let _ = slot.reason.compare_exchange(NO_REASON, USER, Ordering::AcqRel, Ordering::Relaxed);
            slot.cancel.store(1, Ordering::Release);
        }
        drop(st);
        self.inner.wake_watchdog();
    }
}

/// Maps a run's outcome to the job's result and counts it.
fn finish(st: &mut SState, slot: &Slot, r: Result<(), KernelError>, nanos: u64) -> Result<(), JobError> {
    let r = match r {
        Ok(()) => {
            st.stats.completed += 1;
            Ok(())
        }
        Err(KernelError::Cancelled) => match slot.reason.load(Ordering::Acquire) {
            TIMEOUT => {
                st.stats.timed_out += 1;
                Err(JobError::TimedOut)
            }
            _ => {
                st.stats.cancelled += 1;
                Err(JobError::Cancelled)
            }
        },
        Err(e) => {
            st.stats.failed += 1;
            Err(JobError::Kernel(e))
        }
    };
    let ms = nanos as f64 / 1e6;
    st.stats.last_ms = ms;
    let at = st.times_at % 64;
    st.times[at] = (nanos / 1000).min(u32::MAX as u64) as u32;
    st.times_at += 1;
    let n = st.times_at.min(64);
    let mut sorted = st.times;
    sorted[..n].sort_unstable();
    st.stats.p95_ms = sorted[((n * 95).div_ceil(100)).clamp(1, n) - 1] as f64 / 1e3;
    r
}

/// A driver: runs queued jobs, highest priority first, until none is left.
fn drive(weak: &Weak<SInner>) {
    let Some(inner) = weak.upgrade() else { return };
    let threads = inner.config.threads.min(inner.exec.workers() + 1).max(1);
    loop {
        let slot = {
            let mut st = lock(&inner.state);
            let next = st.queues.iter_mut().find_map(|q| q.pop_front());
            match next {
                Some(s) => {
                    st.running.push(s.clone());
                    s
                }
                None => {
                    st.drivers -= 1;
                    return;
                }
            }
        };
        slot.state.store(RUNNING, Ordering::Release);
        inner.wake_watchdog();
        let Some(mut job) = lock(&slot.job).take() else { continue };
        // A job whose deadline passed while queued does not start.
        let r = if inner.now_ns() >= slot.deadline.load(Ordering::Acquire) {
            let _ = slot.reason.compare_exchange(NO_REASON, TIMEOUT, Ordering::AcqRel, Ordering::Relaxed);
            Err(KernelError::Cancelled)
        } else if slot.cancel.load(Ordering::Acquire) != 0 {
            Err(KernelError::Cancelled)
        } else {
            job.run(&*inner.exec, threads).map(|_| ())
        };
        let mut st = lock(&inner.state);
        st.running.retain(|s| !Arc::ptr_eq(s, &slot));
        if r == Err(KernelError::Cancelled) && slot.reason.load(Ordering::Acquire) == PREEMPT {
            // Pre-empted: back to the front of its class, to run again.
            slot.cancel.store(0, Ordering::Release);
            slot.reason.store(NO_REASON, Ordering::Release);
            st.stats.preempted += 1;
            *lock(&slot.job) = Some(job);
            slot.state.store(QUEUED, Ordering::Release);
            let p = slot.priority.load(Ordering::Acquire) as usize;
            st.queues[p.min(3)].push_front(slot);
            continue;
        }
        let r = finish(&mut st, &slot, r, job.stats.nanos);
        drop(st);
        *lock(&slot.ticket) = None;
        *lock(&slot.result) = Some(r);
        let mut g = lock(&slot.job);
        *g = Some(job);
        slot.state.store(DONE, Ordering::Release);
        drop(g);
        slot.done.notify_all();
    }
}

/// Cancels running jobs at their deadlines. Parked while nothing runs.
fn watchdog(weak: Weak<SInner>) {
    loop {
        let wait = {
            let Some(inner) = weak.upgrade() else { return };
            let st = lock(&inner.state);
            if st.closed && st.running.is_empty() {
                return;
            }
            let now = inner.now_ns();
            let mut next = u64::MAX;
            for s in &st.running {
                let d = s.deadline.load(Ordering::Acquire);
                if d <= now {
                    if s.reason.compare_exchange(NO_REASON, TIMEOUT, Ordering::AcqRel, Ordering::Relaxed).is_ok() {
                        s.cancel.store(1, Ordering::Release);
                    }
                } else {
                    next = next.min(d);
                }
            }
            (next != u64::MAX).then(|| Duration::from_nanos(next - now))
        };
        match wait {
            Some(d) => std::thread::park_timeout(d),
            None => std::thread::park(),
        }
    }
}

// =========================================================================
// Realtime: keep the last result
// =========================================================================

/// Two jobs of one kernel in rotation for realtime use: one runs while the
/// last completed one stays readable. A request while a job is still
/// running is a dropped geometry frame (the last result keeps drawing).
pub struct Stream {
    idle: Option<Job>,
    spare: Option<Job>,
    front: Option<Job>,
    pending: Option<JobHandle>,
    /// Requests that found the previous job still running.
    pub dropped: u64,
    /// The last failure (the previous result stays up).
    pub last_error: Option<JobError>,
}

impl Stream {
    /// `a` and `b`: two jobs of the same kernel with their buffers bound.
    pub fn new(a: Job, b: Job) -> Stream {
        Stream { idle: Some(a), spare: Some(b), front: None, pending: None, dropped: 0, last_error: None }
    }

    /// Takes a finished job as the new front. True when the front changed.
    pub fn poll(&mut self) -> bool {
        let Some(h) = &self.pending else { return false };
        match h.try_take() {
            None => false,
            Some(Ok(job)) => {
                self.pending = None;
                if let Some(old) = self.front.replace(job) {
                    self.park(old);
                }
                self.last_error = None;
                true
            }
            Some(Err((job, e))) => {
                self.pending = None;
                self.park(job);
                self.last_error = Some(e);
                false
            }
        }
    }

    fn park(&mut self, job: Job) {
        if self.idle.is_none() {
            self.idle = Some(job);
        } else {
            self.spare = Some(job);
        }
    }

    /// Submits the next frame's job after `setup` updates it (params, time,
    /// inputs). Ok(false): the previous job is still running, and this
    /// frame keeps the last result (a dropped frame).
    pub fn request(&mut self, sched: &Scheduler, priority: Priority, origin: Origin, budget: JobBudget, setup: impl FnOnce(&mut Job)) -> Result<bool, JobError> {
        self.poll();
        if self.pending.is_some() {
            self.dropped += 1;
            return Ok(false);
        }
        let Some(mut job) = self.idle.take().or_else(|| self.spare.take()) else {
            self.dropped += 1;
            return Ok(false);
        };
        setup(&mut job);
        match sched.submit(job, priority, origin, budget) {
            Ok(h) => {
                self.pending = Some(h);
                Ok(true)
            }
            Err((job, e)) => {
                self.park(job);
                Err(e)
            }
        }
    }

    /// The last completed job (its outputs), if any.
    pub fn latest(&self) -> Option<&Job> {
        self.front.as_ref()
    }

    /// Whether a job is in flight.
    pub fn busy(&self) -> bool {
        self.pending.is_some()
    }
}
