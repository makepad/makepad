//! Cross-platform thread runtime.
//!
//! Dedicated tasks are joinable and always publish one terminal result. Short
//! work belongs in [`TaskPool`], whose queue is bounded and whose workers park
//! while idle. Deadlines are Makepad monotonic seconds; no `Instant` crosses
//! the wasm boundary.

use {
    crate::{
        cx::Cx,
        cx_api::CxThreadPriority,
        event::{Event, Timer},
    },
    std::{
        any::{Any, TypeId},
        collections::{HashMap, VecDeque},
        fmt,
        num::NonZeroUsize,
        panic::{catch_unwind, AssertUnwindSafe},
        sync::{
            atomic::{AtomicBool, AtomicU32, Ordering},
            Arc, Condvar, Mutex, OnceLock,
        },
        thread::ThreadId,
        time::Duration,
    },
};

#[cfg(any(target_arch = "wasm32", test))]
use std::sync::atomic::AtomicUsize;

pub use makepad_network::{
    to_ui_bounded, to_ui_oneshot, FromUIReceiver, FromUISender, ReceiverAlreadyTaken,
    SignalFromUI, SignalToUI, ToUIOneshotReceiver, ToUIOneshotSender, ToUIReceiver, ToUISender,
    UiWaker,
};

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
            if let Ok(mut generation) = self.inner.generation.lock() {
                *generation = generation.wrapping_add(1);
                self.inner.wake.notify_all();
            }
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::Acquire)
    }

    /// Park a worker until cancellation or a `Cx::monotonic_now()` deadline.
    pub fn wait_until(&self, deadline: f64) -> WaitOutcome {
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
        let result = self.state.result.lock().unwrap().take();
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

    pub fn spawn<F, T>(&self, f: F) -> Result<TaskHandle<T>, SpawnError>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        self.spawn_with(ThreadOptions::default(), f)
    }

    pub fn spawn_with<F, T>(&self, options: ThreadOptions, f: F) -> Result<TaskHandle<T>, SpawnError>
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

    /// Run borrowed fan-out from a worker. Calling this on the UI thread is
    /// refused before the body runs.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn scope<'env, R>(
        &self,
        body: impl for<'scope> FnOnce(&ScopedSpawner<'scope, 'env>) -> R,
    ) -> Result<R, TaskError> {
        if std::thread::current().id() == self.ui_thread {
            return Err(TaskError::WouldBlockUi);
        }
        catch_unwind(AssertUnwindSafe(|| {
            std::thread::scope(|scope| body(&ScopedSpawner { scope }))
        }))
        .map_err(|payload| TaskError::Panicked(panic_report(payload)))
    }

    #[cfg(target_arch = "wasm32")]
    pub fn scope<'env, R>(
        &self,
        _body: impl for<'scope> FnOnce(&ScopedSpawner<'scope, 'env>) -> R,
    ) -> Result<R, TaskError> {
        if std::thread::current().id() == self.ui_thread {
            Err(TaskError::WouldBlockUi)
        } else {
            Err(TaskError::Spawn(SpawnError::Unsupported))
        }
    }
}

pub struct ScopedSpawner<'scope, 'env: 'scope> {
    #[cfg(not(target_arch = "wasm32"))]
    scope: &'scope std::thread::Scope<'scope, 'env>,
    #[cfg(target_arch = "wasm32")]
    marker: std::marker::PhantomData<&'scope &'env ()>,
}

pub struct ScopedTaskHandle<'scope, T> {
    #[cfg(not(target_arch = "wasm32"))]
    handle: std::thread::ScopedJoinHandle<'scope, T>,
    #[cfg(target_arch = "wasm32")]
    marker: std::marker::PhantomData<&'scope T>,
}

impl<'scope, 'env> ScopedSpawner<'scope, 'env> {
    #[cfg(not(target_arch = "wasm32"))]
    pub fn spawn<F, T>(&self, f: F) -> Result<ScopedTaskHandle<'scope, T>, SpawnError>
    where
        F: FnOnce() -> T + Send + 'scope,
        T: Send + 'scope,
    {
        std::thread::Builder::new()
            .spawn_scoped(self.scope, f)
            .map(|handle| ScopedTaskHandle { handle })
            .map_err(map_spawn_io_error)
    }

