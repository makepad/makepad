//! Cross-platform thread runtime.
//!
//! Background work runs on ONE mechanism on every target: the runtime-owned
//! [`TaskPool`] (`Cx::task_pool()`), whose workers are created once at
//! start-up and stay warm. A job is submitted from any thread and
//! its result comes back through a [`TaskHandle`] polled with `try_take`
//! (never a blocking join on the UI thread) or over whatever channel the job
//! carries. A `submit` completion raises the UI signal, so the handle can be
//! polled on `Event::Signal`; makepad's own jobs use `submit_internal`, whose
//! completion only wakes the runtime. Three FIFO classes share the process CPU
//! budget with compiler work; cooperative checkpoints give higher classes room.
//!
//! [`ThreadSpawner::spawn_worker`] creates a dedicated long-lived thread — an
//! audio, decode or network loop fed over a channel — once at start-up. It is
//! never the mechanism for a job: on the web a Web Worker takes hundreds of
//! milliseconds to come up, on desktop it is churn.
//!
//! Deadlines are Makepad monotonic seconds; no `Instant` crosses the wasm
//! boundary.

pub mod ui_hang;
pub use ui_hang::{ui_event_phase, ui_idle, ui_phase, UiIdleGuard, UiPhase, UiPhaseGuard};

use {
    crate::{
        cx::Cx,
        cx_api::CxThreadPriority,
        event::{Event, Timer},
    },
    std::{
        any::Any,
        collections::{HashMap, VecDeque},
        fmt,
        num::NonZeroUsize,
        panic::{catch_unwind, AssertUnwindSafe},
        sync::{
            atomic::{
                AtomicBool, AtomicPtr, AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering,
            },
            Arc, Condvar, Mutex, OnceLock, Weak,
        },
        thread::ThreadId,
        time::Duration,
    },
};

pub use makepad_network::{
    to_ui_bounded, to_ui_oneshot, wake_ui_loop, FromUIReceiver, FromUISender, ReceiverAlreadyTaken,
    SignalFromUI, SignalToUI, ToUIOneshotReceiver, ToUIOneshotSender, ToUIReceiver, ToUISender,
    UiWaker,
};

pub(crate) fn wake_ui_event_loop() {
    #[cfg(target_arch = "wasm32")]
    unsafe {
        js_wake_ui();
    }

    #[cfg(not(target_arch = "wasm32"))]
    crate::os::wake_ui_event_loop();
}

#[cfg(target_arch = "wasm32")]
#[link(wasm_import_module = "env")]
extern "C" {
    fn js_wake_ui();
}

/// The manual-stack web worker default. It is intentionally 2 MiB.
pub const DEFAULT_WEB_THREAD_STACK_SIZE: usize = 2 * 1024 * 1024;
pub const MIN_THREAD_STACK_SIZE: usize = 64 * 1024;
pub const MAX_THREAD_STACK_SIZE: usize = 128 * 1024 * 1024;

#[derive(Clone, Debug, Default)]
pub struct ThreadOptions {
    pub name: Option<Arc<str>>,
    pub stack_size: Option<usize>,
    pub priority: CxThreadPriority,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpawnError {
    Unsupported,
    RuntimeClosed,
    ResourceLimit,
    InvalidStackSize { requested: usize },
    Backend(Arc<str>),
}

impl fmt::Display for SpawnError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported => write!(f, "thread spawning is unsupported on this target"),
            Self::RuntimeClosed => write!(f, "thread runtime is closed"),
            Self::ResourceLimit => write!(f, "thread resource limit reached"),
            Self::InvalidStackSize { requested } => {
                write!(f, "invalid thread stack size {requested}")
            }
            Self::Backend(message) => write!(f, "thread backend error: {message}"),
        }
    }
}

impl std::error::Error for SpawnError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PanicReport {
    pub message: Arc<str>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TaskError {
    Spawn(SpawnError),
    Cancelled,
    Panicked(PanicReport),
    WorkerLost,
    WouldBlockUi,
}

impl fmt::Display for TaskError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spawn(error) => write!(f, "task spawn failed: {error}"),
            Self::Cancelled => write!(f, "task cancelled"),
            Self::Panicked(report) => write!(f, "task panicked: {}", report.message),
            Self::WorkerLost => write!(f, "task worker was lost"),
            Self::WouldBlockUi => write!(f, "joining would block the UI thread"),
        }
    }
}

impl std::error::Error for TaskError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum PriorityStatus {
    Pending,
    Applied,
    BestEffortUnsupported,
    Failed,
}

impl PriorityStatus {
    fn load(state: &AtomicU8) -> Self {
        match state.load(Ordering::Acquire) {
            0 => Self::Pending,
            1 => Self::Applied,
            2 => Self::BestEffortUnsupported,
            _ => Self::Failed,
        }
    }
}

fn priority_state(status: PriorityStatus) -> Arc<AtomicU8> {
    Arc::new(AtomicU8::new(status as u8))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaitOutcome {
    DeadlineReached,
    Cancelled,
}

#[derive(Clone, Default)]
pub struct CancellationToken {
    inner: Arc<CancellationInner>,
}

#[derive(Default)]
struct CancellationInner {
    cancelled: AtomicBool,
    waiters: AtomicPtr<CancellationWaiter>,
}

struct CancellationWaiter {
    thread: std::thread::Thread,
    next: *mut CancellationWaiter,
}

impl Drop for CancellationInner {
    fn drop(&mut self) {
        let mut next = *self.waiters.get_mut();
        while !next.is_null() {
            // The last token is gone: no caller can traverse or register.
            let waiter = unsafe { Box::from_raw(next) };
            next = waiter.next;
        }
    }
}

impl fmt::Debug for CancellationToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CancellationToken")
            .field("cancelled", &self.is_cancelled())
            .finish()
    }
}

impl CancellationToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        if !self.inner.cancelled.swap(true, Ordering::SeqCst) {
            let mut next = self.inner.waiters.load(Ordering::SeqCst);
            while !next.is_null() {
                // Nodes are immutable and live as long as this token's Arc.
                let waiter = unsafe { &*next };
                waiter.thread.unpark();
                next = waiter.next;
            }
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::Acquire)
    }

    /// Park a worker until cancellation or a `Cx::monotonic_now()` deadline.
    // The std condvar deadline clock is unavailable in wasm workers.
    pub fn wait_until(&self, deadline: f64) -> WaitOutcome {
        #[cfg(target_arch = "wasm32")]
        {
            while !self.is_cancelled() {
                let remaining = deadline - Cx::monotonic_now();
                if remaining <= 0.0 {
                    return WaitOutcome::DeadlineReached;
                }
                unsafe { js_worker_wait((remaining.min(0.01) * 1_000.0).ceil()) };
            }
            return WaitOutcome::Cancelled;
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            if self.is_cancelled() {
                return WaitOutcome::Cancelled;
            }
            let thread = std::thread::current();
            let mut head = self.inner.waiters.load(Ordering::SeqCst);
            let mut next = head;
            let mut registered = false;
            while !next.is_null() {
                let waiter = unsafe { &*next };
                if waiter.thread.id() == thread.id() {
                    registered = true;
                    break;
                }
                next = waiter.next;
            }
            if !registered {
                let node = Box::into_raw(Box::new(CancellationWaiter { thread, next: head }));
                loop {
                    unsafe {
                        (*node).next = head;
                    }
                    match self.inner.waiters.compare_exchange(
                        head,
                        node,
                        Ordering::SeqCst,
                        Ordering::SeqCst,
                    ) {
                        Ok(_) => break,
                        Err(current) => head = current,
                    }
                }
            }
            // SeqCst registration/flag pairs with cancel's flag/list loads:
            // cancellation either sees this waiter or we see cancellation.
            loop {
                if self.inner.cancelled.load(Ordering::SeqCst) {
                    return WaitOutcome::Cancelled;
                }
                let remaining = deadline - Cx::monotonic_now();
                if remaining <= 0.0 {
                    return WaitOutcome::DeadlineReached;
                }
                std::thread::park_timeout(Duration::from_secs_f64(remaining));
            }
        }
    }
}

#[cfg(target_arch = "wasm32")]
#[link(wasm_import_module = "env")]
extern "C" {
    fn js_worker_wait(timeout_ms: f64);
}

struct TaskState<T> {
    finished: AtomicBool,
    result: Mutex<Option<Result<T, TaskError>>>,
    wake: Condvar,
}

impl<T> Default for TaskState<T> {
    fn default() -> Self {
        Self {
            finished: AtomicBool::new(false),
            result: Mutex::new(None),
            wake: Condvar::new(),
        }
    }
}

impl<T> TaskState<T> {
    fn complete(&self, result: Result<T, TaskError>) -> bool {
        let mut slot = self.result.lock().unwrap();
        if self.finished.load(Ordering::Acquire) || slot.is_some() {
            return false;
        }
        *slot = Some(result);
        // Publish completion only after releasing the payload slot. In
        // particular, a UI poll that observes `finished` must not race the
        // producer while it still owns the mutex.
        drop(slot);
        self.finished.store(true, Ordering::Release);
        self.wake.notify_all();
        true
    }
}

#[must_use = "dropping a task handle detaches the task; call detach() when that is intentional"]
pub struct TaskHandle<T> {
    state: Arc<TaskState<T>>,
    token: CancellationToken,
    ui_thread: ThreadId,
    priority_status: Arc<AtomicU8>,
    #[cfg(not(target_arch = "wasm32"))]
    native_join: Option<std::thread::JoinHandle<()>>,
}

impl<T> fmt::Debug for TaskHandle<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TaskHandle")
            .field("finished", &self.is_finished())
            .field("cancelled", &self.token.is_cancelled())
            .field("priority_status", &self.priority_status())
            .finish()
    }
}

impl<T> TaskHandle<T> {
    pub fn is_finished(&self) -> bool {
        self.state.finished.load(Ordering::Acquire)
    }

    pub fn try_take(&mut self) -> Option<Result<T, TaskError>> {
        if !self.is_finished() {
            return None;
        }
        // This method is polled by the browser UI thread. A contended wasm
        // Mutex::lock lowers to Atomics.wait, which is forbidden there, so
        // leave the result on the worker and retry on the next UI frame.
        let result = match self.state.result.try_lock() {
            Ok(mut slot) => slot.take(),
            Err(std::sync::TryLockError::Poisoned(error)) => error.into_inner().take(),
            Err(std::sync::TryLockError::WouldBlock) => return None,
        };
        #[cfg(not(target_arch = "wasm32"))]
        if result.is_some() {
            // Payload publication precedes actual thread exit, including TLS
            // destructors. Polling never joins, even an apparently finished
            // native thread; an explicit worker-side join owns that wait.
            self.native_join.take();
        }
        result
    }

    #[allow(unused_mut)]
    pub fn join(mut self) -> Result<T, TaskError> {
        if std::thread::current().id() == self.ui_thread {
            return Err(TaskError::WouldBlockUi);
        }
        let result = {
            let mut slot = self.state.result.lock().unwrap();
            while slot.is_none() {
                slot = self.state.wake.wait(slot).unwrap();
            }
            slot.take().unwrap()
        };
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(join) = self.native_join.take() {
            if join.join().is_err() && result.is_ok() {
                return Err(TaskError::WorkerLost);
            }
        }
        result
    }

    pub fn detach(self) {}

    pub fn cancel(&self) {
        self.token.cancel();
    }

    pub fn token(&self) -> CancellationToken {
        self.token.clone()
    }

    pub fn priority_status(&self) -> PriorityStatus {
        PriorityStatus::load(&self.priority_status)
    }

    #[allow(dead_code)]
    fn completed(ui_thread: ThreadId, result: Result<T, TaskError>) -> Self {
        let state = Arc::new(TaskState::default());
        state.complete(result);
        Self {
            state,
            token: CancellationToken::new(),
            ui_thread,
            priority_status: priority_state(PriorityStatus::BestEffortUnsupported),
            #[cfg(not(target_arch = "wasm32"))]
            native_join: None,
        }
    }
}

// The page profile without threads (wasm32 without atomics): no worker
// exists, so a [`FrameJob`] runs on the UI thread, one per display frame.
#[cfg(all(target_arch = "wasm32", not(target_feature = "atomics")))]
std::thread_local! {
    static FRAME_JOB_RAN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// A display frame starts: the next deferred [`FrameJob`] may run. The web
/// platform calls this at every animation frame; with threads it does nothing.
pub fn frame_jobs_new_frame() {
    #[cfg(all(target_arch = "wasm32", not(target_feature = "atomics")))]
    FRAME_JOB_RAN.with(|ran| ran.set(false));
}

/// A job on the pool ([`TaskPool::submit_frame_job`]), with
/// [`TaskHandle`]'s polling. Natively and in a threaded web build it is a
/// pool task. Without workers (a single-threaded web build) it runs on the
/// UI thread when its result is first asked for (`try_take`,
/// `is_finished`), one job per display frame ([`frame_jobs_new_frame`]):
/// a page that loads many keeps drawing between them. Poll it from a
/// frame-driven path, since a deferred job only runs when polled.
#[must_use = "dropping a frame job cancels or detaches it"]
pub struct FrameJob<T> {
    inner: FrameJobInner<T>,
}

enum FrameJobInner<T> {
    /// A pool task (natively and in a threaded web build).
    #[cfg_attr(all(target_arch = "wasm32", not(target_feature = "atomics")), allow(dead_code))]
    Task(TaskHandle<T>),
    /// Not run yet, or run and not taken (the page without threads).
    #[cfg_attr(not(all(target_arch = "wasm32", not(target_feature = "atomics"))), allow(dead_code))]
    Later {
        job: Option<Box<dyn FnOnce() -> T + Send>>,
        result: Option<Result<T, TaskError>>,
        token: CancellationToken,
    },
}

impl<T> fmt::Debug for FrameJob<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.inner {
            FrameJobInner::Task(task) => task.fmt(f),
            FrameJobInner::Later { job, .. } => f.debug_struct("FrameJob").field("ran", &job.is_none()).finish(),
        }
    }
}

impl<T> FrameJob<T> {
    /// Run a deferred job here when this frame's turn is free.
    fn run_due(&mut self) {
        let FrameJobInner::Later { job, result, token } = &mut self.inner else { return };
        if job.is_none() {
            return;
        }
        if token.is_cancelled() {
            *job = None;
            *result = Some(Err(TaskError::Cancelled));
            return;
        }
        #[cfg(all(target_arch = "wasm32", not(target_feature = "atomics")))]
        if FRAME_JOB_RAN.with(|ran| ran.replace(true)) {
            return;
        }
        let f = job.take().unwrap();
        *result = Some(catch_unwind(AssertUnwindSafe(f)).map_err(|payload| TaskError::Panicked(panic_report(payload))));
    }

    /// Whether the result is there (a deferred job runs here if it may).
    pub fn is_finished(&mut self) -> bool {
        self.run_due();
        match &self.inner {
            FrameJobInner::Task(task) => task.is_finished(),
            FrameJobInner::Later { job, .. } => job.is_none(),
        }
    }

    /// The result once it is there, taken once (as [`TaskHandle::try_take`]).
    pub fn try_take(&mut self) -> Option<Result<T, TaskError>> {
        self.run_due();
        match &mut self.inner {
            FrameJobInner::Task(task) => task.try_take(),
            FrameJobInner::Later { result, .. } => result.take(),
        }
    }

    pub fn cancel(&self) {
        match &self.inner {
            FrameJobInner::Task(task) => task.cancel(),
            FrameJobInner::Later { token, .. } => token.cancel(),
        }
    }

    pub fn token(&self) -> CancellationToken {
        match &self.inner {
            FrameJobInner::Task(task) => task.token(),
            FrameJobInner::Later { token, .. } => token.clone(),
        }
    }
}

fn panic_report(payload: Box<dyn Any + Send>) -> PanicReport {
    let message: Arc<str> = if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).into()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.as_str().into()
    } else {
        "non-string panic payload".into()
    };
    PanicReport { message }
}

/// Creates dedicated, long-lived threads.
///
/// This is NOT the way to run a job. A job (a decode, a fetch, a scan, an
/// encode — anything with an end) goes to [`Cx::task_pool`], whose workers
/// already exist and are warm on every target. `spawn_worker` exists for the
/// handful of threads an app creates ONCE at start-up and then feeds over a
/// channel for its whole life: an audio engine, a decode loop, a network or
/// websocket pump, a file watcher. On the web every call here boots a fresh
/// Web Worker (hundreds of milliseconds); on desktop it is an OS thread.
#[derive(Clone)]
pub struct ThreadSpawner {
    ui_thread: ThreadId,
    parallelism: NonZeroUsize,
    runtime_open: Arc<AtomicBool>,
    workers: Arc<Mutex<RuntimeWorkers>>,
}

#[derive(Default)]
struct RuntimeWorkers {
    // Weak inners do not count as pool handles or keep completed workers alive.
    pools: Vec<Weak<PoolInner>>,
    tokens: Vec<Weak<CancellationInner>>,
}

