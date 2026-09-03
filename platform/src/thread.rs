//! Cross-platform thread runtime.
//!
//! Background work runs on ONE mechanism on every target: the runtime-owned
//! [`TaskPool`] (`Cx::task_pool()`), whose workers are created once at
//! start-up and stay warm. A job is submitted lock-free from any thread and
//! its result comes back through a [`TaskHandle`] polled with `try_take`
//! (never a blocking join on the UI thread) or over whatever channel the job
//! carries; every completion raises the UI signal. The pool has two lanes so
//! a long job (an mp3 decode, a stem fetch, a bake) never queues in front of
//! a short interactive one (an icon, a thumbnail, a catalog request).
//!
//! [`ThreadSpawner::spawn_worker`] creates a dedicated long-lived thread — an
//! audio, decode or network loop fed over a channel — once at start-up. It is
//! never the mechanism for a job: on the web a Web Worker takes hundreds of
//! milliseconds to come up, on desktop it is churn.
//!
//! Deadlines are Makepad monotonic seconds; no `Instant` crosses the wasm
//! boundary.

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
            atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering},
            mpsc::{sync_channel, Receiver, SyncSender, TrySendError},
            Arc, Condvar, Mutex, MutexGuard, OnceLock,
        },
        thread::ThreadId,
        time::Duration,
    },
};

pub use makepad_network::{
    to_ui_bounded, to_ui_oneshot, FromUIReceiver, FromUISender, ReceiverAlreadyTaken,
    SignalFromUI, SignalToUI, ToUIOneshotReceiver, ToUIOneshotSender, ToUIReceiver, ToUISender,
    UiWaker,
};

fn lock_without_wasm_wait<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // Browser UI and AudioWorklet threads cannot use the futex wait that a
    // contended wasm Mutex::lock performs.
    #[cfg(target_arch = "wasm32")]
    loop {
        match mutex.try_lock() {
            Ok(guard) => return guard,
            Err(std::sync::TryLockError::Poisoned(error)) => return error.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => std::hint::spin_loop(),
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

pub fn lock_from_ui<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    lock_without_wasm_wait(mutex)
}

/// Lock from a realtime audio callback without entering `Atomics.wait` on
/// wasm. Native targets retain the ordinary blocking mutex behaviour.
pub fn lock_from_audio<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    lock_without_wasm_wait(mutex)
}

pub(crate) fn wake_ui_event_loop() {
    #[cfg(all(not(headless), target_os = "macos"))]
    crate::os::apple::macos::macos_app::wake_event_loop();

    #[cfg(all(not(headless), target_arch = "wasm32"))]
    unsafe {
        js_wake_ui();
    }

    #[cfg(any(
        headless,
        target_os = "ios",
        target_os = "tvos",
        target_os = "windows",
        target_os = "linux",
        target_os = "android"
    ))]
    crate::os::wake_ui_event_loop();
}

#[cfg(all(not(headless), target_arch = "wasm32"))]
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
            Self::InvalidStackSize { requested } => write!(f, "invalid thread stack size {requested}"),
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
pub enum PriorityStatus {
    Applied,
    BestEffortUnsupported,
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
    generation: Mutex<u64>,
    wake: Condvar,
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
        if !self.inner.cancelled.swap(true, Ordering::AcqRel) {
            let mut generation = lock_from_ui(&self.inner.generation);
            *generation = generation.wrapping_add(1);
            self.inner.wake.notify_all();
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
        let mut generation = self.inner.generation.lock().unwrap();
        loop {
            if self.is_cancelled() {
                return WaitOutcome::Cancelled;
            }
            let remaining = deadline - Cx::monotonic_now();
            if remaining <= 0.0 {
                return WaitOutcome::DeadlineReached;
            }
            let (next_generation, wait_result) = self
                .inner
                .wake
                .wait_timeout(generation, Duration::from_secs_f64(remaining))
                .unwrap();
            generation = next_generation;
            if wait_result.timed_out() {
                return if self.is_cancelled() {
                    WaitOutcome::Cancelled
                } else {
                    WaitOutcome::DeadlineReached
                };
            }
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
    priority_status: PriorityStatus,
    #[cfg(not(target_arch = "wasm32"))]
    native_join: Option<std::thread::JoinHandle<()>>,
}

impl<T> fmt::Debug for TaskHandle<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TaskHandle")
            .field("finished", &self.is_finished())
            .field("cancelled", &self.token.is_cancelled())
            .field("priority_status", &self.priority_status)
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
            if let Some(join) = self.native_join.take() {
                let _ = join.join();
            }
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
        self.priority_status
    }

    fn completed(ui_thread: ThreadId, result: Result<T, TaskError>) -> Self {
        let state = Arc::new(TaskState::default());
        state.complete(result);
        Self {
            state,
            token: CancellationToken::new(),
            ui_thread,
            priority_status: PriorityStatus::BestEffortUnsupported,
            #[cfg(not(target_arch = "wasm32"))]
            native_join: None,
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
        }
    }

    pub(crate) fn with_parallelism(&self, parallelism: usize) -> Self {
        Self {
            ui_thread: self.ui_thread,
            parallelism: NonZeroUsize::new(parallelism.max(1)).unwrap(),
            runtime_open: self.runtime_open.clone(),
        }
    }

    /// Create one dedicated long-lived thread. Call it once at start-up for a
    /// loop that is fed over a channel; never per job — jobs go to
    /// [`TaskPool::submit`]. The handle reports the thread's terminal result;
    /// `detach()` it when nobody waits for that.
    pub fn spawn_worker<F, T>(&self, options: ThreadOptions, f: F) -> Result<TaskHandle<T>, SpawnError>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        if !self.runtime_open.load(Ordering::Acquire) {
            return Err(SpawnError::RuntimeClosed);
        }
        validate_stack_size(options.stack_size)?;
        let token = CancellationToken::new();
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
            let priority_status = priority_status(priority);
            let native_join = builder
                .spawn(move || {
                    Cx::set_thread_priority(priority);
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
        self.runtime_open.store(false, Ordering::Release);
    }
}

fn validate_stack_size(stack_size: Option<usize>) -> Result<(), SpawnError> {
    if let Some(requested) = stack_size {
        if !(MIN_THREAD_STACK_SIZE..=MAX_THREAD_STACK_SIZE).contains(&requested) || requested % 16 != 0 {
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

#[cfg(any(not(target_arch = "wasm32"), target_feature = "atomics"))]
fn priority_status(priority: CxThreadPriority) -> PriorityStatus {
    if priority == CxThreadPriority::Normal || cfg!(target_os = "android") {
        PriorityStatus::Applied
    } else {
        PriorityStatus::BestEffortUnsupported
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
                let options = PoolOptions::runtime(spawner.available_parallelism());
                match TaskPool::new(spawner, options) {
                    Ok(pool) => pool,
                    Err(error) => {
                        crate::log!("task pool unavailable ({error}); background jobs are refused");
                        TaskPool::closed()
                    }
                }
            })
            .clone()
    }

    pub(crate) fn warm_task_pool(&self) {
        let _ = self.task_pool();
    }

    /// One line: worker count, lanes, job counts and queue waits so far.
    pub fn task_pool_summary(&self) -> String {
        match self.task_pool.get() {
            Some(pool) => pool.summary(),
            None => "pool: not started".to_string(),
        }
    }

    pub(crate) fn close_task_pool(&self) {
        if let Some(pool) = self.task_pool.get() {
            crate::log!("{}", pool.summary());
            pool.close(ShutdownMode::CancelPending);
        }
    }

    /// One dedicated long-lived thread (see [`ThreadSpawner::spawn_worker`]).
    pub fn spawn_worker<F, T>(&self, f: F) -> Result<TaskHandle<T>, SpawnError>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        let result = self.thread_spawner().spawn_worker(ThreadOptions::default(), f);
        log_unsupported_spawn_once(&result);
        result
    }

    pub fn spawn_worker_with<F, T>(&self, options: ThreadOptions, f: F) -> Result<TaskHandle<T>, SpawnError>
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

/// Which lane a job travels on.
///
/// `Light` is short interactive work — an icon or SVG, an image or tile
/// decode, a thumbnail, a catalog request — and is served by every worker,
/// including the ones reserved for it. `Heavy` is anything long — an audio
/// decode, a stem fetch, a loop scan, a bake — and may only use the
/// non-reserved workers, so a burst of heavy jobs never queues in front of the
/// light ones.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Lane {
    Light,
    Heavy,
}

impl Lane {
    const ALL: [Lane; 2] = [Lane::Light, Lane::Heavy];