    #[cfg(target_arch = "wasm32")]
    pub fn spawn<F, T>(&self, _f: F) -> Result<ScopedTaskHandle<'scope, T>, SpawnError>
    where
        F: FnOnce() -> T + Send + 'scope,
        T: Send + 'scope,
    {
        Err(SpawnError::Unsupported)
    }
}

impl<T> ScopedTaskHandle<'_, T> {
    #[cfg(not(target_arch = "wasm32"))]
    pub fn join(self) -> Result<T, TaskError> {
        self.handle
            .join()
            .map_err(|payload| TaskError::Panicked(panic_report(payload)))
    }

    #[cfg(target_arch = "wasm32")]
    pub fn join(self) -> Result<T, TaskError> {
        Err(TaskError::Spawn(SpawnError::Unsupported))
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
    // Stop-gap for the threaded wasm build: std's wasm allocator is one
    // global spin lock, so workers that allocate serialise on it and per-job
    // throughput collapses with the worker count (measured on the map bake:
    // a tile takes 15-20 s with 8 workers, 0.1-0.5 s with 2) while the main
    // thread starves behind them. Two workers until the thread-caching
    // allocator (ALLOC2) lands; then this cap goes.
    let cap = if cfg!(target_arch = "wasm32") { cap.min(2) } else { cap };
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

    pub fn spawn_thread<F, T>(&mut self, f: F) -> Result<TaskHandle<T>, SpawnError>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        let result = self.thread_spawner().spawn(f);
        log_unsupported_spawn_once(&result);
        result
    }

    pub fn spawn_thread_with<F, T>(&mut self, options: ThreadOptions, f: F) -> Result<TaskHandle<T>, SpawnError>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        let result = self.thread_spawner().spawn_with(options, f);
        log_unsupported_spawn_once(&result);
        result
    }
}

fn log_unsupported_spawn_once<T>(result: &Result<TaskHandle<T>, SpawnError>) {
    static LOGGED: AtomicBool = AtomicBool::new(false);
    match result {
        Err(SpawnError::Unsupported) if !LOGGED.swap(true, Ordering::AcqRel) => {
            crate::error!("Cx::spawn_thread is unsupported on wasm without atomics");
        }
        Err(error) if !matches!(error, SpawnError::Unsupported) => {
            crate::error!("Cx::spawn_thread failed: {error}");
        }
        _ => {}
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

#[derive(Clone, Debug)]
pub struct PoolOptions {
    pub workers: NonZeroUsize,
    pub capacity: NonZeroUsize,
    pub name: Arc<str>,
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

struct ErasedTag {
    value: Box<dyn Any + Send>,
    type_id: TypeId,
    equals: fn(&dyn Any, &dyn Any) -> bool,
}

impl ErasedTag {
    fn new<K: PartialEq + Send + 'static>(key: K) -> Self {
        fn equals<K: PartialEq + 'static>(left: &dyn Any, right: &dyn Any) -> bool {
            left.downcast_ref::<K>() == right.downcast_ref::<K>()
        }
        Self {
            value: Box::new(key),
            type_id: TypeId::of::<K>(),
            equals: equals::<K>,
        }
    }

    fn equals_key<K: PartialEq + 'static>(&self, key: &K) -> bool {
        self.type_id == TypeId::of::<K>() && (self.equals)(self.value.as_ref(), key)
    }

    fn downcast_ref<K: 'static>(&self) -> Option<&K> {
        self.value.downcast_ref()
    }
}

struct PoolJob {
    tag: Option<ErasedTag>,
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

struct PoolState {
    queue: VecDeque<PoolJob>,
    shutdown: Option<ShutdownMode>,
}

struct PoolInner {
    state: Mutex<PoolState>,
    wake: Condvar,
    worker_exit: Condvar,
    workers_remaining: Mutex<usize>,
    worker_handles: Mutex<Vec<TaskHandle<()>>>,
    spawner: ThreadSpawner,
    capacity: usize,
    name: Arc<str>,
}

#[cfg(test)]
static LIVE_POOL_WORKERS: AtomicUsize = AtomicUsize::new(0);

pub struct TaskPool {
    inner: Arc<PoolInner>,
}

impl fmt::Debug for TaskPool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = self.inner.state.lock().unwrap();
        f.debug_struct("TaskPool")
            .field("name", &self.inner.name)
            .field("queued", &state.queue.len())
            .field("capacity", &self.inner.capacity)
            .field("shutdown", &state.shutdown)
            .finish()
    }
}