impl fmt::Debug for ThreadSpawner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ThreadSpawner")
            .field("parallelism", &self.parallelism)
            .finish_non_exhaustive()
    }
}

impl ThreadSpawner {
    pub(crate) fn for_current_thread(parallelism: usize) -> Self {
        Self {
            ui_thread: std::thread::current().id(),
            parallelism: NonZeroUsize::new(parallelism.max(1)).unwrap(),
            runtime_open: Arc::new(AtomicBool::new(true)),
            workers: Arc::new(Mutex::new(RuntimeWorkers::default())),
        }
    }

    pub(crate) fn with_parallelism(&self, parallelism: usize) -> Self {
        Self {
            ui_thread: self.ui_thread,
            parallelism: NonZeroUsize::new(parallelism.max(1)).unwrap(),
            runtime_open: self.runtime_open.clone(),
            workers: self.workers.clone(),
        }
    }

    /// Create one dedicated long-lived thread. Call it once at start-up for a
    /// loop that is fed over a channel; never per job — jobs go to
    /// [`TaskPool::submit`]. The handle reports the thread's terminal result;
    /// `detach()` it when nobody waits for that.
    pub fn spawn_worker<F, T>(
        &self,
        options: ThreadOptions,
        f: F,
    ) -> Result<TaskHandle<T>, SpawnError>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        if !self.runtime_open.load(Ordering::Acquire) {
            return Err(SpawnError::RuntimeClosed);
        }
        validate_stack_size(options.stack_size)?;
        let token = CancellationToken::new();
        {
            let mut workers = self.workers.lock().unwrap();
            if !self.runtime_open.load(Ordering::Acquire) {
                return Err(SpawnError::RuntimeClosed);
            }
            workers.tokens.retain(|token| token.strong_count() != 0);
            workers.tokens.push(Arc::downgrade(&token.inner));
        }
        let state = Arc::new(TaskState::default());
        let run_state = state.clone();
        let run_token = token.clone();
        let run = move || {
            if run_token.is_cancelled() {
                run_state.complete(Err(TaskError::Cancelled));
                return;
            }
            let result = catch_unwind(AssertUnwindSafe(f))
                .map_err(|payload| TaskError::Panicked(panic_report(payload)));
            run_state.complete(result);
        };

        #[cfg(not(target_arch = "wasm32"))]
        {
            let mut builder = std::thread::Builder::new();
            if let Some(name) = &options.name {
                builder = builder.name(name.to_string());
            }
            if let Some(stack_size) = options.stack_size {
                builder = builder.stack_size(stack_size);
            }
            let priority = options.priority;
            let priority_status = priority_state(PriorityStatus::Pending);
            let applied_status = priority_status.clone();
            let native_join = builder
                .spawn(move || {
                    applied_status
                        .store(Cx::set_thread_priority(priority) as u8, Ordering::Release);
                    run();
                })
                .map_err(map_spawn_io_error)?;
            Ok(TaskHandle {
                state,
                token,
                ui_thread: self.ui_thread,
                priority_status,
                native_join: Some(native_join),
            })
        }

        #[cfg(all(target_arch = "wasm32", not(target_feature = "atomics")))]
        {
            let _ = (options, run, state, token);
            Err(SpawnError::Unsupported)
        }

        #[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
        {
            spawn_web_task(self.ui_thread, options, state, token, run)
        }
    }

    pub fn available_parallelism(&self) -> NonZeroUsize {
        self.parallelism
    }

    pub fn worker_count(&self, reserve_for_ui: usize, cap: usize) -> NonZeroUsize {
        worker_count_from(self.parallelism, reserve_for_ui, cap)
    }

    pub fn scheduler(&self) -> Result<Scheduler, SpawnError> {
        Scheduler::new(self.clone())
    }

    pub(crate) fn close_runtime(&self) {
        let workers = {
            let mut workers = self.workers.lock().unwrap();
            self.runtime_open.store(false, Ordering::Release);
            std::mem::take(&mut *workers)
        };
        // No registry lock spans cancellation, job drops or waking a worker.
        for pool in workers.pools.into_iter().filter_map(|pool| pool.upgrade()) {
            pool.close(ShutdownMode::CancelPending);
        }
        for inner in workers.tokens.into_iter().filter_map(|token| token.upgrade()) {
            CancellationToken { inner }.cancel();
        }
    }

    fn register_pool(&self, pool: &Arc<PoolInner>) -> Result<(), SpawnError> {
        let mut workers = self.workers.lock().unwrap();
        if !self.runtime_open.load(Ordering::Acquire) {
            return Err(SpawnError::RuntimeClosed);
        }
        workers.pools.retain(|pool| pool.strong_count() != 0);
        workers.pools.push(Arc::downgrade(pool));
        Ok(())
    }
}

fn validate_stack_size(stack_size: Option<usize>) -> Result<(), SpawnError> {
    if let Some(requested) = stack_size {
        if !(MIN_THREAD_STACK_SIZE..=MAX_THREAD_STACK_SIZE).contains(&requested)
            || requested % 16 != 0
        {
            return Err(SpawnError::InvalidStackSize { requested });
        }
    }
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn map_spawn_io_error(error: std::io::Error) -> SpawnError {
    use std::io::ErrorKind;
    match error.kind() {
        ErrorKind::WouldBlock | ErrorKind::OutOfMemory => SpawnError::ResourceLimit,
        _ => SpawnError::Backend(error.to_string().into()),
    }
}

pub fn available_parallelism() -> NonZeroUsize {
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::thread::available_parallelism().unwrap_or(NonZeroUsize::new(1).unwrap())
    }
    #[cfg(target_arch = "wasm32")]
    {
        NonZeroUsize::new(WEB_PARALLELISM.load(Ordering::Acquire).max(1)).unwrap()
    }
}

pub fn worker_count(reserve_for_ui: usize, cap: usize) -> NonZeroUsize {
    worker_count_from(available_parallelism(), reserve_for_ui, cap)
}

/// Physical topology available to this process, detected once at startup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MachineTopology {
    pub logical: usize,
    pub physical: usize,
    pub performance: usize,
    pub efficiency: usize,
    pub source: &'static str,
}

impl MachineTopology {
    pub fn homogeneous(physical: usize) -> Self {
        let physical = physical.max(1);
        Self {
            logical: physical,
            physical,
            performance: physical,
            efficiency: 0,
            source: "homogeneous",
        }
    }


    #[cfg(not(target_arch = "wasm32"))]
    fn limited_to(mut self, logical: usize) -> Self {
        self.logical = self.logical.min(logical.max(1));
        self.physical = self.physical.min(self.logical).max(1);
        self.performance = self.performance.min(self.physical).max(1);
        self.efficiency = self.efficiency.min(self.physical - self.performance);
        self
    }
}

pub fn machine_topology() -> MachineTopology {
    static TOPOLOGY: OnceLock<MachineTopology> = OnceLock::new();
    *TOPOLOGY.get_or_init(|| detect_machine_topology(available_parallelism().get()))
}

#[cfg(target_vendor = "apple")]
fn detect_machine_topology(logical: usize) -> MachineTopology {
    fn count(name: &[u8]) -> Option<usize> {
        unsafe extern "C" {
            fn sysctlbyname(
                name: *const std::ffi::c_char,
                old: *mut std::ffi::c_void,
                size: *mut usize,
                new: *mut std::ffi::c_void,
                new_size: usize,
            ) -> i32;
        }
        let mut value = 0u32;
        let mut size = std::mem::size_of_val(&value);
        let result = unsafe {
            sysctlbyname(
                name.as_ptr().cast(),
                (&mut value as *mut u32).cast(),
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        };
        (result == 0 && size == 4 && value > 0).then_some(value as usize)
    }
    let physical = count(b"hw.physicalcpu\0");
    let performance = count(b"hw.perflevel0.physicalcpu\0");
    let physical = physical.unwrap_or(1);
    let performance = performance.unwrap_or(physical).min(physical);
    MachineTopology {
        logical,
        physical,
        performance,
        efficiency: physical - performance,
        source: "sysctl-physical",
    }
    .limited_to(logical)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn detect_machine_topology(logical: usize) -> MachineTopology {
    use std::{
        collections::{BTreeMap, BTreeSet},
        fs,
    };
    let cpu_list = |list: &str| -> BTreeSet<usize> {
        list.trim()
            .split(',')
            .flat_map(|range| {
                let mut bounds = range.split('-');
                let first = bounds.next().and_then(|n| n.parse::<usize>().ok());
                let last = bounds
                    .next()
                    .and_then(|n| n.parse::<usize>().ok())
                    .or(first);
                first
                    .zip(last)
                    .into_iter()
                    .flat_map(|(first, last)| first..=last)
            })
            .collect()
    };
    // Intel hybrid PMUs identify core classes directly. cpu_capacity can
    // differ even within one class (e.g. a preferred boost core), so equality
    // with the highest capacity must not reduce four P cores to one worker.
    let performance_cpus = cpu_list(
        &fs::read_to_string("/sys/bus/event_source/devices/cpu_core/cpus").unwrap_or_default(),
    );
    let efficiency_cpus = cpu_list(
        &fs::read_to_string("/sys/bus/event_source/devices/cpu_atom/cpus").unwrap_or_default(),
    );
    // Respect affinity/cpuset restrictions, and deduplicate SMT siblings.
    let status = fs::read_to_string("/proc/self/status").unwrap_or_default();
    let allowed = status
        .lines()
        .find_map(|line| line.strip_prefix("Cpus_allowed_list:"));
    let mut cores = BTreeMap::new();
    if let Some(allowed) = allowed {
        for cpu in cpu_list(allowed) {
            let base = format!("/sys/devices/system/cpu/cpu{cpu}");
            let siblings = fs::read_to_string(format!("{base}/topology/thread_siblings_list"));
            let Ok(siblings) = siblings else { continue };
            let capacity = fs::read_to_string(format!("{base}/cpu_capacity"))
                .ok()
                .and_then(|s| s.trim().parse::<u64>().ok());
            let performance = if performance_cpus.contains(&cpu) {
                Some(true)
            } else if efficiency_cpus.contains(&cpu) {
                Some(false)
            } else {
                None
            };
            cores
                .entry(siblings.trim().to_string())
                .or_insert((capacity, performance));
        }
    }
    if cores.is_empty() {
        return MachineTopology {
            source: "physical-unavailable-conservative",
            logical,
            ..MachineTopology::homogeneous(1)
        };
    }
    let physical = cores.len();
    let pmu_classes = cores.values().all(|(_, class)| class.is_some());
    let performance = if pmu_classes {
        let count = cores
            .values()
            .filter(|(_, class)| *class == Some(true))
            .count();
        // An affinity mask containing only efficiency cores is homogeneous
        // for this process; these are its fastest available cores.
        if count == 0 {
            physical
        } else {
            count
        }
    } else if cores.values().all(|(capacity, _)| capacity.is_some()) {
        let max = cores.values().filter_map(|(capacity, _)| *capacity).max();
        cores
            .values()
            .filter(|(capacity, _)| *capacity == max)
            .count()
    } else {
        physical
    };
    MachineTopology {
        logical,
        physical,
        performance,
        efficiency: physical - performance,
        source: if pmu_classes {
            "sysfs-pmu-affinity-physical"
        } else {
            "sysfs-affinity-physical"
        },
    }
    .limited_to(logical)
}

#[cfg(target_os = "windows")]
fn detect_machine_topology(logical: usize) -> MachineTopology {
    // Query RelationProcessorCore. Each variable-size record is one physical
    // core, regardless of the number of SMT bits in its group masks.
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetLogicalProcessorInformationEx(
            relationship: u32,
            buffer: *mut u8,
            length: *mut u32,
        ) -> i32;
    }
    let mut bytes = 0u32;
    unsafe {
        GetLogicalProcessorInformationEx(0, std::ptr::null_mut(), &mut bytes);
    }
    let mut classes = Vec::new();
    if bytes > 0 {
        let mut buffer = vec![0u8; bytes as usize];
        if unsafe { GetLogicalProcessorInformationEx(0, buffer.as_mut_ptr(), &mut bytes) } != 0 {
            let mut offset = 0;
            while offset + 32 <= bytes as usize {
                let relationship =
                    u32::from_ne_bytes(buffer[offset..offset + 4].try_into().unwrap());
                let size =
                    u32::from_ne_bytes(buffer[offset + 4..offset + 8].try_into().unwrap()) as usize;
                if size < 32 || offset + size > bytes as usize {
                    break;
                }
                if relationship == 0 {
                    classes.push(buffer[offset + 9]);
                }
                offset += size;
            }
        }
    }
    if classes.is_empty() {
        return MachineTopology {
            source: "physical-unavailable-conservative",
            logical,
            ..MachineTopology::homogeneous(1)
        };
    }
    let max = classes.iter().copied().max().unwrap();
    let performance = classes.iter().filter(|c| **c == max).count();
    MachineTopology {
        logical,
        physical: classes.len(),
        performance,
        efficiency: classes.len() - performance,
        source: "windows-physical",
    }
    .limited_to(logical)
}

#[cfg(not(any(
    target_vendor = "apple",
    target_os = "linux",
    target_os = "android",
    target_os = "windows"
)))]
fn detect_machine_topology(logical: usize) -> MachineTopology {
    MachineTopology {
        logical,
        source: "physical-unavailable-conservative",
        ..MachineTopology::homogeneous(1)
    }
}

fn worker_count_from(parallelism: NonZeroUsize, reserve_for_ui: usize, cap: usize) -> NonZeroUsize {
    let available = parallelism.get().saturating_sub(reserve_for_ui).max(1);
    NonZeroUsize::new(available.min(cap.max(1))).unwrap()
}

#[cfg(target_arch = "wasm32")]
static WEB_PARALLELISM: AtomicUsize = AtomicUsize::new(1);

#[cfg(target_arch = "wasm32")]
pub(crate) fn set_web_available_parallelism(value: usize) {
    WEB_PARALLELISM.store(value.max(1), Ordering::Release);
}

impl Cx {
    pub fn thread_spawner(&self) -> ThreadSpawner {
        self.thread_spawner.with_parallelism(self.cpu_cores.max(1))
    }

    /// The runtime's background executor: created once (at `Event::Startup`,
    /// or on first use), sized to the machine, workers warm on every target.
    /// The returned handle is a cheap clone that any thread may submit on.
    pub fn task_pool(&self) -> TaskPool {
        self.task_pool
            .get_or_init(|| {
                let spawner = self.thread_spawner();
                match TaskPool::shared(spawner) {
                    Ok(pool) => pool,
                    Err(error) => {
                        crate::log!("task pool unavailable ({error}); background jobs are refused");
                        TaskPool::closed()
                    }
                }
            })
            .clone()
    }

    /// From now on this Cx runs as a target without worker threads does
    /// (wasm without atomics): its pool is [`TaskPool::closed`], so every
    /// submit is refused and callers take their synchronous path. The shared
    /// pool other Cx use stays open, and jobs this Cx submitted to it before
    /// run on.
    pub fn use_closed_task_pool(&mut self) {
        self.task_pool = std::cell::OnceCell::from(TaskPool::closed());
    }

    pub(crate) fn warm_task_pool(&self) {
        let status = Cx::set_thread_priority(CxThreadPriority::UserInteractive);
        crate::trace!("pool", "UI thread priority UserInteractive: {status:?}");
        let _ = self.task_pool();
    }

    /// Shared CPU budget plus app worker and queue counts.
    pub fn task_pool_summary(&self) -> String {
        match self.task_pool.get() {
            Some(pool) => pool.summary(),
            None => "pool: not started".to_string(),
        }
    }

    pub(crate) fn close_task_pool(&self) {
        if let Some(pool) = self.task_pool.get() {
            crate::trace!("pool", "{}", pool.summary());
            pool.close_pool(ShutdownMode::CancelPending);
        }
    }

    /// One dedicated long-lived thread (see [`ThreadSpawner::spawn_worker`]).
    pub fn spawn_worker<F, T>(&self, f: F) -> Result<TaskHandle<T>, SpawnError>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        let result = self
            .thread_spawner()
            .spawn_worker(ThreadOptions::default(), f);
        log_unsupported_spawn_once(&result);
        result
    }

    pub fn spawn_worker_with<F, T>(
        &self,
        options: ThreadOptions,
        f: F,
    ) -> Result<TaskHandle<T>, SpawnError>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        let result = self.thread_spawner().spawn_worker(options, f);
        log_unsupported_spawn_once(&result);
        result
    }
}

fn log_unsupported_spawn_once<T>(result: &Result<TaskHandle<T>, SpawnError>) {
    static LOGGED: AtomicBool = AtomicBool::new(false);
    match result {
        Err(SpawnError::Unsupported) if !LOGGED.swap(true, Ordering::AcqRel) => {
            crate::error!("Cx::spawn_worker is unsupported on wasm without atomics");
        }
        Err(error) if !matches!(error, SpawnError::Unsupported) => {
            crate::error!("Cx::spawn_worker failed: {error}");
        }
        _ => {}
    }
}