    fn index(self) -> usize {
        match self {
            Lane::Light => 0,
            Lane::Heavy => 1,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Lane::Light => "light",
            Lane::Heavy => "heavy",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueOrder {
    Fifo,
    Lifo,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShutdownMode {
    CancelPending,
    Drain,
}

/// The most workers one pool tracks; the idle set is one `u32` bit mask.
pub const MAX_POOL_WORKERS: usize = 32;

#[derive(Clone, Debug)]
pub struct PoolOptions {
    /// Total worker threads, clamped to `1..=MAX_POOL_WORKERS`.
    pub workers: NonZeroUsize,
    /// Workers that only ever run `Lane::Light` jobs. Clamped so at least one
    /// worker can run heavy jobs.
    pub light_reserve: usize,
    /// Bounded queue depth per lane; a full lane refuses the submit.
    pub light_capacity: NonZeroUsize,
    pub heavy_capacity: NonZeroUsize,
    pub name: Arc<str>,
}

impl PoolOptions {
    /// The runtime sizing law. Desktop: hardware concurrency minus one for the
    /// UI thread, clamped to `3..=8`. Web: the same minus one, capped at 6
    /// (Web Workers are expensive to start but cheap to keep) and at least 3.
    /// Two workers are reserved for light jobs on both.
    pub fn runtime(parallelism: NonZeroUsize) -> Self {
        let hardware = parallelism.get();
        #[cfg(target_arch = "wasm32")]
        let total = hardware.saturating_sub(1).clamp(3, 6);
        #[cfg(not(target_arch = "wasm32"))]
        let total = hardware.saturating_sub(1).clamp(3, 8);
        Self {
            workers: NonZeroUsize::new(total).unwrap(),
            light_reserve: 2,
            light_capacity: NonZeroUsize::new(512).unwrap(),
            heavy_capacity: NonZeroUsize::new(128).unwrap(),
            name: "makepad-pool".into(),
        }
    }

    pub fn with_workers(workers: usize, light_reserve: usize) -> Self {
        Self {
            workers: NonZeroUsize::new(workers.max(1)).unwrap(),
            light_reserve,
            light_capacity: NonZeroUsize::new(512).unwrap(),
            heavy_capacity: NonZeroUsize::new(128).unwrap(),
            name: "pool".into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SubmitError {
    QueueFull,
    Closed,
}

impl fmt::Display for SubmitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::QueueFull => write!(f, "task pool queue is full"),
            Self::Closed => write!(f, "task pool is closed"),
        }
    }
}

impl std::error::Error for SubmitError {}

/// A refused submission hands the job back so the caller can retry it on a
/// later frame instead of rebuilding it.
pub struct Refused<F> {
    pub job: F,
    pub error: SubmitError,
}

impl<F> fmt::Debug for Refused<F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Refused").field("error", &self.error).finish_non_exhaustive()
    }
}

struct PoolJob {
    lane: Lane,
    submitted_at: f64,
    run: Option<Box<dyn FnOnce() + Send>>,
    cancel: Option<Box<dyn FnOnce() + Send>>,
}

impl PoolJob {
    fn run(mut self) {
        self.cancel.take();
        if let Some(run) = self.run.take() {
            run();
        }
    }
}

impl Drop for PoolJob {
    fn drop(&mut self) {
        if self.run.is_some() {
            self.run.take();
            if let Some(cancel) = self.cancel.take() {
                cancel();
            }
        }
    }
}

struct LaneQueue {
    /// Lock-free bounded hand-over: `try_send` never blocks, on any thread.
    sender: SyncSender<PoolJob>,
    /// Workers only. The UI thread never touches this mutex.
    receiver: Mutex<Receiver<PoolJob>>,
    capacity: usize,
    queued: AtomicUsize,
    peak_queued: AtomicUsize,
    submitted: AtomicU64,
    completed: AtomicU64,
    wait_total_us: AtomicU64,
    wait_max_us: AtomicU64,
    run_total_us: AtomicU64,
    run_max_us: AtomicU64,
}

impl LaneQueue {
    fn new(capacity: usize) -> Self {
        let (sender, receiver) = sync_channel(capacity);
        Self {
            sender,
            receiver: Mutex::new(receiver),
            capacity,
            queued: AtomicUsize::new(0),
            peak_queued: AtomicUsize::new(0),
            submitted: AtomicU64::new(0),
            completed: AtomicU64::new(0),
            wait_total_us: AtomicU64::new(0),
            wait_max_us: AtomicU64::new(0),
            run_total_us: AtomicU64::new(0),
            run_max_us: AtomicU64::new(0),
        }
    }

    fn try_take(&self) -> Option<PoolJob> {
        let receiver = self.receiver.lock().unwrap_or_else(|error| error.into_inner());
        let job = receiver.try_recv().ok();
        drop(receiver);
        if job.is_some() {
            self.queued.fetch_sub(1, Ordering::AcqRel);
        }
        job
    }

    fn snapshot(&self) -> LaneStats {
        let completed = self.completed.load(Ordering::Relaxed);
        let average = |total: u64| {
            if completed == 0 {
                0.0
            } else {
                total as f64 / completed as f64 / 1000.0
            }
        };
        LaneStats {
            submitted: self.submitted.load(Ordering::Relaxed),
            completed,
            queued: self.queued.load(Ordering::Relaxed),
            peak_queued: self.peak_queued.load(Ordering::Relaxed),
            capacity: self.capacity,
            wait_avg_ms: average(self.wait_total_us.load(Ordering::Relaxed)),
            wait_max_ms: self.wait_max_us.load(Ordering::Relaxed) as f64 / 1000.0,
            run_avg_ms: average(self.run_total_us.load(Ordering::Relaxed)),
            run_max_ms: self.run_max_us.load(Ordering::Relaxed) as f64 / 1000.0,
        }
    }
}

struct WorkerSlot {
    thread: OnceLock<std::thread::Thread>,
    heavy_capable: bool,
}

const POOL_OPEN: u8 = 0;
const POOL_DRAINING: u8 = 1;
const POOL_CANCELLED: u8 = 2;

struct PoolInner {
    lanes: [LaneQueue; 2],
    workers: Vec<WorkerSlot>,
    light_reserve: usize,
    /// One bit per parked worker. Set by the worker before it parks, cleared
    /// by whoever unparks it — the unpark token makes the hand-off lossless.
    idle: AtomicU32,
    closed: AtomicU8,
    started: AtomicUsize,
    exited: AtomicUsize,
    /// Live `TaskPool` handles; the last one to drop closes the pool so a
    /// discarded runtime (a test's `Cx`) does not leak parked workers.
    handles: AtomicUsize,
    shutdown_state: Arc<TaskState<()>>,
    ui_thread: ThreadId,
    name: Arc<str>,
}

impl PoolInner {
    fn take_job(&self, heavy_capable: bool) -> Option<PoolJob> {
        if self.closed.load(Ordering::Acquire) == POOL_CANCELLED {
            // Dropping a queued job completes its handle as cancelled.
            while self.lanes[0].try_take().is_some() {}
            while self.lanes[1].try_take().is_some() {}
            return None;
        }
        if let Some(job) = self.lanes[Lane::Light.index()].try_take() {
            return Some(job);
        }
        if heavy_capable {
            return self.lanes[Lane::Heavy.index()].try_take();
        }
        None
    }

    fn run_job(&self, job: PoolJob) {
        let queue = &self.lanes[job.lane.index()];
        let started = Cx::monotonic_now();
        let wait_us = ((started - job.submitted_at).max(0.0) * 1_000_000.0) as u64;
        queue.wait_total_us.fetch_add(wait_us, Ordering::Relaxed);
        queue.wait_max_us.fetch_max(wait_us, Ordering::Relaxed);
        job.run();
        let run_us = ((Cx::monotonic_now() - started).max(0.0) * 1_000_000.0) as u64;
        queue.run_total_us.fetch_add(run_us, Ordering::Relaxed);
        queue.run_max_us.fetch_max(run_us, Ordering::Relaxed);
        queue.completed.fetch_add(1, Ordering::Relaxed);
    }

    /// Wake one parked worker able to serve `lane`. Any thread may call this;
    /// it is atomics only. Light jobs go to the light-only workers first,
    /// leaving the heavy-capable ones free for heavy work.
    fn wake_one(&self, lane: Lane) {
        std::sync::atomic::fence(Ordering::SeqCst);
        let mask = self.idle.load(Ordering::SeqCst);
        if mask == 0 {
            return;
        }
        let passes: &[bool] = match lane {
            Lane::Light => &[false, true],
            Lane::Heavy => &[true],
        };
        for &heavy_capable in passes {
            for (index, slot) in self.workers.iter().enumerate() {
                if slot.heavy_capable != heavy_capable {
                    continue;
                }
                let bit = 1u32 << index;
                if mask & bit == 0 {
                    continue;
                }
                if self.idle.fetch_and(!bit, Ordering::SeqCst) & bit != 0 {
                    if let Some(thread) = slot.thread.get() {
                        thread.unpark();
                    }
                    return;
                }
            }
        }
    }

    fn unpark_all(&self) {
        for slot in &self.workers {
            if let Some(thread) = slot.thread.get() {
                thread.unpark();
            }
        }
    }
}

fn pool_worker(inner: Arc<PoolInner>, index: usize) {
    let _exit = PoolWorkerExit(inner.clone());
    let slot = &inner.workers[index];
    let _ = slot.thread.set(std::thread::current());
    inner.started.fetch_add(1, Ordering::AcqRel);
    let bit = 1u32 << index;
    loop {
        if let Some(job) = inner.take_job(slot.heavy_capable) {
            inner.run_job(job);
            continue;
        }
        if inner.closed.load(Ordering::Acquire) != POOL_OPEN {
            break;
        }
        inner.idle.fetch_or(bit, Ordering::SeqCst);
        std::sync::atomic::fence(Ordering::SeqCst);
        if let Some(job) = inner.take_job(slot.heavy_capable) {
            inner.idle.fetch_and(!bit, Ordering::SeqCst);
            inner.run_job(job);
            continue;
        }
        if inner.closed.load(Ordering::SeqCst) != POOL_OPEN {
            inner.idle.fetch_and(!bit, Ordering::SeqCst);
            continue;
        }
        std::thread::park();
        inner.idle.fetch_and(!bit, Ordering::SeqCst);
    }
}

struct PoolWorkerExit(Arc<PoolInner>);

impl Drop for PoolWorkerExit {
    fn drop(&mut self) {
        let exited = self.0.exited.fetch_add(1, Ordering::AcqRel) + 1;
        if exited == self.0.workers.len() {
            self.0.shutdown_state.complete(Ok(()));
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LaneStats {
    pub submitted: u64,
    pub completed: u64,
    pub queued: usize,
    pub peak_queued: usize,
    pub capacity: usize,
    pub wait_avg_ms: f64,
    pub wait_max_ms: f64,
    pub run_avg_ms: f64,
    pub run_max_ms: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PoolStats {
    pub workers: usize,
    pub light_reserve: usize,
    pub started: usize,
    pub exited: usize,
    pub light: LaneStats,
    pub heavy: LaneStats,
}

/// The background executor. Clone it freely: every handle submits into the
/// same warm workers. Submission is lock-free on every thread; a full lane
/// hands the job back to be retried next frame; results are polled with
/// [`TaskHandle::try_take`] (never a blocking join on the UI thread) and every
/// completion raises the UI signal.
///
/// A pool job must never wait for another pool job (a worker blocking on a
/// sibling's handle can starve the pool); a dedicated worker made with
/// [`ThreadSpawner::spawn_worker`] may join pool handles.
pub struct TaskPool {
    inner: Arc<PoolInner>,
}

impl Clone for TaskPool {
    fn clone(&self) -> Self {
        self.inner.handles.fetch_add(1, Ordering::AcqRel);
        Self { inner: self.inner.clone() }
    }
}

impl Drop for TaskPool {
    fn drop(&mut self) {
        if self.inner.handles.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.close(ShutdownMode::CancelPending);
        }
    }
}

impl fmt::Debug for TaskPool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TaskPool")
            .field("name", &self.inner.name)
            .field("workers", &self.inner.workers.len())
            .field("light_reserve", &self.inner.light_reserve)
            .field("queued_light", &self.inner.lanes[0].queued.load(Ordering::Relaxed))
            .field("queued_heavy", &self.inner.lanes[1].queued.load(Ordering::Relaxed))
            .field("closed", &self.inner.closed.load(Ordering::Relaxed))
            .finish()
    }
}

impl TaskPool {
    /// Spawn the workers now; they park until the first job.
    pub fn new(spawner: ThreadSpawner, options: PoolOptions) -> Result<Self, SpawnError> {
        let worker_len = options.workers.get().min(MAX_POOL_WORKERS);
        let light_reserve = options.light_reserve.min(worker_len - 1);
        let workers = (0..worker_len)
            .map(|index| WorkerSlot {
                thread: OnceLock::new(),
                heavy_capable: index >= light_reserve,
            })
            .collect();
        let inner = Arc::new(PoolInner {
            lanes: [
                LaneQueue::new(options.light_capacity.get()),
                LaneQueue::new(options.heavy_capacity.get()),
            ],
            workers,
            light_reserve,
            idle: AtomicU32::new(0),
            closed: AtomicU8::new(POOL_OPEN),
            started: AtomicUsize::new(0),
            exited: AtomicUsize::new(0),
            handles: AtomicUsize::new(1),
            shutdown_state: Arc::new(TaskState::default()),
            ui_thread: spawner.ui_thread,
            name: options.name.clone(),
        });
        let pool = Self { inner: inner.clone() };
        for index in 0..worker_len {
            let worker_inner = inner.clone();
            let spawned = spawner.spawn_worker(
                ThreadOptions {
                    name: Some(format!("{}-{index}", options.name).into()),
                    ..Default::default()
                },
                move || pool_worker(worker_inner, index),
            );
            match spawned {
                Ok(handle) => handle.detach(),
                Err(error) => {
                    // The workers that did start exit; the ones that never
                    // will are counted out so shutdown can still complete.
                    let missing = worker_len - index;
                    let exited = inner.exited.fetch_add(missing, Ordering::AcqRel) + missing;
                    pool.close(ShutdownMode::CancelPending);
                    if exited == worker_len {
                        inner.shutdown_state.complete(Ok(()));
                    }
                    return Err(error);
                }
            }
        }
        Ok(pool)
    }

    /// A pool with no workers that refuses every job; what `Cx::task_pool`
    /// hands out when the target cannot run threads.
    pub fn closed() -> Self {
        let inner = Arc::new(PoolInner {
            lanes: [LaneQueue::new(1), LaneQueue::new(1)],
            workers: Vec::new(),
            light_reserve: 0,
            idle: AtomicU32::new(0),
            closed: AtomicU8::new(POOL_CANCELLED),
            started: AtomicUsize::new(0),
            exited: AtomicUsize::new(0),
            handles: AtomicUsize::new(1),
            shutdown_state: Arc::new(TaskState::default()),
            ui_thread: std::thread::current().id(),
            name: "closed".into(),
        });
        inner.shutdown_state.complete(Ok(()));
        Self { inner }
    }

    pub fn is_open(&self) -> bool {
        self.inner.closed.load(Ordering::Acquire) == POOL_OPEN
    }

    pub fn worker_count(&self) -> usize {
        self.inner.workers.len()
    }

    pub fn light_reserve(&self) -> usize {
        self.inner.light_reserve
    }

    /// Workers that can run `Lane::Heavy` jobs.
    pub fn heavy_workers(&self) -> usize {
        self.inner.workers.len() - self.inner.light_reserve
    }

    pub fn started_workers(&self) -> usize {
        self.inner.started.load(Ordering::Acquire)
    }

    /// Workers parked with nothing to do right now — what a staging queue
    /// may hand over without the jobs sitting in the pool's channel.
    pub fn idle_workers(&self) -> usize {
        self.inner.idle.load(Ordering::Acquire).count_ones() as usize
    }

    pub fn queued(&self, lane: Lane) -> usize {
        self.inner.lanes[lane.index()].queued.load(Ordering::Acquire)
    }

    pub fn submit<F, T>(&self, lane: Lane, f: F) -> Result<TaskHandle<T>, SubmitError>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        self.try_submit(lane, f).map_err(|refused| refused.error)
    }

    /// Like [`submit`](Self::submit) but a refused job comes back intact so
    /// the caller can hold it for the next frame.
    pub fn try_submit<F, T>(&self, lane: Lane, f: F) -> Result<TaskHandle<T>, Refused<F>>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        match self.reserve(lane) {
            Ok(slot) => Ok(slot.submit(f)),
            Err(error) => Err(Refused { job: f, error }),
        }
    }

    /// Claim one queue slot on `lane` without a job yet. Lets a caller pop
    /// its own staging structure only once the pool is known to take the
    /// job; an unused reservation gives the slot back on drop.
    pub fn reserve(&self, lane: Lane) -> Result<PoolSlot, SubmitError> {
        if !self.is_open() {
            return Err(SubmitError::Closed);
        }
        let queue = &self.inner.lanes[lane.index()];
        let queued = queue.queued.fetch_add(1, Ordering::AcqRel) + 1;
        if queued > queue.capacity {
            queue.queued.fetch_sub(1, Ordering::AcqRel);
            return Err(SubmitError::QueueFull);
        }
        queue.peak_queued.fetch_max(queued, Ordering::Relaxed);
        Ok(PoolSlot { pool: self.clone(), lane, armed: true })
    }

    pub fn stats(&self) -> PoolStats {
        PoolStats {
            workers: self.inner.workers.len(),
            light_reserve: self.inner.light_reserve,
            started: self.inner.started.load(Ordering::Acquire),
            exited: self.inner.exited.load(Ordering::Acquire),
            light: self.inner.lanes[0].snapshot(),
            heavy: self.inner.lanes[1].snapshot(),
        }
    }

    /// `pool: 7 workers (2 light-only), 143 jobs, peak queue light 6 / heavy 3, ...`
    pub fn summary(&self) -> String {
        let stats = self.stats();
        let mut out = format!(
            "pool: {} workers ({} light-only, {} started), {} jobs, peak queue light {} / heavy {}",
            stats.workers,
            stats.light_reserve,
            stats.started,
            stats.light.submitted + stats.heavy.submitted,
            stats.light.peak_queued,
            stats.heavy.peak_queued,
        );
        for (lane, lane_stats) in Lane::ALL.iter().zip([stats.light, stats.heavy]) {
            out.push_str(&format!(
                "; {} {}: wait avg {:.2} ms max {:.2} ms, run avg {:.1} ms max {:.1} ms",
                lane.label(),
                lane_stats.completed,
                lane_stats.wait_avg_ms,
                lane_stats.wait_max_ms,
                lane_stats.run_avg_ms,
                lane_stats.run_max_ms,
            ));
        }
        out
    }

    /// Stop accepting work and let the workers exit — never waits.
    /// `CancelPending` completes every queued handle as cancelled; `Drain`
    /// runs what is queued first. Running jobs finish either way.
    pub fn close(&self, mode: ShutdownMode) {
        let target = match mode {
            ShutdownMode::CancelPending => POOL_CANCELLED,
            ShutdownMode::Drain => POOL_DRAINING,
        };
        let _ = self.inner.closed.fetch_max(target, Ordering::SeqCst);
        self.inner.unpark_all();
    }

    #[cfg(test)]
    fn shutdown_handle_for_test(&self) -> TaskHandle<()> {
        TaskHandle {
            state: self.inner.shutdown_state.clone(),
            token: CancellationToken::new(),
            ui_thread: self.inner.ui_thread,
            priority_status: PriorityStatus::Applied,
            native_join: None,
        }
    }

    /// Close and return a handle that completes once the last worker has
    /// exited. Poll it with `try_take`; join it only from a dedicated thread.
    pub fn shutdown(&self, mode: ShutdownMode) -> TaskHandle<()> {
        self.close(mode);
        TaskHandle {
            state: self.inner.shutdown_state.clone(),
            token: CancellationToken::new(),
            ui_thread: self.inner.ui_thread,
            priority_status: PriorityStatus::Applied,
            #[cfg(not(target_arch = "wasm32"))]
            native_join: None,
        }
    }
}

impl TaskPool {
    /// Run `f(i)` for every `i in 0..len` on the pool AND the calling thread,
    /// returning once every index has run: the caller-helping replacement
    /// for `std::thread::scope` fan-outs inside a worker. The caller claims
    /// indices like any helper, so the batch completes even when the pool is
    /// saturated or refuses helpers, and a helper never waits on anything,
    /// so it cannot deadlock. Helpers that have not started when the caller
    /// runs out of work are cancelled; the ones mid-index are waited for
    /// (also on unwind), which is what keeps the borrow of `f` sound.
    ///
    /// Never call this on the UI thread: it works and waits. Debug builds
    /// assert; release builds run the batch serially there.
    pub fn fan_out<F>(&self, lane: Lane, len: usize, f: F)
    where
        F: Fn(usize) + Sync,
    {
        if len == 0 {
            return;
        }
        if std::thread::current().id() == self.inner.ui_thread {
            debug_assert!(false, "TaskPool::fan_out on the UI thread would block it");
            for index in 0..len {
                f(index);
            }
            return;
        }
        let shared = Arc::new(FanOutShared {
            f: &f as *const F as *const (),
            call: fan_out_call::<F>,
            next: AtomicUsize::new(0),
            len,
            running: AtomicUsize::new(0),
            cancelled: AtomicBool::new(false),
            parent: std::thread::current(),
        });
        let helpers = (len - 1).min(self.worker_count());
        let mut handles = Vec::with_capacity(helpers);
        for _ in 0..helpers {
            let helper = shared.clone();
            match self.submit(lane, move || fan_out_helper(&helper)) {
                Ok(handle) => handles.push(handle),
                Err(_) => break,
            }
        }
        let _wait = FanOutParent { shared: &shared, handles };
        fan_out_work(&shared);
    }
}

struct FanOutShared {
    /// `&F` with its lifetime erased; only dereferenced while the parent's
    /// frame is alive, which `FanOutParent` guarantees.
    f: *const (),
    call: unsafe fn(*const (), usize),
    next: AtomicUsize,
    len: usize,
    running: AtomicUsize,
    cancelled: AtomicBool,
    parent: std::thread::Thread,
}

// The erased pointer is only ever used as `&F` where `F: Sync`, so sharing
// it between threads is exactly as safe as sharing `&F`.
unsafe impl Send for FanOutShared {}
unsafe impl Sync for FanOutShared {}

unsafe fn fan_out_call<F: Fn(usize) + Sync>(f: *const (), index: usize) {
    (*(f as *const F))(index);
}

fn fan_out_work(shared: &FanOutShared) {
    loop {
        let index = shared.next.fetch_add(1, Ordering::AcqRel);
        if index >= shared.len {
            break;
        }
        unsafe { (shared.call)(shared.f, index) };
    }
}

struct FanOutRunning<'a>(&'a FanOutShared);

impl Drop for FanOutRunning<'_> {
    fn drop(&mut self) {
        if self.0.running.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.0.parent.unpark();
        }
    }
}