impl TaskPool {
    pub fn new(spawner: ThreadSpawner, options: PoolOptions) -> Result<Self, SpawnError> {
        let worker_len = options.workers.get();
        let inner = Arc::new(PoolInner {
            state: Mutex::new(PoolState { queue: VecDeque::new(), shutdown: None }),
            wake: Condvar::new(),
            worker_exit: Condvar::new(),
            workers_remaining: Mutex::new(worker_len),
            worker_handles: Mutex::new(Vec::with_capacity(worker_len)),
            spawner: spawner.clone(),
            capacity: options.capacity.get(),
            name: options.name.clone(),
        });

        for index in 0..worker_len {
            let worker_inner = inner.clone();
            let handle = match spawner.spawn_with(
                ThreadOptions {
                    name: Some(format!("{}-{index}", options.name).into()),
                    ..Default::default()
                },
                move || pool_worker(worker_inner),
            ) {
                Ok(handle) => handle,
                Err(error) => {
                    let started = inner.worker_handles.lock().unwrap().len();
                    *inner.workers_remaining.lock().unwrap() = started;
                    initiate_pool_shutdown(&inner, ShutdownMode::CancelPending);
                    return Err(error);
                }
            };
            inner.worker_handles.lock().unwrap().push(handle);
        }
        Ok(Self { inner })
    }

    pub fn submit<F, T>(&self, order: QueueOrder, f: F) -> Result<TaskHandle<T>, SubmitError>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        self.submit_inner::<(), F, T>(None, false, order, f)
    }

    pub fn submit_tagged<K, F, T>(&self, key: K, replace_queued: bool, order: QueueOrder, f: F) -> Result<TaskHandle<T>, SubmitError>
    where
        K: Clone + PartialEq + Send + 'static,
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        self.submit_inner(Some(key), replace_queued, order, f)
    }

    fn submit_inner<K, F, T>(&self, key: Option<K>, replace_queued: bool, order: QueueOrder, f: F) -> Result<TaskHandle<T>, SubmitError>
    where
        K: Clone + PartialEq + Send + 'static,
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        let state = Arc::new(TaskState::default());
        let token = CancellationToken::new();
        let run_state = state.clone();
        let run_token = token.clone();
        let cancel_state = state.clone();
        let run = move || {
            if run_token.is_cancelled() {
                run_state.complete(Err(TaskError::Cancelled));
                return;
            }
            let result = catch_unwind(AssertUnwindSafe(f))
                .map_err(|payload| TaskError::Panicked(panic_report(payload)));
            run_state.complete(result);
        };
        let job = PoolJob {
            tag: key.as_ref().cloned().map(ErasedTag::new),
            run: Some(Box::new(run)),
            cancel: Some(Box::new(move || {
                cancel_state.complete(Err(TaskError::Cancelled));
            })),
        };

        let mut pool_state = self.inner.state.lock().unwrap();
        if pool_state.shutdown.is_some() {
            drop(pool_state);
            drop(job);
            return Err(SubmitError::Closed);
        }
        if replace_queued {
            if let Some(key) = key.as_ref() {
                pool_state.queue.retain(|job| !job.tag.as_ref().is_some_and(|tag| tag.equals_key(key)));
            }
        }
        if pool_state.queue.len() >= self.inner.capacity {
            drop(pool_state);
            drop(job);
            return Err(SubmitError::QueueFull);
        }
        match order {
            QueueOrder::Fifo => pool_state.queue.push_back(job),
            QueueOrder::Lifo => pool_state.queue.push_front(job),
        }
        drop(pool_state);
        self.inner.wake.notify_one();
        Ok(TaskHandle {
            state,
            token,
            ui_thread: self.inner.spawner.ui_thread,
            priority_status: PriorityStatus::Applied,
            #[cfg(not(target_arch = "wasm32"))]
            native_join: None,
        })
    }

    pub fn retain_queued<K>(&self, mut keep: impl FnMut(&K) -> bool) -> Vec<K>
    where
        K: Clone + Send + 'static,
    {
        let mut dropped = Vec::new();
        let mut state = self.inner.state.lock().unwrap();
        state.queue.retain(|job| {
            let Some(key) = job.tag.as_ref().and_then(ErasedTag::downcast_ref::<K>) else { return true };
            if keep(key) {
                true
            } else {
                dropped.push(key.clone());
                false
            }
        });
        dropped
    }

    pub fn shutdown(&self, mode: ShutdownMode) -> TaskHandle<()> {
        initiate_pool_shutdown(&self.inner, mode);
        let inner = self.inner.clone();
        let handles = std::mem::take(&mut *inner.worker_handles.lock().unwrap());
        match self.inner.spawner.spawn_with(
            ThreadOptions {
                name: Some(format!("{}-shutdown", self.inner.name).into()),
                ..Default::default()
            },
            move || {
                for handle in handles {
                    let _ = handle.join();
                }
                let mut remaining = inner.workers_remaining.lock().unwrap();
                while *remaining != 0 {
                    remaining = inner.worker_exit.wait(remaining).unwrap();
                }
            },
        ) {
            Ok(handle) => handle,
            Err(error) => TaskHandle::completed(self.inner.spawner.ui_thread, Err(TaskError::Spawn(error))),
        }
    }
}