/// Scheduling priority. Ordering puts Interactive above Visible above Background.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Class {
    Background,
    Visible,
    Interactive,
}
impl Class {
    fn budget(self) -> u8 {
        match self {
            Self::Interactive => makepad_cpu_budget::INTERACTIVE,
            Self::Visible => makepad_cpu_budget::VISIBLE,
            Self::Background => makepad_cpu_budget::BACKGROUND,
        }
    }
}

#[derive(Debug)]
pub struct Stale;
impl fmt::Display for Stale {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("superseded or cancelled")
    }
}
impl std::error::Error for Stale {}

#[derive(Clone, Default)]
pub struct Version(Arc<AtomicU64>);
impl Version {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn next(&self) -> Ticket {
        let value = self.0.fetch_add(1, Ordering::AcqRel).wrapping_add(1);
        Ticket {
            version: Some(self.0.clone()),
            value,
        }
    }
    pub fn bump(&self) {
        self.0.fetch_add(1, Ordering::AcqRel);
    }
}
#[derive(Clone)]
pub struct Ticket {
    version: Option<Arc<AtomicU64>>,
    value: u64,
}
impl Ticket {
    pub fn none() -> Self {
        Self {
            version: None,
            value: 0,
        }
    }
    pub fn is_stale(&self) -> bool {
        self.version
            .as_ref()
            .is_some_and(|v| v.load(Ordering::Acquire) != self.value)
    }
}

struct JobContext {
    permit: Option<u32>,
    class: Class,
    label: &'static str,
    ticket: Ticket,
    token: CancellationToken,
    parents: Vec<(Ticket, CancellationToken)>,
    helper: bool,
}
thread_local! {
    static JOB: std::cell::RefCell<Option<JobContext>> = const { std::cell::RefCell::new(None) };
}
/// Shared dispatch policy: a gesture, not hover or ordinary pointer motion.
pub(crate) fn ends_warmup(event: &Event) -> bool {
    matches!(event, Event::MouseDown(_) | Event::KeyDown(_) | Event::KeyUp(_)
        | Event::TextInput(_) | Event::Scroll(_) | Event::Pinch(_)
        | Event::Drag(_) | Event::Drop(_) | Event::SelectionHandleDrag(_))
        || matches!(event, Event::TouchUpdate(e) if e.touches.iter().any(|t|
            matches!(t.state, crate::event::TouchState::Start | crate::event::TouchState::Move)))
}

pub fn current_class() -> Option<Class> {
    JOB.with(|j| j.borrow().as_ref().map(|j| j.class))
}

pub fn checkpoint() -> Result<(), Stale> {
    JOB.with(|cell| {
        let mut context = cell.borrow_mut();
        let Some(job) = context.as_mut() else {
            return Ok(());
        };
        if job.ticket.is_stale()
            || job.token.is_cancelled()
            || job
                .parents
                .iter()
                .any(|(ticket, token)| ticket.is_stale() || token.is_cancelled())
        {
            return Err(Stale);
        }
        if let Some(permit) = job.permit {
            // A helper observes should_yield at its item boundary and leaves.
            // Calling the parking operation here would race a newly arriving
            // waiter even after should_yield returned false.
            if !job.helper {
                #[cfg(relax)]
                let (held, location) = std::relax::held_lock();
                #[cfg(not(relax))]
                let (held, location): (
                    u32,
                    Option<&std::panic::Location<'static>>,
                ) = (0, None);
                let site = location.map(|l| format!("{}:{}", l.file(), l.line()));
                job.permit = Some(makepad_cpu_budget::checkpoint(
                    permit,
                    held,
                    site.as_deref().unwrap_or(""),
                ));
            }
        }
        Ok(())
    })
}

// A nested, caller-helped job borrows the caller's permit. Checkpoints can change
// its index, so the returned index (not the saved one) is restored on unwind too.
struct JobGuard {
    previous: Option<JobContext>,
    borrowed: bool,
}
impl JobGuard {
    fn enter(
        class: Class,
        label: &'static str,
        ticket: Ticket,
        token: CancellationToken,
        permit: Option<u32>,
    ) -> Self {
        let mut previous = JOB.with(|j| j.borrow_mut().take());
        debug_assert!(permit.is_none() || previous.is_none());
        let borrowed_permit = previous.as_mut().and_then(|j| j.permit.take());
        let borrowed = borrowed_permit.is_some();
        let permit = permit.or(borrowed_permit).unwrap_or_else(|| {
            makepad_cpu_budget::admit(class.budget(), makepad_cpu_budget::APP, label)
        });
        if borrowed {
            makepad_cpu_budget::reclass(permit, class.budget());
        }
        makepad_cpu_budget::set_thread_class(class.budget());
        let parents = previous
            .as_ref()
            .map(|p| {
                let mut parents = p.parents.clone();
                parents.push((p.ticket.clone(), p.token.clone()));
                parents
            })
            .unwrap_or_default();
        JOB.with(|j| {
            *j.borrow_mut() = Some(JobContext {
                permit: Some(permit),
                class,
                label,
                ticket,
                token,
                parents,
                helper: false,
            })
        });
        Self { previous, borrowed }
    }
}
impl Drop for JobGuard {
    fn drop(&mut self) {
        let job = JOB.with(|j| j.borrow_mut().take()).unwrap();
        if let Some(permit) = job.permit {
            if self.borrowed {
                let previous = self.previous.as_mut().unwrap();
                makepad_cpu_budget::reclass(permit, previous.class.budget());
                previous.permit = Some(permit);
                makepad_cpu_budget::set_thread_class(previous.class.budget());
            } else {
                makepad_cpu_budget::release(permit);
            }
        }
        JOB.with(|j| *j.borrow_mut() = self.previous.take());
    }
}
// Waiting for a child never retains a CPU permit. Restore it even on unwind.
fn without_permit<R>(f: impl FnOnce() -> R) -> R {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            if self.0 {
                JOB.with(|j| {
                    let mut context = j.borrow_mut();
                    let job = context.as_mut().unwrap();
                    job.permit = Some(makepad_cpu_budget::admit(
                        job.class.budget(),
                        makepad_cpu_budget::APP,
                        job.label,
                    ));
                });
            }
        }
    }
    let released = JOB.with(|j| j.borrow_mut().as_mut().and_then(|j| j.permit.take()));
    if let Some(permit) = released {
        makepad_cpu_budget::release(permit);
    }
    let _restore = Restore(released.is_some());
    f()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueOrder {
    Fifo,
    Lifo,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShutdownMode {
    Drain,
    CancelPending,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SubmitError {
    Closed,
}
impl fmt::Display for SubmitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("task pool closed")
    }
}
impl std::error::Error for SubmitError {}

struct PoolJob {
    class: Class,
    label: &'static str,
    submitted_at: f64,
    ticket: Ticket,
    token: CancellationToken,
    run: Option<Box<dyn FnOnce() + Send>>,
    cancel: Option<Box<dyn FnOnce() + Send>>,
    /// Its place in the jobs of the handle it was submitted through (`JobSet`): left once it
    /// ran or was dropped, after its handle is answered.
    member: Option<SetMember>,
}
impl PoolJob {
    fn run(mut self) {
        if self.ticket.is_stale() || self.token.is_cancelled() {
            return;
        }
        self.cancel.take();
        if let Some(run) = self.run.take() {
            run();
        }
    }
}
impl Drop for PoolJob {
    fn drop(&mut self) {
        if self.run.is_some() {
            self.token.cancel();
            struct CancelOnDrop(Option<Box<dyn FnOnce() + Send>>);
            impl Drop for CancelOnDrop {
                fn drop(&mut self) {
                    if let Some(cancel) = self.0.take() { cancel(); }
                }
            }
            let _complete = CancelOnDrop(self.cancel.take());
            self.run.take();
        }
    }
}
fn task_job<F, T>(
    ui_thread: ThreadId,
    class: Class,
    label: &'static str,
    ticket: Ticket,
    f: F,
    internal: bool,
) -> (PoolJob, TaskHandle<T>)
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let state = Arc::new(TaskState::default());
    let token = CancellationToken::new();
    let run_state = state.clone();
    let cancel_state = state.clone();
    let priority_status = priority_state(PriorityStatus::Pending);
    let status = priority_status.clone();
    let job = PoolJob {
        class,
        label,
        submitted_at: Cx::monotonic_now(),
        ticket,
        token: token.clone(),
        run: Some(Box::new(move || {
            status.store(
                if makepad_cpu_budget::set_thread_class(class.budget()) {
                    PriorityStatus::Applied
                } else {
                    PriorityStatus::BestEffortUnsupported
                } as u8,
                Ordering::Release,
            );
            let result =
                catch_unwind(AssertUnwindSafe(f)).map_err(|p| TaskError::Panicked(panic_report(p)));
            run_state.complete(result);
            signal_completion(internal);
        })),
        cancel: Some(Box::new(move || {
            cancel_state.complete(Err(TaskError::Cancelled));
            signal_completion(internal);
        })),
        member: None,
    };
    (
        job,
        TaskHandle {
            state,
            token,
            ui_thread,
            priority_status,
            #[cfg(not(target_arch = "wasm32"))]
            native_join: None,
        },
    )
}

/// The jobs submitted through one pool handle and its clones (each `Cx::task_pool` handle has
/// its own): what that handle's [`TaskPool::close`] and [`TaskPool::shutdown`] act on. The pool
/// the handles share (the process's pool: every Cx's) is closed by no handle: it closes when
/// its last handle drops, or when the app ends (`Cx::close_task_pool`).
#[derive(Default)]
struct JobSet {
    state: Mutex<JobSetState>,
}
#[derive(Default)]
struct JobSetState {
    closed: u8,
    next: u64,
    /// the set's jobs that have not yet run or been dropped, by number
    live: HashMap<u64, CancellationToken>,
    /// what the set's shutdown handles wait on: completed once it is closed and empty
    done: Option<Arc<TaskState<()>>>,
}
impl JobSet {
    /// A place in the set for a job with `token`; None once the set is closed.
    fn join(self: &Arc<Self>, token: &CancellationToken) -> Option<SetMember> {
        let mut state = self.state.lock().unwrap();
        if state.closed != POOL_OPEN {
            return None;
        }
        let id = state.next;
        state.next += 1;
        state.live.insert(id, token.clone());
        Some(SetMember { set: self.clone(), id })
    }
    /// Refuses the set's new jobs; `CancelPending` also cancels the ones it holds (a queued
    /// one is dropped when its turn comes, a running one sees its token). The state completes
    /// once none is left.
    fn close(&self, mode: ShutdownMode) -> Arc<TaskState<()>> {
        let (cancel, done, empty) = {
            let mut state = self.state.lock().unwrap();
            state.closed = state.closed.max(match mode {
                ShutdownMode::Drain => POOL_DRAINING,
                ShutdownMode::CancelPending => POOL_CANCELLED,
            });
            let cancel: Vec<CancellationToken> = if state.closed == POOL_CANCELLED { state.live.values().cloned().collect() } else { Vec::new() };
            (cancel, state.done.get_or_insert_with(Default::default).clone(), state.live.is_empty())
        };
        for token in cancel {
            token.cancel();
        }
        if empty {
            done.complete(Ok(()));
        }
        done
    }
    fn is_open(&self) -> bool {
        self.state.lock().unwrap().closed == POOL_OPEN
    }
}
/// A job's place in its set: leaving it ends a closed set that it leaves empty.
struct SetMember {
    set: Arc<JobSet>,
    id: u64,
}
impl Drop for SetMember {
    fn drop(&mut self) {
        let done = {
            let mut state = self.set.state.lock().unwrap();
            state.live.remove(&self.id);
            (state.closed != POOL_OPEN && state.live.is_empty()).then(|| state.done.clone()).flatten()
        };
        if let Some(done) = done {
            done.complete(Ok(()));
        }
    }
}

#[derive(Default)]
struct PoolState {
    queues: [VecDeque<PoolJob>; 3],
    waiting: [bool; 3],
    closed: u8,
    running: Vec<Option<CancellationToken>>,
}
struct WorkerSlot {
    thread: OnceLock<std::thread::Thread>,
    idle: AtomicBool,
}
struct PoolInner {
    state: Mutex<PoolState>,
    /// Jobs queued per class (as `state.queues`, read without the lock).
    queued: [AtomicUsize; 3],
    workers: Vec<WorkerSlot>,
    started: AtomicUsize,
    exited: AtomicUsize,
    completed: AtomicU64,
    handles: AtomicUsize,
    shutdown_state: Arc<TaskState<()>>,
    ui_thread: ThreadId,
}
const POOL_OPEN: u8 = 0;
const POOL_DRAINING: u8 = 1;
const POOL_CANCELLED: u8 = 2;
impl PoolInner {
    fn enqueue(&self, job: PoolJob) -> Result<(), SubmitError> {
        let mut state = self.state.lock().unwrap();
        if state.closed != POOL_OPEN {
            return Err(SubmitError::Closed);
        }
        self.push(&mut state, job);
        drop(state);
        self.wake_one();
        Ok(())
    }
    // Strand heads were accepted before close. Drain includes their successors.
    fn continue_strand(&self, job: PoolJob) {
        let mut state = self.state.lock().unwrap();
        if state.closed == POOL_CANCELLED {
            return;
        }
        self.push(&mut state, job);
        drop(state);
        self.wake_one();
    }
    fn push(&self, state: &mut PoolState, job: PoolJob) {
        let index = job.class.budget() as usize;
        makepad_cpu_budget::queued(job.class.budget(), makepad_cpu_budget::APP, 1);
        self.queued[index].fetch_add(1, Ordering::Relaxed);
        state.queues[index].push_back(job);
    }
    /// A job of a higher class than `class` waits in the queue: every worker is busy, so it
    /// starts only when one leaves.
    fn higher_queued(&self, class: Class) -> bool {
        self.queued[..class.budget() as usize].iter().any(|q| q.load(Ordering::Relaxed) > 0)
    }
    fn wake_one(&self) {
        for worker in &self.workers {
            if worker.idle.swap(false, Ordering::SeqCst) {
                if let Some(t) = worker.thread.get() {
                    t.unpark();
                }
                return;
            }
        }
    }
    fn unpark_all(&self) {
        for worker in &self.workers {
            if let Some(t) = worker.thread.get() {
                t.unpark();
            }
        }
    }
    /// One more handle on a pool that is open and still held. The last
    /// handle's drop closes the pool, and its workers keep the inner alive
    /// while they wind down, so a pool with no handle left (or a closed one)
    /// is on its way out and a new user must not join it.
    fn add_live_handle(&self) -> bool {
        if self.state.lock().unwrap().closed != POOL_OPEN {
            return false;
        }
        let mut held = self.handles.load(Ordering::Acquire);
        while held > 0 {
            match self.handles.compare_exchange_weak(held, held + 1, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => return true,
                Err(now) => held = now,
            }
        }
        false
    }
    fn close(&self, mode: ShutdownMode) {
        let mut state = self.state.lock().unwrap();
        state.closed = state.closed.max(match mode {
            ShutdownMode::Drain => POOL_DRAINING,
            ShutdownMode::CancelPending => POOL_CANCELLED,
        });
        let (discarded, running) = if state.closed == POOL_CANCELLED {
            (std::mem::take(&mut state.queues), state.running.iter().flatten().cloned().collect::<Vec<_>>())
        } else {
            (std::array::from_fn(|_| VecDeque::new()), Vec::new())
        };
        drop(state);
        for token in running {
            token.cancel();
        }
        for (class, queue) in discarded.into_iter().enumerate() {
            for job in queue {
                makepad_cpu_budget::queued(class as u8, makepad_cpu_budget::APP, -1);
                self.queued[class].fetch_sub(1, Ordering::Relaxed);
                drop(job);
            }
        }
        self.unpark_all();
    }
    fn summary(&self) -> String {
        let (queued, closed) = {
            let state = self.state.lock().unwrap();
            (state.queues.each_ref().map(VecDeque::len), state.closed != POOL_OPEN)
        };
        format!(
            "{}pool: workers={} started={} exited={} queued={}/{}/{} completed={} closed={}\n",
            makepad_cpu_budget::report_string(),
            self.workers.len(),
            self.started.load(Ordering::Acquire),
            self.exited.load(Ordering::Acquire),
            queued[0],
            queued[1],
            queued[2],
            self.completed.load(Ordering::Relaxed),
            closed
        )
    }
    fn run_job(&self, job: PoolJob, permit: Option<u32>) {
        let class = job.class;
        let label = job.label;
        let _context = JobGuard::enter(class, label, job.ticket.clone(), job.token.clone(), permit);
        makepad_cpu_budget::waited(
            class.budget(),
            makepad_cpu_budget::APP,
            ((Cx::monotonic_now() - job.submitted_at).max(0.0) * 1e9) as u64,
            label,
        );
        #[cfg(any(target_vendor = "apple", target_os = "linux"))]
        let traced = ui_hang::relax_timeline::hooks()
            .inspect(|h| (h.begin)(label.as_ptr(), label.len()))
            .is_some();
        if let Err(panic) = catch_unwind(AssertUnwindSafe(|| job.run())) {
            crate::error!("pool job cleanup panicked: {}", panic_report(panic).message);
        }
        #[cfg(any(target_vendor = "apple", target_os = "linux"))]
        if traced {
            if let Some(h) = ui_hang::relax_timeline::hooks() {
                (h.end)(label.as_ptr(), label.len());
            }
        }
        self.completed.fetch_add(1, Ordering::Relaxed);
    }
}
fn pool_worker(inner: Arc<PoolInner>, index: usize) {
    let _ = inner.workers[index].thread.set(std::thread::current());
    inner.started.fetch_add(1, Ordering::AcqRel);
    loop {
        inner.workers[index].idle.store(true, Ordering::SeqCst);
        let candidate = {
            let mut state = inner.state.lock().unwrap();
            if state.closed != POOL_OPEN && state.queues.iter().all(VecDeque::is_empty) {
                break;
            }
            let candidate = (0..3).find(|&i| !state.waiting[i] && !state.queues[i].is_empty());
            candidate.map(|i| {
                state.waiting[i] = true;
                (i, state.queues[i].front().unwrap().label)
            })
        };
        let Some((waiting, label)) = candidate else {
            std::thread::park();
            continue;
        };
        inner.workers[index].idle.store(false, Ordering::SeqCst);
        // Only one worker per class enters admit. Others remain available to
        // submit a higher-class request, including while the JIT owns all cores.
        let permit = makepad_cpu_budget::admit(waiting as u8, makepad_cpu_budget::APP, label);
        let job = {
            let mut state = inner.state.lock().unwrap();
            state.waiting[waiting] = false;
            let job = state.queues.iter_mut().find_map(VecDeque::pop_front);
            if let Some(job) = &job {
                makepad_cpu_budget::queued(job.class.budget(), makepad_cpu_budget::APP, -1);
                inner.queued[job.class.budget() as usize].fetch_sub(1, Ordering::Relaxed);
                state.running[index] = Some(job.token.clone());
            }
            job
        };
        inner.wake_one();
        if let Some(job) = job {
            if job.class.budget() != waiting as u8 {
                makepad_cpu_budget::reclass(permit, job.class.budget());
            }
            inner.run_job(job, Some(permit));
            inner.state.lock().unwrap().running[index] = None;
        } else {
            makepad_cpu_budget::release(permit);
        }
    }
}
struct PoolWorkerExit(Arc<PoolInner>);
impl Drop for PoolWorkerExit {
    fn drop(&mut self) {
        self.0.unpark_all();
        if self.0.exited.fetch_add(1, Ordering::AcqRel) + 1 == self.0.workers.len() {
            self.0.shutdown_state.complete(Ok(()));
        }
    }
}