fn fan_out_helper(shared: &FanOutShared) {
    shared.running.fetch_add(1, Ordering::SeqCst);
    let _running = FanOutRunning(shared);
    // SeqCst pairs with the parent's cancelled-store / running-load: either
    // this helper sees the cancellation, or the parent sees it running.
    if shared.cancelled.load(Ordering::SeqCst) {
        return;
    }
    fan_out_work(shared);
}

struct FanOutParent<'a> {
    shared: &'a Arc<FanOutShared>,
    handles: Vec<TaskHandle<()>>,
}

impl Drop for FanOutParent<'_> {
    fn drop(&mut self) {
        self.shared.cancelled.store(true, Ordering::SeqCst);
        for handle in &self.handles {
            handle.cancel();
        }
        while self.shared.running.load(Ordering::SeqCst) != 0 {
            #[cfg(target_arch = "wasm32")]
            std::thread::yield_now();
            #[cfg(not(target_arch = "wasm32"))]
            std::thread::park_timeout(Duration::from_millis(1));
        }
    }
}

/// A claimed queue slot; see [`TaskPool::reserve`].
#[must_use = "an unused reservation is released on drop; submit a job into it"]
pub struct PoolSlot {
    pool: TaskPool,
    lane: Lane,
    armed: bool,
}