impl Drop for TaskPool {
    fn drop(&mut self) {
        initiate_pool_shutdown(&self.inner, ShutdownMode::CancelPending);
        if !self.inner.worker_handles.lock().unwrap().is_empty() {
            self.shutdown(ShutdownMode::CancelPending).detach();
        }
    }
}

fn initiate_pool_shutdown(inner: &PoolInner, mode: ShutdownMode) {
    let mut state = inner.state.lock().unwrap();
    match (state.shutdown, mode) {
        (None, mode) => state.shutdown = Some(mode),
        (Some(ShutdownMode::Drain), ShutdownMode::CancelPending) => state.shutdown = Some(ShutdownMode::CancelPending),
        _ => {}
    }
    if state.shutdown == Some(ShutdownMode::CancelPending) {
        state.queue.clear();
    }
    drop(state);
    inner.wake.notify_all();
}

fn pool_worker(inner: Arc<PoolInner>) {
    let _exit = PoolWorkerExit(inner.clone());
    #[cfg(test)]
    LIVE_POOL_WORKERS.fetch_add(1, Ordering::AcqRel);
    loop {
        let job = {
            let mut state = inner.state.lock().unwrap();
            loop {
                if let Some(job) = state.queue.pop_front() {
                    break Some(job);
                }
                if state.shutdown.is_some() {
                    break None;
                }
                state = inner.wake.wait(state).unwrap();
            }
        };
        let Some(job) = job else { break };
        let _permit = SharedWorkerPermit::acquire();
        job.run();
    }
}

struct PoolWorkerExit(Arc<PoolInner>);

impl Drop for PoolWorkerExit {
    fn drop(&mut self) {
        if let Ok(mut remaining) = self.0.workers_remaining.lock() {
            *remaining = remaining.saturating_sub(1);
            self.0.worker_exit.notify_all();
        }
        #[cfg(test)]
        LIVE_POOL_WORKERS.fetch_sub(1, Ordering::AcqRel);
    }
}

struct WorkerBudget {
    active: Mutex<usize>,
    wake: Condvar,
}

fn worker_budget() -> &'static WorkerBudget {
    static BUDGET: OnceLock<WorkerBudget> = OnceLock::new();
    BUDGET.get_or_init(|| WorkerBudget { active: Mutex::new(0), wake: Condvar::new() })
}

struct SharedWorkerPermit;

impl SharedWorkerPermit {
    fn acquire() -> Self {
        let budget = worker_budget();
        let limit = worker_count(1, usize::MAX).get();
        let mut active = budget.active.lock().unwrap();
        while *active >= limit {
            active = budget.wake.wait(active).unwrap();
        }
        *active += 1;
        Self
    }
}