/// A handle on a pool of workers. Clones share the pool and the handle's jobs (`JobSet`):
/// closing or shutting down a handle acts on the jobs submitted through it and its clones,
/// never on those of another handle of the same pool.
pub struct TaskPool {
    inner: Arc<PoolInner>,
    set: Arc<JobSet>,
}
impl Clone for TaskPool {
    fn clone(&self) -> Self {
        self.inner.handles.fetch_add(1, Ordering::AcqRel);
        Self {
            inner: self.inner.clone(),
            set: self.set.clone(),
        }
    }
}
impl Drop for TaskPool {
    fn drop(&mut self) {
        if self.inner.handles.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.inner.close(ShutdownMode::CancelPending);
        }
    }
}
impl fmt::Debug for TaskPool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TaskPool")
            .field("workers", &self.worker_count())
            .finish()
    }
}
fn app_pool() -> &'static Mutex<Weak<PoolInner>> {
    static POOL: OnceLock<Mutex<Weak<PoolInner>>> = OnceLock::new();
    POOL.get_or_init(|| Mutex::new(Weak::new()))
}
pub(crate) fn cpu_summary() -> String {
    let pool = app_pool().lock().unwrap().upgrade();
    pool.map_or_else(
        || format!("{}pool: not started\n", makepad_cpu_budget::report_string()),
        |p| p.summary(),
    )
}
impl TaskPool {
    fn shared(spawner: ThreadSpawner) -> Result<Self, SpawnError> {
        let mut global = app_pool().lock().unwrap();
        if let Some(inner) = global.upgrade().filter(|inner| inner.add_live_handle()) {
            return Ok(Self { inner, set: Arc::default() });
        }
        #[cfg(not(target_arch = "wasm32"))]
        let workers = makepad_cpu_budget::limit() as usize;
        #[cfg(target_arch = "wasm32")]
        let workers = spawner
            .available_parallelism()
            .get()
            .saturating_sub(1)
            .clamp(3, 6);
        let pool = Self::create(spawner, workers)?;
        *global = Arc::downgrade(&pool.inner);
        Ok(pool)
    }
    fn create(spawner: ThreadSpawner, worker_len: usize) -> Result<Self, SpawnError> {
        let inner = Arc::new(PoolInner {
            state: Mutex::new(PoolState {
                running: vec![None; worker_len],
                ..Default::default()
            }),
            queued: Default::default(),
            workers: (0..worker_len)
                .map(|_| WorkerSlot {
                    thread: OnceLock::new(),
                    idle: AtomicBool::new(false),
                })
                .collect(),
            started: AtomicUsize::new(0),
            exited: AtomicUsize::new(0),
            completed: AtomicU64::new(0),
            handles: AtomicUsize::new(1),
            shutdown_state: Arc::new(TaskState::default()),
            ui_thread: spawner.ui_thread,
        });
        let pool = Self {
            inner: inner.clone(),
            set: Arc::default(),
        };
        spawner.register_pool(&inner)?;
        for index in 0..worker_len {
            let worker_inner = inner.clone();
            let exit = PoolWorkerExit(inner.clone());
            let run = move || {
                let _exit = exit;
                pool_worker(worker_inner, index);
            };
            #[cfg(not(target_arch = "wasm32"))]
            let spawned = std::thread::Builder::new()
                .name(format!("makepad-pool-{index}"))
                .spawn(run)
                .map(|_| ())
                .map_err(map_spawn_io_error);
            #[cfg(target_arch = "wasm32")]
            let spawned = spawner
                .spawn_worker(
                    ThreadOptions {
                        name: Some(format!("makepad-pool-{index}").into()),
                        ..Default::default()
                    },
                    run,
                )
                .map(TaskHandle::detach);
            if let Err(error) = spawned {
                let missing = worker_len - index - 1;
                let exited = inner.exited.fetch_add(missing, Ordering::AcqRel) + missing;
                pool.inner.close(ShutdownMode::CancelPending);
                if exited == worker_len {
                    inner.shutdown_state.complete(Ok(()));
                }
                return Err(error);
            }
        }
        if worker_len == 0 {
            inner.shutdown_state.complete(Ok(()));
        }
        Ok(pool)
    }
    /// A pool of its own with no workers that refuses every job, as a target
    /// without threads has (wasm without atomics): callers take their
    /// synchronous path. The shared pool other users hold is not touched.
    pub fn closed() -> Self {
        let pool = Self::create(ThreadSpawner::for_current_thread(1), 0).unwrap();
        pool.inner.close(ShutdownMode::CancelPending);
        pool
    }
    /// Whether a job submitted through this handle is taken: the pool and the handle's jobs open.
    pub(crate) fn is_open(&self) -> bool {
        self.inner.state.lock().unwrap().closed == POOL_OPEN && self.set.is_open()
    }
    pub fn worker_count(&self) -> usize {
        self.inner.workers.len()
    }
    pub fn summary(&self) -> String {
        self.inner.summary()
    }
    pub fn submit<F, T>(&self, class: Class, f: F) -> Result<TaskHandle<T>, SubmitError>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        self.submit_named(class, std::any::type_name::<F>(), f)
    }
    pub fn submit_named<F, T>(
        &self,
        class: Class,
        label: &'static str,
        f: F,
    ) -> Result<TaskHandle<T>, SubmitError>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        self.submit_at(class, label, Ticket::none(), f)
    }
    pub fn submit_at<F, T>(
        &self,
        class: Class,
        label: &'static str,
        ticket: Ticket,
        f: F,
    ) -> Result<TaskHandle<T>, SubmitError>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        self.submit_with(class, label, ticket, f, false)
    }
    fn submit_with<F, T>(
        &self,
        class: Class,
        label: &'static str,
        ticket: Ticket,
        f: F,
        internal: bool,
    ) -> Result<TaskHandle<T>, SubmitError>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        let (mut job, handle) = task_job(self.inner.ui_thread, class, label, ticket, f, internal);
        job.member = Some(self.set.join(&job.token).ok_or(SubmitError::Closed)?);
        self.inner.enqueue(job)?;
        Ok(handle)
    }
    pub fn submit_internal<F, T>(&self, class: Class, f: F) -> Result<TaskHandle<T>, SubmitError>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        self.submit_with(class, std::any::type_name::<F>(), Ticket::none(), f, true)
    }
    pub fn submit_frame_job<F, T>(&self, class: Class, f: F) -> Result<FrameJob<T>, SubmitError>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        #[cfg(all(target_arch = "wasm32", not(target_feature = "atomics")))]
        {
            let _ = class;
            Ok(FrameJob {
                inner: FrameJobInner::Later {
                    job: Some(Box::new(f)),
                    result: None,
                    token: CancellationToken::new(),
                },
            })
        }
        #[cfg(not(all(target_arch = "wasm32", not(target_feature = "atomics"))))]
        self.submit(class, f).map(|task| FrameJob {
            inner: FrameJobInner::Task(task),
        })
    }
    /// Refuses this handle's (and its clones') new jobs; `CancelPending` also cancels the
    /// ones it submitted that have not finished. The pool and its other handles go on.
    pub fn close(&self, mode: ShutdownMode) {
        self.set.close(mode);
    }
    /// [`Self::close`], and a handle that finishes once every job this handle and its clones
    /// submitted has run or been cancelled.
    pub fn shutdown(&self, mode: ShutdownMode) -> TaskHandle<()> {
        TaskHandle {
            state: self.set.close(mode),
            token: CancellationToken::new(),
            ui_thread: self.inner.ui_thread,
            priority_status: priority_state(PriorityStatus::Applied),
            #[cfg(not(target_arch = "wasm32"))]
            native_join: None,
        }
    }
    /// Closes the pool itself, for every handle (the app's end: `Cx::close_task_pool`).
    pub(crate) fn close_pool(&self, mode: ShutdownMode) {
        self.inner.close(mode);
    }
    #[cfg(test)]
    fn shutdown_pool(&self, mode: ShutdownMode) -> TaskHandle<()> {
        self.close_pool(mode);
        self.shutdown_handle()
    }
    /// Finishes once the pool's workers have exited.
    #[cfg(test)]
    fn shutdown_handle(&self) -> TaskHandle<()> {
        TaskHandle {
            state: self.inner.shutdown_state.clone(),
            token: CancellationToken::new(),
            ui_thread: self.inner.ui_thread,
            priority_status: priority_state(PriorityStatus::Applied),
            #[cfg(not(target_arch = "wasm32"))]
            native_join: None,
        }
    }
}

struct BatchState {
    running: usize,
    queued: bool,
    closed: bool,
    panic: Option<Box<dyn Any + Send>>,
}
struct FanOutShared {
    f: *const (),
    call: unsafe fn(*const (), usize),
    next: AtomicUsize,
    len: usize,
    stopped: AtomicBool,
    state: Mutex<BatchState>,
    wake: Condvar,
    pool: Weak<PoolInner>,
    class: Class,
}
// Only dereferenced as &F (F: Sync), while the caller's completion guard lives.
unsafe impl Send for FanOutShared {}
unsafe impl Sync for FanOutShared {}
unsafe fn fan_out_call<F: Fn(usize) + Sync>(f: *const (), index: usize) {
    (*(f as *const F))(index);
}
impl FanOutShared {
    fn enqueue(self: &Arc<Self>) {
        let Some(pool) = self.pool.upgrade() else {
            return;
        };
        {
            let mut state = self.state.lock().unwrap();
            if state.closed
                || state.queued
                || state.running >= pool.workers.len().saturating_sub(1)
                || self.next.load(Ordering::Relaxed) >= self.len
            {
                return;
            }
            state.queued = true;
        }
        let shared = self.clone();
        let (job, handle) = task_job(
            pool.ui_thread,
            self.class,
            "fan_out",
            Ticket::none(),
            move || shared.helper(),
            true,
        );
        handle.detach();
        if pool.enqueue(job).is_err() {
            self.state.lock().unwrap().queued = false;
        }
    }
    fn helper(self: &Arc<Self>) {
        {
            let mut state = self.state.lock().unwrap();
            state.queued = false;
            if state.closed {
                return;
            }
            state.running += 1;
        }
        self.enqueue();
        JOB.with(|j| j.borrow_mut().as_mut().unwrap().helper = true);
        let result = catch_unwind(AssertUnwindSafe(|| self.work(true)));
        {
            let mut state = self.state.lock().unwrap();
            state.running -= 1;
            if let Err(panic) = result {
                if state.panic.is_none() {
                    state.panic = Some(panic);
                }
                state.closed = true;
                self.stopped.store(true, Ordering::Release);
            }
        }
        self.wake.notify_all();
        self.enqueue();
    }
    // Every index runs exactly once. A fan-out has no partial result, so a stale or
    // cancelled job still runs its remaining indices; skipping work is the closure's own
    // decision (its caller's cancel check). The checkpoint here only yields the permit.
    // A helper leaves at an item boundary when a higher class waits for its permit, or for
    // its thread: a higher-class job queued while every worker is busy (none can admit it
    // until one leaves).
    fn work(&self, helper: bool) {
        // One upgrade per helper, not one per item (the workers share the pool's count).
        let pool = if helper { self.pool.upgrade() } else { None };
        loop {
            let _ = checkpoint();
            if helper
                && (JOB.with(|j| {
                    j.borrow()
                        .as_ref()
                        .and_then(|j| j.permit)
                        .is_some_and(makepad_cpu_budget::should_yield)
                }) || pool.as_ref().is_some_and(|p| p.higher_queued(self.class)))
            {
                break;
            }
            if self.stopped.load(Ordering::Acquire) {
                break;
            }
            let index = self.next.fetch_add(1, Ordering::Relaxed);
            if index >= self.len {
                break;
            }
            unsafe {
                (self.call)(self.f, index);
            }
        }
    }
    fn finish(&self) {
        self.stopped.store(true, Ordering::Release);
        let mut state = self.state.lock().unwrap();
        state.closed = true;
        if state.running != 0 {
            without_permit(move || {
                while state.running != 0 {
                    state = self.wake.wait(state).unwrap();
                }
                drop(state);
            });
        }
    }
}
struct FanOutParent<'a>(&'a FanOutShared);
impl Drop for FanOutParent<'_> {
    fn drop(&mut self) {
        self.0.finish();
    }
}
impl TaskPool {
    pub fn fan_out<F: Fn(usize) + Sync>(&self, len: usize, f: F) {
        if len == 0 {
            return;
        }
        if std::thread::current().id() == self.inner.ui_thread {
            debug_assert!(false, "TaskPool::fan_out on the UI thread would block it");
            for i in 0..len {
                f(i);
            }
            return;
        }
        let class = current_class().unwrap_or(Class::Background);
        let (ticket, token) = JOB.with(|j| {
            j.borrow()
                .as_ref()
                .map(|j| (j.ticket.clone(), j.token.clone()))
                .unwrap_or_else(|| (Ticket::none(), CancellationToken::new()))
        });
        let _context = JobGuard::enter(class, "fan_out caller", ticket, token, None);
        let shared = Arc::new(FanOutShared {
            f: &f as *const F as *const (),
            call: fan_out_call::<F>,
            next: AtomicUsize::new(0),
            len,
            stopped: AtomicBool::new(false),
            state: Mutex::new(BatchState {
                running: 0,
                queued: false,
                closed: false,
                panic: None,
            }),
            wake: Condvar::new(),
            pool: Arc::downgrade(&self.inner),
            class,
        });
        let parent = FanOutParent(&shared);
        shared.enqueue();
        shared.work(false);
        drop(parent);
        let panic = shared.state.lock().unwrap().panic.take();
        if let Some(panic) = panic {
            std::panic::resume_unwind(panic);
        }
    }
}