impl PoolSlot {
    pub fn lane(&self) -> Lane {
        self.lane
    }

    /// Queue the job. It cannot be refused any more: if the pool closed in
    /// the meantime the handle completes as `TaskError::Cancelled`.
    pub fn submit<F, T>(mut self, f: F) -> TaskHandle<T>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        self.armed = false;
        let inner = &self.pool.inner;
        let queue = &inner.lanes[self.lane.index()];
        let state = Arc::new(TaskState::default());
        let token = CancellationToken::new();
        let run_state = state.clone();
        let run_token = token.clone();
        let cancel_state = state.clone();
        let run = move || {
            if run_token.is_cancelled() {
                run_state.complete(Err(TaskError::Cancelled));
                signal_ui_completion();
                return;
            }
            let result = catch_unwind(AssertUnwindSafe(f))
                .map_err(|payload| TaskError::Panicked(panic_report(payload)));
            run_state.complete(result);
            signal_ui_completion();
        };
        let job = PoolJob {
            lane: self.lane,
            submitted_at: Cx::monotonic_now(),
            run: Some(Box::new(run)),
            cancel: Some(Box::new(move || {
                cancel_state.complete(Err(TaskError::Cancelled));
            })),
        };
        queue.submitted.fetch_add(1, Ordering::Relaxed);
        match queue.sender.try_send(job) {
            Ok(()) => inner.wake_one(self.lane),
            Err(TrySendError::Full(job)) | Err(TrySendError::Disconnected(job)) => {
                // The reservation bounds the channel, so this is the pool
                // going away underneath us: give the slot back and let the
                // job's drop complete the handle as cancelled.
                queue.queued.fetch_sub(1, Ordering::AcqRel);
                drop(job);
            }
        }
        TaskHandle {
            state,
            token,
            ui_thread: inner.ui_thread,
            priority_status: PriorityStatus::Applied,
            #[cfg(not(target_arch = "wasm32"))]
            native_join: None,
        }
    }
}