impl Drop for SharedWorkerPermit {
    fn drop(&mut self) {
        let budget = worker_budget();
        let mut active = budget.active.lock().unwrap();
        *active = active.saturating_sub(1);
        budget.wake.notify_one();
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
        self.state.inner.lock().unwrap().entries.push(TimerEntry {
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
        let mut inner = state.inner.lock().unwrap();
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
                state.inner.lock().unwrap().entries.push(entry);
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
        let mut inner = state.inner.lock().unwrap();
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
        let mut inner = state.inner.lock().unwrap();
        if inner
            .armed
            .is_some_and(|(timer, _)| Some(timer.0) == fired)
        {
            inner.armed = None;
        }
    }

    let now = Cx::monotonic_now();
    run_scheduler_due(&state, now);

    let mut inner = state.inner.lock().unwrap();
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
    web_requests().lock().unwrap().insert(request_id, WebRequest {
        context_ptr,
        completion: state.clone(),
        stage: WebWorkerStage::Requested,
    });
    let stack_size = options.stack_size.unwrap_or(DEFAULT_WEB_THREAD_STACK_SIZE) as u32;
    let name = options.name.as_deref().unwrap_or("");
    let accepted = unsafe { js_spawn_thread(request_id, context_ptr, stack_size, name.as_ptr(), name.len()) };
    if accepted == 0 {
        if let Some(request) = web_requests().lock().unwrap().remove(&request_id) {
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
pub unsafe extern "C" fn wasm_thread_entrypoint(request_id: u32, closure_ptr: u32) {
    let closure = Box::from_raw(closure_ptr as *mut WebClosure);
    closure();
    web_requests().lock().unwrap().remove(&request_id);
}

#[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
#[export_name = "wasm_thread_started"]
pub extern "C" fn wasm_thread_started(request_id: u32) {
    if let Some(request) = web_requests().lock().unwrap().get_mut(&request_id) {
        request.stage = WebWorkerStage::Started;
    }
}

#[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
#[export_name = "wasm_thread_failed_to_start"]
pub unsafe extern "C" fn wasm_thread_failed_to_start(request_id: u32) {
    if let Some(request) = web_requests().lock().unwrap().remove(&request_id) {
        drop(Box::from_raw(request.context_ptr as *mut WebClosure));
        request.completion.complete_error(TaskError::Spawn(SpawnError::Backend("web worker failed to start".into())));
    }
}

#[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
#[export_name = "wasm_thread_worker_lost"]
pub extern "C" fn wasm_thread_worker_lost(request_id: u32) {
    if let Some(request) = web_requests().lock().unwrap().remove(&request_id) {
        request.completion.complete_error(TaskError::WorkerLost);
    }
}

#[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
#[export_name = "wasm_thread_finished"]
pub extern "C" fn wasm_thread_finished(request_id: u32) {
    web_requests().lock().unwrap().remove(&request_id);
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

    #[test]
    fn task_completion_is_taken_once_and_ui_join_is_refused() {
        let spawner = ThreadSpawner::for_current_thread(2);
        let mut handle = spawner.spawn(|| 42).unwrap();
        while !handle.is_finished() {
            std::thread::yield_now();
        }
        assert_eq!(handle.try_take().unwrap().unwrap(), 42);
        assert!(handle.try_take().is_none());

        let handle = spawner.spawn(|| 7).unwrap();
        assert_eq!(handle.join(), Err(TaskError::WouldBlockUi));
    }

    #[test]
    fn task_name_stack_and_panic_are_reported() {
        let spawner = ThreadSpawner::for_current_thread(2);
        let named = spawner
            .spawn_with(
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
            spawner.spawn_with(
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
            .spawn_with(
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
        let panicked = spawner.spawn(|| panic!("completion panic")).unwrap();
        assert!(matches!(worker_join(panicked), Err(TaskError::Panicked(_))));
    }

    #[test]
    fn scoped_fanout_is_worker_only() {
        let spawner = ThreadSpawner::for_current_thread(2);
        assert_eq!(spawner.scope(|_| 1), Err(TaskError::WouldBlockUi));
        let worker_spawner = spawner.clone();
        let outer = spawner
            .spawn(move || {
                let values = [2, 3];
                worker_spawner
                    .scope(|scope| {
                        let left = scope.spawn(|| values[0]).unwrap();
                        let right = scope.spawn(|| values[1]).unwrap();
                        left.join().unwrap() + right.join().unwrap()
                    })
                    .unwrap()
            })
            .unwrap();
        assert_eq!(worker_join(outer).unwrap(), 5);
    }

    #[test]
    fn cancellation_wakes_waiter() {
        let spawner = ThreadSpawner::for_current_thread(2);
        let token = CancellationToken::new();
        let waiter_token = token.clone();
        let handle = spawner.spawn(move || waiter_token.wait_until(Cx::monotonic_now() + 30.0)).unwrap();
        token.cancel();
        assert_eq!(worker_join(handle).unwrap(), WaitOutcome::Cancelled);
    }

    #[test]
    fn closed_runtime_refuses_new_work() {
        let spawner = ThreadSpawner::for_current_thread(2);
        spawner.close_runtime();
        assert!(matches!(spawner.spawn(|| ()), Err(SpawnError::RuntimeClosed)));
        assert!(matches!(spawner.scheduler(), Err(SpawnError::RuntimeClosed)));
    }

    #[test]
    fn pool_bounds_replaces_prunes_and_shuts_down() {
        let baseline = LIVE_POOL_WORKERS.load(Ordering::Acquire);
        let spawner = ThreadSpawner::for_current_thread(2);
        let pool = TaskPool::new(spawner, PoolOptions {
            workers: NonZeroUsize::new(1).unwrap(),
            capacity: NonZeroUsize::new(2).unwrap(),
            name: "test-pool".into(),
        }).unwrap();
        let (gate_tx, gate_rx) = mpsc::channel();
        let (started_tx, started_rx) = mpsc::channel();
        let running = pool.submit(QueueOrder::Fifo, move || {
            started_tx.send(()).unwrap();
            gate_rx.recv().unwrap()
        }).unwrap();
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let replaced = pool.submit_tagged(1_u32, true, QueueOrder::Fifo, || 1).unwrap();
        let other = pool.submit_tagged(2_u32, false, QueueOrder::Fifo, || 3).unwrap();
        assert!(matches!(
            pool.submit(QueueOrder::Fifo, || 99),
            Err(SubmitError::QueueFull)
        ));
        let kept = pool.submit_tagged(1_u32, true, QueueOrder::Fifo, || 2).unwrap();
        assert_eq!(worker_join(replaced), Err(TaskError::Cancelled));
        let mut pruned = pool.retain_queued::<u32>(|_| false);
        pruned.sort_unstable();
        assert_eq!(pruned, vec![1, 2]);
        assert_eq!(worker_join(other), Err(TaskError::Cancelled));
        assert_eq!(worker_join(kept), Err(TaskError::Cancelled));
        gate_tx.send(9).unwrap();
        assert_eq!(worker_join(running).unwrap(), 9);
        worker_join(pool.shutdown(ShutdownMode::Drain)).unwrap();
        assert_eq!(LIVE_POOL_WORKERS.load(Ordering::Acquire), baseline);
    }

    #[test]
    fn pool_keeps_capacity_after_task_panic() {
        let spawner = ThreadSpawner::for_current_thread(2);
        let pool = TaskPool::new(
            spawner,
            PoolOptions {
                workers: NonZeroUsize::new(1).unwrap(),
                capacity: NonZeroUsize::new(2).unwrap(),
                name: "panic-pool".into(),
            },
        )
        .unwrap();
        let failed = pool
            .submit(QueueOrder::Fifo, || panic!("pool task panic"))
            .unwrap();
        assert!(matches!(worker_join(failed), Err(TaskError::Panicked(_))));
        let next = pool.submit(QueueOrder::Fifo, || 31).unwrap();
        assert_eq!(worker_join(next).unwrap(), 31);
        worker_join(pool.shutdown(ShutdownMode::Drain)).unwrap();
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