// A scoped closure can be claimed either by a worker or by its caller. The
// pool's queue only owns this container, never an unguarded borrowed closure.
struct ScopedJob {
    job: Mutex<Option<PoolJob>>,
    finished: Arc<TaskState<()>>,
    panicked: Arc<AtomicBool>,
    joined: AtomicBool,
}
struct ScopedFinished(Arc<TaskState<()>>);
impl Drop for ScopedFinished {
    fn drop(&mut self) {
        self.0.complete(Ok(()));
    }
}
impl ScopedJob {
    fn run(&self, pool: &PoolInner) {
        let job = self.job.lock().unwrap().take();
        if let Some(job) = job {
            let _finished = ScopedFinished(self.finished.clone());
            pool.run_job(job, None);
        }
    }
    fn cancel(&self) {
        let job = self.job.lock().unwrap().take();
        if let Some(job) = job {
            // Publish only after every borrowed capture has been destroyed.
            let _finished = ScopedFinished(self.finished.clone());
            drop(job);
        }
    }
}
struct ScopedQueued(Arc<ScopedJob>);
impl ScopedQueued {
    fn run(&self, pool: &PoolInner) {
        self.0.run(pool);
    }
}
impl Drop for ScopedQueued {
    fn drop(&mut self) {
        self.0.cancel();
    }
}
pub struct TaskScope<'scope, 'env: 'scope> {
    pool: &'env TaskPool,
    class: Class,
    jobs: Mutex<Vec<Arc<ScopedJob>>>,
    scope: std::marker::PhantomData<&'scope mut &'scope ()>,
    env: std::marker::PhantomData<&'env mut &'env ()>,
}
pub struct ScopedTaskHandle<'scope, T> {
    task: TaskHandle<T>,
    job: Arc<ScopedJob>,
    pool: &'scope TaskPool,
    scope: std::marker::PhantomData<&'scope mut &'scope ()>,
}
impl<T> ScopedTaskHandle<'_, T> {
    pub fn join(self) -> Result<T, TaskError> {
        if std::thread::current().id() == self.task.ui_thread {
            return Err(TaskError::WouldBlockUi);
        }
        self.job.run(&self.pool.inner);
        let result = without_permit(|| self.task.join());
        self.job.joined.store(true, Ordering::Release);
        result
    }
    pub fn is_finished(&self) -> bool {
        self.task.is_finished()
    }
    pub fn cancel(&self) {
        self.task.cancel();
    }
}
impl<'scope, 'env> TaskScope<'scope, 'env> {
    pub fn spawn<F, T>(&'scope self, f: F) -> ScopedTaskHandle<'scope, T>
    where
        F: FnOnce() -> T + Send + 'scope,
        T: Send + 'scope,
    {
        let state = Arc::new(TaskState::default());
        let result_state = state.clone();
        let cancel_state = state.clone();
        let token = CancellationToken::new();
        let finished = Arc::new(TaskState::default());
        let panicked = Arc::new(AtomicBool::new(false));
        let run_panicked = panicked.clone();
        let run: Box<dyn FnOnce() + Send + 'scope> = Box::new(move || {
            let result =
                catch_unwind(AssertUnwindSafe(f)).map_err(|p| TaskError::Panicked(panic_report(p)));
            run_panicked.store(result.is_err(), Ordering::Release);
            result_state.complete(result);
        });
        let cancel: Box<dyn FnOnce() + Send + 'scope> = Box::new(move || {
            cancel_state.complete(Err(TaskError::Cancelled));
        });
        // scope drains/joins every ScopedJob even on unwind; queued wrappers
        // can outlive it only after their borrowed job has been taken away.
        let (run, cancel) = unsafe {
            (
                std::mem::transmute::<Box<dyn FnOnce() + Send + 'scope>, Box<dyn FnOnce() + Send>>(
                    run,
                ),
                std::mem::transmute::<Box<dyn FnOnce() + Send + 'scope>, Box<dyn FnOnce() + Send>>(
                    cancel,
                ),
            )
        };
        let job = Arc::new(ScopedJob {
            job: Mutex::new(Some(PoolJob {
                class: self.class,
                label: std::any::type_name::<F>(),
                submitted_at: Cx::monotonic_now(),
                ticket: Ticket::none(),
                token: token.clone(),
                run: Some(run),
                cancel: Some(cancel),
                // borrowing jobs end inside their scope: its caller waits for them
                member: None,
            })),
            finished,
            panicked,
            joined: AtomicBool::new(false),
        });
        self.jobs.lock().unwrap().push(job.clone());
        let queued = ScopedQueued(job.clone());
        let pool = self.pool.inner.clone();
        // If the pool is closed, dropping this closure completes the scoped job.
        if let Ok(handle) = self
            .pool
            .submit_internal(self.class, move || queued.run(&pool))
        {
            handle.detach();
        }
        ScopedTaskHandle {
            task: TaskHandle {
                state,
                token,
                ui_thread: self.pool.inner.ui_thread,
                priority_status: priority_state(PriorityStatus::Pending),
                #[cfg(not(target_arch = "wasm32"))]
                native_join: None,
            },
            job,
            pool: self.pool,
            scope: std::marker::PhantomData,
        }
    }
    fn finish(&self) -> bool {
        let mut panicked = false;
        loop {
            let jobs = std::mem::take(&mut *self.jobs.lock().unwrap());
            if jobs.is_empty() {
                break;
            }
            // Run all still-queued children before waiting for any running one.
            for job in &jobs {
                // Cleanup of a cancelled closure can itself unwind. Finish
                // every other borrow before allowing the scope to return.
                panicked |= catch_unwind(AssertUnwindSafe(|| job.run(&self.pool.inner))).is_err();
            }
            without_permit(|| {
                for job in jobs {
                    let mut result = job.finished.result.lock().unwrap();
                    while result.is_none() {
                        result = job.finished.wake.wait(result).unwrap();
                    }
                    panicked |=
                        job.panicked.load(Ordering::Acquire) && !job.joined.load(Ordering::Acquire);
                }
            });
        }
        panicked
    }
}
impl TaskPool {
    pub fn scope<'env, F, R>(&'env self, f: F) -> R
    where
        F: for<'scope> FnOnce(&'scope TaskScope<'scope, 'env>) -> R,
    {
        assert_ne!(
            std::thread::current().id(),
            self.inner.ui_thread,
            "TaskPool::scope on the UI thread would block it"
        );
        let scope = TaskScope {
            pool: self,
            class: current_class().unwrap_or(Class::Background),
            jobs: Mutex::new(Vec::new()),
            scope: std::marker::PhantomData,
            env: std::marker::PhantomData,
        };
        let result = catch_unwind(AssertUnwindSafe(|| f(&scope)));
        let panicked = scope.finish();
        match result {
            Err(panic) => std::panic::resume_unwind(panic),
            Ok(value) => {
                assert!(!panicked, "a scoped task panicked");
                value
            }
        }
    }
}

struct StrandState<S> {
    value: Option<S>,
    jobs: VecDeque<(Ticket, PoolJob)>,
    scheduled: bool,
}
struct StrandInner<S> {
    pool: TaskPool,
    label: &'static str,
    state: Mutex<StrandState<S>>,
}
/// Serial owner jobs. Tickets replace queued work; once started, a job
/// finishes with its state intact. Use checkpoints to yield its CPU budget.
pub struct Strand<S = ()> {
    inner: Arc<StrandInner<S>>,
}
impl Strand {
    pub fn new(pool: &TaskPool, label: &'static str) -> Self {
        Self::with_state(pool, label, ())
    }
}
impl<S: Send + 'static> Strand<S> {
    pub fn with_state(pool: &TaskPool, label: &'static str, value: S) -> Self {
        Self {
            inner: Arc::new(StrandInner {
                pool: pool.clone(),
                label,
                state: Mutex::new(StrandState {
                    value: Some(value),
                    jobs: VecDeque::new(),
                    scheduled: false,
                }),
            }),
        }
    }
    pub fn submit<F, T>(&self, class: Class, ticket: Ticket, f: F) -> TaskHandle<T>
    where
        F: FnOnce(&mut S) -> T + Send + 'static,
        T: Send + 'static,
    {
        let inner = self.inner.clone();
        let (job, handle) = task_job(
            self.inner.pool.inner.ui_thread,
            class,
            self.inner.label,
            Ticket::none(),
            move || {
                // Move the owner state out: checkpoints in user code must never
                // see a strand lock held, including when the job unwinds.
                let mut value = inner.state.lock().unwrap().value.take().unwrap();
                let result = catch_unwind(AssertUnwindSafe(|| f(&mut value)));
                inner.state.lock().unwrap().value = Some(value);
                match result {
                    Ok(value) => value,
                    Err(panic) => std::panic::resume_unwind(panic),
                }
            },
            false,
        );
        let mut job = job;
        let Some(member) = self.inner.pool.set.join(&job.token) else {
            drop(job);
            return handle;
        };
        job.member = Some(member);
        let schedule = {
            let pool = self.inner.pool.inner.state.lock().unwrap();
            if pool.closed != POOL_OPEN {
                drop(pool);
                drop(job);
                return handle;
            }
            let mut state = self.inner.state.lock().unwrap();
            state.jobs.push_back((ticket, job));
            let schedule = !state.scheduled;
            state.scheduled = true;
            schedule
        };
        if schedule {
            self.inner.schedule();
        }
        handle
    }
}
struct StrandQueued<S> {
    strand: Arc<StrandInner<S>>,
    armed: bool,
}
impl<S> Drop for StrandQueued<S> {
    fn drop(&mut self) {
        if self.armed {
            let jobs = {
                let mut state = self.strand.state.lock().unwrap();
                state.scheduled = false;
                std::mem::take(&mut state.jobs)
            };
            drop(jobs);
        }
    }
}
impl<S: Send + 'static> StrandInner<S> {
    fn schedule(self: &Arc<Self>) {
        let class = {
            let mut state = self.state.lock().unwrap();
            match state.jobs.front() {
                Some((_, job)) => job.class,
                None => {
                    state.scheduled = false;
                    return;
                }
            }
        };
        let mut queued = StrandQueued {
            strand: self.clone(),
            armed: true,
        };
        let (job, handle) = task_job(
            self.pool.inner.ui_thread,
            class,
            self.label,
            Ticket::none(),
            move || {
                let job = queued.strand.state.lock().unwrap().jobs.pop_front();
                if let Some((ticket, job)) = job {
                    if ticket.is_stale() {
                        drop(job);
                    } else {
                        queued.strand.pool.inner.run_job(job, None);
                    }
                }
                queued.armed = false;
                queued.strand.schedule();
            },
            false,
        );
        handle.detach();
        self.pool.inner.continue_strand(job);
    }
}

fn signal_completion(internal: bool) {
    #[cfg(test)]
    let _ = internal;
    #[cfg(not(test))]
    if internal {
        SignalToUI::set_internal_signal();
    } else {
        SignalToUI::set_ui_signal();
    }
}

/// A UI-owned staging queue in front of the pool for work that needs
/// per-key replacement, newest-first order or pruning of what has not started
/// yet (map tiles, image decodes, archive reads).
///
/// Nothing here locks: the queue lives on the thread that owns it, hands at
/// most `in_flight_limit` jobs to the pool at a time and keeps the rest where
/// `retain`/replace can still reach them. Call [`pump`](Self::pump) after
/// pushes and on every `Event::Signal` (each pool completion raises it) so
/// freed slots are refilled.
pub struct TaskQueue<K> {
    class: Class,
    in_flight_limit: usize,
    in_flight: Arc<AtomicUsize>,
    staged: VecDeque<(K, Box<dyn FnOnce() + Send>)>,
}

impl<K> fmt::Debug for TaskQueue<K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TaskQueue")
            .field("class", &self.class)
            .field("staged", &self.staged.len())
            .field("in_flight", &self.in_flight.load(Ordering::Relaxed))
            .field("in_flight_limit", &self.in_flight_limit)
            .finish()
    }
}

struct InFlightGuard(Arc<AtomicUsize>);

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

impl<K: PartialEq> TaskQueue<K> {
    /// `in_flight_limit` is how many jobs may sit with the pool at once —
    /// the pool's worker count keeps every worker busy while the staging
    /// order still rules everything else.
    pub fn new(class: Class, in_flight_limit: usize, capacity: usize) -> Self {
        Self {
            class,
            in_flight_limit: in_flight_limit.max(1),
            in_flight: Arc::new(AtomicUsize::new(0)),
            staged: VecDeque::with_capacity(capacity),
        }
    }

    pub fn class(&self) -> Class {
        self.class
    }

    /// Resize the in-flight window, typically to the pool's worker count
    /// once it is known. Zero holds everything in staging until the next
    /// `pump` with a wider window.
    pub fn set_in_flight_limit(&mut self, limit: usize) {
        self.in_flight_limit = limit;
    }

    pub fn staged_len(&self) -> usize {
        self.staged.len()
    }

    pub fn in_flight(&self) -> usize {
        self.in_flight.load(Ordering::Acquire)
    }

    pub fn is_idle(&self) -> bool {
        self.staged.is_empty() && self.in_flight() == 0
    }

    /// Stage a job and hand over what fits. With `replace_queued` a staged
    /// job with the same key is dropped first. `Lifo` makes this job the
    /// next to run.
    pub fn push<F>(
        &mut self,
        pool: &TaskPool,
        key: K,
        replace_queued: bool,
        order: QueueOrder,
        job: F,
    ) -> Result<(), SubmitError>
    where
        F: FnOnce() + Send + 'static,
    {
        if !pool.is_open() {
            return Err(SubmitError::Closed);
        }
        if replace_queued {
            self.staged.retain(|(staged_key, _)| *staged_key != key);
        }
        let job: Box<dyn FnOnce() + Send> = Box::new(job);
        match order {
            QueueOrder::Fifo => self.staged.push_back((key, job)),
            QueueOrder::Lifo => self.staged.push_front((key, job)),
        }
        self.pump(pool);
        Ok(())
    }

    /// Hand staged jobs to the pool while the in-flight window has room.
    pub fn pump(&mut self, pool: &TaskPool) {
        while !self.staged.is_empty()
            && self.in_flight.load(Ordering::Acquire) < self.in_flight_limit
        {
            if !pool.is_open() { break; }
            let Some((_, job)) = self.staged.pop_front() else {
                break;
            };
            self.in_flight.fetch_add(1, Ordering::AcqRel);
            let guard = InFlightGuard(self.in_flight.clone());
            if let Ok(handle) = pool.submit(self.class, move || {
                let _guard = guard;
                job();
            }) { handle.detach(); }
        }
    }

    /// Drop staged jobs whose key fails `keep`; returns the dropped keys.
    /// Jobs already with the pool are not affected.
    pub fn retain(&mut self, mut keep: impl FnMut(&K) -> bool) -> Vec<K>
    where
        K: Clone,
    {
        let mut dropped = Vec::new();
        self.staged.retain(|(key, _)| {
            if keep(key) {
                true
            } else {
                dropped.push(key.clone());
                false
            }
        });
        dropped
    }

    pub fn contains(&self, key: &K) -> bool {
        self.staged.iter().any(|(staged_key, _)| staged_key == key)
    }