impl Drop for PoolSlot {
    fn drop(&mut self) {
        if self.armed {
            self.pool.inner.lanes[self.lane.index()]
                .queued
                .fetch_sub(1, Ordering::AcqRel);
        }
    }
}

fn signal_ui_completion() {
    #[cfg(not(test))]
    SignalToUI::set_ui_signal();
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
    lane: Lane,
    in_flight_limit: usize,
    capacity: usize,
    in_flight: Arc<AtomicUsize>,
    staged: VecDeque<(K, Box<dyn FnOnce() + Send>)>,
}

impl<K> fmt::Debug for TaskQueue<K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TaskQueue")
            .field("lane", &self.lane)
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
    pub fn new(lane: Lane, in_flight_limit: usize, capacity: usize) -> Self {
        Self {
            lane,
            in_flight_limit: in_flight_limit.max(1),
            capacity: capacity.max(1),
            in_flight: Arc::new(AtomicUsize::new(0)),
            staged: VecDeque::new(),
        }
    }

    pub fn lane(&self) -> Lane {
        self.lane
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
    pub fn push<F>(&mut self, pool: &TaskPool, key: K, replace_queued: bool, order: QueueOrder, job: F) -> Result<(), SubmitError>
    where
        F: FnOnce() + Send + 'static,
    {
        if !pool.is_open() {
            return Err(SubmitError::Closed);
        }
        if replace_queued {
            self.staged.retain(|(staged_key, _)| *staged_key != key);
        }
        if self.staged.len() >= self.capacity {
            return Err(SubmitError::QueueFull);
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
        while !self.staged.is_empty() && self.in_flight.load(Ordering::Acquire) < self.in_flight_limit {
            let Ok(slot) = pool.reserve(self.lane) else { break };
            let Some((_, job)) = self.staged.pop_front() else { break };
            self.in_flight.fetch_add(1, Ordering::AcqRel);
            let guard = InFlightGuard(self.in_flight.clone());
            slot.submit(move || {
                let _guard = guard;
                job();
            })
            .detach();
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

    pub fn interval<F>(&self, period: Duration, missed: MissedTick, token: CancellationToken, job: F) -> Result<TimerHandle, SpawnError>
    where
        F: FnMut() + Send + 'static,
    {
        if period.is_zero() {
            return Err(SpawnError::Backend("timer period must be non-zero".into()));
        }
        self.insert(Cx::monotonic_now() + period.as_secs_f64(), Some(period.as_secs_f64()), missed, token, TimerCallback::Interval(Box::new(job)))
    }

    pub fn at<F>(&self, deadline: f64, token: CancellationToken, job: F) -> Result<TimerHandle, SpawnError>
    where
        F: FnOnce() + Send + 'static,
    {
        self.insert(deadline, None, MissedTick::Skip, token, TimerCallback::Once(Some(Box::new(job))))
    }

    fn insert(&self, deadline: f64, period: Option<f64>, missed: MissedTick, external_token: CancellationToken, callback: TimerCallback) -> Result<TimerHandle, SpawnError> {
        if !self.runtime_open.load(Ordering::Acquire) {
            return Err(SpawnError::RuntimeClosed);
        }
        if !deadline.is_finite() {
            return Err(SpawnError::Backend("timer deadline must be finite".into()));
        }
        let id = self.state.next_id.fetch_add(1, Ordering::Relaxed).max(1) as u64;
        let handle_token = CancellationToken::new();
        lock_from_ui(&self.state.inner).entries.push(TimerEntry {
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
    #[cfg(not(test))]
    SignalToUI::set_ui_signal();
}

fn run_scheduler_due(state: &Arc<SchedulerState>, now: f64) {
    let mut due = Vec::new();
    {
        let mut inner = lock_from_ui(&state.inner);
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
                lock_from_ui(&state.inner).entries.push(entry);
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
        let mut inner = lock_from_ui(&state.inner);
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
        let mut inner = lock_from_ui(&state.inner);
        if inner
            .armed
            .is_some_and(|(timer, _)| Some(timer.0) == fired)
        {
            inner.armed = None;
        }
    }

    let now = Cx::monotonic_now();
    run_scheduler_due(&state, now);

    let mut inner = lock_from_ui(&state.inner);
    inner.entries.retain(|entry| {
        !entry.external_token.is_cancelled() && !entry.handle_token.is_cancelled()
    });
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
        self.requests.insert(id, WebWorkerStage::Requested).is_none()
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
            (Some(WebWorkerStage::Requested), WebWorkerTerminal::FailedToStart)
                | (Some(WebWorkerStage::Started), WebWorkerTerminal::Finished)
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
fn spawn_web_task<T: Send + 'static>(ui_thread: ThreadId, options: ThreadOptions, state: Arc<TaskState<T>>, token: CancellationToken, run: impl FnOnce() + Send + 'static) -> Result<TaskHandle<T>, SpawnError> {
    static NEXT_REQUEST: AtomicU32 = AtomicU32::new(1);
    let request_id = NEXT_REQUEST.fetch_add(1, Ordering::Relaxed).max(1);
    let closure: WebClosure = Box::new(run);
    let context_ptr = Box::into_raw(Box::new(closure)) as u32;
    lock_from_ui(web_requests()).insert(request_id, WebRequest {
        context_ptr,
        completion: state.clone(),
        stage: WebWorkerStage::Requested,
    });
    let stack_size = options.stack_size.unwrap_or(DEFAULT_WEB_THREAD_STACK_SIZE) as u32;
    let name = options.name.as_deref().unwrap_or("");
    let accepted = unsafe { js_spawn_thread(request_id, context_ptr, stack_size, name.as_ptr(), name.len()) };
    if accepted == 0 {
        if let Some(request) = lock_from_ui(web_requests()).remove(&request_id) {
            unsafe { drop(Box::from_raw(request.context_ptr as *mut WebClosure)) };
        }
        return Err(SpawnError::Unsupported);
    }
    Ok(TaskHandle {
        state,
        token,
        ui_thread,
        priority_status: priority_status(options.priority),
    })
}

#[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
#[link(wasm_import_module = "env")]
extern "C" {
    fn js_spawn_thread(request_id: u32, context_ptr: u32, stack_size: u32, name_ptr: *const u8, name_len: usize) -> u32;
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
    if let Some(request) = lock_from_ui(web_requests()).get_mut(&request_id) {
        request.stage = WebWorkerStage::Started;
    }
}

#[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
#[export_name = "wasm_thread_failed_to_start"]
pub unsafe extern "C" fn wasm_thread_failed_to_start(request_id: u32) {
    if let Some(request) = lock_from_ui(web_requests()).remove(&request_id) {
        drop(Box::from_raw(request.context_ptr as *mut WebClosure));
        request.completion.complete_error(TaskError::Spawn(SpawnError::Backend("web worker failed to start".into())));
    }
}

#[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
#[export_name = "wasm_thread_worker_lost"]
pub extern "C" fn wasm_thread_worker_lost(request_id: u32) {
    if let Some(request) = lock_from_ui(web_requests()).remove(&request_id) {
        request.completion.complete_error(TaskError::WorkerLost);
    }
}

#[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
#[export_name = "wasm_thread_finished"]
pub extern "C" fn wasm_thread_finished(request_id: u32) {
    lock_from_ui(web_requests()).remove(&request_id);
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

    fn worker_join<T: Send + 'static>(handle: TaskHandle<T>) -> Result<T, TaskError> {
        std::thread::spawn(move || handle.join()).join().unwrap()
    }

    fn wait_for(what: &str, mut ready: impl FnMut() -> bool) {
        let deadline = Cx::monotonic_now() + 10.0;
        while !ready() {
            assert!(Cx::monotonic_now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn test_pool(workers: usize, light_reserve: usize, light_capacity: usize, heavy_capacity: usize) -> TaskPool {
        let spawner = ThreadSpawner::for_current_thread(workers + 1);
        TaskPool::new(
            spawner,
            PoolOptions {
                workers: NonZeroUsize::new(workers).unwrap(),
                light_reserve,
                light_capacity: NonZeroUsize::new(light_capacity).unwrap(),
                heavy_capacity: NonZeroUsize::new(heavy_capacity).unwrap(),
                name: "test-pool".into(),
            },
        )
        .unwrap()
    }

    /// A job that reports it started and then waits for a release.
    fn gate_job(started: mpsc::Sender<()>, release: mpsc::Receiver<u32>) -> impl FnOnce() -> u32 + Send + 'static {
        move || {
            started.send(()).unwrap();
            release.recv().unwrap_or(0)
        }
    }

    #[test]
    fn lock_from_ui_returns_uncontended_and_contended_guards() {
        let mutex = Mutex::new(1_u32);
        *lock_from_ui(&mutex) += 1;
        assert_eq!(*mutex.lock().unwrap(), 2);

        let mutex = Arc::new(Mutex::new(7_u32));
        let worker_mutex = mutex.clone();
        let (held_tx, held_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let _guard = worker_mutex.lock().unwrap();
            held_tx.send(()).unwrap();
            std::thread::sleep(Duration::from_millis(10));
        });
        held_rx.recv().unwrap();
        assert_eq!(*lock_from_ui(&mutex), 7);
        worker.join().unwrap();
    }

    #[test]
    fn worker_completion_is_taken_once_and_ui_join_is_refused() {
        let spawner = ThreadSpawner::for_current_thread(2);
        let mut handle = spawner.spawn_worker(ThreadOptions::default(), || 42).unwrap();
        while !handle.is_finished() {
            std::thread::yield_now();
        }
        assert_eq!(handle.try_take().unwrap().unwrap(), 42);
        assert!(handle.try_take().is_none());

        let handle = spawner.spawn_worker(ThreadOptions::default(), || 7).unwrap();
        assert_eq!(handle.join(), Err(TaskError::WouldBlockUi));
    }

    #[test]
    fn task_poll_retries_instead_of_waiting_for_the_result_mutex() {
        let state = Arc::new(TaskState::default());
        let mut handle = TaskHandle {
            state: state.clone(),
            token: CancellationToken::new(),
            ui_thread: std::thread::current().id(),
            priority_status: PriorityStatus::Applied,
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
        assert_eq!(worker_join(named).unwrap().as_deref(), Some("runtime-test-name"));
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
        assert_eq!(
            prioritized.priority_status(),
            if cfg!(target_os = "android") {
                PriorityStatus::Applied
            } else {
                PriorityStatus::BestEffortUnsupported
            }
        );
        worker_join(prioritized).unwrap();
        let panicked = spawner.spawn_worker(ThreadOptions::default(), || panic!("completion panic")).unwrap();
        assert!(matches!(worker_join(panicked), Err(TaskError::Panicked(_))));
    }

    #[test]
    fn cancellation_wakes_waiter() {
        let spawner = ThreadSpawner::for_current_thread(2);
        let token = CancellationToken::new();
        let waiter_token = token.clone();
        let handle = spawner
            .spawn_worker(ThreadOptions::default(), move || waiter_token.wait_until(Cx::monotonic_now() + 30.0))
            .unwrap();
        token.cancel();
        assert_eq!(worker_join(handle).unwrap(), WaitOutcome::Cancelled);
    }

    #[test]
    fn closed_runtime_refuses_new_work() {
        let spawner = ThreadSpawner::for_current_thread(2);
        spawner.close_runtime();
        assert!(matches!(spawner.spawn_worker(ThreadOptions::default(), || ()), Err(SpawnError::RuntimeClosed)));
        assert!(matches!(spawner.scheduler(), Err(SpawnError::RuntimeClosed)));
        assert!(matches!(
            TaskPool::new(spawner, PoolOptions::with_workers(1, 0)),
            Err(SpawnError::RuntimeClosed)
        ));
    }

    #[test]
    fn runtime_sizing_reserves_the_ui_thread_and_two_light_workers() {
        let tiny = PoolOptions::runtime(NonZeroUsize::new(1).unwrap());
        assert_eq!(tiny.workers.get(), 3);
        assert_eq!(tiny.light_reserve, 2);
        let mid = PoolOptions::runtime(NonZeroUsize::new(6).unwrap());
        assert_eq!(mid.workers.get(), 5);
        let big = PoolOptions::runtime(NonZeroUsize::new(32).unwrap());
        assert_eq!(big.workers.get(), if cfg!(target_arch = "wasm32") { 6 } else { 8 });
    }

    #[test]
    fn pool_workers_are_warm_before_the_first_job() {
        let pool = test_pool(3, 1, 8, 8);
        wait_for("workers to start", || pool.started_workers() == 3);
        let stats = pool.stats();
        assert_eq!(stats.started, 3);
        assert_eq!(stats.light.submitted + stats.heavy.submitted, 0);
        assert_eq!(pool.worker_count(), 3);
        assert_eq!(pool.light_reserve(), 1);
        assert_eq!(pool.heavy_workers(), 2);

        let first = pool.submit(Lane::Light, || 5).unwrap();
        assert_eq!(worker_join(first).unwrap(), 5);
        assert_eq!(pool.stats().light.completed, 1);

        worker_join(pool.shutdown(ShutdownMode::Drain)).unwrap();
        assert_eq!(pool.stats().exited, 3);
        assert!(!pool.is_open());
        assert!(matches!(pool.submit(Lane::Light, || ()), Err(SubmitError::Closed)));
    }

    #[test]
    fn pool_jobs_complete_in_order_of_availability() {
        let pool = test_pool(2, 0, 16, 16);
        let order = Arc::new(Mutex::new(Vec::new()));
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let blocking_order = order.clone();
        let blocking = pool
            .submit(Lane::Light, {
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
                pool.submit(Lane::Light, move || {
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
        assert!(pool.stats().light.wait_max_ms < 5_000.0);
        worker_join(pool.shutdown(ShutdownMode::Drain)).unwrap();
    }

    #[test]
    fn pool_full_lane_is_refused_without_blocking() {
        let pool = test_pool(1, 0, 1, 1);
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let running = pool.submit(Lane::Light, gate_job(started_tx, release_rx)).unwrap();
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();

        let queued = pool.submit(Lane::Light, || 1).unwrap();
        assert_eq!(pool.queued(Lane::Light), 1);
        let before = Cx::monotonic_now();
        let refused = pool.try_submit(Lane::Light, || 2).unwrap_err();
        assert!(Cx::monotonic_now() - before < 0.5, "a full lane must answer at once");
        assert_eq!(refused.error, SubmitError::QueueFull);
        // The job comes back intact.
        assert_eq!((refused.job)(), 2);

        // The heavy lane is bounded on its own.
        let heavy = pool.submit(Lane::Heavy, || 3).unwrap();
        assert!(matches!(pool.submit(Lane::Heavy, || 4), Err(SubmitError::QueueFull)));

        // Polling never blocks while the worker is busy.
        let mut probe = pool.submit(Lane::Heavy, || 5);
        assert!(matches!(probe, Err(SubmitError::QueueFull)));
        let mut queued = queued;
        assert!(queued.try_take().is_none());

        release_tx.send(7).unwrap();
        assert_eq!(worker_join(running).unwrap(), 7);
        assert_eq!(worker_join(queued).unwrap(), 1);
        assert_eq!(worker_join(heavy).unwrap(), 3);
        probe = pool.submit(Lane::Heavy, || 5);
        assert_eq!(worker_join(probe.unwrap()).unwrap(), 5);
        assert_eq!(pool.stats().light.peak_queued, 1, "the running job left the queue; one waited behind it");
        worker_join(pool.shutdown(ShutdownMode::Drain)).unwrap();
    }

    #[test]
    fn pool_light_lane_never_waits_behind_heavy_jobs() {
        let pool = test_pool(3, 2, 8, 8);
        wait_for("workers to start", || pool.started_workers() == 3);
        let mut releases = Vec::new();
        let mut heavy = Vec::new();
        let (started_tx, started_rx) = mpsc::channel();
        for _ in 0..3 {
            let (release_tx, release_rx) = mpsc::channel();
            releases.push(release_tx);
            heavy.push(pool.submit(Lane::Heavy, gate_job(started_tx.clone(), release_rx)).unwrap());
        }
        // Exactly one heavy worker: one job runs, two stay queued.
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(started_rx.recv_timeout(Duration::from_millis(200)).is_err());
        assert_eq!(pool.queued(Lane::Heavy), 2);

        let before = Cx::monotonic_now();
        let light = pool.submit(Lane::Light, || "icon").unwrap();
        assert_eq!(worker_join(light).unwrap(), "icon");
        assert!(Cx::monotonic_now() - before < 2.0, "light work must not queue behind heavy work");
        assert!(pool.stats().light.wait_max_ms < 1_000.0);

        for release in releases {
            release.send(1).unwrap();
        }
        for handle in heavy {
            assert_eq!(worker_join(handle).unwrap(), 1);
        }
        worker_join(pool.shutdown(ShutdownMode::Drain)).unwrap();
    }

    #[test]
    fn pool_cancel_pending_shutdown_cancels_queued_and_joins() {
        let pool = test_pool(1, 0, 4, 4);
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let running = pool.submit(Lane::Light, gate_job(started_tx, release_rx)).unwrap();
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let queued_light = pool.submit(Lane::Light, || 1).unwrap();
        let queued_heavy = pool.submit(Lane::Heavy, || 2).unwrap();

        let shutdown = pool.shutdown(ShutdownMode::CancelPending);
        assert!(!pool.is_open());
        release_tx.send(3).unwrap();
        assert_eq!(worker_join(running).unwrap(), 3);
        assert_eq!(worker_join(queued_light), Err(TaskError::Cancelled));
        assert_eq!(worker_join(queued_heavy), Err(TaskError::Cancelled));
        worker_join(shutdown).unwrap();
        assert_eq!(pool.stats().exited, 1);
        assert_eq!(pool.queued(Lane::Light), 0);
        assert_eq!(pool.queued(Lane::Heavy), 0);
    }

    #[test]
    fn pool_keeps_capacity_after_task_panic() {
        let pool = test_pool(1, 0, 2, 2);
        let failed = pool.submit(Lane::Light, || panic!("pool task panic")).unwrap();
        assert!(matches!(worker_join(failed), Err(TaskError::Panicked(_))));
        let next = pool.submit(Lane::Light, || 31).unwrap();
        assert_eq!(worker_join(next).unwrap(), 31);
        assert_eq!(pool.queued(Lane::Light), 0);
        worker_join(pool.shutdown(ShutdownMode::Drain)).unwrap();
    }

    #[test]
    fn pool_reservation_returns_its_slot_when_unused() {
        let pool = test_pool(1, 0, 1, 1);
        let slot = pool.reserve(Lane::Light).unwrap();
        assert!(matches!(pool.reserve(Lane::Light), Err(SubmitError::QueueFull)));
        drop(slot);
        let slot = pool.reserve(Lane::Light).unwrap();
        assert_eq!(worker_join(slot.submit(|| 4)).unwrap(), 4);
        worker_join(pool.shutdown(ShutdownMode::Drain)).unwrap();
    }

    #[test]
    fn task_queue_orders_replaces_and_prunes_without_locking() {
        let pool = test_pool(1, 0, 8, 8);
        let mut queue: TaskQueue<u32> = TaskQueue::new(Lane::Light, 1, 16);
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
                .push(&pool, key, true, QueueOrder::Lifo, move || order.lock().unwrap().push(key))
                .unwrap();
        }
        assert_eq!(queue.staged_len(), 2);
        assert_eq!(queue.retain(|key| *key != 2), vec![2]);
        assert!(queue.contains(&3));
        let replaced_order = order.clone();
        queue
            .push(&pool, 3, true, QueueOrder::Lifo, move || replaced_order.lock().unwrap().push(30))
            .unwrap();
        assert_eq!(queue.staged_len(), 1, "replace keeps one job per key");

        release_tx.send(0).unwrap();
        wait_for("first job to finish", || queue.in_flight() == 0);
        queue.pump(&pool);
        wait_for("replacement to run", || queue.is_idle() && order.lock().unwrap().len() == 2);
        assert_eq!(*order.lock().unwrap(), vec![1, 30]);
        worker_join(pool.shutdown(ShutdownMode::Drain)).unwrap();
    }

    #[test]
    fn fan_out_completes_with_a_saturated_pool_and_from_a_worker() {
        let pool = test_pool(2, 0, 8, 8);
        wait_for("workers to start", || pool.started_workers() == 2);
        let sum = Arc::new(AtomicU64::new(0));
        let worker_pool = pool.clone();
        let worker_sum = sum.clone();
        let spawner = ThreadSpawner::for_current_thread(3);
        let batch = spawner
            .spawn_worker(ThreadOptions::default(), move || {
                let seen = Mutex::new(Vec::new());
                worker_pool.fan_out(Lane::Heavy, 1000, |index| {
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
            blockers.push(pool.submit(Lane::Light, gate_job(started_tx.clone(), release_rx)).unwrap());
        }
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let saturated_pool = pool.clone();
        let saturated = spawner
            .spawn_worker(ThreadOptions::default(), move || {
                let count = AtomicUsize::new(0);
                saturated_pool.fan_out(Lane::Light, 64, |_| {
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
        assert_eq!(worker_join(pool.submit(Lane::Light, || 8).unwrap()).unwrap(), 8);
        worker_join(pool.shutdown(ShutdownMode::Drain)).unwrap();
    }

    #[test]
    fn dropping_the_last_handle_closes_the_pool() {
        let pool = test_pool(2, 0, 4, 4);
        wait_for("workers to start", || pool.started_workers() == 2);
        let shutdown = pool.shutdown_handle_for_test();
        let clone = pool.clone();
        drop(pool);
        assert!(clone.is_open(), "a live handle keeps the pool open");
        drop(clone);
        worker_join(shutdown).unwrap();
    }

    #[test]
    fn scheduler_at_interval_and_cancel() {
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
        let (sender, receiver) = to_ui_oneshot();
        sender.send(23).unwrap();
        assert_eq!(receiver.try_recv().unwrap(), 23);
        assert!(receiver.try_recv().is_err());
    }
}