    pub fn clear(&mut self) {
        self.staged.clear();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MissedTick {
    Skip,
    CoalesceOne,
}

enum TimerCallback {
    Once(Option<Box<dyn FnOnce() + Send>>),
    Interval(Box<dyn FnMut() + Send>),
}

struct TimerEntry {
    deadline: f64,
    period: Option<f64>,
    missed: MissedTick,
    external_token: CancellationToken,
    handle_token: CancellationToken,
    callback: TimerCallback,
}

struct SchedulerState {
    inner: Mutex<SchedulerInner>,
    next_id: AtomicU32,
}

struct SchedulerInner {
    entries: Vec<TimerEntry>,
    armed: Option<(Timer, f64)>,
}

static SCHEDULER_STATE: OnceLock<Arc<SchedulerState>> = OnceLock::new();

#[derive(Clone)]
/// Deadline scheduler backed by one re-armed Makepad platform timer. Callbacks
/// run during timer dispatch; blocking work should be submitted to a pool.
pub struct Scheduler {
    state: Arc<SchedulerState>,
    runtime_open: Arc<AtomicBool>,
}

#[must_use = "dropping a timer handle cancels the timer"]
pub struct TimerHandle {
    id: u64,
    token: CancellationToken,
}

impl TimerHandle {
    pub fn cancel(&self) {
        self.token.cancel();
        wake_scheduler_ui();
    }

    pub fn id(&self) -> u64 {
        self.id
    }
}

impl Drop for TimerHandle {
    fn drop(&mut self) {
        self.cancel();
    }
}

impl Scheduler {
    pub fn new(spawner: ThreadSpawner) -> Result<Self, SpawnError> {
        if !spawner.runtime_open.load(Ordering::Acquire) {
            return Err(SpawnError::RuntimeClosed);
        }
        let new_state = || {
            Arc::new(SchedulerState {
                inner: Mutex::new(SchedulerInner {
                    entries: Vec::new(),
                    armed: None,
                }),
                next_id: AtomicU32::new(1),
            })
        };
        #[cfg(not(test))]
        let state = SCHEDULER_STATE.get_or_init(new_state).clone();
        #[cfg(test)]
        let state = new_state();
        Ok(Self {
            state,
            runtime_open: spawner.runtime_open,
        })
    }

    pub fn interval<F>(
        &self,
        period: Duration,
        missed: MissedTick,
        token: CancellationToken,
        job: F,
    ) -> Result<TimerHandle, SpawnError>
    where
        F: FnMut() + Send + 'static,
    {
        if period.is_zero() {
            return Err(SpawnError::Backend("timer period must be non-zero".into()));
        }
        self.insert(
            Cx::monotonic_now() + period.as_secs_f64(),
            Some(period.as_secs_f64()),
            missed,
            token,
            TimerCallback::Interval(Box::new(job)),
        )
    }

    pub fn at<F>(
        &self,
        deadline: f64,
        token: CancellationToken,
        job: F,
    ) -> Result<TimerHandle, SpawnError>
    where
        F: FnOnce() + Send + 'static,
    {
        self.insert(
            deadline,
            None,
            MissedTick::Skip,
            token,
            TimerCallback::Once(Some(Box::new(job))),
        )
    }

    fn insert(
        &self,
        deadline: f64,
        period: Option<f64>,
        missed: MissedTick,
        external_token: CancellationToken,
        callback: TimerCallback,
    ) -> Result<TimerHandle, SpawnError> {
        if !self.runtime_open.load(Ordering::Acquire) {
            return Err(SpawnError::RuntimeClosed);
        }
        if !deadline.is_finite() {
            return Err(SpawnError::Backend("timer deadline must be finite".into()));
        }
        let id = self.state.next_id.fetch_add(1, Ordering::Relaxed).max(1) as u64;
        let handle_token = CancellationToken::new();
        self.state.inner.lock().unwrap_or_else(|e| e.into_inner()).entries.push(TimerEntry {
            deadline,
            period,
            missed,
            external_token,
            handle_token: handle_token.clone(),
            callback,
        });
        wake_scheduler_ui();
        Ok(TimerHandle {
            id,
            token: handle_token,
        })
    }
}

fn wake_scheduler_ui() {
    // The app signal, not the internal one: `service_scheduler` re-arms the
    // platform timer from `call_event_handler`, which only the app half runs.
    #[cfg(not(test))]
    SignalToUI::set_ui_signal();
}

fn run_scheduler_due(state: &Arc<SchedulerState>, now: f64) {
    let mut due = Vec::new();
    {
        let mut inner = state.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.entries.retain(|entry| {
            !entry.external_token.is_cancelled() && !entry.handle_token.is_cancelled()
        });
        let mut index = 0;
        while index < inner.entries.len() {
            if inner.entries[index].deadline <= now {
                due.push(inner.entries.swap_remove(index));
            } else {
                index += 1;
            }
        }
    }

    for mut entry in due {
        if entry.external_token.is_cancelled() || entry.handle_token.is_cancelled() {
            continue;
        }
        match &mut entry.callback {
            TimerCallback::Once(callback) => {
                if let Some(callback) = callback.take() {
                    let _ = catch_unwind(AssertUnwindSafe(callback));
                }
            }
            TimerCallback::Interval(callback) => {
                let _ = catch_unwind(AssertUnwindSafe(&mut **callback));
                if entry.external_token.is_cancelled() || entry.handle_token.is_cancelled() {
                    continue;
                }
                let period = entry.period.unwrap();
                let after = Cx::monotonic_now().max(now);
                entry.deadline = match entry.missed {
                    MissedTick::Skip => after + period,
                    MissedTick::CoalesceOne => {
                        let next = entry.deadline + period;
                        if next <= after {
                            after + period
                        } else {
                            next
                        }
                    }
                };
                state.inner.lock().unwrap_or_else(|e| e.into_inner()).entries.push(entry);
            }
        }
    }
}

/// Drive the global scheduler from Makepad's single platform timer source.
/// Insertion/cancellation only wakes the UI so this function can re-arm that
/// source; no worker or polling loop exists between deadlines.
pub(crate) fn service_scheduler(cx: &mut Cx, event: &Event) {
    let Some(state) = SCHEDULER_STATE.get().cloned() else {
        return;
    };
    if matches!(event, Event::Shutdown) {
        let mut inner = state.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((timer, _)) = inner.armed.take() {
            cx.stop_timer(timer);
        }
        inner.entries.clear();
        return;
    }

    let fired = match event {
        Event::Timer(timer) => Some(timer.timer_id),
        _ => None,
    };
    {
        let mut inner = state.inner.lock().unwrap_or_else(|e| e.into_inner());
        if inner.armed.is_some_and(|(timer, _)| Some(timer.0) == fired) {
            inner.armed = None;
        }
    }

    let now = Cx::monotonic_now();
    run_scheduler_due(&state, now);

    let mut inner = state.inner.lock().unwrap_or_else(|e| e.into_inner());
    inner
        .entries
        .retain(|entry| !entry.external_token.is_cancelled() && !entry.handle_token.is_cancelled());
    let next = inner
        .entries
        .iter()
        .map(|entry| entry.deadline)
        .min_by(f64::total_cmp);
    if inner.armed.map(|(_, deadline)| deadline) != next {
        if let Some((timer, _)) = inner.armed.take() {
            cx.stop_timer(timer);
        }
        if let Some(deadline) = next {
            let timer = cx.start_timeout((deadline - now).max(0.000_001));
            inner.armed = Some((timer, deadline));
        }
    }
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WebWorkerStage {
    Requested,
    Started,
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WebWorkerTerminal {
    Finished,
    Trapped,
    FailedToStart,
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Default)]
pub(crate) struct WebWorkerBookkeeping {
    requests: HashMap<u32, WebWorkerStage>,
}

#[cfg_attr(not(test), allow(dead_code))]
impl WebWorkerBookkeeping {
    pub(crate) fn request(&mut self, id: u32) -> bool {
        self.requests
            .insert(id, WebWorkerStage::Requested)
            .is_none()
    }

    pub(crate) fn started(&mut self, id: u32) -> bool {
        match self.requests.get_mut(&id) {
            Some(stage @ WebWorkerStage::Requested) => {
                *stage = WebWorkerStage::Started;
                true
            }
            _ => false,
        }
    }

    pub(crate) fn terminal(&mut self, id: u32, terminal: WebWorkerTerminal) -> bool {
        let valid = matches!(
            (self.requests.get(&id), terminal),
            (
                Some(WebWorkerStage::Requested),
                WebWorkerTerminal::FailedToStart
            ) | (Some(WebWorkerStage::Started), WebWorkerTerminal::Finished)
                | (Some(WebWorkerStage::Started), WebWorkerTerminal::Trapped)
        );
        if valid {
            self.requests.remove(&id);
        }
        valid
    }

    pub(crate) fn len(&self) -> usize {
        self.requests.len()
    }
}

#[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
type WebClosure = Box<dyn FnOnce() + Send + 'static>;

#[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
trait WebCompletion: Send + Sync {
    fn complete_error(&self, error: TaskError);
}

#[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
impl<T: Send + 'static> WebCompletion for TaskState<T> {
    fn complete_error(&self, error: TaskError) {
        self.complete(Err(error));
    }
}

#[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
struct WebRequest {
    context_ptr: u32,
    completion: Arc<dyn WebCompletion>,
    stage: WebWorkerStage,
}

#[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
fn web_requests() -> &'static Mutex<HashMap<u32, WebRequest>> {
    static REQUESTS: OnceLock<Mutex<HashMap<u32, WebRequest>>> = OnceLock::new();
    REQUESTS.get_or_init(|| Mutex::new(HashMap::new()))
}

#[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
fn spawn_web_task<T: Send + 'static>(
    ui_thread: ThreadId,
    options: ThreadOptions,
    state: Arc<TaskState<T>>,
    token: CancellationToken,
    run: impl FnOnce() + Send + 'static,
) -> Result<TaskHandle<T>, SpawnError> {
    static NEXT_REQUEST: AtomicU32 = AtomicU32::new(1);
    let request_id = NEXT_REQUEST.fetch_add(1, Ordering::Relaxed).max(1);
    let closure: WebClosure = Box::new(run);
    let context_ptr = Box::into_raw(Box::new(closure)) as u32;
    web_requests().lock().unwrap_or_else(|e| e.into_inner()).insert(
        request_id,
        WebRequest {
            context_ptr,
            completion: state.clone(),
            stage: WebWorkerStage::Requested,
        },
    );
    let stack_size = options.stack_size.unwrap_or(DEFAULT_WEB_THREAD_STACK_SIZE) as u32;
    let name = options.name.as_deref().unwrap_or("");
    let accepted = unsafe {
        js_spawn_thread(
            request_id,
            context_ptr,
            stack_size,
            name.as_ptr(),
            name.len(),
        )
    };
    if accepted == 0 {
        if let Some(request) = web_requests().lock().unwrap_or_else(|e| e.into_inner()).remove(&request_id) {
            unsafe { drop(Box::from_raw(request.context_ptr as *mut WebClosure)) };
        }
        return Err(SpawnError::Unsupported);
    }
    Ok(TaskHandle {
        state,
        token,
        ui_thread,
        priority_status: priority_state(PriorityStatus::BestEffortUnsupported),
    })
}

#[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
#[link(wasm_import_module = "env")]
extern "C" {
    fn js_spawn_thread(
        request_id: u32,
        context_ptr: u32,
        stack_size: u32,
        name_ptr: *const u8,
        name_len: usize,
    ) -> u32;
}

#[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
#[export_name = "wasm_thread_entrypoint"]
pub unsafe extern "C" fn wasm_thread_entrypoint(_request_id: u32, closure_ptr: u32) {
    let closure = Box::from_raw(closure_ptr as *mut WebClosure);
    closure();
    // JavaScript posts `finished` after this returns and that UI callback
    // removes the request. Duplicating cleanup here only creates a
    // worker/UI mutex race when the UI starts the next task.
    crate::web_alloc::thread_exit();
}

#[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
#[export_name = "wasm_thread_started"]
pub extern "C" fn wasm_thread_started(request_id: u32) {
    if let Some(request) = web_requests().lock().unwrap_or_else(|e| e.into_inner()).get_mut(&request_id) {
        request.stage = WebWorkerStage::Started;
    }
}

#[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
#[export_name = "wasm_thread_failed_to_start"]
pub unsafe extern "C" fn wasm_thread_failed_to_start(request_id: u32) {
    if let Some(request) = web_requests().lock().unwrap_or_else(|e| e.into_inner()).remove(&request_id) {
        drop(Box::from_raw(request.context_ptr as *mut WebClosure));
        request
            .completion
            .complete_error(TaskError::Spawn(SpawnError::Backend(
                "web worker failed to start".into(),
            )));
    }
}

#[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
#[export_name = "wasm_thread_worker_lost"]
pub extern "C" fn wasm_thread_worker_lost(request_id: u32) {
    if let Some(request) = web_requests().lock().unwrap_or_else(|e| e.into_inner()).remove(&request_id) {
        request.completion.complete_error(TaskError::WorkerLost);
    }
}

#[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
#[export_name = "wasm_thread_finished"]
pub extern "C" fn wasm_thread_finished(request_id: u32) {
    web_requests().lock().unwrap_or_else(|e| e.into_inner()).remove(&request_id);
}

#[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
#[export_name = "wasm_thread_alloc_tls_and_stack"]
pub extern "C" fn wasm_thread_alloc_tls_and_stack(words: u32) -> u32 {
    let allocation = vec![0_u64; words as usize].into_boxed_slice();
    Box::into_raw(allocation) as *mut u64 as u32
}

#[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
#[export_name = "wasm_thread_dealloc_tls_and_stack"]
pub unsafe extern "C" fn wasm_thread_dealloc_tls_and_stack(ptr: u32, words: u32) {
    let slice = std::ptr::slice_from_raw_parts_mut(ptr as *mut u64, words as usize);
    drop(Box::from_raw(slice));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    static POOL_TESTS: Mutex<()> = Mutex::new(());

    fn worker_join<T: Send + 'static>(handle: TaskHandle<T>) -> Result<T, TaskError> {
        std::thread::spawn(move || handle.join()).join().unwrap()
    }

    fn wait_for(what: &str, mut ready: impl FnMut() -> bool) {
        let deadline = Cx::monotonic_now() + 10.0;
        while !ready() {
            assert!(
                Cx::monotonic_now() < deadline,
                "timed out waiting for {what}"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn test_pool(workers: usize) -> TaskPool {
        makepad_cpu_budget::end_warmup();
        TaskPool::create(ThreadSpawner::for_current_thread(workers + 1), workers).unwrap()
    }

    /// A job that reports it started and then waits for a release.
    fn gate_job(
        started: mpsc::Sender<()>,
        release: mpsc::Receiver<u32>,
    ) -> impl FnOnce() -> u32 + Send + 'static {
        move || {
            started.send(()).unwrap();
            release.recv().unwrap_or(0)
        }
    }

    #[test]
    fn worker_completion_is_taken_once_and_ui_join_is_refused() {
        let _serial = POOL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
        let spawner = ThreadSpawner::for_current_thread(2);
        let mut handle = spawner
            .spawn_worker(ThreadOptions::default(), || 42)
            .unwrap();
        while !handle.is_finished() {
            std::thread::yield_now();
        }
        assert_eq!(handle.try_take().unwrap().unwrap(), 42);
        assert!(handle.try_take().is_none());

        let handle = spawner
            .spawn_worker(ThreadOptions::default(), || 7)
            .unwrap();
        assert_eq!(handle.join(), Err(TaskError::WouldBlockUi));
    }

    #[test]
    fn task_poll_retries_instead_of_waiting_for_the_result_mutex() {
        let _serial = POOL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
        let state = Arc::new(TaskState::default());
        let mut handle = TaskHandle {
            state: state.clone(),
            token: CancellationToken::new(),
            ui_thread: std::thread::current().id(),
            priority_status: priority_state(PriorityStatus::Applied),
            native_join: None,
        };
        let mut slot = state.result.lock().unwrap();
        *slot = Some(Ok(42));
        state.finished.store(true, Ordering::Release);

        assert!(handle.try_take().is_none());
        drop(slot);
        assert_eq!(handle.try_take().unwrap().unwrap(), 42);
    }

    #[test]
    fn worker_name_stack_and_panic_are_reported() {
        let _serial = POOL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
        let spawner = ThreadSpawner::for_current_thread(2);
        let named = spawner
            .spawn_worker(
                ThreadOptions {
                    name: Some("runtime-test-name".into()),
                    stack_size: Some(MIN_THREAD_STACK_SIZE),
                    ..Default::default()
                },
                || std::thread::current().name().map(str::to_owned),
            )
            .unwrap();
        assert_eq!(
            worker_join(named).unwrap().as_deref(),
            Some("runtime-test-name")
        );
        assert!(matches!(
            spawner.spawn_worker(
                ThreadOptions {
                    stack_size: Some(MIN_THREAD_STACK_SIZE - 1),
                    ..Default::default()
                },
                || (),
            ),
            Err(SpawnError::InvalidStackSize { requested })
                if requested == MIN_THREAD_STACK_SIZE - 1
        ));
        let prioritized = spawner
            .spawn_worker(
                ThreadOptions {
                    priority: CxThreadPriority::Utility,
                    ..Default::default()
                },
                || (),
            )
            .unwrap();
        wait_for("priority application", || {
            prioritized.priority_status() != PriorityStatus::Pending
        });
        assert_eq!(prioritized.priority_status(), PriorityStatus::Applied);
        worker_join(prioritized).unwrap();
        let panicked = spawner
            .spawn_worker(ThreadOptions::default(), || panic!("completion panic"))
            .unwrap();
        assert!(matches!(worker_join(panicked), Err(TaskError::Panicked(_))));
    }

    #[test]
    fn cancellation_wakes_waiter() {
        let _serial = POOL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
        let spawner = ThreadSpawner::for_current_thread(2);
        let token = CancellationToken::new();
        let waiter_token = token.clone();
        let handle = spawner
            .spawn_worker(ThreadOptions::default(), move || {
                waiter_token.wait_until(Cx::monotonic_now() + 30.0)
            })
            .unwrap();
        token.cancel();
        assert_eq!(worker_join(handle).unwrap(), WaitOutcome::Cancelled);
    }

    #[test]
    fn closed_runtime_refuses_new_work() {
        let _serial = POOL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
        let spawner = ThreadSpawner::for_current_thread(2);
        spawner.close_runtime();
        assert!(matches!(
            spawner.spawn_worker(ThreadOptions::default(), || ()),
            Err(SpawnError::RuntimeClosed)
        ));
        assert!(matches!(
            spawner.scheduler(),
            Err(SpawnError::RuntimeClosed)
        ));
        assert!(matches!(
            TaskPool::create(spawner, 1),
            Err(SpawnError::RuntimeClosed)
        ));
    }

    #[test]
    fn cancellation_registration_race_wakes_every_waiter_and_reuses_nodes() {
        let _serial = POOL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
        let token = CancellationToken::new();
        let waiters: Vec<_> = (0..8)
            .map(|_| {
                let token = token.clone();
                std::thread::spawn(move || {
                    // Multiple waits on one thread reuse one registry node.
                    let _ = token.wait_until(Cx::monotonic_now() - 1.0);
                    token.wait_until(Cx::monotonic_now() + 10.0)
                })
            })
            .collect();
        // Some registrations may still be in flight.
        token.cancel();
        for waiter in waiters {
            assert_eq!(waiter.join().unwrap(), WaitOutcome::Cancelled);
        }
    }

    #[test]
    fn task_poll_does_not_join_a_published_but_live_thread() {
        let _serial = POOL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
        let state = Arc::new(TaskState::default());
        let worker_state = state.clone();
        let (release_tx, release_rx) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            worker_state.complete(Ok(42));
            release_rx.recv().unwrap();
        });
        let mut handle = TaskHandle {
            state,
            token: CancellationToken::new(),
            ui_thread: std::thread::current().id(),
            priority_status: priority_state(PriorityStatus::Applied),
            native_join: Some(thread),
        };
        wait_for("payload publication", || handle.is_finished());
        // This must return before the thread is allowed to exit.
        assert_eq!(handle.try_take(), Some(Ok(42)));
        release_tx.send(()).unwrap();
    }

    #[test]
    fn pool_workers_are_warm_before_the_first_job() {
        let _serial = POOL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
        let pool = test_pool(3);
        wait_for("workers to start", || {
            pool.inner.started.load(Ordering::Acquire) == 3
        });
        assert_eq!(pool.inner.started.load(Ordering::Acquire), 3);
        assert_eq!(pool.inner.completed.load(Ordering::Relaxed), 0);
        assert_eq!(pool.worker_count(), 3);

        let first = pool.submit(Class::Visible, || 5).unwrap();
        assert_eq!(worker_join(first).unwrap(), 5);
        assert_eq!(pool.inner.completed.load(Ordering::Relaxed), 1);

        worker_join(pool.shutdown_pool(ShutdownMode::Drain)).unwrap();
        assert_eq!(pool.inner.exited.load(Ordering::Acquire), 3);
        assert!(!pool.is_open());
        assert!(matches!(
            pool.submit(Class::Visible, || ()),
            Err(SubmitError::Closed)
        ));
    }

    #[test]
    fn pool_jobs_complete_in_order_of_availability() {
        let _serial = POOL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
        let pool = test_pool(2);
        let order = Arc::new(Mutex::new(Vec::new()));
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let blocking_order = order.clone();
        let blocking = pool
            .submit(Class::Visible, {
                let gate = gate_job(started_tx, release_rx);
                move || {
                    let value = gate();
                    blocking_order.lock().unwrap().push("A");
                    value
                }
            })
            .unwrap();
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();

        let mut handles = Vec::new();
        for tag in ["B", "C", "D"] {
            let order = order.clone();
            handles.push(
                pool.submit(Class::Visible, move || {
                    order.lock().unwrap().push(tag);
                })
                .unwrap(),
            );
        }
        for handle in handles {
            worker_join(handle).unwrap();
        }
        assert_eq!(*order.lock().unwrap(), vec!["B", "C", "D"]);

        release_tx.send(9).unwrap();
        assert_eq!(worker_join(blocking).unwrap(), 9);
        assert_eq!(*order.lock().unwrap(), vec!["B", "C", "D", "A"]);
        worker_join(pool.shutdown_pool(ShutdownMode::Drain)).unwrap();
    }

    #[test]
    fn pool_cancel_pending_shutdown_cancels_queued_and_joins() {
        let _serial = POOL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
        let pool = test_pool(1);
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let running = pool
            .submit(Class::Visible, gate_job(started_tx, release_rx))
            .unwrap();
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let queued_light = pool.submit(Class::Visible, || 1).unwrap();
        let queued_heavy = pool.submit(Class::Background, || 2).unwrap();

        let shutdown = pool.shutdown_pool(ShutdownMode::CancelPending);
        assert!(!pool.is_open());
        assert!(running.token().is_cancelled());
        release_tx.send(3).unwrap();
        assert_eq!(worker_join(running).unwrap(), 3);
        assert_eq!(worker_join(queued_light), Err(TaskError::Cancelled));
        assert_eq!(worker_join(queued_heavy), Err(TaskError::Cancelled));
        worker_join(shutdown).unwrap();
        assert_eq!(pool.inner.exited.load(Ordering::Acquire), 1);
        assert_eq!(pool.inner.state.lock().unwrap().queues[1].len(), 0);
        assert_eq!(pool.inner.state.lock().unwrap().queues[2].len(), 0);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn stop_workers_closes_static_pools_and_cancels_dedicated_workers() {
        let _serial = POOL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
        static POOLS: OnceLock<(TaskPool, TaskPool)> = OnceLock::new();
        let spawner = ThreadSpawner::for_current_thread(4);
        let (idle, busy) = POOLS.get_or_init(|| {
            (
                TaskPool::create(spawner.clone(), 2).unwrap(),
                TaskPool::create(spawner.clone(), 1).unwrap(),
            )
        });
        wait_for("idle static workers", || {
            idle.inner.started.load(Ordering::Acquire) == 2
        });
        let (token_tx, token_rx) = mpsc::channel::<CancellationToken>();
        let (started_tx, started_rx) = mpsc::channel();
        let running = busy
            .submit(Class::Background, move || {
                let token = token_rx.recv().unwrap();
                started_tx.send(()).unwrap();
                while !token.is_cancelled() {
                    std::thread::park();
                }
                7
            })
            .unwrap();
        // Closing the pool cancels the token and unparks its running worker.
        let worker = running.token();
        token_tx.send(worker.clone()).unwrap();
        started_rx.recv().unwrap();
        let queued = busy
            .submit(Class::Background, || -> () {
                panic!("queued work ran after stop")
            })
            .unwrap();
        let (token_tx, token_rx) = mpsc::channel::<CancellationToken>();
        let (began_tx, began_rx) = mpsc::channel();
        let dedicated = spawner
            .spawn_worker(ThreadOptions::default(), move || {
                let token = token_rx.recv().unwrap();
                began_tx.send(()).unwrap();
                while !token.is_cancelled() {
                    std::thread::yield_now();
                }
            })
            .unwrap();
        token_tx.send(dedicated.token()).unwrap();
        // running when the runtime closes (a worker cancelled before it starts never runs)
        began_rx.recv().unwrap();

        crate::restart::stop_workers(&spawner);
        assert!(!idle.is_open() && !busy.is_open());
        assert!(worker.is_cancelled());
        assert_eq!(worker_join(queued), Err(TaskError::Cancelled));
        assert_eq!(worker_join(running), Ok(7));
        assert_eq!(worker_join(dedicated), Ok(()));
        worker_join(idle.shutdown_handle()).unwrap();
        worker_join(busy.shutdown_handle()).unwrap();
        assert_eq!(idle.inner.exited.load(Ordering::Acquire), 2);
        assert_eq!(busy.inner.exited.load(Ordering::Acquire), 1);
        assert_eq!(
            spawner
                .spawn_worker(ThreadOptions::default(), || ())
                .unwrap_err(),
            SpawnError::RuntimeClosed
        );
    }

    #[test]
    fn pool_reports_panic_and_keeps_worker_alive() {
        let _serial = POOL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
        let pool = test_pool(1);
        let failed = pool
            .submit(Class::Visible, || panic!("pool task panic"))
            .unwrap();
        let Err(TaskError::Panicked(report)) = worker_join(failed) else {
            panic!("panicking job did not report TaskError::Panicked");
        };
        assert_eq!(&*report.message, "pool task panic");
        let next = pool.submit(Class::Visible, || 31).unwrap();
        assert_eq!(worker_join(next).unwrap(), 31);
        assert_eq!(pool.inner.state.lock().unwrap().queues[1].len(), 0);
        worker_join(pool.shutdown_pool(ShutdownMode::Drain)).unwrap();
    }

    #[test]
    fn task_queue_orders_replaces_and_prunes_without_locking() {
        let _serial = POOL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
        let pool = test_pool(1);
        let mut queue: TaskQueue<u32> = TaskQueue::new(Class::Visible, 1, 16);
        let order = Arc::new(Mutex::new(Vec::new()));
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel::<u32>();
        let first_order = order.clone();
        queue
            .push(&pool, 1, true, QueueOrder::Lifo, move || {
                started_tx.send(()).unwrap();
                let _ = release_rx.recv();
                first_order.lock().unwrap().push(1);
            })
            .unwrap();
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(queue.in_flight(), 1);

        for key in [2, 3] {
            let order = order.clone();
            queue
                .push(&pool, key, true, QueueOrder::Lifo, move || {
                    order.lock().unwrap().push(key)
                })
                .unwrap();
        }
        assert_eq!(queue.staged_len(), 2);
        assert_eq!(queue.retain(|key| *key != 2), vec![2]);
        assert!(queue.contains(&3));
        let replaced_order = order.clone();
        queue
            .push(&pool, 3, true, QueueOrder::Lifo, move || {
                replaced_order.lock().unwrap().push(30)
            })
            .unwrap();
        assert_eq!(queue.staged_len(), 1, "replace keeps one job per key");

        release_tx.send(0).unwrap();
        wait_for("first job to finish", || queue.in_flight() == 0);
        queue.pump(&pool);
        wait_for("replacement to run", || {
            queue.is_idle() && order.lock().unwrap().len() == 2
        });
        assert_eq!(*order.lock().unwrap(), vec![1, 30]);
        worker_join(pool.shutdown_pool(ShutdownMode::Drain)).unwrap();
    }

    #[test]
    fn fan_out_runs_every_index_after_its_job_goes_stale() {
        // A non-cancellable map inside a job a newer version made stale still gets every
        // result: fan_out never drops indices (the slicer's par::map panicked on one).
        let _serial = POOL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
        let pool = test_pool(3);
        let version = Version::new();
        let (inner_pool, inner_version) = (pool.clone(), version.clone());
        let job = pool
            .submit_at(Class::Visible, "stale fan_out", version.next(), move || {
                inner_version.bump();
                assert!(checkpoint().is_err(), "the job is stale before its fan-out");
                let runs: Vec<AtomicUsize> = (0..256).map(|_| AtomicUsize::new(0)).collect();
                inner_pool.fan_out(runs.len(), |i| {
                    runs[i].fetch_add(1, Ordering::Relaxed);
                });
                runs.iter().map(|r| r.load(Ordering::Relaxed)).collect::<Vec<_>>()
            })
            .unwrap();
        assert_eq!(worker_join(job).unwrap(), vec![1; 256], "every index exactly once");
        worker_join(pool.shutdown_pool(ShutdownMode::Drain)).unwrap();
    }

    #[test]
    fn fan_out_completes_with_a_saturated_pool_and_from_a_worker() {
        let _serial = POOL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
        let pool = test_pool(2);
        wait_for("workers to start", || {
            pool.inner.started.load(Ordering::Acquire) == 2
        });
        let sum = Arc::new(AtomicU64::new(0));
        let worker_pool = pool.clone();
        let worker_sum = sum.clone();
        let spawner = ThreadSpawner::for_current_thread(3);
        let batch = spawner
            .spawn_worker(ThreadOptions::default(), move || {
                let seen = Mutex::new(Vec::new());
                worker_pool.fan_out(1000, |index| {
                    worker_sum.fetch_add(index as u64, Ordering::Relaxed);
                    seen.lock().unwrap().push(index);
                });
                let mut seen = seen.into_inner().unwrap();
                seen.sort_unstable();
                seen
            })
            .unwrap();
        let seen = worker_join(batch).unwrap();
        assert_eq!(seen, (0..1000).collect::<Vec<_>>());
        assert_eq!(sum.load(Ordering::Relaxed), 499_500);

        // Every worker blocked: the caller still finishes the whole batch.
        let mut releases = Vec::new();
        let mut blockers = Vec::new();
        let (started_tx, started_rx) = mpsc::channel();
        for _ in 0..2 {
            let (release_tx, release_rx) = mpsc::channel();
            releases.push(release_tx);
            blockers.push(
                pool.submit(Class::Visible, gate_job(started_tx.clone(), release_rx))
                    .unwrap(),
            );
        }
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let saturated_pool = pool.clone();
        let saturated = spawner
            .spawn_worker(ThreadOptions::default(), move || {
                let count = AtomicUsize::new(0);
                let _context = JobGuard::enter(
                    Class::Interactive,
                    "saturated caller",
                    Ticket::none(),
                    CancellationToken::new(),
                    None,
                );
                saturated_pool.fan_out(64, |_| {
                    count.fetch_add(1, Ordering::Relaxed);
                });
                count.into_inner()
            })
            .unwrap();
        assert_eq!(worker_join(saturated).unwrap(), 64);
        for release in releases {
            release.send(1).unwrap();
        }
        for blocker in blockers {
            worker_join(blocker).unwrap();
        }
        // The cancelled helpers drained cleanly; the pool is still usable.
        assert_eq!(
            worker_join(pool.submit(Class::Visible, || 8).unwrap()).unwrap(),
            8
        );
        worker_join(pool.shutdown_pool(ShutdownMode::Drain)).unwrap();
    }

    #[test]
    fn dropping_the_last_handle_closes_the_pool() {
        let _serial = POOL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
        let pool = test_pool(2);
        wait_for("workers to start", || {
            pool.inner.started.load(Ordering::Acquire) == 2
        });
        let shutdown = pool.shutdown_handle();
        let clone = pool.clone();
        drop(pool);
        assert!(clone.is_open(), "a live handle keeps the pool open");
        drop(clone);
        worker_join(shutdown).unwrap();
    }

    #[test]
    fn scheduler_at_interval_and_cancel() {
        let _serial = POOL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
        let spawner = ThreadSpawner::for_current_thread(2);
        let scheduler = Scheduler::new(spawner).unwrap();
        let (at_tx, at_rx) = mpsc::channel();
        let at_deadline = Cx::monotonic_now() + 0.01;
        let _at = scheduler
            .at(at_deadline, CancellationToken::new(), move || {
                at_tx.send(()).unwrap();
            })
            .unwrap();
        run_scheduler_due(&scheduler.state, at_deadline + 0.001);
        at_rx.try_recv().unwrap();

        let (tick_tx, tick_rx) = mpsc::channel();
        let interval = scheduler
            .interval(
                Duration::from_millis(5),
                MissedTick::CoalesceOne,
                CancellationToken::new(),
                move || {
                    let _ = tick_tx.send(());
                },
            )
            .unwrap();
        run_scheduler_due(&scheduler.state, Cx::monotonic_now() + 1.0);
        tick_rx.try_recv().unwrap();
        interval.cancel();
        run_scheduler_due(&scheduler.state, Cx::monotonic_now() + 2.0);
        assert!(tick_rx.try_recv().is_err());
    }

    #[test]
    fn web_worker_bookkeeping_terminal_events_prune() {
        let _serial = POOL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
        let mut state = WebWorkerBookkeeping::default();
        assert!(state.request(7));
        assert!(!state.request(7));
        assert!(state.started(7));
        assert!(!state.started(7));
        assert!(!state.terminal(7, WebWorkerTerminal::FailedToStart));
        assert!(state.terminal(7, WebWorkerTerminal::Finished));
        assert!(!state.terminal(7, WebWorkerTerminal::Finished));
        assert!(state.request(8));
        assert!(state.terminal(8, WebWorkerTerminal::FailedToStart));
        assert!(state.request(9));
        assert!(state.started(9));
        assert!(state.terminal(9, WebWorkerTerminal::Trapped));
        assert_eq!(state.len(), 0);
    }

    #[test]
    fn to_ui_oneshot_completes_once() {
        let _serial = POOL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
        let (sender, receiver) = to_ui_oneshot();
        sender.send(23).unwrap();
        assert_eq!(receiver.try_recv().unwrap(), 23);
        assert!(receiver.try_recv().is_err());
    }
    #[test]
    fn classes_are_prioritized_and_fifo_within_each_class() {
        let _serial = POOL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
        assert!(Class::Interactive > Class::Visible && Class::Visible > Class::Background);
        let pool = test_pool(1);
        let (began, started) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let running = pool
            .submit(Class::Interactive, gate_job(began, released))
            .unwrap();
        started.recv().unwrap();
        let order = Arc::new(Mutex::new(Vec::new()));
        let mut jobs = Vec::new();
        for class in [Class::Background, Class::Visible, Class::Interactive] {
            for i in 0..3 {
                let order = order.clone();
                jobs.push(
                    pool.submit(class, move || {
                        assert_eq!(current_class(), Some(class));
                        order.lock().unwrap().push((class, i));
                    })
                    .unwrap(),
                );
            }
        }
        release.send(0).unwrap();
        worker_join(running).unwrap();
        for job in jobs {
            worker_join(job).unwrap();
        }
        let expected: Vec<_> = [Class::Interactive, Class::Visible, Class::Background]
            .into_iter()
            .flat_map(|c| (0..3).map(move |i| (c, i)))
            .collect();
        assert_eq!(*order.lock().unwrap(), expected);
        worker_join(pool.shutdown_pool(ShutdownMode::Drain)).unwrap();
    }

    #[test]
    fn stale_ticket_is_cancelled_without_running() {
        let _serial = POOL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
        let pool = test_pool(1);
        let version = Version::new();
        let stale = version.next();
        let latest = version.next();
        assert!(stale.is_stale());
        assert!(!latest.is_stale());
        let job = pool
            .submit_at(Class::Visible, "stale", stale, || panic!("stale job ran"))
            .unwrap();
        assert_eq!(worker_join(job), Err(TaskError::Cancelled));
        version.bump();
        assert!(latest.is_stale());
        assert!(!Ticket::none().is_stale());
        worker_join(pool.shutdown_pool(ShutdownMode::Drain)).unwrap();
    }

    #[test]
    fn cancellation_and_version_changes_reach_running_checkpoints() {
        let _serial = POOL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
        assert!(checkpoint().is_ok());
        assert_eq!(current_class(), None);
        let pool = test_pool(1);
        for supersede in [false, true] {
            let version = Version::new();
            let (started, began) = mpsc::channel();
            let (go, wait) = mpsc::channel();
            let job = pool
                .submit_at(Class::Visible, "checkpoint", version.next(), move || {
                    started.send(()).unwrap();
                    wait.recv().unwrap();
                    checkpoint().is_err()
                })
                .unwrap();
            began.recv().unwrap();
            if supersede {
                version.bump();
            } else {
                job.cancel();
            }
            go.send(()).unwrap();
            assert!(worker_join(job).unwrap());
        }
        worker_join(pool.shutdown_pool(ShutdownMode::Drain)).unwrap();
    }

    #[test]
    fn strand_preserves_submit_order_across_classes_and_stale_jobs() {
        let _serial = POOL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
        let pool = test_pool(3);
        let strand = Strand::with_state(&pool, "ordered", Vec::<usize>::new());
        let (started, began) = mpsc::channel();
        let (go, wait) = mpsc::channel();
        let first = strand.submit(Class::Visible, Ticket::none(), move |_| {
            started.send(()).unwrap();
            wait.recv().unwrap();
        });
        began.recv().unwrap();
        let order = Arc::new(Mutex::new(Vec::new()));
        let mut jobs = Vec::new();
        let version = Version::new();
        let ticket = version.next();
        version.bump();
        let stale = strand.submit(Class::Interactive, ticket, |_| {
            panic!("stale strand job ran")
        });
        for (i, class) in [Class::Background, Class::Interactive, Class::Visible]
            .into_iter()
            .enumerate()
        {
            let order = order.clone();
            jobs.push(strand.submit(class, Ticket::none(), move |state| {
                assert_eq!(current_class(), Some(class));
                assert!(checkpoint().is_ok());
                state.push(i);
                order.lock().unwrap().push(i);
            }));
        }
        go.send(()).unwrap();
        worker_join(first).unwrap();
        assert_eq!(worker_join(stale), Err(TaskError::Cancelled));
        for job in jobs {
            worker_join(job).unwrap();
        }
        assert_eq!(*order.lock().unwrap(), [0, 1, 2]);
        let panicked = strand.submit(Class::Visible, Ticket::none(), |state| {
            assert_eq!(*state, [0, 1, 2]);
            state.push(3);
            panic!("owner state survives unwind");
        });
        assert!(matches!(worker_join(panicked), Err(TaskError::Panicked(_))));
        let after = strand.submit(Class::Visible, Ticket::none(), |state| state.clone());
        assert_eq!(worker_join(after).unwrap(), [0, 1, 2, 3]);
        drop(strand);
        worker_join(pool.shutdown_pool(ShutdownMode::Drain)).unwrap();
    }

    #[test]
    fn scope_borrows_and_helps_when_the_only_worker_is_its_caller() {
        let _serial = POOL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
        let pool = test_pool(1);
        let child_pool = pool.clone();
        let job = pool
            .submit(Class::Visible, move || {
                let mut a = 0;
                let mut b = 0;
                child_pool.scope(|s| {
                    s.spawn(|| {
                        a = 7;
                        assert_eq!(current_class(), Some(Class::Visible));
                    });
                    let child = s.spawn(|| {
                        b = 11;
                        13
                    });
                    assert_eq!(child.join().unwrap(), 13);
                    s.spawn(|| {
                        s.spawn(|| assert!(checkpoint().is_ok()));
                    });
                });
                (a, b)
            })
            .unwrap();
        assert_eq!(worker_join(job).unwrap(), (7, 11));
        worker_join(pool.shutdown_pool(ShutdownMode::Drain)).unwrap();
    }

    #[test]
    fn scope_and_fan_out_wait_for_borrows_on_panic() {
        let _serial = POOL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
        let pool = test_pool(2);
        let child_pool = pool.clone();
        let job = pool
            .submit(Class::Visible, move || {
                let count = AtomicUsize::new(0);
                assert!(catch_unwind(AssertUnwindSafe(|| child_pool.scope(|s| {
                    s.spawn(|| {
                        count.fetch_add(1, Ordering::Relaxed);
                    });
                    panic!("scope body");
                })))
                .is_err());
                assert_eq!(count.load(Ordering::Relaxed), 1);
                assert!(
                    catch_unwind(AssertUnwindSafe(|| child_pool.fan_out(100, |i| {
                        if i == 3 {
                            panic!("batch item");
                        }
                        count.fetch_add(1, Ordering::Relaxed);
                    })))
                    .is_err()
                );
            })
            .unwrap();
        worker_join(job).unwrap();
        worker_join(pool.shutdown_pool(ShutdownMode::Drain)).unwrap();
    }

    /// A higher-class job queued while every worker runs a lower class's fan-out starts at a
    /// helper's next item boundary, not once the fan-out ends: with no worker free, nothing
    /// admits it, so the helpers cannot learn of it from the budget.
    #[test]
    fn fan_out_helpers_leave_for_a_queued_higher_class_job() {
        let _serial = POOL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
        // Two Visible permits besides any other test's.
        if makepad_cpu_budget::limit() < 3 {
            return;
        }
        const N: usize = 2000;
        let pool = test_pool(2);
        let (inner_pool, threads, next) = (pool.clone(), Arc::new(Mutex::new(std::collections::HashSet::new())), Arc::new(AtomicUsize::new(0)));
        let (seen, claimed) = (threads.clone(), next.clone());
        let batch = pool
            .submit(Class::Visible, move || {
                inner_pool.fan_out(N, |_| {
                    seen.lock().unwrap().insert(std::thread::current().id());
                    claimed.fetch_add(1, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(1));
                });
            })
            .unwrap();
        wait_for("the fan-out on both workers", || threads.lock().unwrap().len() == 2);
        let claimed = next.clone();
        let interactive = pool
            .submit(Class::Interactive, move || N - claimed.load(Ordering::SeqCst))
            .unwrap();
        let left = worker_join(interactive).unwrap();
        worker_join(batch).unwrap();
        assert!(left > N / 2, "the Interactive job waited for the Visible fan-out: {left} of {N} items were left when it started");
        worker_join(pool.shutdown_pool(ShutdownMode::Drain)).unwrap();
    }

    #[test]
    fn fan_out_finishes_when_all_helpers_yield_to_visible_work() {
        let _serial = POOL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
        // Smaller budgets cannot admit even one Background helper alongside its caller.
        let helpers = (makepad_cpu_budget::limit() as usize)
            .saturating_sub(3)
            .min(2);
        if helpers == 0 {
            return;
        }
        let pool = test_pool(helpers + 1);
        let child_pool = pool.clone();
        let job = pool
            .submit(Class::Background, move || {
                let caller = std::thread::current().id();
                let arrived = AtomicUsize::new(0);
                let caller_started = AtomicBool::new(false);
                let pressure_ready = AtomicBool::new(false);
                let seen = Mutex::new(Vec::new());
                let barrier = std::sync::Barrier::new(helpers + 1);
                child_pool.fan_out(256, |i| {
                    seen.lock().unwrap().push(i);
                    if std::thread::current().id() == caller
                        && !caller_started.swap(true, Ordering::Relaxed)
                    {
                        barrier.wait();
                        let permits: Vec<_> = (helpers + 1..makepad_cpu_budget::cores() as usize).map(|_| {
                            makepad_cpu_budget::admit(makepad_cpu_budget::INTERACTIVE, makepad_cpu_budget::APP, "batch occupancy")
                        }).collect();
                        let (release, released) = mpsc::channel();
                        let waiter = std::thread::spawn(move || {
                            let permit = makepad_cpu_budget::admit(makepad_cpu_budget::VISIBLE, makepad_cpu_budget::APP, "batch waiter");
                            released.recv().unwrap();
                            makepad_cpu_budget::release(permit);
                        });
                        let permit = JOB.with(|j| j.borrow().as_ref().unwrap().permit.unwrap());
                        wait_for("higher class waiting for the batch", || makepad_cpu_budget::should_yield(permit));
                        pressure_ready.store(true, Ordering::Release);
                        // Keep the caller in its first item so both helpers must
                        // relinquish their permits before the caller checkpoints.
                        wait_for("all batch helpers to leave", || {
                            child_pool
                                .inner
                                .state
                                .lock()
                                .unwrap()
                                .running
                                .iter()
                                .flatten()
                                .count()
                                == 1
                        });
                        for permit in permits { makepad_cpu_budget::release(permit); }
                        release.send(()).unwrap();
                        waiter.join().unwrap();
                    } else if std::thread::current().id() != caller
                        && arrived.fetch_add(1, Ordering::SeqCst) < helpers
                    {
                        barrier.wait();
                        while !pressure_ready.load(Ordering::Acquire) {
                            std::thread::yield_now();
                        }
                    }
                });
                // The contract: every index ran exactly once and the caller returned (here). The
                // helpers that left come back once the pressure is gone (the helper chain
                // re-enqueues itself: the fan-out widens again), so how many items they ran
                // besides their first is timing, not part of it.
                let mut seen = seen.into_inner().unwrap();
                seen.sort_unstable();
                assert_eq!(seen, (0..256).collect::<Vec<_>>());
            })
            .unwrap();
        worker_join(job).unwrap();
        worker_join(pool.shutdown_pool(ShutdownMode::Drain)).unwrap();
    }

    #[test]
    fn input_dispatch_ends_warmup_before_app_work_and_does_not_restart_it() {
        let _serial = POOL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
        use crate::event::{Event, MouseButton, MouseDownEvent, MouseMoveEvent};
        makepad_cpu_budget::begin_warmup();
        let before = makepad_cpu_budget::warming();
        let mut cx = crate::Cx::new(Box::new(|_, event| {
            if matches!(event, Event::MouseDown(_)) {
                assert!(!makepad_cpu_budget::warming(), "input restores width before app dispatch");
            }
        }));
        let motion = Event::MouseMove(MouseMoveEvent {
            abs: Default::default(), lock_delta: Default::default(),
            window_id: crate::window::WindowId(0, 0), modifiers: Default::default(),
            time: 0.0, handled: Default::default(),
        });
        assert!(!ends_warmup(&motion), "plain motion leaves warm-up alone");
        assert!(!ends_warmup(&Event::Signal), "worker completion is not interaction");
        cx.call_event_handler(&motion);
        assert_eq!(makepad_cpu_budget::warming(), before, "hover is not interaction");
        let press = Event::MouseDown(MouseDownEvent {
            abs: Default::default(), button: MouseButton::PRIMARY,
            window_id: crate::window::WindowId(0, 0), modifiers: Default::default(),
            time: 1.0, handled: Default::default(),
        });
        assert!(ends_warmup(&press), "a press ends warm-up even if another test ended it already");
        cx.call_event_handler(&press);
        makepad_cpu_budget::begin_warmup();
        assert!(!makepad_cpu_budget::warming(), "later compilation cannot restart warm-up");
        let width = makepad_cpu_budget::limit().saturating_sub(2).clamp(1, 3) as usize;
        let pool = test_pool(width);
        let (started, began) = mpsc::channel();
        let mut jobs = Vec::new();
        let mut releases = Vec::new();
        for _ in 0..width {
            let (release, wait) = mpsc::channel();
            jobs.push(pool.submit(Class::Background, gate_job(started.clone(), wait)).unwrap());
            releases.push(release);
        }
        for _ in 0..width { began.recv_timeout(Duration::from_secs(5)).unwrap(); }
        for release in releases { release.send(0).unwrap(); }
        for job in jobs { worker_join(job).unwrap(); }
        worker_join(pool.shutdown_pool(ShutdownMode::Drain)).unwrap();
    }
    #[test]
    fn admission_waiters_leave_a_worker_for_higher_classes() {
        let _serial = POOL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
        let permits: Vec<_> = (0..makepad_cpu_budget::cores())
            .map(|_| {
                makepad_cpu_budget::admit(
                    makepad_cpu_budget::INTERACTIVE,
                    makepad_cpu_budget::APP,
                    "admission gate",
                )
            })
            .collect();
        let pool = test_pool(3);
        let (began, started) = mpsc::channel();
        let bg = pool.submit(Class::Background, || ()).unwrap();
        wait_for("Background admission waiter", || {
            pool.inner.state.lock().unwrap().waiting[2]
        });
        let visible = pool.submit(Class::Visible, || ()).unwrap();
        wait_for("Visible admission waiter", || {
            pool.inner.state.lock().unwrap().waiting[1]
        });
        let interactive = pool
            .submit(Class::Interactive, move || began.send(()).unwrap())
            .unwrap();
        wait_for("Interactive admission waiter", || {
            pool.inner.state.lock().unwrap().waiting[0]
        });
        let mut permits = permits;
        makepad_cpu_budget::release(permits.pop().unwrap());
        started.recv_timeout(Duration::from_secs(5)).unwrap();
        worker_join(interactive).unwrap();
        for permit in permits {
            makepad_cpu_budget::release(permit);
        }
        worker_join(visible).unwrap();
        worker_join(bg).unwrap();
        worker_join(pool.shutdown_pool(ShutdownMode::Drain)).unwrap();
    }

    #[test]
    fn scope_wait_releases_its_permit_for_work_that_unblocks_a_child() {
        let _serial = POOL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
        // Two permits let the caller and its child start together.
        if makepad_cpu_budget::cores() < 2 {
            return;
        }
        let permits: Vec<_> = (2..makepad_cpu_budget::cores())
            .map(|_| {
                makepad_cpu_budget::admit(
                    makepad_cpu_budget::INTERACTIVE,
                    makepad_cpu_budget::APP,
                    "scope gate",
                )
            })
            .collect();
        let pool = test_pool(3);
        let child_pool = pool.clone();
        let job = pool
            .submit(Class::Interactive, move || {
                let (started, began) = mpsc::channel();
                let (go, wait) = mpsc::channel();
                let value = AtomicUsize::new(0);
                child_pool.scope(|s| {
                    let value = &value;
                    s.spawn(move || {
                        started.send(()).unwrap();
                        wait.recv().unwrap();
                        value.store(42, Ordering::Release);
                    });
                    began.recv().unwrap();
                    child_pool
                        .submit(Class::Interactive, move || go.send(()).unwrap())
                        .unwrap()
                        .detach();
                });
                assert_eq!(value.load(Ordering::Acquire), 42);
            })
            .unwrap();
        worker_join(job).unwrap();
        for permit in permits {
            makepad_cpu_budget::release(permit);
        }
        worker_join(pool.shutdown_pool(ShutdownMode::Drain)).unwrap();
    }

    #[test]
    fn scoped_cancellation_drops_borrowed_captures_before_returning() {
        let _serial = POOL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
        let pool = test_pool(1);
        let child_pool = pool.clone();
        let job = pool
            .submit(Class::Visible, move || {
                struct Borrowed<'a>(&'a AtomicUsize);
                impl Drop for Borrowed<'_> {
                    fn drop(&mut self) {
                        self.0.fetch_add(1, Ordering::Relaxed);
                    }
                }
                let dropped = AtomicUsize::new(0);
                child_pool.scope(|s| {
                    let borrowed = Borrowed(&dropped);
                    s.spawn(move || {
                        drop(borrowed);
                        panic!("cancelled child ran");
                    })
                    .cancel();
                });
                assert_eq!(dropped.load(Ordering::Relaxed), 1);
                child_pool.scope(|s| {
                    assert!(matches!(
                        s.spawn(|| panic!("joined panic")).join(),
                        Err(TaskError::Panicked(_))
                    ));
                });
            })
            .unwrap();
        worker_join(job).unwrap();
        worker_join(pool.shutdown_pool(ShutdownMode::Drain)).unwrap();
    }

    #[test]
    fn draining_a_strand_runs_accepted_successors_and_refuses_new_submits() {
        let _serial = POOL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
        let pool = test_pool(2);
        let strand = Strand::new(&pool, "draining");
        let (started, began) = mpsc::channel();
        let (go, wait) = mpsc::channel();
        let first = strand.submit(Class::Visible, Ticket::none(), move |_| {
            started.send(()).unwrap();
            wait.recv().unwrap();
        });
        began.recv().unwrap();
        let next = strand.submit(Class::Interactive, Ticket::none(), |_| 42);
        let shutdown = pool.shutdown(ShutdownMode::Drain);
        let late = strand.submit(Class::Interactive, Ticket::none(), |_| {
            panic!("late strand job ran")
        });
        assert_eq!(worker_join(late), Err(TaskError::Cancelled));
        go.send(()).unwrap();
        worker_join(first).unwrap();
        assert_eq!(worker_join(next).unwrap(), 42);
        worker_join(shutdown).unwrap();
        worker_join(pool.shutdown_pool(ShutdownMode::Drain)).unwrap();
    }
}
