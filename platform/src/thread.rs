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
//! budget. Cooperative checkpoints give higher classes room.
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
    makepad_cpu_budget::BudgetHandle,
    std::{
        any::Any,
        collections::{HashMap, VecDeque},
        fmt,
        future::Future,
        num::NonZeroUsize,
        ops::Range,
        panic::{catch_unwind, AssertUnwindSafe},
        pin::Pin,
        sync::{
            atomic::{
                AtomicBool, AtomicPtr, AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering,
            },
            Arc, Condvar, Mutex, OnceLock, Weak,
        },
        task::{Context, Poll, Waker},
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
}

const FRAME_JOBS_INLINE: bool = cfg!(all(target_arch = "wasm32", not(target_feature = "atomics")));

struct FrameTurns {
    frame: u64,
    ran: bool,
    previous_wait: u64,
    longest_wait: u64,
}

impl FrameTurns {
    fn take(&mut self, last_turn: &mut Option<u64>) -> bool {
        let last = last_turn.get_or_insert(self.frame.wrapping_sub(1));
        let waited = self.frame.wrapping_sub(*last);
        // Last frame's oldest refused job has now waited one more frame.
        // A repeat poll of this frame's winner has waited zero.
        if !self.ran && waited > self.previous_wait {
            self.ran = true;
            *last = self.frame;
            true
        } else {
            self.longest_wait = self.longest_wait.max(waited);
            false
        }
    }
}

std::thread_local! {
    static FRAME_TURNS: std::cell::RefCell<FrameTurns> = const {
        std::cell::RefCell::new(FrameTurns { frame: 0, ran: false, previous_wait: 0, longest_wait: 0 })
    };
}

/// Begin a display frame for inline frame jobs on this thread. The web
/// platform calls this at every animation frame.
pub fn frame_jobs_new_frame() {
    FRAME_TURNS.with_borrow_mut(|turns| {
        turns.frame = turns.frame.wrapping_add(1);
        turns.ran = false;
        turns.previous_wait = std::mem::take(&mut turns.longest_wait);
    });
}

/// A one-shot or stepped job with [`TaskHandle`]'s polling. On a page without
/// workers or a pool configured for inline frame jobs, poll it from a
/// frame-driven path. Across all inline jobs on that thread, the first eligible
/// `try_take` or `is_finished` poll runs one step per display frame
/// ([`frame_jobs_new_frame`]).
#[must_use = "dropping a frame job cancels or detaches it"]
pub struct FrameJob<T> {
    inner: FrameJobInner<T>,
}

enum FrameJobInner<T> {
    /// A pool task (natively and in a threaded web build).
    Task { task: TaskHandle<T>, cancel_on_drop: bool },
    /// Created and polled on the thread driving the inline frames.
    Later {
        future: Option<Pin<Box<dyn Future<Output = T>>>>,
        last_turn: Option<u64>,
        result: Option<Result<T, TaskError>>,
        token: CancellationToken,
    },
}

impl<T> fmt::Debug for FrameJob<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.inner {
            FrameJobInner::Task { task, .. } => task.fmt(f),
            FrameJobInner::Later { future, .. } => f.debug_struct("FrameJob").field("finished", &future.is_none()).finish(),
        }
    }
}

impl<T> FrameJob<T> {
    fn later<Fut>(start: impl FnOnce() -> Fut + Send + 'static) -> Self
    where
        Fut: Future<Output = T> + 'static,
    {
        Self {
            inner: FrameJobInner::Later {
                future: Some(Box::pin(async move { start().await })),
                last_turn: None,
                result: None,
                token: CancellationToken::new(),
            },
        }
    }

    fn later_once(f: impl FnOnce() -> T + Send + 'static) -> Self where T: 'static {
        Self::later(move || async move { f() })
    }

    /// Run one deferred step here when this frame's turn is free.
    fn run_due(&mut self) {
        let FrameJobInner::Later { future, last_turn, result, token } = &mut self.inner else { return };
        let Some(next) = future.as_mut() else { return };
        let answer = if token.is_cancelled() {
            Err(TaskError::Cancelled)
        } else {
            if !FRAME_TURNS.with_borrow_mut(|turns| turns.take(last_turn)) {
                return;
            }
            match catch_unwind(AssertUnwindSafe(|| next.as_mut().poll(&mut Context::from_waker(Waker::noop())))) {
                Ok(Poll::Pending) => return,
                Ok(Poll::Ready(value)) => Ok(value),
                Err(payload) => Err(TaskError::Panicked(panic_report(payload))),
            }
        };
        *future = None;
        *result = Some(answer);
    }

    /// Whether the result is there (a deferred job runs here if it may).
    pub fn is_finished(&mut self) -> bool {
        self.run_due();
        match &self.inner {
            FrameJobInner::Task { task, .. } => task.is_finished(),
            FrameJobInner::Later { future, .. } => future.is_none(),
        }
    }

    /// The result once it is there, taken once (as [`TaskHandle::try_take`]).
    pub fn try_take(&mut self) -> Option<Result<T, TaskError>> {
        self.run_due();
        match &mut self.inner {
            FrameJobInner::Task { task, .. } => task.try_take(),
            FrameJobInner::Later { result, .. } => result.take(),
        }
    }

    pub fn cancel(&self) {
        match &self.inner {
            FrameJobInner::Task { task, .. } => task.cancel(),
            FrameJobInner::Later { token, .. } => token.cancel(),
        }
    }

    pub fn token(&self) -> CancellationToken {
        match &self.inner {
            FrameJobInner::Task { task, .. } => task.token(),
            FrameJobInner::Later { token, .. } => token.clone(),
        }
    }
}

impl<T> Drop for FrameJob<T> {
    fn drop(&mut self) {
        if let FrameJobInner::Task { task, cancel_on_drop: true } = &self.inner {
            if !task.is_finished() { task.cancel(); }
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
        // The process's pool is not this runtime's, whichever Cx happened to start it: other Cx
        // run on it (`Cx::close_task_pool`). No registry lock spans cancellation, job drops or
        // waking a worker.
        let shared = app_pool().lock().unwrap().clone();
        for pool in workers.pools.into_iter().filter(|pool| !pool.ptr_eq(&shared)).filter_map(|pool| pool.upgrade()) {
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

/// The machine's cores ([`makepad_cpu_budget::machine_cores`]: what the OS shows the process, or
/// the machine its run was told, `MAKEPAD_MACHINE_CORES`, where a CPU quota at its start would
/// size it for its whole life).
pub fn available_parallelism() -> NonZeroUsize {
    #[cfg(not(target_arch = "wasm32"))]
    {
        NonZeroUsize::new(makepad_cpu_budget::machine_cores()).unwrap_or(NonZeroUsize::new(1).unwrap())
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

    /// Configure this Cx before taking pool handles to drive their frame jobs inline. Call
    /// [`frame_jobs_new_frame`] at the start of each display frame.
    pub fn use_inline_frame_jobs(&mut self) {
        self.task_pool = std::cell::OnceCell::from(self.task_pool().with_inline_frame_jobs());
    }

    /// From now on this Cx's pool is one of its own: one worker on a CPU budget of its own, so
    /// what it reads of its pool (`/settled`) is its own jobs alone. The process's app pool is
    /// every Cx's, and so every test's in one process.
    #[cfg(all(test, not(target_arch = "wasm32")))]
    pub(crate) fn use_own_task_pool(&mut self) {
        let pool = TaskPool::create(self.thread_spawner(), 1, BudgetHandle::of_cores(2)).expect("a pool of its own");
        self.task_pool = std::cell::OnceCell::from(pool);
    }

    pub(crate) fn warm_task_pool(&self) {
        let status = Cx::set_thread_priority(CxThreadPriority::UserInteractive);
        crate::trace!("pool", "UI thread priority UserInteractive: {status:?}");
        let _ = self.task_pool();
    }

    /// The app pool's backlog once it has started (`/settled`).
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn task_pool_backlog(&self) -> Option<PoolBacklog> {
        self.task_pool.get().map(TaskPool::backlog)
    }

    /// Shared CPU budget plus app worker and queue counts.
    pub fn task_pool_summary(&self) -> String {
        match self.task_pool.get() {
            Some(pool) => pool.summary(),
            None => "pool: not started".to_string(),
        }
    }

    /// This Cx's end: the jobs it submitted stop and it submits no more. The process's pool goes
    /// on for the other Cx, and closes with its last handle.
    pub(crate) fn close_task_pool(&self) {
        if let Some(pool) = self.task_pool.get() {
            crate::trace!("pool", "{}", pool.summary());
            pool.close(ShutdownMode::CancelPending);
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
    fn of_budget(class: u8) -> Self {
        match class {
            makepad_cpu_budget::INTERACTIVE => Self::Interactive,
            makepad_cpu_budget::VISIBLE => Self::Visible,
            _ => Self::Background,
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

#[cfg(not(target_arch = "wasm32"))]
use std::time::Instant;

// Web workers get their monotonic clock from the host.
#[cfg(target_arch = "wasm32")]
struct Instant(f64);
#[cfg(target_arch = "wasm32")]
impl Instant {
    fn now() -> Self {
        Self(Cx::monotonic_now())
    }

    fn elapsed(&self) -> Duration {
        Duration::from_secs_f64((Cx::monotonic_now() - self.0).max(0.0))
    }
}

const CHECKPOINT_QUANTUM: Duration = Duration::from_millis(10);

struct JobContext {
    budget: BudgetHandle,
    permit: Option<u32>,
    checkpoint_at: Instant,
    class: Class,
    label: &'static str,
    ticket: Ticket,
    token: CancellationToken,
    parents: Vec<(Ticket, CancellationToken)>,
    helper: bool,
    /// The group of jobs it takes its turns with ([`worker_wanted`]).
    share: Option<Arc<Share>>,
    /// A strand's job: the class of what queued behind it ([`Strand::submit`]).
    raised: Option<Arc<Raised>>,
}
impl JobContext {
    fn should_yield(&self) -> bool {
        self.permit.is_some_and(|permit| self.budget.should_yield(permit))
    }

    /// Takes on the class the job was raised to: a job whose permit is out now follows it there.
    fn follow_raise(&mut self) {
        let Some(raised) = &self.raised else { return };
        let class = Class::of_budget(raised.class.load(Ordering::Relaxed));
        if class.budget() < self.class.budget() {
            self.class = class;
            if let Some(permit) = self.permit {
                self.budget.reclass(permit, class.budget());
                makepad_cpu_budget::set_thread_class(class.budget());
            }
        }
    }
}
/// The class of the job a strand runs. Whoever comes after a job on a strand waits for all of it,
/// so it runs at the class of the most urgent job behind it: a plate load behind a thumbnail waited
/// for what a thumbnail may get, with the thumbnail parked at a checkpoint for Background's share.
struct Raised {
    class: AtomicU8,
    /// Wakes the job's thread where it waits for a token: it waits for its new class's from then on.
    wake: makepad_cpu_budget::Wake,
}
impl Raised {
    fn raise(&self, class: Class) {
        if self.class.fetch_min(class.budget(), Ordering::Relaxed) > class.budget() {
            self.wake.unpark();
        }
    }
}
thread_local! {
    static JOB: std::cell::RefCell<Option<JobContext>> = const { std::cell::RefCell::new(None) };
    /// The physical worker, including while it runs a nested job on another budget.
    static WORKER_POOL: std::cell::RefCell<Option<(Arc<PoolInner>, usize)>> = const { std::cell::RefCell::new(None) };
}
pub fn current_class() -> Option<Class> {
    JOB.with(|j| j.borrow().as_ref().map(|j| j.class))
}

/// A job that takes task after task (a crew's helper) gives its worker back at its next task
/// boundary when a higher class waits in its pool or for a CPU permit, or the budget is over its
/// cores. A scope's job also gives way if another job of its class waits while its scope holds
/// more than its share of the workers.
/// Fan-out helpers leave at an item boundary so the next job can use this thread.
pub fn worker_wanted() -> bool {
    let Some((class, should_yield)) = JOB.with(|j| j.borrow().as_ref().map(|j| (j.class, j.should_yield()))) else {
        return false;
    };
    should_yield || higher_queued_in_pool(class) || over_share_in_pool()
}

/// The pool's half of [`worker_wanted`]: a job of a higher class than `class` waits in the calling
/// worker's pool. The permit's half answers for the job's budget.
fn higher_queued_in_pool(class: Class) -> bool {
    WORKER_POOL.with(|p| p.borrow().as_ref().is_some_and(|(pool, _)| pool.higher_queued(class)))
}

/// The calling job's group holds more than its part of its pool's workers while another job of
/// its class waits ([`PoolInner::over_share`]).
fn over_share_in_pool() -> bool {
    let Some(share) = JOB.with(|j| j.borrow().as_ref().and_then(|j| j.share.clone())) else {
        return false;
    };
    WORKER_POOL.with(|p| p.borrow().as_ref().is_some_and(|(pool, _)| pool.over_share(&share)))
}

/// A job that yields its CPU permit frees its worker's place until it is admitted again.
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
            // The quantum does not restart: every later poll reports the stale job too, so a
            // caller that ignores one `Stale` (a pacing hook) never hides it from the next.
            return Err(Stale);
        }
        job.checkpoint_at = Instant::now();
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
                job.follow_raise();
                let site = site.as_deref().unwrap_or("");
                let mut admission = job.budget.admission();
                job.permit = admission.checkpoint(permit, held, site);
                if job.permit.is_none() {
                    let _parked = ParkedWorker::park_if(|_| true);
                    loop {
                        let class = job.raised.as_ref().map_or(job.class.budget(), |raised| {
                            raised.class.load(Ordering::Relaxed)
                        });
                        if let Some(permit) = admission.poll(class, makepad_cpu_budget::APP, job.label) {
                            job.permit = Some(permit);
                            job.class = Class::of_budget(class);
                            break;
                        }
                        makepad_cpu_budget::park_admission(class);
                    }
                }
            }
        }
        Ok(())
    })
}

fn checkpoint_due() -> bool {
    JOB.with(|j| {
        j.borrow().as_ref().is_some_and(|job| job.checkpoint_at.elapsed() >= CHECKPOINT_QUANTUM)
    })
}

/// Poll from a loop whose iterations take microseconds. A full [`checkpoint`] takes the
/// budget lock and records the stretch since the previous checkpoint, which can cost more
/// than the iteration. The first poll after a scheduling quantum calls that checkpoint.
/// Outside a pool job this returns `Ok(())`.
pub fn checkpoint_when_due() -> Result<(), Stale> {
    if checkpoint_due() { checkpoint() } else { Ok(()) }
}

/// The calling thread is a pool's own worker, whose OS class (macOS QoS, Linux nice and scheduling
/// policy) follows the job it runs. A thread the pool does not own (an app's long-lived thread, a
/// tool's, a test's) that fans out or runs a claimed job keeps its own priority while it is in the
/// job (`JobGuard`): its share runs at that priority, its helpers at the job's class. A Linux thread
/// cannot climb back from Background, and a thread outside the pool's jobs has no class to go back
/// to, so its first Background fan-out used to demote it for good.
fn on_pool_worker() -> bool {
    WORKER_POOL.with(|p| p.borrow().is_some())
}

// On the same budget a nested job borrows the caller's row, which a checkpoint can replace.
// Across budgets the caller keeps its row and lends its token until the nested guard drops.
// A thread `set_ui_thread` marked (the UI's, the JIT owner's) takes no row: it is outside
// admission, and its wait for a token would stall the UI, or every thread waiting on the JIT.
struct JobGuard {
    previous: Option<JobContext>,
    borrowed: bool,
    /// on a thread outside the pool: whether it kept its priority before this job
    kept: Option<bool>,
}
impl JobGuard {
    fn enter(
        class: Class,
        label: &'static str,
        ticket: Ticket,
        token: CancellationToken,
        permit: Option<u32>,
        raised: Option<Arc<Raised>>,
        budget: &BudgetHandle,
    ) -> Self {
        // before admission, which applies the job's class to the admitted thread
        let kept = (!on_pool_worker()).then(|| makepad_cpu_budget::set_thread_keeps_priority(true));
        let mut previous = JOB.with(|j| j.borrow_mut().take());
        let borrowed_permit = previous.as_mut().and_then(|job| {
            let row = job.permit?;
            if job.budget.id() == budget.id() {
                debug_assert!(permit.is_none());
                budget.reclass(row, class.budget());
                job.permit.take()
            } else {
                job.budget.lend_permit(row);
                None
            }
        });
        let borrowed = borrowed_permit.is_some();
        let permit = match borrowed_permit.or(permit) {
            Some(row) => Some(row),
            None if makepad_cpu_budget::on_ui_thread() => None,
            None => Some(budget.admit(class.budget(), makepad_cpu_budget::APP, label)),
        };
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
                budget: budget.clone(),
                permit,
                checkpoint_at: Instant::now(),
                class,
                label,
                ticket,
                token,
                parents,
                helper: false,
                share: None,
                raised: raised.or_else(|| if borrowed { previous.as_ref().and_then(|p| p.raised.clone()) } else { None }),
            })
        });
        Self { previous, borrowed, kept }
    }
}
impl Drop for JobGuard {
    fn drop(&mut self) {
        let job = JOB.with(|j| j.borrow_mut().take()).unwrap();
        if let Some(permit) = job.permit {
            if self.borrowed {
                let previous = self.previous.as_mut().unwrap();
                if let Some(raised) = &previous.raised {
                    previous.class = Class::of_budget(raised.class.load(Ordering::Relaxed));
                }
                job.budget.reclass(permit, previous.class.budget());
                previous.permit = Some(permit);
            } else {
                job.budget.release(permit);
            }
        }
        if let Some(JobContext { budget, permit: Some(row), class, .. }) = &self.previous {
            if !self.borrowed {
                budget.take_back_permit(*row);
            }
            makepad_cpu_budget::set_thread_class(class.budget());
        }
        JOB.with(|j| *j.borrow_mut() = self.previous.take());
        if let Some(kept) = self.kept {
            makepad_cpu_budget::set_thread_keeps_priority(kept);
        }
    }
}
/// Runs `f`, which waits for other jobs, without the calling job's CPU permit: waiting never
/// retains one. The job is admitted again once `f` returns (or unwinds). Hold no lock across it
/// that a job admitted meanwhile needs: admission may wait for one of them.
/// The waiting job frees its place for a spare worker through its re-admission.
pub fn without_permit<R>(f: impl FnOnce() -> R) -> R {
    struct Restore {
        released: bool,
        // Fields drop after Restore::drop re-admits the job, including on unwind.
        _parked: Option<ParkedWorker>,
    }
    impl Drop for Restore {
        fn drop(&mut self) {
            if self.released {
                JOB.with(|j| {
                    let mut context = j.borrow_mut();
                    let job = context.as_mut().unwrap();
                    job.permit = Some(match &job.raised {
                        Some(raised) => {
                            let permit = job.budget.admit_following(raised.class.as_ptr(), makepad_cpu_budget::APP, job.label);
                            job.class = Class::of_budget(raised.class.load(Ordering::Relaxed));
                            permit
                        }
                        None => job.budget.admit(job.class.budget(), makepad_cpu_budget::APP, job.label),
                    });
                });
            }
        }
    }
    let released = JOB.with(|j| {
        let mut context = j.borrow_mut();
        let Some(job) = context.as_mut() else { return false };
        let Some(permit) = job.permit.take() else { return false };
        job.budget.release(permit);
        true
    });
    let mut restore = Restore { released, _parked: None };
    if released {
        restore._parked = ParkedWorker::park_if(|_| true);
    }
    let result = f();
    drop(restore);
    result
}

/// Gives the calling job's CPU permit back and asks for it again behind every job that waits for
/// one: a fresh FIFO ticket, where a [`checkpoint`] keeps the job's first. A long job of short
/// steps calls it between them so that a job of its class queued meanwhile runs first. Otherwise
/// the oldest job keeps its class's last permit to its end, which on a small budget (one Background
/// permit at four cores) holds every other job of that class. With nothing waiting, it goes on at
/// once. Outside a pool job it does nothing.
pub fn yield_to_waiting() {
    without_permit(|| ());
}

/// Waits the UI thread made through [`makepad_core_util::blocking`], for every pool: the platform
/// test asserts it stays put.
static UI_THREAD_WAITS: AtomicUsize = AtomicUsize::new(0);

/// [`makepad_core_util::blocking`]'s hook, installed with the first pool: a pool job waiting for
/// another thread's work (a [`makepad_core_util::once::ComputeOnce`] another job computes) holds
/// no CPU permit while it waits ([`without_permit`]), so the job it waits for can pace. A UI
/// thread must not wait so; one that does is counted and logged once per waited-for type, and
/// still waits: a wait is better than a crash. Runs only on a contended wait, never per frame.
fn blocking_wait(wait: &mut dyn FnMut(), what: &'static str) {
    if makepad_core_util::ui_thread::is_ui_thread() {
        UI_THREAD_WAITS.fetch_add(1, Ordering::Relaxed);
        static LOGGED: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());
        let mut logged = LOGGED.lock().unwrap_or_else(|e| e.into_inner());
        if !logged.contains(&what) {
            logged.push(what);
            drop(logged);
            crate::log!("the UI thread waited for another thread to compute a {what}: read it with `get` and ask a job for it");
        }
    }
    without_permit(|| wait())
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
    /// The work, then its handle's answer: it counts the job in `completed` between the two.
    run: Option<Box<dyn FnOnce(&AtomicU64) + Send>>,
    cancel: Option<Box<dyn FnOnce() + Send>>,
    /// Its place in the jobs of the handle it was submitted through (`JobSet`): left once it
    /// ran or was dropped, after its handle is answered.
    member: Option<SetMember>,
    /// A strand's job, as it runs ([`Raised`]).
    raised: Option<Arc<Raised>>,
}
impl PoolJob {
    /// Runs the job and counts it in `completed` before its handle is answered.
    fn run(mut self, completed: &AtomicU64) {
        if self.ticket.is_stale() || self.token.is_cancelled() {
            completed.fetch_add(1, Ordering::Relaxed);
            // the drop answers the handle (cancelled)
            return;
        }
        self.cancel.take();
        if let Some(run) = self.run.take() {
            run(completed);
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
    task_job_with_token(
        ui_thread, class, label, ticket, CancellationToken::new(), move || Ok(f()), internal,
    )
}

fn task_job_with_token<F, T>(
    ui_thread: ThreadId,
    class: Class,
    label: &'static str,
    ticket: Ticket,
    token: CancellationToken,
    f: F,
    internal: bool,
) -> (PoolJob, TaskHandle<T>)
where
    F: FnOnce() -> Result<T, TaskError> + Send + 'static,
    T: Send + 'static,
{
    let state = Arc::new(TaskState::default());
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
        run: Some(Box::new(move |completed: &AtomicU64| {
            // (the class it runs at: a strand's job may have been raised since it was queued)
            status.store(
                if on_pool_worker() && makepad_cpu_budget::set_thread_class(current_class().unwrap_or(class).budget()) {
                    PriorityStatus::Applied
                } else {
                    // refused, or run by a thread outside the pool, which keeps its own priority
                    PriorityStatus::BestEffortUnsupported
                } as u8,
                Ordering::Release,
            );
            let result = catch_unwind(AssertUnwindSafe(f))
                .unwrap_or_else(|p| Err(TaskError::Panicked(panic_report(p))));
            completed.fetch_add(1, Ordering::Relaxed);
            run_state.complete(result);
            signal_completion(internal);
        })),
        cancel: Some(Box::new(move || {
            cancel_state.complete(Err(TaskError::Cancelled));
            signal_completion(internal);
        })),
        member: None,
        raised: None,
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
/// its last handle drops.
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

/// The [`Share`]s of one class of a pool: how many have a job on a worker or queued, and all
/// their jobs on a worker and queued.
#[derive(Default)]
struct ClassShares {
    groups: AtomicUsize,
    workers: AtomicUsize,
    queued: AtomicUsize,
}

/// One class's jobs that work together: a fan-out's caller and helpers, or a scope's jobs. A
/// class's workers are shared between these groups ([`PoolInner::over_share`]).
struct Share {
    class: usize,
    of: Arc<ClassShares>,
    /// Its jobs on a worker or queued.
    jobs: AtomicUsize,
    /// Its jobs on a worker.
    workers: AtomicUsize,
    /// Its jobs queued.
    queued: AtomicUsize,
}

impl Share {
    fn new(pool: &PoolInner, class: Class) -> Arc<Share> {
        let index = class.budget() as usize;
        Arc::new(Share { class: index, of: pool.shares[index].clone(), jobs: AtomicUsize::new(0), workers: AtomicUsize::new(0), queued: AtomicUsize::new(0) })
    }
    fn joined(&self) {
        if self.jobs.fetch_add(1, Ordering::Relaxed) == 0 {
            self.of.groups.fetch_add(1, Ordering::Relaxed);
        }
    }
    fn left(&self) {
        if self.jobs.fetch_sub(1, Ordering::Relaxed) == 1 {
            self.of.groups.fetch_sub(1, Ordering::Relaxed);
        }
    }
    fn add_worker(&self) {
        self.workers.fetch_add(1, Ordering::Relaxed);
        self.of.workers.fetch_add(1, Ordering::Relaxed);
    }
    fn sub_queued(&self) {
        self.queued.fetch_sub(1, Ordering::Relaxed);
        self.of.queued.fetch_sub(1, Ordering::Relaxed);
    }
}

/// A job counted in its [`Share`], from its queueing or its start to its end. Dropped before it
/// started, it left the queue unrun.
struct ShareJob {
    share: Arc<Share>,
    on_worker: bool,
}

impl ShareJob {
    /// The job calling this, already on a worker.
    fn running(share: Arc<Share>) -> Self {
        share.joined();
        share.add_worker();
        ShareJob { share, on_worker: true }
    }
    fn queued(share: Arc<Share>) -> Self {
        share.joined();
        share.queued.fetch_add(1, Ordering::Relaxed);
        share.of.queued.fetch_add(1, Ordering::Relaxed);
        ShareJob { share, on_worker: false }
    }
    fn start(&mut self) {
        self.share.add_worker();
        self.share.sub_queued();
        self.on_worker = true;
    }
}

impl Drop for ShareJob {
    fn drop(&mut self) {
        if self.on_worker {
            self.share.workers.fetch_sub(1, Ordering::Relaxed);
            self.share.of.workers.fetch_sub(1, Ordering::Relaxed);
        } else {
            self.share.sub_queued();
        }
        self.share.left();
    }
}

#[derive(Default)]
struct PoolState {
    queues: [VecDeque<PoolJob>; 3],
    waiting: [bool; 3],
    closed: u8,
    running: Vec<Option<CancellationToken>>,
    starts: HashMap<&'static str, u64>,
    /// All-class workers reserved for or running Visible and Background jobs, excluding parked jobs.
    active: usize,
    parked: usize,
    /// All-class workers with neither an admission request nor a job.
    free: usize,
    slots: Vec<WorkerCount>,
    /// Episodes with no spare worker for queued work, reported per stretch, not per episode.
    no_spare: NoSpare,
    /// The stretch reports a worker writes outside the lock.
    pending_log: Vec<SpareReport>,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum WorkerCount {
    Free,
    Interactive,
    Active,
    Parked,
}
impl WorkerCount {
    fn for_class(class: usize) -> Self {
        if class == makepad_cpu_budget::INTERACTIVE as usize { Self::Interactive } else { Self::Active }
    }
}
/// How far apart two no-spare episodes may be and still belong to one stretch. A stretch is
/// reported as it begins, with the queues then, and once after this long without an episode, with
/// its episodes counted; an app that runs out of spares many times a second logs two lines, not
/// one per episode (13,703 of an app's 13,757 lines, 11-26).
const NO_SPARE_QUIET_S: f64 = 1.0;

/// The pool's stretch of no-spare episodes: an episode begins when queued lower-class work finds
/// no free all-class worker and a parked one, and ends when a worker next goes idle. Times are
/// monotonic seconds.
#[derive(Default)]
struct NoSpare {
    /// When the stretch began, while one lasts.
    since: Option<f64>,
    /// When the running episode began.
    episode: Option<f64>,
    /// When the stretch's last episode ended, while none runs.
    ended: Option<f64>,
    episodes: u64,
    longest: f64,
}
#[derive(Debug, PartialEq)]
enum SpareReport {
    Began { queued: [usize; 3], parked: usize },
    Ended { lasted: f64, episodes: u64, longest: f64 },
}
impl SpareReport {
    fn log(self) {
        match self {
            Self::Began { queued: [i, v, b], parked } => crate::log!(
                "pool: no spare worker free, queued I{i} V{v} B{b}, {parked} parked; further waits are counted until there is a spare for {NO_SPARE_QUIET_S} s"
            ),
            Self::Ended { lasted, episodes, longest } => crate::log!(
                "pool: a spare worker again after {:.0} ms with {episodes} wait(s) for one, the longest {:.0} ms",
                lasted * 1e3, longest * 1e3
            ),
        }
    }
}
impl NoSpare {
    /// An episode begins at `now` unless one runs; the first of a stretch is reported.
    fn begin(&mut self, now: impl FnOnce() -> f64, queued: [usize; 3], parked: usize, reports: &mut Vec<SpareReport>) {
        if self.episode.is_some() {
            return;
        }
        let now = now();
        self.episode = Some(now);
        self.ended = None;
        self.episodes += 1;
        if self.since.is_none() {
            self.since = Some(now);
            reports.push(SpareReport::Began { queued, parked });
        }
    }
    /// A worker went idle: the running episode, if any, ends at `now`.
    fn end_episode(&mut self, now: impl FnOnce() -> f64) {
        if let Some(began) = self.episode.take() {
            let now = now();
            self.longest = self.longest.max(now - began);
            self.ended = Some(now);
        }
    }
    /// The stretch ends once no episode has run for [`NO_SPARE_QUIET_S`] (or, `closing`, now).
    fn end_stretch_if_quiet(&mut self, now: impl FnOnce() -> f64, closing: bool, reports: &mut Vec<SpareReport>) {
        let (Some(since), None) = (self.since, self.episode) else { return };
        let Some(ended) = self.ended else { return };
        if !closing && now() - ended < NO_SPARE_QUIET_S {
            return;
        }
        reports.push(SpareReport::Ended { lasted: ended - since, episodes: self.episodes, longest: self.longest });
        *self = Self::default();
    }
}
/// Only the outermost park of the physical worker owns its return to the count.
struct ParkedWorker {
    pool: Arc<PoolInner>,
    index: usize,
}
impl ParkedWorker {
    fn park_if(should_park: impl FnOnce(&PoolInner) -> bool) -> Option<Self> {
        WORKER_POOL.with(|worker| {
            let worker = worker.borrow();
            let (pool, index) = worker.as_ref()?;
            if *index >= pool.all_class || !should_park(pool) {
                return None;
            }
            let (guard, wake) = pool.with_worker_state(|state| {
                if state.slots.get(*index) != Some(&WorkerCount::Active) {
                    return None;
                }
                state.set_slot(*index, WorkerCount::Parked);
                state.record_exhaustion();
                Some((Self { pool: pool.clone(), index: *index }, pool.idle_candidates(state)))
            })?;
            pool.wake_one(wake);
            Some(guard)
        })
    }
}
impl Drop for ParkedWorker {
    fn drop(&mut self) {
        self.pool.with_worker_state(|state| state.set_slot(self.index, WorkerCount::Active));
    }
}
impl PoolState {
    fn needs_worker(&self, class: usize) -> bool {
        !self.waiting[class] && !self.queues[class].is_empty()
    }
    fn set_slot(&mut self, index: usize, next: WorkerCount) {
        // Only the all-class workers have places to lend. The reserve is independent.
        let Some(slot) = self.slots.get_mut(index) else { return };
        if *slot == next {
            return;
        }
        match *slot {
            WorkerCount::Active => self.active -= 1,
            WorkerCount::Parked => self.parked -= 1,
            WorkerCount::Free => self.free -= 1,
            WorkerCount::Interactive => {}
        }
        match next {
            WorkerCount::Active => self.active += 1,
            WorkerCount::Parked => self.parked += 1,
            WorkerCount::Free => self.free += 1,
            WorkerCount::Interactive => {}
        }
        *slot = next;
    }
    /// An unclaimed lower-class job with no free all-class thread begins a no-spare episode
    /// ([`NoSpare`]). A worker writes a stretch's report outside the lock.
    fn record_exhaustion(&mut self) {
        if self.free == 0 && self.parked != 0 && self.no_spare.episode.is_none()
            && (1..3).any(|i| self.queues[i].len() > usize::from(self.waiting[i]))
        {
            let queued = self.queues.each_ref().map(VecDeque::len);
            self.no_spare.begin(Cx::monotonic_now, queued, self.parked, &mut self.pending_log);
        }
    }
}
struct WorkerSlot {
    thread: OnceLock<std::thread::Thread>,
    idle: AtomicBool,
    /// The worker's native park token, and the class it waits for a permit of with it (its budget
    /// index + 1; 0: none, [`PoolInner::admit`]): work of a higher class, or the pool closing,
    /// wakes it to look again.
    wake: OnceLock<makepad_cpu_budget::Wake>,
    admitting: AtomicU8,
    /// Number of classes it takes, from Interactive through Visible to Background.
    takes: usize,
}
struct PoolInner {
    budget: BudgetHandle,
    state: Mutex<PoolState>,
    /// Jobs queued per class (as `state.queues`, read without the lock).
    queued: [AtomicUsize; 3],
    /// The groups of jobs each class's workers are shared between.
    shares: [Arc<ClassShares>; 3],
    /// The classes a worker waits for a permit of (as `state.waiting`, one bit per class, read
    /// without the lock): work of a class no worker waits for wakes one waiting for a lower class.
    waiting: AtomicU8,
    /// Workers in wake order: general, spare, Visible reserve, Interactive reserve.
    workers: Vec<WorkerSlot>,
    /// Limit for new all-class worker reservations below Interactive, and width for work splits.
    general: usize,
    /// End of the general and spare workers in `workers`. Only these have counted slots.
    all_class: usize,
    /// End of the Visible reserve, where the Interactive-only workers begin.
    visible_end: usize,
    started: AtomicUsize,
    exited: AtomicUsize,
    /// Jobs the workers ran, each counted before its handle is answered: whoever has a job's
    /// result sees it here.
    completed: AtomicU64,
    /// Jobs the workers took off the queues with their permits, so far.
    taken: AtomicU64,
    handles: AtomicUsize,
    shutdown_state: Arc<TaskState<()>>,
    ui_thread: ThreadId,
}
/// What a job's time from its submit to its start was, for the CPU budget's report.
enum Turn {
    /// waiting for a worker and a permit: the pool's wait
    Pool,
    /// a strand's job: until `at` it waited behind the strand's earlier jobs; from `at` its
    /// strand's wrapper waited for a worker, and reported that as the pool's
    Strand { at: f64 },
}
const POOL_OPEN: u8 = 0;
const POOL_DRAINING: u8 = 1;
const POOL_CANCELLED: u8 = 2;
impl PoolInner {
    /// Worker-side transitions also deliver any exhaustion report captured by a submitter.
    fn with_worker_state<T>(&self, f: impl FnOnce(&mut PoolState) -> T) -> T {
        let mut guard = self.state.lock().unwrap();
        let result = f(&mut guard);
        let state = &mut *guard;
        state.no_spare.end_stretch_if_quiet(Cx::monotonic_now, false, &mut state.pending_log);
        let reports = std::mem::take(&mut state.pending_log);
        drop(guard);
        for report in reports {
            report.log();
        }
        result
    }
    fn enqueue(&self, job: PoolJob) -> Result<(), SubmitError> {
        let mut state = self.state.lock().unwrap();
        if state.closed != POOL_OPEN {
            return Err(SubmitError::Closed);
        }
        self.push(&mut state, job);
        let wake = self.idle_candidates(&state);
        drop(state);
        self.wake_one(wake);
        Ok(())
    }
    /// A strand's wrapper waiting in the queue takes `class`: a more urgent job is queued behind
    /// the one it runs ([`Strand::submit`]).
    fn raise(&self, state: &mut PoolState, token: &CancellationToken, class: Class) {
        for from in 0..state.queues.len() {
            if let Some(at) = state.queues[from].iter().position(|job| Arc::ptr_eq(&job.token.inner, &token.inner)) {
                let mut job = state.queues[from].remove(at).expect("the wrapper is queued");
                self.budget.queued(job.class.budget(), makepad_cpu_budget::APP, -1);
                self.queued[from].fetch_sub(1, Ordering::Relaxed);
                job.class = class;
                self.push(state, job);
                return;
            }
        }
    }
    fn push(&self, state: &mut PoolState, job: PoolJob) {
        let index = job.class.budget() as usize;
        self.budget.queued(job.class.budget(), makepad_cpu_budget::APP, 1);
        self.queued[index].fetch_add(1, Ordering::Relaxed);
        state.queues[index].push_back(job);
        state.record_exhaustion();
    }
    /// A job of a higher class than `class` waits to start in this pool.
    fn higher_queued(&self, class: Class) -> bool {
        self.queued[..class.budget() as usize].iter().any(|q| q.load(Ordering::Relaxed) > 0)
    }
    /// The calling thread is one of this pool's workers.
    fn on_worker(&self) -> bool {
        WORKER_POOL.with(|p| p.borrow().as_ref().is_some_and(|(pool, _)| std::ptr::eq(Arc::as_ptr(pool), self)))
    }
    /// A job of `share` should give its worker back at its next boundary: another job of its class
    /// waits (every worker is busy, or that job would have one) and `share` holds more than its
    /// part of the general width. The groups and any waiting ungrouped work share that width.
    /// With only grouped work waiting, the groups divide what they hold up to that width.
    /// Its own queued jobs wait for nothing it holds. A group at its part stays, so groups
    /// at rest do not trade workers; its queued job starts when another group's work thins out.
    fn over_share(&self, share: &Share) -> bool {
        let queued = self.queued[share.class].load(Ordering::Relaxed);
        if queued <= share.queued.load(Ordering::Relaxed) {
            return false;
        }
        let groups = share.of.groups.load(Ordering::Relaxed).max(1);
        let width = self.workers_of(share.class);
        let part = if queued > share.of.queued.load(Ordering::Relaxed) {
            width.div_ceil(groups + 1)
        } else {
            share.of.workers.load(Ordering::Relaxed).min(width).div_ceil(groups)
        };
        share.workers.load(Ordering::Relaxed) > part
    }
    /// Every class splits by the general width. The reserves keep threads available for new jobs.
    fn workers_of(&self, _class: usize) -> usize {
        self.general
    }
    /// Snapshot under the queue lock, so the submitter can wake outside it. An empty range
    /// for Visible can still mean a Background admission needs to switch its request.
    fn idle_candidates(&self, state: &PoolState) -> Option<Range<usize>> {
        if state.needs_worker(makepad_cpu_budget::INTERACTIVE as usize) {
            return Some(0..self.workers.len());
        }
        let room = state.active < self.general;
        if state.needs_worker(makepad_cpu_budget::VISIBLE as usize) {
            let start = if room { 0 } else { self.all_class };
            return Some(start..self.visible_end);
        }
        (room && state.needs_worker(makepad_cpu_budget::BACKGROUND as usize)).then_some(0..self.all_class)
    }
    fn wake_one(&self, candidates: Option<Range<usize>>) {
        let Some(candidates) = candidates else { return };
        for index in candidates {
            let worker = &self.workers[index];
            if worker.idle.swap(false, Ordering::SeqCst) {
                if let Some(t) = worker.thread.get() {
                    t.unpark();
                }
                if index < self.all_class {
                    return;
                }
                break;
            }
        }
        self.wake_admitting(false);
    }
    /// Workers waiting for a permit look at the queues again ([`PoolInner::admit`]): `all`, or
    /// those whose class is below work that queues with no worker waiting for its class.
    fn wake_admitting(&self, all: bool) {
        let waiting = self.waiting.load(Ordering::SeqCst);
        let unclaimed = |h: usize| self.queued[h].load(Ordering::SeqCst) > 0 && waiting & (1 << h) == 0;
        for worker in &self.workers {
            let class = match worker.admitting.load(Ordering::SeqCst) {
                0 => continue,
                at => (at - 1) as usize,
            };
            if all || (0..class).any(unclaimed) {
                if let Some(wake) = worker.wake.get() {
                    wake.unpark();
                }
            }
        }
    }
    fn unpark_all(&self) {
        for worker in &self.workers {
            if let Some(t) = worker.thread.get() {
                t.unpark();
            }
        }
        self.wake_admitting(true);
    }
    /// A worker waits for a permit of `class`, or no longer.
    fn set_waiting(&self, state: &mut PoolState, class: usize, on: bool) {
        state.waiting[class] = on;
        if on {
            self.waiting.fetch_or(1 << class, Ordering::SeqCst);
        } else {
            self.waiting.fetch_and(!(1 << class), Ordering::SeqCst);
        }
    }
    fn eligible_classes(&self, state: &PoolState, index: usize) -> usize {
        if index >= self.all_class || state.active < self.general || state.slots[index] == WorkerCount::Active {
            self.workers[index].takes
        } else {
            1 // Interactive always has access to the worker.
        }
    }
    /// A permit for worker `index`, asked for at `class` (`state.waiting` set for it, the class's
    /// head job `label`). While it waits, a higher class whose work no worker waits for takes the
    /// request over (it keeps its place in the budget's queue): a worker never waits in a lower
    /// class's admission while higher-class work queues with no worker free for it. None: the
    /// queues it takes from emptied meanwhile (the pool closed, or other workers took their jobs).
    fn admit(&self, index: usize, class: &mut usize, label: &mut &'static str) -> Option<u32> {
        let worker = &self.workers[index];
        let mut admission = self.budget.admission();
        // (before the queues are read: whoever queues higher work after that read wakes it)
        worker.admitting.store(*class as u8 + 1, Ordering::SeqCst);
        let permit = loop {
            let (wake, any) = self.with_worker_state(|state| {
                let takes = self.eligible_classes(state, index);
                let any = state.queues[..takes].iter().any(|q| !q.is_empty());
                let higher = (0..*class).find(|&i| state.needs_worker(i));
                if !any || higher.is_some() {
                    self.set_waiting(state, *class, false);
                }
                if !any {
                    // With a reservation, every queue this worker takes must be empty here.
                    state.set_slot(index, WorkerCount::Free);
                }
                if let Some(higher) = higher {
                    self.set_waiting(state, higher, true);
                    if higher == makepad_cpu_budget::INTERACTIVE as usize {
                        state.set_slot(index, WorkerCount::Interactive);
                    }
                    state.record_exhaustion();
                    *class = higher;
                    *label = state.queues[higher].front().unwrap().label;
                    worker.admitting.store(higher as u8 + 1, Ordering::SeqCst);
                }
                (higher.and_then(|_| self.idle_candidates(state)), any)
            });
            if !any {
                break None;
            }
            // The class it left may now have work and no worker waiting for it.
            self.wake_one(wake);
            if let Some(permit) = admission.poll(*class as u8, makepad_cpu_budget::APP, label) {
                break Some(permit);
            }
            makepad_cpu_budget::park_admission(*class as u8);
        };
        worker.admitting.store(0, Ordering::SeqCst);
        permit
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
                self.budget.queued(class as u8, makepad_cpu_budget::APP, -1);
                self.queued[class].fetch_sub(1, Ordering::Relaxed);
                drop(job);
            }
        }
        self.unpark_all();
    }
    fn summary(&self) -> String {
        use std::fmt::Write;
        let (queued, closed, active, parked, mut starts) = {
            let state = self.state.lock().unwrap();
            let starts: Vec<_> = state.starts.iter().map(|(&label, &count)| (std::cmp::Reverse(count), label)).collect();
            (state.queues.each_ref().map(VecDeque::len), state.closed != POOL_OPEN, state.active, state.parked, starts)
        };
        starts.sort_unstable();
        let mut summary = format!(
            "{}pool: general={} spares={} visible_reserve={} interactive_reserve={} active={active} parked={parked} started={} exited={} queued={}/{}/{} taken={} completed={} closed={}\nstarts:",
            self.budget.report_string(),
            self.general,
            self.all_class - self.general,
            self.visible_end - self.all_class,
            self.workers.len() - self.visible_end,
            self.started.load(Ordering::Acquire),
            self.exited.load(Ordering::Acquire),
            queued[0],
            queued[1],
            queued[2],
            self.taken.load(Ordering::Relaxed),
            self.completed.load(Ordering::Relaxed),
            closed
        );
        for (std::cmp::Reverse(count), label) in starts {
            write!(summary, " {label}={count}").unwrap();
        }
        summary.push('\n');
        summary
    }
    /// One line of the pool's timeline ([`trace_timeline`]): its workers on a job (including
    /// the parked subset), idle, or admitting one; the jobs queued per class; the budget's
    /// running tokens and its waiters per class; who holds a permit, by class and label (`~`: lent by a thread
    /// asleep on other work); and what is queued, by class and label. A worker on a job with no
    /// permit waits for other jobs.
    #[cfg(not(target_arch = "wasm32"))]
    fn timeline(&self) -> String {
        let (on_jobs, parked, queued, queued_labels) = {
            let state = self.state.lock().unwrap();
            let mut labels: Vec<(usize, &str, usize)> = Vec::new();
            for (c, q) in state.queues.iter().enumerate() {
                for job in q {
                    let label = job.label.split('<').next().unwrap_or(job.label);
                    match labels.iter_mut().find(|e| e.0 == c && e.1 == label) {
                        Some(e) => e.2 += 1,
                        None => labels.push((c, label, 1)),
                    }
                }
            }
            (state.running.iter().filter(|r| r.is_some()).count(), state.parked, state.queues.each_ref().map(VecDeque::len), labels)
        };
        let idle = self.workers.iter().filter(|w| w.idle.load(Ordering::Relaxed)).count();
        let tokens = self.budget.tokens();
        let mut waiting = [0usize; 3];
        for w in &tokens.waiting {
            waiting[w.class as usize] += 1;
        }
        let mut held: Vec<(u8, bool, &str, usize)> = Vec::new();
        for h in &tokens.holders {
            let label = h.label.as_str();
            match held.iter_mut().find(|e| e.0 == h.class && e.1 == h.lent && e.2 == label) {
                Some(e) => e.3 += 1,
                None => held.push((h.class, h.lent, label, 1)),
            }
        }
        held.sort_by(|a, b| (a.0, a.1, std::cmp::Reverse(a.3)).cmp(&(b.0, b.1, std::cmp::Reverse(b.3))));
        let class = |c: u8| ["I", "V", "B"][c as usize];
        // on Linux, the workers the kernel has on a CPU or ready for one: a permit held by a
        // worker that is not is held by a thread asleep (on a lock, a condition)
        #[cfg(target_os = "linux")]
        let on_cpu = std::fs::read_dir("/proc/self/task").map_or(0, |dir| {
            dir.filter_map(Result::ok)
                .filter_map(|task| std::fs::read_to_string(task.path().join("stat")).ok())
                .filter(|stat| {
                    stat.split_once('(').and_then(|(_, rest)| rest.rsplit_once(')')).is_some_and(|(name, rest)| {
                        name.starts_with("makepad-pool-") && rest.trim_start().starts_with('R')
                    })
                })
                .count()
        });
        #[cfg(not(target_os = "linux"))]
        let on_cpu = "?";
        let mut line = format!(
            "pool timeline t={:.3} workers {}+{}+{}+{}: {on_jobs} on jobs, {parked} parked, {idle} idle, {on_cpu} on a CPU | queued I{} V{} B{} | cpu {}/{} running, waiting I{} V{} B{} |",
            Cx::monotonic_now(),
            self.general,
            self.all_class - self.general,
            self.visible_end - self.all_class,
            self.workers.len() - self.visible_end,
            queued[0],
            queued[1],
            queued[2],
            tokens.running,
            tokens.limits[1],
            waiting[0],
            waiting[1],
            waiting[2],
        );
        for (c, lent, label, n) in held {
            line.push_str(&format!(" {}{} {label} x{n},", class(c), if lent { "~" } else { "" }));
        }
        if !queued_labels.is_empty() {
            line.push_str(" queued:");
            for (c, label, n) in queued_labels {
                line.push_str(&format!(" {} {label} x{n},", class(c as u8)));
            }
        }
        line
    }
    fn run_job(&self, mut job: PoolJob, permit: Option<u32>, turn: Turn) {
        let class = job.class;
        let label = job.label;
        let _context = JobGuard::enter(class, label, job.ticket.clone(), job.token.clone(), permit, job.raised.take(), &self.budget);
        let ns = |secs: f64| (secs.max(0.0) * 1e9) as u64;
        match turn {
            Turn::Pool => self.budget.waited(
                class.budget(),
                makepad_cpu_budget::APP,
                ns(Cx::monotonic_now() - job.submitted_at),
                label,
            ),
            Turn::Strand { at } => {
                self.budget.strand_waited(class.budget(), ns(at - job.submitted_at), label)
            }
        }
        #[cfg(any(target_vendor = "apple", target_os = "linux"))]
        let traced = ui_hang::relax_timeline::hooks()
            .inspect(|h| (h.begin)(label.as_ptr(), label.len()))
            .is_some();
        if let Err(panic) = catch_unwind(AssertUnwindSafe(|| job.run(&self.completed))) {
            crate::error!("pool job cleanup panicked: {}", panic_report(panic).message);
        }
        #[cfg(any(target_vendor = "apple", target_os = "linux"))]
        if traced {
            if let Some(h) = ui_hang::relax_timeline::hooks() {
                (h.end)(label.as_ptr(), label.len());
            }
        }
    }
}
fn pool_worker(inner: Arc<PoolInner>, index: usize) {
    enum Next {
        Stop,
        Idle(Option<Range<usize>>),
        Admit(usize, &'static str),
    }
    let _ = inner.workers[index].thread.set(std::thread::current());
    let _ = inner.workers[index].wake.set(makepad_cpu_budget::Wake::current());
    WORKER_POOL.with(|p| *p.borrow_mut() = Some((inner.clone(), index)));
    inner.started.fetch_add(1, Ordering::AcqRel);
    let takes = inner.workers[index].takes;
    loop {
        let next = inner.with_worker_state(|state| {
            if state.closed != POOL_OPEN && state.queues[..takes].iter().all(VecDeque::is_empty) {
                // a stretch still open is reported as the pool closes
                state.no_spare.end_episode(Cx::monotonic_now);
                state.no_spare.end_stretch_if_quiet(Cx::monotonic_now, true, &mut state.pending_log);
                return Next::Stop;
            }
            let eligible = inner.eligible_classes(state, index);
            let candidate = (0..eligible).find(|&i| state.needs_worker(i));
            if let Some(i) = candidate {
                inner.set_waiting(state, i, true);
                state.set_slot(index, WorkerCount::for_class(i));
                state.record_exhaustion();
                inner.workers[index].idle.store(false, Ordering::SeqCst);
                Next::Admit(i, state.queues[i].front().unwrap().label)
            } else {
                // A parked job may have reclaimed the last place since this worker was woken.
                // Pass unclaimed Visible work to its reserve or a lower admission.
                let wake = if eligible < takes { inner.idle_candidates(state) } else { None };
                state.no_spare.end_episode(Cx::monotonic_now);
                inner.workers[index].idle.store(true, Ordering::SeqCst);
                Next::Idle(wake)
            }
        });
        let (mut waiting, mut label) = match next {
            Next::Stop => break,
            Next::Idle(wake) => {
                inner.wake_one(wake);
                std::thread::park();
                continue;
            }
            Next::Admit(class, label) => (class, label),
        };
        // Only one worker per class waits for a permit. Others remain available to
        // submit a higher-class request.
        let Some(permit) = inner.admit(index, &mut waiting, &mut label) else {
            continue;
        };
        let (job, wake) = inner.with_worker_state(|state| {
            inner.set_waiting(state, waiting, false);
            let eligible = inner.eligible_classes(state, index);
            let job = state.queues[..eligible].iter_mut().find_map(VecDeque::pop_front);
            if let Some(job) = &job {
                state.set_slot(index, WorkerCount::for_class(job.class.budget() as usize));
                inner.budget.queued(job.class.budget(), makepad_cpu_budget::APP, -1);
                inner.queued[job.class.budget() as usize].fetch_sub(1, Ordering::Relaxed);
                inner.taken.fetch_add(1, Ordering::Relaxed);
                *state.starts.entry(job.label).or_default() += 1;
                state.running[index] = Some(job.token.clone());
                state.record_exhaustion();
            } else {
                state.set_slot(index, WorkerCount::Free);
            }
            (job, inner.idle_candidates(state))
        });
        inner.wake_one(wake);
        if let Some(job) = job {
            if job.class.budget() != waiting as u8 {
                inner.budget.reclass(permit, job.class.budget());
            }
            inner.run_job(job, Some(permit), Turn::Pool);
            let wake = inner.with_worker_state(|state| {
                state.running[index] = None;
                let counted = state.slots.get(index) == Some(&WorkerCount::Active);
                state.set_slot(index, WorkerCount::Free);
                if counted { inner.idle_candidates(state) } else { None }
            });
            inner.wake_one(wake);
        } else {
            inner.budget.release(permit);
        }
    }
    WORKER_POOL.with(|p| *p.borrow_mut() = None);
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

/// What a pool holds back ([`TaskPool::backlog`]): the jobs that wait for a worker and its CPU
/// permit, per class; the jobs on its workers; and the jobs its workers have taken so far (a
/// count that only grows: queued jobs and no new ones taken is a starved pool).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct PoolBacklog {
    pub(crate) queued: [usize; 3],
    pub(crate) running: usize,
    pub(crate) taken: u64,
}

/// A handle on a pool of workers. Clones share the pool and the handle's jobs (`JobSet`):
/// closing or shutting down a handle acts on the jobs submitted through it and its clones,
/// never on those of another handle of the same pool.
pub struct TaskPool {
    inner: Arc<PoolInner>,
    set: Arc<JobSet>,
    inline_frame_jobs: bool,
}
impl Clone for TaskPool {
    fn clone(&self) -> Self {
        self.inner.handles.fetch_add(1, Ordering::AcqRel);
        Self {
            inner: self.inner.clone(),
            set: self.set.clone(),
            inline_frame_jobs: self.inline_frame_jobs,
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
/// The app pool's workers that take only Interactive work. A parked job frees its place but
/// keeps a thread for its stack. With every spare occupied, Interactive work (a view's tiles,
/// a pick) would still have a permit and no thread. The budget gives Interactive every core
/// and the other classes `limit` of them. The reserve covers the difference. At one core
/// there are none between, and one worker still waits for the permit, so that the lower class's
/// next checkpoint yields it. Parked until Interactive work queues with no other worker idle,
/// they cost nothing per frame.
fn interactive_reserve(budget: &BudgetHandle) -> usize {
    budget.cores().saturating_sub(budget.limit()).max(1) as usize
}
/// Workers that take Interactive and Visible work. They give Visible a thread for each
/// token Background leaves free, even when every general and spare worker is occupied.
/// When the limits coincide, one worker asks for Visible so Background yields at a checkpoint.
fn visible_reserve(budget: &BudgetHandle) -> usize {
    (budget.limit() - budget.background_limit()).max(1) as usize
}
fn app_pool() -> &'static Mutex<Weak<PoolInner>> {
    static POOL: OnceLock<Mutex<Weak<PoolInner>>> = OnceLock::new();
    POOL.get_or_init(|| Mutex::new(Weak::new()))
}
/// `MAKEPAD_TRACE=pool.timeline`: the app's pool logs a line of its state every 25 ms
/// ([`PoolInner::timeline`]) for as long as it is up, so a window where cores sit idle while work
/// waits shows what holds them: no work offered, work queued for a permit, or permits held by
/// threads that do not run.
#[cfg(not(target_arch = "wasm32"))]
fn trace_timeline(pool: Weak<PoolInner>) {
    let sampler = std::thread::Builder::new().name("makepad-timeline".into()).spawn(move || {
        while let Some(inner) = pool.upgrade() {
            let line = inner.timeline();
            drop(inner);
            crate::log!("{line}");
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
    });
    if let Err(error) = sampler {
        crate::log!("pool timeline: no sampler thread: {error}");
    }
}
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn cpu_summary() -> String {
    let pool = app_pool().lock().unwrap().upgrade();
    pool.map_or_else(
        || format!("{}pool: not started\n", BudgetHandle::process().report_string()),
        |p| p.summary(),
    )
}
impl TaskPool {
    /// The pool whose worker runs the caller, when one does: work deep inside a job (a library's
    /// bake, a loader's decode) spreads over the job's own pool ([`TaskPool::fan_out`]) without a
    /// handle carried down to it. `None` on any other thread (the UI's, a tool's, a test's, a
    /// thread outside the pool that runs a claimed job) and once the pool is closing: such a
    /// caller runs its work itself.
    pub fn of_current_worker() -> Option<TaskPool> {
        WORKER_POOL.with(|p| {
            let p = p.borrow();
            let (inner, _) = p.as_ref()?;
            inner.add_live_handle().then(|| TaskPool { inner: inner.clone(), set: Arc::default(), inline_frame_jobs: FRAME_JOBS_INLINE })
        })
    }

    fn shared(spawner: ThreadSpawner) -> Result<Self, SpawnError> {
        let mut global = app_pool().lock().unwrap();
        if let Some(inner) = global.upgrade().filter(|inner| inner.add_live_handle()) {
            return Ok(Self { inner, set: Arc::default(), inline_frame_jobs: FRAME_JOBS_INLINE });
        }
        let budget = BudgetHandle::process();
        // (the budget's limit at its long-lived pools' cores, [`makepad_cpu_budget::pool_cores`]:
        // a live test session's call charge, not its box, which its JIT keeps)
        #[cfg(not(target_arch = "wasm32"))]
        let workers = (budget.pool_cores() as usize).saturating_sub(1).clamp(1, budget.limit() as usize);
        #[cfg(target_arch = "wasm32")]
        let workers = spawner
            .available_parallelism()
            .get()
            .saturating_sub(1)
            .clamp(3, 6);
        // One spare per general worker on every platform (on wasm each is a web worker).
        let pool = PoolInner::create_with(spawner, workers, workers, visible_reserve(&budget), interactive_reserve(&budget), budget)?;
        *global = Arc::downgrade(&pool.inner);
        #[cfg(not(target_arch = "wasm32"))]
        if crate::makepad_error_log::trace_enabled("pool.timeline") {
            trace_timeline(Arc::downgrade(&pool.inner));
        }
        Ok(pool)
    }
    /// A pool of `worker_len` workers that take every class, and no reserve.
    fn create(spawner: ThreadSpawner, worker_len: usize, budget: BudgetHandle) -> Result<Self, SpawnError> {
        PoolInner::create_with(spawner, worker_len, 0, 0, 0, budget)
    }
}
impl PoolInner {
    /// General and spare workers take every class. A spare runs queued work while a job is
    /// parked, at the cost of a parked thread's stack and nothing per frame. The reserves
    /// serve Visible and higher work ([`visible_reserve`]) or Interactive alone ([`interactive_reserve`]).
    fn create_with(
        spawner: ThreadSpawner, general: usize, spares: usize, visible_reserve: usize,
        interactive_reserve: usize, budget: BudgetHandle,
    ) -> Result<TaskPool, SpawnError> {
        let all_class = general + spares;
        let visible_end = all_class + visible_reserve;
        let worker_len = visible_end + interactive_reserve;
        let inner = Arc::new(Self {
            budget,
            state: Mutex::new(PoolState {
                running: vec![None; worker_len],
                free: all_class,
                slots: vec![WorkerCount::Free; all_class],
                ..Default::default()
            }),
            queued: Default::default(),
            shares: Default::default(),
            waiting: AtomicU8::new(0),
            workers: (0..worker_len)
                .map(|index| WorkerSlot {
                    thread: OnceLock::new(),
                    idle: AtomicBool::new(false),
                    wake: OnceLock::new(),
                    admitting: AtomicU8::new(0),
                    takes: if index < all_class { 3 } else if index < visible_end { 2 } else { 1 },
                })
                .collect(),
            general,
            all_class,
            visible_end,
            started: AtomicUsize::new(0),
            exited: AtomicUsize::new(0),
            completed: AtomicU64::new(0),
            taken: AtomicU64::new(0),
            handles: AtomicUsize::new(1),
            shutdown_state: Arc::new(TaskState::default()),
            ui_thread: spawner.ui_thread,
        });
        let pool = TaskPool {
            inner: inner.clone(),
            set: Arc::default(),
            inline_frame_jobs: FRAME_JOBS_INLINE,
        };
        makepad_core_util::blocking::install(blocking_wait);
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
}
impl TaskPool {
    /// Drive frame jobs submitted through this handle and its future clones
    /// inline, sharing one turn per [`frame_jobs_new_frame`] on the polling thread.
    pub fn with_inline_frame_jobs(mut self) -> Self {
        self.inline_frame_jobs = true;
        self
    }

    /// A pool of its own with no workers that refuses every job, as a target
    /// without threads has (wasm without atomics): callers take their
    /// synchronous path. The shared pool other users hold is not touched.
    pub fn closed() -> Self {
        let pool = Self::create(ThreadSpawner::for_current_thread(1), 0, BudgetHandle::process()).unwrap();
        pool.inner.close(ShutdownMode::CancelPending);
        pool
    }
    pub(crate) fn budget(&self) -> &BudgetHandle {
        &self.inner.budget
    }
    /// Whether a job submitted through this handle is taken: the pool and the handle's jobs open.
    pub(crate) fn is_open(&self) -> bool {
        self.inner.state.lock().unwrap().closed == POOL_OPEN && self.set.is_open()
    }
    /// General-worker width used to split work. Spares and reserves serve queued jobs.
    pub fn worker_count(&self) -> usize {
        self.inner.general
    }
    /// The jobs of `class` or a higher one that wait for a worker, besides those submitted under
    /// the label `besides` and the helpers of work already running (a fan-out's, a scope's: they
    /// wait on a job that has its worker). A job that could go on with the next of a stream of its
    /// kind's jobs on its worker (each submitted under `besides`) leaves it for one of these, and
    /// not for its kind's: those would trade workers job by job, each waiting on every other.
    pub fn waiting(&self, class: Class, besides: &str) -> usize {
        let at = ..=class.budget() as usize;
        let state = self.inner.state.lock().unwrap();
        let jobs = state.queues[at].iter().flatten().filter(|job| job.label != besides).count();
        let helpers: usize = self.inner.shares[at].iter().map(|s| s.queued.load(Ordering::Relaxed)).sum();
        jobs.saturating_sub(helpers)
    }
    pub fn summary(&self) -> String {
        self.inner.summary()
    }
    /// What the pool holds back now (`/settled`, a test's idle rule).
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn backlog(&self) -> PoolBacklog {
        let queued = self.inner.queued.each_ref().map(|q| q.load(Ordering::Relaxed));
        let running = self.inner.state.lock().unwrap().running.iter().filter(|r| r.is_some()).count();
        PoolBacklog { queued, running, taken: self.inner.taken.load(Ordering::Relaxed) }
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
        let (job, handle) = task_job(self.inner.ui_thread, class, label, ticket, f, internal);
        self.enqueue_task(job)?;
        Ok(handle)
    }
    fn enqueue_task(&self, mut job: PoolJob) -> Result<(), SubmitError> {
        job.member = Some(self.set.join(&job.token).ok_or(SubmitError::Closed)?);
        self.inner.enqueue(job)
    }
    pub fn submit_internal<F, T>(&self, class: Class, f: F) -> Result<TaskHandle<T>, SubmitError>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        self.submit_internal_named(class, std::any::type_name::<F>(), f)
    }
    /// [`Self::submit_internal`] under `label`, as [`Self::submit_named`] names a job: the name
    /// the pool's summary, the CPU budget's holders and [`Self::waiting`] tell it by (a boxed
    /// closure's type name is every boxed job's).
    pub fn submit_internal_named<F, T>(
        &self,
        class: Class,
        label: &'static str,
        f: F,
    ) -> Result<TaskHandle<T>, SubmitError>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        self.submit_with(class, label, Ticket::none(), f, true)
    }
    pub fn submit_frame_job<F, T>(&self, class: Class, f: F) -> Result<FrameJob<T>, SubmitError>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        if self.inline_frame_jobs {
            return Ok(FrameJob::later_once(f));
        }
        self.submit_worker_frame_future(class, std::any::type_name::<F>(), move || async move { f() }, false)
    }

    /// Submit long, resumable work. Each poll does a bounded slice of the
    /// caller's choosing. The driver polls again after [`Poll::Pending`],
    /// without waiting for a wake or limiting the number of polls.
    ///
    /// A worker calls `start` once and polls the future on that thread until
    /// ready or cancelled, so the future may hold thread-local state. Inline,
    /// the first due frame calls `start` and polls once. Dropping the handle
    /// stops further polls after any poll already in flight.
    pub fn submit_stepped_frame_job<Fut, T>(
        &self,
        class: Class,
        start: impl FnOnce() -> Fut + Send + 'static,
    ) -> Result<FrameJob<T>, SubmitError>
    where
        Fut: Future<Output = T> + 'static,
        T: Send + 'static,
    {
        if self.inline_frame_jobs {
            return Ok(FrameJob::later(start));
        }
        self.submit_worker_frame_future(class, std::any::type_name::<Fut>(), start, true)
    }

    fn submit_worker_frame_future<Fut, T>(
        &self,
        class: Class,
        label: &'static str,
        start: impl FnOnce() -> Fut + Send + 'static,
        stepped: bool,
    ) -> Result<FrameJob<T>, SubmitError>
    where
        Fut: Future<Output = T> + 'static,
        T: Send + 'static,
    {
        let token = CancellationToken::new();
        let worker_token = token.clone();
        let run = move || {
            let mut future = std::pin::pin!(start());
            let mut cx = Context::from_waker(Waker::noop());
            while !worker_token.is_cancelled() {
                if let Poll::Ready(value) = future.as_mut().poll(&mut cx) {
                    // A one-shot that has started publishes its result even if cancelled in flight.
                    if stepped && worker_token.is_cancelled() { break; }
                    return Ok(value);
                }
            }
            Err(TaskError::Cancelled)
        };
        let (job, task) = task_job_with_token(
            self.inner.ui_thread,
            class,
            label,
            Ticket::none(),
            token,
            run,
            false,
        );
        self.enqueue_task(job)?;
        Ok(FrameJob { inner: FrameJobInner::Task { task, cancel_on_drop: stepped } })
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
    #[cfg(test)]
    fn shutdown_pool(&self, mode: ShutdownMode) -> TaskHandle<()> {
        self.inner.close(mode);
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
    /// Helpers inside an item, or about to claim one. The caller's `finish` waits for these
    /// only: a helper that is queued, waiting for a permit or at a checkpoint has no borrow.
    active: AtomicUsize,
    state: Mutex<BatchState>,
    wake: Condvar,
    pool: Weak<PoolInner>,
    class: Class,
    parents: Vec<(Ticket, CancellationToken)>,
    share: Arc<Share>,
    /// The helpers it takes at most, its caller aside.
    helpers: usize,
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
                || state.running >= pool.workers_of(self.class.budget() as usize).saturating_sub(1).min(self.helpers)
                || self.next.load(Ordering::Relaxed) >= self.len
            {
                return;
            }
            state.queued = true;
        }
        let shared = self.clone();
        let mut member = ShareJob::queued(self.share.clone());
        let (job, handle) = task_job(
            pool.ui_thread,
            self.class,
            "fan_out",
            Ticket::none(),
            move || {
                member.start();
                shared.helper()
            },
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
            // Admitted after the caller closed the fan-out, or after the last index was
            // claimed: there is nothing to help with.
            if state.closed || self.next.load(Ordering::Relaxed) >= self.len {
                return;
            }
            state.running += 1;
        }
        self.enqueue();
        JOB.with(|j| {
            let mut context = j.borrow_mut();
            let job = context.as_mut().unwrap();
            job.parents.extend(self.parents.iter().cloned());
            job.helper = true;
            job.share = Some(self.share.clone());
        });
        let result = catch_unwind(AssertUnwindSafe(|| self.work(true)));
        {
            let mut state = self.state.lock().unwrap();
            state.running -= 1;
            if let Err(panic) = result {
                if state.panic.is_none() {
                    state.panic = Some(panic);
                }
                state.closed = true;
                self.stopped.store(true, Ordering::SeqCst);
                // The item unwound past its own `leave_item`: the panic is stored first,
                // so the caller that waits for this helper finds it.
                self.active.fetch_sub(1, Ordering::SeqCst);
            }
        }
        self.wake.notify_all();
        self.enqueue();
    }
    // Every index runs exactly once. A fan-out has no partial result, so a stale or
    // cancelled job still runs its remaining indices; skipping work is the closure's own
    // decision (its caller's cancel check).
    // A helper leaves at an item boundary when a higher class waits for its permit, or for
    // its thread: a higher-class job queued while every worker is busy (none can admit it
    // until one leaves), or a job of its own class while this fan-out holds more than its
    // part of the workers (over_share). It queues a helper again as it leaves.
    fn work(&self, helper: bool) {
        // One upgrade per helper, not one per item (the workers share the pool's count).
        let pool = if helper { self.pool.upgrade() } else { None };
        loop {
            // No index left (or the fan-out is closed): leave before the checkpoint, which
            // can wait for a permit. Nothing waits for a helper that has no item.
            if self.stopped.load(Ordering::Acquire) || self.next.load(Ordering::Relaxed) >= self.len {
                break;
            }
            let _ = checkpoint();
            if helper
                && (JOB.with(|j| j.borrow().as_ref().is_some_and(JobContext::should_yield))
                    || pool.as_ref().is_some_and(|p| p.higher_queued(self.class) || p.over_share(&self.share)))
            {
                break;
            }
            // A helper is counted before it looks: the caller, which stores `stopped` and then
            // reads the count, either sees this helper or this helper sees `stopped`.
            if helper {
                self.active.fetch_add(1, Ordering::SeqCst);
            }
            if self.stopped.load(Ordering::SeqCst) {
                if helper {
                    self.leave_item();
                }
                break;
            }
            let index = self.next.fetch_add(1, Ordering::Relaxed);
            if index >= self.len {
                if helper {
                    self.leave_item();
                }
                break;
            }
            unsafe {
                (self.call)(self.f, index);
            }
            if helper {
                self.leave_item();
            }
        }
    }
    /// A helper is out of its item. The caller sleeps on `wake` only once it has stored
    /// `stopped`, so the lock is taken only then.
    fn leave_item(&self) {
        if self.active.fetch_sub(1, Ordering::SeqCst) == 1 && self.stopped.load(Ordering::SeqCst) {
            // (taken and let go: the caller looked at the count under it before its sleep)
            drop(self.state.lock().unwrap());
            self.wake.notify_all();
        }
    }
    // The fan-out ends when its items are done: the caller waits for the helpers that are
    // inside an item (they borrow the closure), never for one that is queued, waiting for a
    // permit or at a checkpoint. Those find the fan-out closed and leave without touching it.
    fn finish(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        let mut state = self.state.lock().unwrap();
        state.closed = true;
        if self.active.load(Ordering::SeqCst) != 0 {
            // Asleep for them, the caller keeps its permit and lends its token to the budget;
            // awake, it takes the token back at once. Given up, the permit took a new admission
            // behind its class's waiters (95-150 ms each time). Never another thread's permit:
            // a permit is a row of its own thread, which lends it whenever that thread sleeps
            // (a helper's handed to the caller was lent by the helper's sleeps while the caller
            // ran on it, and a checkpoint that yielded it then counted it twice: a box's visible
            // tokens leaked one by one until no pool thread was admitted).
            let permit = JOB.with(|j| {
                let context = j.borrow();
                let job = context.as_ref()?;
                let permit = job.permit?;
                job.budget.lend_permit(permit);
                Some(permit)
            });
            while self.active.load(Ordering::SeqCst) != 0 {
                state = self.wake.wait(state).unwrap();
            }
            drop(state);
            if let Some(permit) = permit {
                JOB.with(|j| {
                    let mut context = j.borrow_mut();
                    let job = context.as_mut().unwrap();
                    job.budget.take_back_permit(permit);
                    if let Some(raised) = &job.raised {
                        job.class = Class::of_budget(raised.class.load(Ordering::Relaxed));
                        job.budget.reclass(permit, job.class.budget());
                    }
                });
            }
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
    /// Spread indices over the pool while the caller helps. The caller's job takes the
    /// enclosing job's static label, so a long item in the CPU budget's report names the work
    /// it belongs to (the caller checkpoints before every item). Helpers stay `fan_out`: they
    /// give their permit back by leaving at an item boundary, never through the budget's
    /// checkpoint, so their reported span is how long they helped, not how long an item ran.
    /// A fan-out shares its class's workers: it holds them all only while no other job of the
    /// class waits.
    pub fn fan_out<F: Fn(usize) + Sync>(&self, len: usize, f: F) {
        self.fan_out_on(usize::MAX, len, f);
    }

    /// [`TaskPool::fan_out`] on `most` workers at most, its caller among them. An index is where a
    /// helper can leave: for work the caller can split, one for each piece, however few workers
    /// it is to run on.
    pub fn fan_out_on<F: Fn(usize) + Sync>(&self, most: usize, len: usize, f: F) {
        self.fan_out_as(Class::Background, most, len, f);
    }

    /// [`TaskPool::fan_out_on`] from a thread outside the pool's jobs at the class it names: a
    /// long-lived thread of the app's own (one that steps a simulation the user watches, say)
    /// spreads at the class of what it does for the user, where `fan_out_on` from it runs at
    /// Background. A job's spread runs at its job's class whatever it names.
    pub fn fan_out_as<F: Fn(usize) + Sync>(&self, class: Class, most: usize, len: usize, f: F) {
        if len == 0 {
            return;
        }
        if std::thread::current().id() == self.inner.ui_thread {
            // a closed pool (TaskPool::closed: tests, targets without threads) runs every spread inline by design
            debug_assert!(!self.is_open(), "TaskPool::fan_out on the UI thread would block it on an open pool's helpers (a closed pool runs inline)");
            for i in 0..len {
                f(i);
            }
            return;
        }
        let class = current_class().unwrap_or(class);
        let (label, ticket, token) = JOB.with(|j| {
            j.borrow()
                .as_ref()
                .map(|j| (Some(j.label), j.ticket.clone(), j.token.clone()))
                .unwrap_or_else(|| (None, Ticket::none(), CancellationToken::new()))
        });
        let _context = JobGuard::enter(class, label.unwrap_or("fan_out caller"), ticket, token, None, None, self.budget());
        let share = Share::new(&self.inner, class);
        let _member = self.inner.on_worker().then(|| ShareJob::running(share.clone()));
        let shared = Arc::new(FanOutShared {
            f: &f as *const F as *const (),
            call: fan_out_call::<F>,
            next: AtomicUsize::new(0),
            len,
            stopped: AtomicBool::new(false),
            active: AtomicUsize::new(0),
            state: Mutex::new(BatchState {
                running: 0,
                queued: false,
                closed: false,
                panic: None,
            }),
            wake: Condvar::new(),
            pool: Arc::downgrade(&self.inner),
            class,
            parents: JOB.with(|j| j.borrow().as_ref().unwrap().parents.clone()),
            share,
            helpers: most.saturating_sub(1),
        });
        let parent = FanOutParent(&shared);
        // One item is the caller's own: no helper is asked for.
        if len > 1 {
            shared.enqueue();
        }
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
            pool.run_job(job, None, Turn::Pool);
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
    share: Arc<Share>,
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
        let share = self.share.clone();
        let run: Box<dyn FnOnce(&AtomicU64) + Send + 'scope> = Box::new(move |completed: &AtomicU64| {
            JOB.with(|j| j.borrow_mut().as_mut().unwrap().share = Some(share));
            let result =
                catch_unwind(AssertUnwindSafe(f)).map_err(|p| TaskError::Panicked(panic_report(p)));
            run_panicked.store(result.is_err(), Ordering::Release);
            completed.fetch_add(1, Ordering::Relaxed);
            result_state.complete(result);
        });
        let cancel: Box<dyn FnOnce() + Send + 'scope> = Box::new(move || {
            cancel_state.complete(Err(TaskError::Cancelled));
        });
        // scope drains/joins every ScopedJob even on unwind; queued wrappers
        // can outlive it only after their borrowed job has been taken away.
        let (run, cancel) = unsafe {
            (
                std::mem::transmute::<
                    Box<dyn FnOnce(&AtomicU64) + Send + 'scope>,
                    Box<dyn FnOnce(&AtomicU64) + Send>,
                >(run),
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
                raised: None,
            })),
            finished,
            panicked,
            joined: AtomicBool::new(false),
        });
        self.jobs.lock().unwrap().push(job.clone());
        let queued = ScopedQueued(job.clone());
        let pool = self.pool.inner.clone();
        let mut member = ShareJob::queued(self.share.clone());
        // If the pool is closed, dropping this closure completes the scoped job.
        if let Ok(handle) = self.pool.submit_internal(self.class, move || {
            member.start();
            queued.run(&pool)
        }) {
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
        let class = current_class().unwrap_or(Class::Background);
        let share = Share::new(&self.inner, class);
        // its caller waits on its worker for what it spawned, so that worker is the scope's too
        let _member = self.inner.on_worker().then(|| ShareJob::running(share.clone()));
        let scope = TaskScope {
            pool: self,
            class,
            share,
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
    /// The wrapper while it waits in the pool for a worker: its token and class, the most urgent
    /// of the jobs queued. One more urgent queued behind it raises it (`submit`): the jobs before
    /// run first, so they take its class, never leaving it to wait for what a lower class may take.
    waiting: Option<(CancellationToken, Class)>,
    /// The job that left the queue for a worker, until it ends.
    running: Option<Arc<Raised>>,
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
                    waiting: None,
                    running: None,
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
        let (schedule, wake) = {
            let mut pool = self.inner.pool.inner.state.lock().unwrap();
            if pool.closed != POOL_OPEN {
                drop(pool);
                drop(job);
                return handle;
            }
            let mut state = self.inner.state.lock().unwrap();
            state.jobs.push_back((ticket, job));
            let schedule = !state.scheduled;
            state.scheduled = true;
            let raise = state.waiting.as_mut().filter(|(_, waiting)| class.budget() < waiting.budget());
            let wake = if let Some((token, waiting)) = raise {
                *waiting = class;
                self.inner.pool.inner.raise(&mut pool, token, class);
                self.inner.pool.inner.idle_candidates(&pool)
            } else {
                None
            };
            if let Some(running) = &state.running {
                running.raise(class);
            }
            (schedule, wake)
        };
        if schedule {
            self.inner.schedule();
        } else {
            self.inner.pool.inner.wake_one(wake);
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
        // The pool's lock first, as `submit` takes them: the wrapper is in the pool's queue by
        // the time a job submitted behind it may raise it.
        let mut pool = self.pool.inner.state.lock().unwrap();
        let mut state = self.state.lock().unwrap();
        // The front job runs first, at the class of the most urgent job queued.
        let Some(class) = state.jobs.iter().map(|(_, job)| job.class).min_by_key(|class| class.budget()) else {
            state.scheduled = false;
            return;
        };
        let mut queued = StrandQueued {
            strand: self.clone(),
            armed: true,
        };
        // The front job's turn: its wait for a worker is this wrapper's, from here.
        let at = Cx::monotonic_now();
        let (job, handle) = task_job(
            self.pool.inner.ui_thread,
            class,
            self.label,
            Ticket::none(),
            move || {
                let job = {
                    let mut state = queued.strand.state.lock().unwrap();
                    // the class it was raised to, by a job behind the one it runs
                    let class = state.waiting.take().map_or(class, |(_, class)| class);
                    state.jobs.pop_front().map(|(ticket, mut job)| {
                        if class.budget() < job.class.budget() {
                            job.class = class;
                        }
                        let raised = Arc::new(Raised {
                            class: AtomicU8::new(job.class.budget()),
                            wake: makepad_cpu_budget::Wake::current(),
                        });
                        state.running = Some(raised.clone());
                        job.raised = Some(raised);
                        (ticket, job)
                    })
                };
                if let Some((ticket, job)) = job {
                    if ticket.is_stale() {
                        drop(job);
                    } else {
                        queued.strand.pool.inner.run_job(job, None, Turn::Strand { at });
                    }
                    queued.strand.state.lock().unwrap().running = None;
                }
                queued.armed = false;
                queued.strand.schedule();
            },
            false,
        );
        handle.detach();
        // Strand heads were accepted before close. Drain includes their successors.
        if pool.closed == POOL_CANCELLED {
            // Dropped without the locks: an armed wrapper takes the strand's lock to drop its jobs.
            drop(state);
            drop(pool);
            drop(job);
            return;
        }
        state.waiting = Some((job.token.clone(), class));
        drop(state);
        self.pool.inner.push(&mut pool, job);
        let wake = self.pool.inner.idle_candidates(&pool);
        drop(pool);
        self.pool.inner.wake_one(wake);
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

    /// Each test pool has room for its concurrent jobs even on a small lease.
    const POOL_TEST_CORES: usize = 8;
    /// Set in the child whose process budget is sized for the two tests below.
    const POOL_CHILD: &str = "MAKEPAD_POOL_TESTS_CHILD";
    /// Cx uses the process pool, and Relax's thread-wide lend acts on the process budget.
    /// Only these tests need the child. All other test pools own their budgets.
    const IN_OWN_BUDGET: &[&str] = &[
        "a_cx_that_ends_leaves_the_process_pool_to_the_others",
        "a_fan_out_caller_takes_its_own_token_back_instead_of_a_new_admission",
    ];

    /// A failed child test may leave threads holding process permits. Stop there so another
    /// test cannot wait for them indefinitely.
    static POOL_TEST_FAILED: AtomicBool = AtomicBool::new(false);
    struct MarksAFailure;
    impl Drop for MarksAFailure {
        fn drop(&mut self) {
            if std::thread::panicking() {
                POOL_TEST_FAILED.store(true, Ordering::SeqCst);
            }
        }
    }

    /// Runs a process-budget test only in the child and checks the runner lists it.
    macro_rules! in_own_budget {
        ($name:ident) => {
            if std::env::var_os(POOL_CHILD).is_none() {
                assert!(IN_OWN_BUDGET.contains(&stringify!($name)), "{} runs in the pool tests' process: list it in IN_OWN_BUDGET", stringify!($name));
                return;
            }
            if POOL_TEST_FAILED.load(Ordering::SeqCst) {
                println!("{}: not run, a pool test before it failed", stringify!($name));
                return;
            }
            let _marks_a_failure = MarksAFailure;
        };
    }

    /// Runs both process-budget tests serially with [`POOL_TEST_CORES`] set before first use.
    /// Reports the child's first failure or any listed test it missed.
    #[test]
    fn the_pool_tests_run_in_a_budget_of_their_own() {
        if std::env::var_os(POOL_CHILD).is_some() {
            return;
        }
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "--test-threads=1"])
            .args(IN_OWN_BUDGET.iter().map(|name| format!("thread::tests::{name}")))
            .env(POOL_CHILD, "1")
            .env("MAKEPAD_CPU_CORES", POOL_TEST_CORES.to_string())
            .output()
            .unwrap();
        let all = format!("{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        let with = |verdict: &str| -> Vec<&str> {
            IN_OWN_BUDGET.iter().copied().filter(|name| all.contains(&format!("test thread::tests::{name} ... {verdict}"))).collect()
        };
        if let Some(failed) = with("FAILED").first() {
            // its panic: libtest's line for its thread and the message under it, else the JIT's report
            let lines: Vec<&str> = all.lines().collect();
            let mark = format!("thread 'thread::tests::{failed}' ");
            let panic = lines
                .windows(2)
                .find(|w| w[0].starts_with(&mark) && w[0].contains(" panicked at "))
                .map(|w| format!("{} ({})", w[1].trim(), w[0].trim()))
                .or_else(|| lines.iter().find(|l| l.contains("\"kind\":\"panic\"")).map(|l| l.trim().to_string()))
                .unwrap_or_else(|| "no panic line".to_string());
            panic!("{failed} failed in the pool tests' process: {panic}\n{all}");
        }
        let passed = with("ok");
        assert!(
            passed.len() == IN_OWN_BUDGET.len() && output.status.success(),
            "the pool tests' process ({}) passed {} of the {} listed; not run: {:?}\n{all}",
            output.status,
            passed.len(),
            IN_OWN_BUDGET.len(),
            IN_OWN_BUDGET.iter().filter(|name| !passed.contains(name)).collect::<Vec<_>>()
        );
    }

    fn worker_join<T: Send + 'static>(handle: TaskHandle<T>) -> Result<T, TaskError> {
        std::thread::spawn(move || handle.join()).join().unwrap()
    }

    /// Polls state the test cannot wait on otherwise, such as the pool's own. Stays awake:
    /// under the JIT a thread asleep (on a lock, a channel, the clock, a build) lends its
    /// process-budget tokens.
    fn wait_for(what: &str, mut ready: impl FnMut() -> bool) {
        let began = std::time::Instant::now();
        while !ready() {
            assert!(began.elapsed() < NOTHING_MOVES, "timed out waiting for {what}");
            std::thread::yield_now();
        }
    }

    /// State a test and its jobs wait on together, each for the other's next step: none of them
    /// watches the clock. A broken rule shows in the state itself, with every worker held and a job
    /// still queued, and the test asserts on that. The bound only ends a wait that nothing can end.
    struct Together<T> {
        state: Mutex<T>,
        wake: Condvar,
    }
    /// No step of these tests comes near it, on any load: a step is a handoff between threads.
    const NOTHING_MOVES: Duration = Duration::from_secs(300);
    impl<T> Together<T> {
        fn new(state: T) -> Arc<Self> {
            Arc::new(Together { state: Mutex::new(state), wake: Condvar::new() })
        }
        fn change<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
            let r = f(&mut self.state.lock().unwrap_or_else(|p| p.into_inner()));
            self.wake.notify_all();
            r
        }
        fn until(&self, what: &str, mut done: impl FnMut(&T) -> bool) -> std::sync::MutexGuard<'_, T> {
            let state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            let (state, waited) = self.wake.wait_timeout_while(state, NOTHING_MOVES, |s| !done(s)).unwrap_or_else(|p| p.into_inner());
            assert!(!waited.timed_out(), "nothing moved in {NOTHING_MOVES:?} waiting for {what}");
            state
        }
    }
    /// Ends the jobs a test holds when it ends, by its failing too.
    struct AtEnd<F: FnMut()>(F);
    impl<F: FnMut()> Drop for AtEnd<F> {
        fn drop(&mut self) {
            (self.0)()
        }
    }

    /// CPU tokens a test keeps out of the budget, on a thread of their own that stays awake (see
    /// `wait_for`): a test's own thread sleeps all the time. Dropped, it gives back what it still
    /// holds, so a test that failed leaves the budget as it found it.
    struct HeldTokens {
        shared: Arc<(AtomicUsize, AtomicBool)>,
        holder: Option<std::thread::JoinHandle<()>>,
    }

    impl HeldTokens {
        fn new(budget: &BudgetHandle, count: usize, label: &'static str) -> Self {
            fn hold(budget: &BudgetHandle, permits: &mut Vec<u32>, keep: &AtomicUsize) -> bool {
                while permits.len() > keep.load(Ordering::Relaxed) {
                    budget.release(permits.pop().unwrap());
                }
                std::thread::yield_now();
                !permits.is_empty()
            }
            let shared = Arc::new((AtomicUsize::new(count), AtomicBool::new(false)));
            let theirs = shared.clone();
            let budget = budget.clone();
            let holder = std::thread::spawn(move || {
                let (keep, held) = &*theirs;
                let mut permits = Vec::new();
                // Warm this loop before taking tokens: waiting for a build can lend this thread's tokens.
                hold(&budget, &mut permits, keep);
                permits.extend((0..count).map(|_| {
                    budget.admit(makepad_cpu_budget::INTERACTIVE, makepad_cpu_budget::APP, label)
                }));
                held.store(true, Ordering::Release);
                while hold(&budget, &mut permits, keep) {}
            });
            wait_for("the test's tokens to be held", || shared.1.load(Ordering::Acquire));
            Self { shared, holder: Some(holder) }
        }
        fn give_back_one(&self) {
            self.shared.0.fetch_sub(1, Ordering::Relaxed);
        }
    }

    impl Drop for HeldTokens {
        fn drop(&mut self) {
            self.shared.0.store(0, Ordering::Relaxed);
            let _ = self.holder.take().unwrap().join();
        }
    }

    fn test_pool(workers: usize) -> TaskPool {
        test_pool_on(workers, BudgetHandle::of_cores(POOL_TEST_CORES as u32))
    }

    fn test_pool_on(workers: usize, budget: BudgetHandle) -> TaskPool {
        TaskPool::create(ThreadSpawner::for_current_thread(workers + 1), workers, budget).unwrap()
    }

    impl TaskPool {
        /// The existing reserve fixture has no spares.
        fn create_with(spawner: ThreadSpawner, general: usize, reserve: usize, budget: BudgetHandle) -> Result<Self, SpawnError> {
            PoolInner::create_with(spawner, general, 0, 0, reserve, budget)
        }
    }

    /// The calling thread's QoS class and relative priority as one rank, higher runs first.
    #[cfg(target_vendor = "apple")]
    fn thread_rank() -> i32 {
        unsafe extern "C" {
            fn pthread_self() -> *mut std::ffi::c_void;
            fn pthread_get_qos_class_np(thread: *mut std::ffi::c_void, class: *mut u32, relative: *mut i32) -> i32;
        }
        let (mut class, mut relative) = (0, 0);
        assert_eq!(unsafe { pthread_get_qos_class_np(pthread_self(), &mut class, &mut relative) }, 0, "the thread's QoS reads back");
        class as i32 * 100 + relative
    }

    /// The pool is made by the UI thread once it runs at UserInteractive (`warm_task_pool`), and a
    /// new thread takes its creator's QoS. Its workers still run below it, at the class of the job
    /// they hold, so a frame wins the cores it shares with jobs.
    #[cfg(target_vendor = "apple")]
    #[test]
    fn pool_workers_run_below_the_ui_thread() {
        let _serial = POOL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
        assert_eq!(Cx::set_thread_priority(CxThreadPriority::UserInteractive), PriorityStatus::Applied, "the UI thread's priority applies");
        let ui = thread_rank();
        let pool = PoolInner::create_with(ThreadSpawner::for_current_thread(3), 1, 0, 0, 1, BudgetHandle::of_cores(POOL_TEST_CORES as u32)).unwrap();
        // The general worker parks with a Background job, so the Interactive job goes to the reserve.
        let (open, parked) = parked_background(&pool, thread_rank);
        let reserve = worker_join(pool.submit(Class::Interactive, thread_rank).unwrap()).unwrap();
        drop(open);
        let general = worker_join(parked).unwrap();
        assert!(general < ui && reserve < ui, "every worker runs below the UI thread: ranks {general} (general, Background job) and {reserve} (Interactive reserve) against the UI's {ui}");
    }

    fn test_pool_with_spares(general: usize, spares: usize) -> TaskPool {
        let budget = BudgetHandle::of_cores(8);
        PoolInner::create_with(ThreadSpawner::for_current_thread(general + spares + 1), general, spares, 0, 0, budget).unwrap()
    }

    /// Returns once the job is inside without_permit. Dropping the sender opens its gate,
    /// including if an assertion on the test thread unwinds.
    fn parked_background<T: Send + 'static>(
        pool: &TaskPool,
        resumed: impl FnOnce() -> T + Send + 'static,
    ) -> (mpsc::Sender<()>, TaskHandle<T>) {
        let (inside, entered) = mpsc::channel();
        let (open, gate) = mpsc::channel();
        let job = pool.submit(Class::Background, move || {
            without_permit(|| {
                inside.send(()).unwrap();
                let _ = gate.recv();
            });
            resumed()
        }).unwrap();
        entered.recv_timeout(NOTHING_MOVES).expect("the Background job must reach its gate inside without_permit");
        (open, job)
    }

    /// A job that yields between its steps lets a job of its class queued behind it run first,
    /// though their class has one permit (a four-core budget): the budget's FIFO alone keeps that
    /// permit with the older job until it ends.
    #[test]
    fn a_job_yielding_between_steps_lets_a_queued_class_mate_run_first() {
        let _serial = POOL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
        let pool = PoolInner::create_with(ThreadSpawner::for_current_thread(3), 2, 0, 0, 0, BudgetHandle::of_cores(4)).unwrap();
        assert_eq!(pool.budget().background_limit(), 1, "the budget has one Background permit");
        let order = Arc::new(Mutex::new(Vec::new()));
        let (started, running) = mpsc::channel();
        let (go_on, queued) = mpsc::channel::<()>();
        let long = pool.submit(Class::Background, {
            let order = order.clone();
            move || {
                order.lock().unwrap().push("long job, first step");
                started.send(()).unwrap();
                queued.recv_timeout(NOTHING_MOVES).expect("the class mate queues");
                yield_to_waiting();
                order.lock().unwrap().push("long job, second step");
            }
        }).unwrap();
        running.recv_timeout(NOTHING_MOVES).expect("the long job starts");
        let mate = pool.submit(Class::Background, {
            let order = order.clone();
            move || order.lock().unwrap().push("class mate")
        }).unwrap();
        wait_for("the class mate to wait for the Background permit", || {
            pool.budget().tokens().waiting.iter().any(|w| w.class == makepad_cpu_budget::BACKGROUND)
        });
        go_on.send(()).unwrap();
        worker_join(long).unwrap();
        worker_join(mate).unwrap();
        assert_eq!(*order.lock().unwrap(), ["long job, first step", "class mate", "long job, second step"],
            "the queued class mate runs between the long job's steps");
        worker_join(pool.shutdown_pool(ShutdownMode::Drain)).unwrap();
    }

    #[test]
    fn a_visible_job_runs_while_every_general_workers_job_waits_without_its_permit() {
        let _serial = POOL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
        let pool = test_pool_with_spares(2, 2);
        let (open_a, a) = parked_background(&pool, || ());
        let (open_b, b) = parked_background(&pool, || ());
        assert_eq!(pool.inner.state.lock().unwrap().parked, 2, "both Background jobs must be parked before Visible queues");
        let visible = pool.submit(Class::Visible, || ()).unwrap();
        wait_for("Visible to finish with both Background gates closed", || visible.is_finished());
        worker_join(visible).expect("Visible must complete on a spare");
        assert_eq!(pool.inner.state.lock().unwrap().parked, 2, "Visible must complete while both jobs still wait");
        drop((open_a, open_b));
        worker_join(a).unwrap();
        worker_join(b).unwrap();
        worker_join(pool.shutdown_pool(ShutdownMode::Drain)).unwrap();
    }

    #[test]
    fn a_job_waiting_for_a_token_at_a_checkpoint_frees_its_worker() {
        let _serial = POOL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
        let pool = test_pool_with_spares(2, 2);
        let budget = pool.budget();
        let gate = Together::new((0, false));
        let open = || gate.change(|(_, go)| *go = true);
        let _end = AtEnd(open);
        let jobs = [(); 2].map(|_| {
            let gate = gate.clone();
            pool.submit(Class::Background, move || {
                gate.change(|(started, _)| *started += 1);
                drop(gate.until("permission to checkpoint", |(_, go)| *go));
                checkpoint().expect("the Background job must resume with a token");
            }).unwrap()
        });
        drop(gate.until("both Background jobs to hold their tokens", |(started, _)| *started == 2));
        let held = HeldTokens::new(budget, 6, "six Interactive tokens");
        let mut requests = [budget.admission(), budget.admission()];
        let poll = |request: &mut makepad_cpu_budget::Admission| {
            request.poll(makepad_cpu_budget::INTERACTIVE, makepad_cpu_budget::APP, "checkpoint pressure")
        };
        for request in &mut requests {
            assert!(poll(request).is_none(), "each Interactive request must wait while all eight tokens are held");
        }
        // Keep work queued at both checkpoints. This job also needs a Background token,
        // so it stays queued when a spare starts its admission.
        let queued_background = pool.submit(Class::Background, || ()).unwrap();
        open();
        wait_for("both Background jobs to park in budget admission at their checkpoints", || {
            let tokens = budget.tokens();
            pool.inner.state.lock().unwrap().parked == 2
                && tokens.running == 6
                && tokens.waiting.iter().filter(|w| w.class == makepad_cpu_budget::BACKGROUND).count() >= 2
        });
        let permits = requests.map(|mut request| {
            let permit = poll(&mut request).expect("a yielded token must admit each Interactive waiter in order");
            AtEnd(move || budget.release(permit))
        });
        let visible = pool.submit(Class::Visible, || ()).unwrap();
        drop(permits);
        // Six tokens stay held. Visible can start below seven, Background only below five.
        wait_for("Visible to finish while the Background budget is full", || visible.is_finished());
        worker_join(visible).expect("Visible must complete on a spare");
        assert_eq!(pool.inner.state.lock().unwrap().parked, 2, "both checkpoint jobs must stay parked through Visible completion");
        drop(held);
        for job in jobs {
            worker_join(job).unwrap();
        }
        worker_join(queued_background).unwrap();
        worker_join(pool.shutdown_pool(ShutdownMode::Drain)).unwrap();
    }

    #[test]
    fn a_job_that_waits_for_a_token_with_nothing_queued_frees_its_worker() {
        let _serial = POOL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
        let pool = test_pool_with_spares(2, 2);
        let budget = pool.budget();
        let gate = Together::new((0, false));
        let open = || gate.change(|(_, go)| *go = true);
        let _end = AtEnd(open);
        let jobs = [(); 2].map(|_| {
            let gate = gate.clone();
            pool.submit_named(Class::Background, "model", move || {
                gate.change(|(started, _)| *started += 1);
                drop(gate.until("permission to checkpoint", |(_, go)| *go));
                checkpoint().expect("the Background job must regain its token");
            }).unwrap()
        });
        drop(gate.until("both Background jobs to hold their tokens", |(started, _)| *started == 2));
        let held = HeldTokens::new(budget, 6, "six Interactive tokens");
        let mut outside = budget.admission();
        let mut poll = || outside.poll(makepad_cpu_budget::INTERACTIVE, makepad_cpu_budget::APP, "outside the pool");
        assert!(poll().is_none(), "the external request must wait for a checkpoint to release a token");
        open();
        wait_for("both workers to park with an empty pool queue", || {
            let waiting = budget.tokens().waiting.iter().filter(|w| w.class == makepad_cpu_budget::BACKGROUND).count();
            let state = pool.inner.state.lock().unwrap();
            waiting == 2 && state.parked == 2 && state.queues.iter().all(VecDeque::is_empty)
        });
        let visible = pool.submit_named(Class::Visible, "draw", || ()).unwrap();
        budget.release(poll().expect("the external request must receive a yielded token"));
        // Six tokens stay held: Visible has room and neither Background job can resume.
        wait_for("late Visible work to complete on a spare", || visible.is_finished());
        worker_join(visible).expect("Visible must finish while the checkpoint jobs wait");
        assert_eq!(pool.inner.state.lock().unwrap().parked, 2, "neither Background job may resume before Visible completes");
        assert!(pool.summary().lines().any(|line| line == "starts: model=2 draw=1"),
            "the summary must count each popped label and list the most starts first");
        drop(held);
        for job in jobs {
            worker_join(job).expect("each parked Background job must resume");
        }
        worker_join(pool.shutdown_pool(ShutdownMode::Drain)).unwrap();
    }

    #[test]
    fn a_parked_job_resumes_at_once_and_holds_back_new_starts() {
        let _serial = POOL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
        let pool = test_pool_with_spares(1, 2);
        let (resumed, at_resume) = mpsc::channel();
        let (finish_a, end_a) = mpsc::channel::<()>();
        let a_ending = Arc::new(AtomicBool::new(false));
        let (g1, a) = parked_background(&pool, {
            let inner = pool.inner.clone();
            let ending = a_ending.clone();
            move || {
                let snapshot = {
                    let state = inner.state.lock().unwrap();
                    (state.active, state.queues[2].len())
                };
                resumed.send(snapshot).unwrap();
                let _ = end_a.recv();
                ending.store(true, Ordering::Release);
            }
        });
        let (started, b_started) = mpsc::channel();
        let (g2, end_b) = mpsc::channel();
        let b = pool.submit(Class::Background, gate_job(started, end_b)).unwrap();
        b_started.recv_timeout(NOTHING_MOVES).expect("B must take A's place");
        let c = pool.submit(Class::Background, move || a_ending.load(Ordering::Acquire)).unwrap();
        drop(g1);
        assert_eq!(at_resume.recv_timeout(NOTHING_MOVES), Ok((2, 1)), "A must resume alongside B with C still queued");
        drop(g2);
        worker_join(b).unwrap();
        wait_for("both spare threads to go idle after B ends", || {
            pool.inner.workers.iter().filter(|w| w.idle.load(Ordering::SeqCst)).count() == 2
        });
        {
            let state = pool.inner.state.lock().unwrap();
            assert_eq!((state.active, state.queues[2].len()), (1, 1), "A alone must hold back C at the general limit");
        }
        drop(finish_a);
        worker_join(a).unwrap();
        wait_for("C to finish when A gives back the last place", || c.is_finished());
        assert!(worker_join(c).unwrap(), "C must start after A is allowed to end");
        worker_join(pool.shutdown_pool(ShutdownMode::Drain)).unwrap();
    }

    #[test]
    fn a_stretch_of_no_spare_waits_is_reported_once_as_it_begins_and_once_after_it() {
        let (mut spare, mut reports) = (NoSpare::default(), Vec::new());
        let at = |t: f64| move || t;
        spare.begin(at(10.0), [0, 2, 5], 2, &mut reports);
        assert_eq!(reports, vec![SpareReport::Began { queued: [0, 2, 5], parked: 2 }], "the stretch's first wait, with its queues");
        spare.begin(at(10.1), [0, 3, 5], 2, &mut reports);
        assert_eq!(reports.len(), 1, "a wait already running is not a new one");
        spare.end_episode(at(10.25));
        // hundreds of short waits in a busy second: one stretch, nothing per wait
        for k in 0..300 {
            let t = 10.3 + k as f64 * 0.002;
            spare.begin(at(t), [0, 1, 1], 2, &mut reports);
            spare.end_episode(at(t + 0.001));
            spare.end_stretch_if_quiet(at(t + 0.001), false, &mut reports);
        }
        assert_eq!(reports.len(), 1, "waits less than {NO_SPARE_QUIET_S} s apart add no lines");
        let last = 10.3 + 299.0 * 0.002 + 0.001;
        spare.end_stretch_if_quiet(at(last + NO_SPARE_QUIET_S * 0.5), false, &mut reports);
        assert_eq!(reports.len(), 1, "not yet quiet for long enough");
        spare.end_stretch_if_quiet(at(last + NO_SPARE_QUIET_S), false, &mut reports);
        let Some(SpareReport::Ended { lasted, episodes, longest }) = reports.pop() else { panic!("the stretch's end: {reports:?}") };
        assert_eq!(episodes, 301, "every wait counted");
        assert!((longest - 0.25).abs() < 1e-9, "the longest wait: {longest}");
        assert!((lasted - (last - 10.0)).abs() < 1e-9, "from the first wait to the last one's end: {lasted}");
        assert_eq!(reports.len(), 1, "the stretch made two lines in all: its start and its end");
        reports.clear();
        spare.begin(at(20.0), [0, 1, 0], 1, &mut reports);
        assert!(matches!(reports.pop(), Some(SpareReport::Began { .. })), "a later stretch is reported again");
        spare.end_stretch_if_quiet(at(20.5), true, &mut reports);
        assert!(reports.is_empty(), "a running wait is no stretch's end");
        spare.end_episode(at(20.5));
        spare.end_stretch_if_quiet(at(20.5), true, &mut reports);
        assert_eq!(reports, vec![SpareReport::Ended { lasted: 0.5, episodes: 1, longest: 0.5 }], "a pool that closes reports its open stretch");
    }

    #[test]
    fn with_no_spare_free_queued_work_waits_for_a_job_to_end() {
        let _serial = POOL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
        let pool = test_pool_with_spares(1, 1);
        let (open_a, a) = parked_background(&pool, || std::thread::current().id());
        let (open_b, b) = parked_background(&pool, || ());
        let c = pool.submit(Class::Visible, || std::thread::current().id()).unwrap();
        {
            let state = pool.inner.state.lock().unwrap();
            assert_eq!(state.parked, 2, "both physical workers must still be parked");
            assert_eq!((state.active, state.queues[1].len()), (0, 1), "C must stay queued with room in the count and no free thread");
        }
        drop(open_a);
        let first_worker = worker_join(a).unwrap();
        wait_for("C to finish while B is still parked", || c.is_finished());
        assert_eq!(worker_join(c).unwrap(), first_worker, "C must run on A's released worker");
        drop(open_b);
        worker_join(b).unwrap();
        worker_join(pool.shutdown_pool(ShutdownMode::Drain)).unwrap();
    }

    #[test]
    fn a_visible_job_runs_on_the_visible_reserve_when_every_thread_is_parked() {
        let _serial = POOL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
        let pool = PoolInner::create_with(ThreadSpawner::for_current_thread(8), 1, 1, 1, 0, BudgetHandle::of_cores(8)).unwrap();
        wait_for("all three workers to go idle before the first submit", || {
            pool.inner.workers.iter().all(|w| w.idle.load(Ordering::SeqCst))
        });
        let (open_a, a) = parked_background(&pool, || ());
        let (open_b, b) = parked_background(&pool, || ());
        let reserve = &pool.inner.workers[pool.inner.all_class];
        assert_eq!(pool.inner.state.lock().unwrap().parked, 2, "both all-class workers must have parked before Visible queues");
        let visible = pool.submit(Class::Visible, || std::thread::current().id()).unwrap();
        wait_for("Visible to finish with both gates closed", || visible.is_finished());
        assert_eq!(worker_join(visible).unwrap(), reserve.thread.get().unwrap().id(), "Visible must run on its reserve");
        assert_eq!(pool.inner.state.lock().unwrap().parked, 2, "both Background jobs must still be parked after Visible completes");
        drop((open_a, open_b));
        worker_join(a).unwrap();
        worker_join(b).unwrap();
        worker_join(pool.shutdown_pool(ShutdownMode::Drain)).unwrap();
    }

    #[test]
    fn at_one_worker_a_visible_job_preempts_a_background_job_at_its_checkpoint() {
        assert_visible_preempts_background(checkpoint);
    }

    #[test]
    fn a_job_polling_checkpoint_when_due_gives_its_token_to_a_waiting_higher_class() {
        assert_visible_preempts_background(checkpoint_when_due);
    }

    fn assert_visible_preempts_background(poll: fn() -> Result<(), Stale>) {
        let _serial = POOL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
        let budget = BudgetHandle::of_cores(2);
        let pool = PoolInner::create_with(ThreadSpawner::for_current_thread(2), 1, 1,
            visible_reserve(&budget), interactive_reserve(&budget), budget).unwrap();
        let gate = Arc::new(AtomicBool::new(false));
        let _end = AtEnd(|| gate.store(true, Ordering::Release));
        let (started, running) = mpsc::channel();
        let go = gate.clone();
        let budget = pool.budget().clone();
        let background = pool.submit(Class::Background, move || {
            started.send(()).unwrap();
            loop {
                poll().expect("Background must keep or regain its token");
                if go.load(Ordering::Acquire) {
                    break;
                }
                std::thread::yield_now();
            }
            assert!(budget.tokens().holders.iter().any(|h| h.class == makepad_cpu_budget::BACKGROUND && !h.lent),
                "Background must hold its token again after Visible finishes");
        }).unwrap();
        running.recv_timeout(NOTHING_MOVES).expect("Background must hold the only lower-class token");
        wait_for("the other workers to be idle before Visible queues", || {
            pool.inner.workers.iter().filter(|w| w.idle.load(Ordering::SeqCst)).count() == 3
        });
        let inner = pool.inner.clone();
        let visible = pool.submit(Class::Visible, move || {
            // The budget releases Background's token before its worker marks itself parked.
            // Hold the Visible token until both halves of the yield are observable.
            wait_for("Background to park at its checkpoint", || inner.state.lock().unwrap().parked == 1);
            let waiting = inner.budget.tokens().waiting.iter().any(|w| w.class == makepad_cpu_budget::BACKGROUND);
            let active = inner.state.lock().unwrap().active;
            (std::thread::current().id(), waiting, active)
        }).unwrap();
        wait_for("Visible to complete while the Background gate is closed", || visible.is_finished());
        let (worker, waiting, active) = worker_join(visible).expect("Visible must finish before Background is released");
        assert_eq!(worker, pool.inner.workers[pool.inner.all_class].thread.get().unwrap().id(), "Visible must start on its reserve");
        assert!(waiting, "the checkpoint must retain Background's ticket while Visible holds the token");
        assert_eq!(active, 0, "neither the parked job nor the Visible reserve may use a general place");
        assert!(!background.is_finished(), "Background must stay in its loop through Visible completion");
        gate.store(true, Ordering::Release);
        worker_join(background).expect("Background must return from its checkpoint and finish after its gate opens");
        worker_join(pool.shutdown_pool(ShutdownMode::Drain)).unwrap();
    }

    /// A Background job computes a `ComputeOnce` value and paces at its checkpoints; a Visible job
    /// that preempted it asks for the same value. The waiting job frees the token, so the
    /// computation goes on and both finish (with `OnceLock`, the waiter blocks holding the only
    /// token while the computation stays parked for it). No pool job's wait counts as the UI
    /// thread's; a UI thread's contended wait does, once.
    #[test]
    fn a_job_waiting_on_a_compute_once_value_frees_its_token_for_the_job_computing_it() {
        use makepad_core_util::once::ComputeOnce;
        #[derive(Debug, PartialEq)]
        struct Basis(u32);
        let _serial = POOL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
        let budget = BudgetHandle::of_cores(2);
        let pool = PoolInner::create_with(ThreadSpawner::for_current_thread(2), 1, 1,
            visible_reserve(&budget), interactive_reserve(&budget), budget).unwrap();
        let ui_waits = UI_THREAD_WAITS.load(Ordering::SeqCst);
        let cell = Arc::new(ComputeOnce::<Basis>::new());
        let asked = Arc::new(AtomicBool::new(false));
        let _end = AtEnd(|| asked.store(true, Ordering::Release));
        let (started, building) = mpsc::channel();
        let background = pool.submit(Class::Background, {
            let (cell, asked) = (cell.clone(), asked.clone());
            move || cell.get_or_init(|| {
                started.send(()).unwrap();
                // Paces until the Visible job asked; it can only ask once this job gave way.
                while !asked.load(Ordering::Acquire) {
                    checkpoint_when_due().expect("Background must keep or regain its token");
                    std::thread::yield_now();
                }
                checkpoint_when_due().expect("Background must regain its token from the waiting job");
                Basis(5)
            }).0
        }).unwrap();
        building.recv_timeout(NOTHING_MOVES).expect("Background must start computing the value");
        wait_for("the other workers to be idle before Visible queues", || {
            pool.inner.workers.iter().filter(|w| w.idle.load(Ordering::SeqCst)).count() == 3
        });
        let visible = pool.submit(Class::Visible, {
            let (cell, asked) = (cell.clone(), asked.clone());
            move || {
                asked.store(true, Ordering::Release);
                cell.get_or_init(|| Basis(0)).0
            }
        }).unwrap();
        assert_eq!(worker_join(visible).expect("the waiting job must finish"), 5, "the waiting job reads the computed value");
        assert_eq!(worker_join(background).expect("the computing job must finish"), 5, "the computing job returns its value");
        worker_join(pool.shutdown_pool(ShutdownMode::Drain)).unwrap();
        assert_eq!(UI_THREAD_WAITS.load(Ordering::SeqCst), ui_waits, "a pool job's wait is not the UI thread's");

        // A UI thread asks while a thread computes: it waits (no crash), counted.
        #[derive(Debug, PartialEq)]
        struct OnUi(u32);
        let cell = Arc::new(ComputeOnce::<OnUi>::new());
        let (started, building) = mpsc::channel();
        let computing = std::thread::spawn({
            let cell = cell.clone();
            move || cell.get_or_init(|| {
                started.send(()).unwrap();
                wait_for("the UI thread to wait for the value", || UI_THREAD_WAITS.load(Ordering::SeqCst) > ui_waits);
                OnUi(3)
            }).0
        });
        building.recv_timeout(NOTHING_MOVES).expect("the thread must start computing the value");
        let read = std::thread::spawn(move || {
            makepad_core_util::ui_thread::mark_ui_thread();
            cell.get_or_init(|| OnUi(0)).0
        }).join().unwrap();
        assert_eq!((read, computing.join().unwrap()), (3, 3), "the UI thread waits for the computed value");
        assert_eq!(UI_THREAD_WAITS.load(Ordering::SeqCst), ui_waits + 1, "the UI thread's wait is counted");
    }

    fn process_test_budget() -> BudgetHandle {
        assert!(std::env::var_os(POOL_CHILD).is_some(), "a process-budget test must run in the pool tests' child");
        BudgetHandle::process()
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

    #[cfg(not(target_arch = "wasm32"))]
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
                waiter_token.wait_until(Cx::monotonic_now() + NOTHING_MOVES.as_secs_f64())
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
            TaskPool::create(spawner, 1, BudgetHandle::of_cores(POOL_TEST_CORES as u32)),
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
                    token.wait_until(Cx::monotonic_now() + NOTHING_MOVES.as_secs_f64())
                })
            })
            .collect();
        // Some registrations may still be in flight.
        token.cancel();
        for waiter in waiters {
            assert_eq!(waiter.join().unwrap(), WaitOutcome::Cancelled);
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
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
    fn deferred_frame_jobs_share_one_step_per_frame() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.use_closed_task_pool();
        cx.use_inline_frame_jobs();
        let pool = cx.task_pool();
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = calls.clone();
        let frame_thread = std::thread::current().id();
        let mut stepped = pool.submit_stepped_frame_job(Class::Visible, move || {
            let started_on = std::thread::current().id();
            assert_eq!(started_on, frame_thread, "the frame thread creates the step");
            std::future::poll_fn(move |_| {
                assert_eq!(std::thread::current().id(), started_on, "every inline step runs where it started");
                let n = counted.fetch_add(1, Ordering::Relaxed) + 1;
                if n == 3 { Poll::Ready(n) } else { Poll::Pending }
            })
        }).unwrap();
        let value = String::from("one shot");
        let mut once = pool.submit_frame_job(Class::Visible, move || value).unwrap();

        frame_jobs_new_frame();
        assert_eq!(stepped.try_take(), None, "the first step yields");
        assert!(!stepped.is_finished(), "polling again in this frame must leave the job pending");
        assert_eq!(calls.load(Ordering::Relaxed), 1, "two polls in one frame run exactly one step");
        assert_eq!(once.try_take(), None, "the stepped job took the one-shot job's turn");

        frame_jobs_new_frame();
        assert_eq!(once.try_take(), Some(Ok(String::from("one shot"))), "the one-shot job gets the next turn");
        assert!(!stepped.is_finished(), "the one-shot job took this frame's turn");
        assert_eq!(calls.load(Ordering::Relaxed), 1, "the stepped job waits for a free turn");

        frame_jobs_new_frame();
        assert!(!stepped.is_finished(), "the second step yields");
        assert_eq!(calls.load(Ordering::Relaxed), 2, "is_finished runs exactly one step on a free turn");
        frame_jobs_new_frame();
        assert!(stepped.is_finished(), "the third step completes");
        assert_eq!(stepped.try_take(), Some(Ok(3)), "a ready value can be taken in the same frame");
        assert_eq!(stepped.try_take(), None, "a ready value is delivered only once");

        let total = Arc::new(AtomicUsize::new(0));
        let job = |finish_at: Option<usize>| {
            let polls = total.clone();
            pool.submit_stepped_frame_job(Class::Visible, move || {
                let mut steps = 0;
                std::future::poll_fn(move |_| {
                    polls.fetch_add(1, Ordering::Relaxed);
                    steps += 1;
                    if Some(steps) == finish_at { Poll::Ready(steps) } else { Poll::Pending }
                })
            }).unwrap()
        };
        let mut endless = job(None);
        let mut five = job(Some(5));
        let mut result = None;
        for frame in 1..=10 {
            frame_jobs_new_frame();
            assert!(!endless.is_finished(), "the first job stays pending");
            assert_eq!(endless.try_take(), None, "repeat polls of the winner cannot reserve its next turn");
            result = five.try_take();
            assert_eq!(total.load(Ordering::Relaxed), frame, "exactly one poll runs in frame {frame}");
            if result.is_some() { break; }
        }
        assert_eq!(result, Some(Ok(5)), "the second job gets five turns within ten frames");
    }

    #[test]
    fn deferred_frame_jobs_release_the_step_on_cancel_drop_or_panic() {
        // The counter's captured Arc also probes when the driver drops the closure.
        let calls = Arc::new(AtomicUsize::new(0));
        let starts = Arc::new(AtomicUsize::new(0));
        let pending = || {
            let counted = calls.clone();
            let started = starts.clone();
            FrameJob::later(move || {
                started.fetch_add(1, Ordering::Relaxed);
                std::future::poll_fn(move |_| {
                    counted.fetch_add(1, Ordering::Relaxed);
                    Poll::<()>::Pending
                })
            })
        };
        drop(pending());
        assert_eq!(starts.load(Ordering::Relaxed), 0, "dropping before the first frame never starts the job");
        assert_eq!(Arc::strong_count(&calls), 1, "dropping the unstarted job releases the start closure");
        frame_jobs_new_frame();
        let mut cancelled = pending();
        assert_eq!(starts.load(Ordering::Relaxed), 0, "submitting a deferred job leaves start for its frame");
        assert_eq!(cancelled.try_take(), None, "the job yields before cancellation");
        assert_eq!(starts.load(Ordering::Relaxed), 1, "the first due frame creates and runs the step");
        cancelled.token().cancel();
        assert_eq!(cancelled.try_take(), Some(Err(TaskError::Cancelled)), "cancellation needs no frame turn");
        assert_eq!(calls.load(Ordering::Relaxed), 1, "a cancelled job never takes another step");
        assert_eq!(Arc::strong_count(&calls), 1, "cancellation releases the closure while the handle lives");

        frame_jobs_new_frame();
        let mut discarded = pending();
        assert!(!discarded.is_finished(), "the job yields before drop");
        drop(discarded);
        assert_eq!(calls.load(Ordering::Relaxed), 2, "dropping a pending job calls no further step");
        assert_eq!(Arc::strong_count(&calls), 1, "dropping the job releases its closure");

        frame_jobs_new_frame();
        let counted = calls.clone();
        let mut panicked = FrameJob::later(move || std::future::poll_fn(move |_| -> Poll<()> {
            counted.fetch_add(1, Ordering::Relaxed);
            panic!("frame step panicked");
        }));
        let report = PanicReport { message: "frame step panicked".into() };
        assert_eq!(panicked.try_take(), Some(Err(TaskError::Panicked(report))), "a step panic preserves its message");
        assert_eq!(Arc::strong_count(&calls), 1, "a panicked job releases its closure");
        assert_eq!(panicked.try_take(), None, "a panic is delivered only once");
        assert_eq!(calls.load(Ordering::Relaxed), 3, "later frames never poll the dropped future again");
    }

    #[cfg(not(all(target_arch = "wasm32", not(target_feature = "atomics"))))]
    #[test]
    fn worker_frame_jobs_step_until_ready_or_cancelled() {
        async fn add_after_yield(sum: &std::cell::Cell<u32>, amount: u32, owner: ThreadId) {
            let mut first = true;
            std::future::poll_fn(|_| {
                assert_eq!(std::thread::current().id(), owner, "the future is always polled on its start thread");
                if std::mem::take(&mut first) { Poll::Pending } else { Poll::Ready(()) }
            }).await;
            sum.set(sum.get() + amount);
        }

        let _serial = POOL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
        let pool = test_pool(1);
        let submitter = std::thread::current().id();
        let starts = Arc::new(AtomicUsize::new(0));
        let submit = |pool: &TaskPool, inline| {
            let starts = starts.clone();
            pool.submit_stepped_frame_job(Class::Visible, move || {
                starts.fetch_add(1, Ordering::Relaxed);
                let owner = std::thread::current().id();
                assert_eq!(owner == submitter, inline, "only the inline pool starts on the submitting frame thread");
                let sum = std::rc::Rc::new(std::cell::Cell::new(0));
                async move {
                    add_after_yield(&sum, 2, owner).await;
                    add_after_yield(&sum, 3, owner).await;
                    sum.get()
                }
            }).unwrap()
        };
        let mut ready = submit(&pool, false);
        let mut inline = submit(&pool.clone().with_inline_frame_jobs(), true);
        for _ in 0..2 {
            frame_jobs_new_frame();
            assert_eq!(inline.try_take(), None, "each await yields to a new frame");
        }
        frame_jobs_new_frame();
        let inline_result = inline.try_take();
        assert_eq!(inline_result, Some(Ok(5)), "the inline future retains its Rc across both awaits");

        let hold_poll = || {
            let polls = Arc::new(AtomicUsize::new(0));
            let counted = polls.clone();
            let (entered, started) = mpsc::channel();
            let (release, wait) = mpsc::channel();
            let job = pool.submit_stepped_frame_job(Class::Visible, move || std::future::poll_fn(move |_| {
                if counted.fetch_add(1, Ordering::Relaxed) != 0 { return Poll::Ready(()); }
                entered.send(()).unwrap();
                wait.recv().unwrap();
                Poll::Pending
            })).unwrap();
            started.recv_timeout(NOTHING_MOVES).expect("the worker enters the poll before the owner stops it");
            (job, release, polls)
        };
        let (mut cancelled, release, cancel_polls) = hold_poll();
        cancelled.cancel();
        release.send(()).unwrap();
        let (discarded, release, drop_polls) = hold_poll();
        let (finished, detached) = mpsc::channel();
        drop(pool.submit_frame_job(Class::Visible, move || finished.send(()).unwrap()).unwrap());
        drop(discarded);
        release.send(()).unwrap();
        worker_join(pool.shutdown_pool(ShutdownMode::Drain)).expect("all worker jobs must finish");

        assert_eq!(ready.try_take(), inline_result, "the worker and inline futures give the same result");
        assert_eq!(starts.load(Ordering::Relaxed), 2, "each driver calls start exactly once");
        assert_eq!(cancelled.try_take(), Some(Err(TaskError::Cancelled)), "the worker reports cancellation after Pending");
        assert_eq!(cancel_polls.load(Ordering::Relaxed), 1, "no poll followed the one in flight at cancellation");
        assert_eq!(drop_polls.load(Ordering::Relaxed), 1, "no poll followed the one in flight at drop");
        assert_eq!(detached.try_recv(), Ok(()), "dropping a queued worker one-shot still detaches it");
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
        started_rx.recv_timeout(NOTHING_MOVES).unwrap();

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
        started_rx.recv_timeout(NOTHING_MOVES).unwrap();
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
                TaskPool::create(spawner.clone(), 2, BudgetHandle::of_cores(POOL_TEST_CORES as u32)).unwrap(),
                TaskPool::create(spawner.clone(), 1, BudgetHandle::of_cores(POOL_TEST_CORES as u32)).unwrap(),
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
        started_rx.recv_timeout(NOTHING_MOVES).unwrap();
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
    fn fan_out_helpers_observe_the_callers_stale_ticket() {
        let _serial = POOL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
        let pool = test_pool(3);
        let worker_count = pool.worker_count();
        let version = Version::new();
        let inner_pool = pool.clone();
        let job = pool
            .submit_at(Class::Interactive, "stale fan_out checkpoints", version.next(), move || {
                version.bump();
                assert!(checkpoint().is_err(), "the job sees its stale ticket");
                let first_items = std::sync::Barrier::new(worker_count);
                let workers = Mutex::new(std::collections::HashSet::new());
                let checked = AtomicUsize::new(0);
                let ran = AtomicUsize::new(0);
                inner_pool.fan_out(10_000, |i| {
                    if i < worker_count {
                        workers.lock().unwrap().insert(std::thread::current().id());
                        without_permit(|| {
                            first_items.wait();
                        });
                    }
                    checked.fetch_add(1, Ordering::Relaxed);
                    if checkpoint().is_ok() {
                        ran.fetch_add(1, Ordering::Relaxed);
                    }
                });
                (
                    checked.into_inner(),
                    ran.into_inner(),
                    workers.into_inner().unwrap().len(),
                )
            })
            .unwrap();
        let (checked, ran, workers) = worker_join(job).unwrap();
        assert_eq!(workers, worker_count, "every worker checks the ticket");
        assert_eq!(checked, 10_000, "every index is visited");
        assert_eq!(ran, 0, "no item runs after the version bump");
        worker_join(pool.shutdown_pool(ShutdownMode::Drain)).unwrap();
    }

    #[test]
    fn fan_out_runs_every_index_after_its_job_goes_stale() {
        // A non-cancellable map inside a job a newer version made stale still gets every
        // result: fan_out never drops indices (an app's par::map panicked on one).
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
        started_rx.recv_timeout(NOTHING_MOVES).unwrap();
        started_rx.recv_timeout(NOTHING_MOVES).unwrap();
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
                    None,
                    saturated_pool.budget(),
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
        // A job finds its own pool without a handle (`of_current_worker`) and spreads over it; a
        // thread outside the pool (this test's, the spawner's) finds none.
        assert!(TaskPool::of_current_worker().is_none(), "the test thread is no pool worker");
        let outside = spawner.spawn_worker(ThreadOptions::default(), || TaskPool::of_current_worker().is_none()).unwrap();
        assert!(worker_join(outside).unwrap(), "a thread the pool does not own is no pool worker");
        let pool_at = Arc::as_ptr(&pool.inner) as usize;
        let found = pool
            .submit(Class::Background, move || {
                let own = TaskPool::of_current_worker().expect("a job's worker knows its pool");
                let count = AtomicUsize::new(0);
                own.fan_out(64, |_| {
                    count.fetch_add(1, Ordering::Relaxed);
                });
                (Arc::as_ptr(&own.inner) as usize == pool_at, count.into_inner())
            })
            .unwrap();
        assert_eq!(worker_join(found).unwrap(), (true, 64), "the job's own pool, every index run");
        worker_join(pool.shutdown_pool(ShutdownMode::Drain)).unwrap();
    }

    #[test]
    fn a_job_on_a_ui_thread_takes_no_token() {
        let _serial = POOL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
        let budget = BudgetHandle::of_cores(2);
        let pool = test_pool_on(1, budget.clone());
        let count = budget.cores() as usize;
        let held = HeldTokens::new(&budget, count, "token holder");
        let caller_pool = pool.clone();
        let caller = std::thread::spawn(move || {
            makepad_cpu_budget::set_ui_thread();
            let items = AtomicUsize::new(0);
            caller_pool.fan_out(3, |_| {
                assert_eq!(current_class(), Some(Class::Background), "each item runs in the caller's job");
                items.fetch_add(1, Ordering::Relaxed);
            });
            items.into_inner()
        });
        // With one worker the fan-out queues no helpers, so only the caller could be waiting.
        wait_for("the marked caller to finish or request a token", || {
            caller.is_finished() || !budget.tokens().waiting.is_empty()
        });
        let tokens = budget.tokens();
        drop(held);
        let items = caller.join().unwrap();
        worker_join(pool.shutdown_pool(ShutdownMode::Drain)).unwrap();
        assert!(tokens.waiting.is_empty(), "the marked caller waited for admission");
        assert_eq!(items, 3, "the fan-out must complete every item");
        assert_eq!(tokens.holders.len(), count, "all of the holder's rows must remain");
        assert!(tokens.holders.iter().all(|row| row.label == "token holder" && !row.lent), "only the holder's running rows may remain");
    }

    /// The class every item of a 64-item spread from the calling thread ran at: `fan_out`'s, or
    /// `fan_out_as`'s naming `class`.
    fn spread_classes(pool: &TaskPool, class: Option<Class>) -> Vec<Option<Class>> {
        let seen = Mutex::new(Vec::new());
        let item = |_: usize| seen.lock().unwrap().push(current_class());
        match class {
            Some(class) => pool.fan_out_as(class, usize::MAX, 64, item),
            None => pool.fan_out(64, item),
        }
        seen.into_inner().unwrap()
    }

    /// The calling thread's OS priority as the pool could set it: Linux nice and scheduling policy
    /// (fields 19 and 41 of /proc/thread-self/stat, the thread's /proc/self/task/<tid>/stat), the
    /// QoS rank on Apple targets.
    fn os_priority() -> (i64, i64) {
        #[cfg(target_os = "linux")]
        {
            let stat = std::fs::read_to_string("/proc/thread-self/stat").expect("the thread's stat reads");
            let fields: Vec<&str> = stat[stat.rfind(')').expect("the stat names its thread") + 2..].split(' ').collect();
            // `fields` starts at field 3 (the state)
            (fields[16].parse().expect("nice"), fields[38].parse().expect("policy"))
        }
        #[cfg(target_vendor = "apple")]
        {
            (thread_rank() as i64, 0)
        }
        #[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
        {
            (0, 0)
        }
    }

    #[test]
    fn a_fan_out_from_outside_the_pool_runs_at_the_class_it_names() {
        let _serial = POOL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
        let pool = test_pool(2);
        wait_for("workers to start", || {
            pool.inner.started.load(Ordering::Acquire) == 2
        });
        let own_pool = pool.clone();
        let spawner = ThreadSpawner::for_current_thread(1);
        let own = spawner
            .spawn_worker(ThreadOptions::default(), move || {
                let plain = spread_classes(&own_pool, None);
                let named = spread_classes(&own_pool, Some(Class::Visible));
                (plain, named, current_class())
            })
            .unwrap();
        let (plain, named, after) = worker_join(own).unwrap();
        // A plain std thread's own OS priority never changes: neither its spread at Background nor
        // one at a named class moves it (a Linux thread could not climb back).
        let std_pool = pool.clone();
        let (before, plain_std, named_std, later) = std::thread::spawn(move || {
            let before = os_priority();
            let plain = spread_classes(&std_pool, None);
            let named = spread_classes(&std_pool, Some(Class::Visible));
            (before, plain, named, os_priority())
        }).join().unwrap();
        assert!(plain_std.iter().all(|c| *c == Some(Class::Background)) && named_std.iter().all(|c| *c == Some(Class::Visible)), "a std thread's spreads run at their classes: {plain_std:?} {named_std:?}");
        assert_eq!(later, before, "a std thread that fans out keeps its OS priority (nice and policy, or QoS rank)");
        assert!(plain.len() == 64 && plain.iter().all(|c| *c == Some(Class::Background)), "a plain spread from the app's own thread runs at Background: {plain:?}");
        assert!(named.len() == 64 && named.iter().all(|c| *c == Some(Class::Visible)), "a spread from the app's own thread that names Visible runs at Visible, its helpers too: {named:?}");
        assert_eq!(after, None, "the thread is outside the pool's jobs once its spreads return");

        let job_pool = pool.clone();
        let job = pool.submit(Class::Background, move || spread_classes(&job_pool, Some(Class::Visible))).unwrap();
        let in_job = worker_join(job).unwrap();
        assert!(in_job.len() == 64 && in_job.iter().all(|c| *c == Some(Class::Background)), "a Background job's spread stays at its class whatever it names: {in_job:?}");
        worker_join(pool.shutdown_pool(ShutdownMode::Drain)).unwrap();
    }

    #[test]
    fn a_job_helping_another_budgets_pool_keeps_each_permit_on_its_own_budget() {
        let _serial = POOL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
        let a = test_pool(1);
        let b = test_pool(1);
        let a_budget = a.budget().clone();
        let b_budget = b.budget().clone();
        let nested_pool = b.clone();
        let job = a.submit(Class::Visible, move || {
            let before = a_budget.tokens();
            let during = OnceLock::new();
            let panic = catch_unwind(AssertUnwindSafe(|| {
                nested_pool.fan_out_on(1, 2, |i| {
                    if i == 1 {
                        panic!("item 1");
                    }
                    assert!(during.set((a_budget.tokens(), b_budget.tokens(), checkpoint().is_ok())).is_ok(), "item 0 must run only once");
                });
            })).expect_err("the caller must catch the second item's panic");
            assert_eq!(&*panic_report(panic).message, "item 1", "the panic must come from item 1");
            let (outer, inner, checked) = during.into_inner().expect("the caller must run item 0");
            assert_eq!((before.holders.len(), before.running), (1, 1), "the outer job must start with one running holder on a");
            assert!(!before.holders[0].lent, "a's initial token must be running");
            assert_eq!((outer.holders.len(), outer.running), (1, 0), "a must retain its row and free its token during the nested job");
            assert!(outer.holders[0].lent, "a must lend its token while the caller runs on b");
            assert_eq!((inner.holders.len(), inner.running), (1, 1), "the nested job must hold its own running row on b");
            assert!(!inner.holders[0].lent, "the nested job must run on b's token");
            assert!(checked, "item 0 must checkpoint on b");

            let restored = a_budget.tokens();
            assert_eq!((restored.holders.len(), restored.running), (1, 1), "unwinding must leave a's row running with the outer job");
            assert!(!restored.holders[0].lent, "unwinding must take back a's token");
            assert_eq!(b_budget.tokens().running, 0, "unwinding must release b's permit");
            assert!(checkpoint().is_ok(), "the restored job must checkpoint on a");
            assert_eq!(current_class(), Some(Class::Visible), "the restored job must run at Visible");
        }).unwrap();
        worker_join(job).expect("the outer job must finish after catching the nested panic");
        for (pool, idle) in [(&a, "a to release its last permit"), (&b, "b to release its last permit")] {
            wait_for(idle, || {
                let tokens = pool.budget().tokens();
                tokens.running == 0 && tokens.holders.is_empty() && tokens.waiting.is_empty()
            });
            worker_join(pool.shutdown_pool(ShutdownMode::Drain)).unwrap();
        }
    }

    #[test]
    fn a_cx_that_ends_leaves_the_process_pool_to_the_others() {
        in_own_budget!(a_cx_that_ends_leaves_the_process_pool_to_the_others);
        let budget = process_test_budget();
        let mut ending = crate::Cx::new(Box::new(|_, _| {}));
        let going_on = crate::Cx::new(Box::new(|_, _| {}));
        let (ended, pool) = (ending.task_pool(), going_on.task_pool());
        assert_eq!(pool.budget().id(), budget.id(), "Cx must use the process budget");
        ending.call_event_handler(&crate::event::Event::Shutdown);
        assert!(!ended.is_open(), "its own jobs are over");
        assert!(pool.is_open(), "the other Cx's are not");
        assert_eq!(worker_join(pool.submit(Class::Visible, || 5).unwrap()).unwrap(), 5);
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
        assert!(checkpoint().is_ok(), "a checkpoint outside a pool job must succeed");
        assert!(checkpoint_when_due().is_ok(), "a poll outside a pool job must succeed");
        assert_eq!(current_class(), None, "the calling thread must be outside a pool job");
        let pool = test_pool(1);
        for supersede in [false, true] {
            let version = Version::new();
            let (started, began) = mpsc::channel();
            let (go, wait) = mpsc::channel();
            let job = pool
                .submit_at(Class::Visible, "checkpoint", version.next(), move || {
                    started.send(()).unwrap();
                    wait.recv().unwrap();
                    let immediate = checkpoint().is_err();
                    wait_for("the stale job's next checkpoint to be due", checkpoint_due);
                    (immediate, checkpoint_when_due().is_err(), checkpoint_when_due().is_err())
                })
                .unwrap();
            began.recv().unwrap();
            if supersede {
                version.bump();
            } else {
                job.cancel();
            }
            go.send(()).unwrap();
            let (immediate, when_due, again) = worker_join(job).unwrap();
            assert!(immediate, "an explicit checkpoint must report the stale job (supersede={supersede})");
            assert!(when_due, "the first due poll must report the stale job (supersede={supersede})");
            assert!(again, "a reported stale result must not be consumed: the next poll reports it too (supersede={supersede})");
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
        // in the order they came, each at its class or that of a more urgent one behind it
        for (i, (class, runs_at)) in [
            (Class::Background, Class::Interactive),
            (Class::Interactive, Class::Interactive),
            (Class::Visible, Class::Visible),
        ]
        .into_iter()
        .enumerate()
        {
            let order = order.clone();
            jobs.push(strand.submit(class, Ticket::none(), move |state| {
                assert_eq!(current_class(), Some(runs_at));
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
        const WORKERS: usize = 2;
        const ITEMS: usize = 64;
        #[derive(Default)]
        struct State {
            entered: usize,
            let_go: usize,
            interactive_at: Option<usize>,
        }
        let pool = test_pool(WORKERS);
        let state = Together::new(State::default());
        let ending = state.clone();
        let _end = AtEnd(move || ending.change(|s| s.let_go = ITEMS));
        let (inner_pool, items) = (pool.clone(), state.clone());
        let batch = pool
            .submit(Class::Visible, move || {
                inner_pool.fan_out(ITEMS, |i| {
                    items.change(|s| s.entered += 1);
                    drop(items.until("its turn", |s| i < s.let_go));
                });
            })
            .unwrap();
        drop(state.until("the fan-out on both workers", |s| s.entered == WORKERS));
        let starts = state.clone();
        let interactive = pool
            .submit(Class::Interactive, move || starts.change(|s| s.interactive_at = Some(s.entered)))
            .unwrap();
        state.change(|s| s.let_go = WORKERS);
        let at = state.until("the Interactive job, or both workers on their next item", |s| s.interactive_at.is_some() || s.entered == 2 * WORKERS).interactive_at;
        assert!(at.is_some(), "both workers took another item of the Visible fan-out while the Interactive job waited");
        state.change(|s| s.let_go = ITEMS);
        worker_join(interactive).unwrap();
        worker_join(batch).unwrap();
        assert_eq!(state.until("the items", |_| true).entered, ITEMS, "every item ran");
        worker_join(pool.shutdown_pool(ShutdownMode::Drain)).unwrap();
    }

    /// An Interactive job starts while a long Visible job that only checkpoints holds every worker
    /// that takes it, and another Visible job waits: the reserve's worker runs it, and only it. A
    /// checkpoint yields a job's permit, never its thread, so with no reserve the Interactive job
    /// would wait for the Visible job's end.
    #[test]
    fn an_interactive_job_starts_while_long_visible_jobs_hold_every_worker() {
        let _serial = POOL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
        #[derive(Default)]
        struct State {
            visible_started: usize,
            interactive_ran: bool,
        }
        let budget = BudgetHandle::of_cores(POOL_TEST_CORES as u32);
        let pool = TaskPool::create_with(ThreadSpawner::for_current_thread(3), 1, interactive_reserve(&budget), budget).unwrap();
        assert_eq!(pool.worker_count(), 1, "a split of work is sized by the workers that take every class");
        let state = Together::new(State::default());
        let released = Arc::new(AtomicBool::new(false));
        let _end = AtEnd({
            let released = released.clone();
            move || released.store(true, Ordering::Release)
        });
        let long_visible = || {
            let (state, released) = (state.clone(), released.clone());
            pool.submit(Class::Visible, move || {
                state.change(|s| s.visible_started += 1);
                while !released.load(Ordering::Acquire) {
                    let _ = checkpoint();
                    std::thread::yield_now();
                }
            })
            .unwrap()
        };
        let first = long_visible();
        drop(state.until("the long Visible job on the worker", |s| s.visible_started == 1));
        let second = long_visible();
        let starts = state.clone();
        let interactive = pool.submit(Class::Interactive, move || starts.change(|s| s.interactive_ran = true)).unwrap();
        let seen = state.until("the Interactive job, with the long Visible job on the worker", |s| s.interactive_ran);
        assert_eq!(seen.visible_started, 1, "the reserve's worker took a Visible job");
        drop(seen);
        released.store(true, Ordering::Release);
        worker_join(interactive).unwrap();
        worker_join(first).unwrap();
        worker_join(second).unwrap();
        worker_join(pool.shutdown_pool(ShutdownMode::Drain)).unwrap();
    }

    /// `most` counts the caller: the rest of the pool stays free for other work.
    #[test]
    fn a_fan_out_on_few_workers_leaves_the_others_free() {
        let _serial = POOL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
        const WORKERS: usize = 4;
        const ITEMS: usize = 64;
        #[derive(Default)]
        struct State {
            entered: usize,
            let_go: usize,
            other: bool,
        }
        let pool = test_pool(WORKERS);
        let state = Together::new(State::default());
        let ending = state.clone();
        let _end = AtEnd(move || ending.change(|s| s.let_go = ITEMS));
        let (inner_pool, items) = (pool.clone(), state.clone());
        let batch = pool
            .submit(Class::Visible, move || {
                inner_pool.fan_out_on(2, ITEMS, |i| {
                    items.change(|s| s.entered += 1);
                    drop(items.until("its turn", |s| i < s.let_go));
                });
            })
            .unwrap();
        drop(state.until("the fan-out on its two workers", |s| s.entered == 2));
        // a worker is free for this while each of the two holds an item
        let others = state.clone();
        let other = pool.submit(Class::Visible, move || others.change(|s| s.other = true)).unwrap();
        let at = state.until("the other job", |s| s.other);
        assert_eq!(at.entered, 2, "the fan-out took more than its two workers");
        drop(at);
        state.change(|s| s.let_go = ITEMS);
        worker_join(other).unwrap();
        worker_join(batch).unwrap();
        assert_eq!(state.until("the items", |_| true).entered, ITEMS, "every item ran");
        worker_join(pool.shutdown_pool(ShutdownMode::Drain)).unwrap();
    }

    /// A job of a fan-out's own class queued behind it starts at a helper's next item boundary,
    /// not when the fan-out ends. Each item holds until it is let go: past one round of items, a
    /// fan-out that keeps its workers is on the next item on every worker, with the job queued.
    #[test]
    fn a_fan_out_gives_a_worker_to_a_job_of_its_class_queued_behind_it() {
        let _serial = POOL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
        const WORKERS: usize = 4;
        const ITEMS: usize = 64;
        #[derive(Default)]
        struct State {
            entered: usize,
            let_go: usize,
            behind_at: Option<usize>,
        }
        let pool = test_pool(WORKERS);
        let state = Together::new(State::default());
        let ending = state.clone();
        let _end = AtEnd(move || ending.change(|s| s.let_go = ITEMS));
        let (inner_pool, items) = (pool.clone(), state.clone());
        let batch = pool
            .submit(Class::Visible, move || {
                inner_pool.fan_out(ITEMS, |i| {
                    items.change(|s| s.entered += 1);
                    drop(items.until("its turn", |s| i < s.let_go));
                });
            })
            .unwrap();
        drop(state.until("the fan-out on every worker", |s| s.entered == WORKERS));
        let starts = state.clone();
        let behind = pool.submit(Class::Visible, move || starts.change(|s| s.behind_at = Some(s.entered))).unwrap();
        state.change(|s| s.let_go = WORKERS);
        let at = state.until("the job behind, or every worker on its next item", |s| s.behind_at.is_some() || s.entered == 2 * WORKERS).behind_at;
        assert!(at.is_some(), "every worker took another item of the fan-out while a job of its class waited");
        state.change(|s| s.let_go = ITEMS);
        worker_join(behind).unwrap();
        worker_join(batch).unwrap();
        assert_eq!(state.until("the items", |_| true).entered, ITEMS, "every item ran");
        worker_join(pool.shutdown_pool(ShutdownMode::Drain)).unwrap();
    }

    /// Two fan-outs of a class end up on half the workers each, whichever came first, and at their
    /// parts they stop trading workers. Each item holds until the round ends, so a round's end
    /// reads where every worker went. No job stays counted once all ended.
    #[test]
    fn fan_outs_of_a_class_share_its_workers_and_rest_at_their_parts() {
        let _serial = POOL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
        const WORKERS: usize = 6;
        #[derive(Default)]
        struct State {
            round: usize,
            /// Items each fan-out entered this round, one per worker on it.
            held: [usize; 2],
            ended: bool,
        }
        let pool = test_pool(WORKERS);
        let state = Together::new(State::default());
        let ending = state.clone();
        let _end = AtEnd(move || ending.change(|s| s.ended = true));
        let fan = |which: usize| {
            let (inner_pool, state) = (pool.clone(), state.clone());
            pool.submit(Class::Visible, move || {
                inner_pool.fan_out(100_000, |_| {
                    let round = state.change(|s| {
                        s.held[which] += 1;
                        s.round
                    });
                    drop(state.until("the round's end", |s| s.round != round || s.ended));
                });
            })
            .unwrap()
        };
        let next_round = || {
            state.change(|s| {
                s.round += 1;
                s.held = [0, 0];
            });
            state.until("every worker on an item", |s| s.held[0] + s.held[1] == WORKERS).held
        };
        let a = fan(0);
        drop(state.until("the first on every worker", |s| s.held[0] == WORKERS));
        let b = fan(1);
        // a worker moves in a round, so far fewer rounds than this; the first alone keeps them all
        let rounds = (0..100).position(|_| next_round() == [WORKERS / 2, WORKERS / 2]);
        assert!(rounds.is_some(), "100 rounds of items and each fan-out is not on its part");
        let before = pool.inner.completed.load(Ordering::SeqCst);
        for _ in 0..20 {
            next_round();
        }
        let traded = pool.inner.completed.load(Ordering::SeqCst) - before;
        // 4 helpers leave each round if they trade; a permit's yield on a loaded box ends a few
        assert!(traded < 40, "at their parts they do not trade workers at every item: {traded} helpers ended in 20 rounds");
        state.change(|s| s.ended = true);
        worker_join(a).unwrap();
        worker_join(b).unwrap();
        worker_join(pool.shutdown_pool(ShutdownMode::Drain)).unwrap();
        let shares = &pool.inner.shares[Class::Visible.budget() as usize];
        assert_eq!(
            (shares.groups.load(Ordering::SeqCst), shares.workers.load(Ordering::SeqCst), shares.queued.load(Ordering::SeqCst)),
            (0, 0, 0),
            "no job stays counted"
        );
    }

    /// A group's part, by the counts alone, on 7 workers.
    #[test]
    fn a_group_gives_up_workers_only_past_its_part() {
        let _serial = POOL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
        let pool = test_pool(7);
        let inner = &pool.inner;
        let class = Class::Visible.budget() as usize;
        let (a, b) = (Share::new(inner, Class::Visible), Share::new(inner, Class::Visible));
        let on = |share: &Arc<Share>, n: usize| (0..n).map(|_| ShareJob::running(share.clone())).collect::<Vec<_>>();
        let helper = |share: &Arc<Share>| {
            inner.queued[class].fetch_add(1, Ordering::Relaxed);
            ShareJob::queued(share.clone())
        };
        let plain = |n: isize| {
            if n > 0 {
                inner.queued[class].fetch_add(n as usize, Ordering::Relaxed);
            } else {
                inner.queued[class].fetch_sub(n.unsigned_abs(), Ordering::Relaxed);
            }
        };
        let mut a_on = on(&a, 7);
        assert!(!inner.over_share(&a), "nothing waits");
        let a_helper = helper(&a);
        assert!(!inner.over_share(&a), "only its own helper waits");
        plain(1);
        assert!(inner.over_share(&a), "a job of no group waits behind a group on all of the workers");
        a_on.truncate(4);
        assert!(!inner.over_share(&a), "4 of 7 is its part, with the rest for the jobs of no group");
        plain(-1);
        // a second fan-out, from a worker the first gave up
        a_on.truncate(6);
        let mut b_on = on(&b, 1);
        let b_helper = helper(&b);
        assert!(inner.over_share(&a), "its part of what they hold is 4, not 6");
        assert!(!inner.over_share(&b), "1 is below its part");
        a_on.truncate(4);
        b_on.extend(on(&b, 2));
        assert!(!inner.over_share(&a) && !inner.over_share(&b), "4 and 3 on 7: at rest, no worker trades");
        plain(1);
        assert!(inner.over_share(&a), "three sharers: 3 each");
        assert!(!inner.over_share(&b), "3 is its part");
        plain(-1);
        drop((a_on, b_on, a_helper, b_helper));
        inner.queued[class].fetch_sub(2, Ordering::Relaxed);
        let shares = &inner.shares[class];
        assert_eq!(
            (shares.groups.load(Ordering::Relaxed), shares.workers.load(Ordering::Relaxed), shares.queued.load(Ordering::Relaxed)),
            (0, 0, 0),
            "no job stays counted"
        );
        worker_join(pool.shutdown_pool(ShutdownMode::Drain)).unwrap();
    }

    /// A scope's job taking task after task learns that a job of its class waits while its scope
    /// holds more than its part of the workers, and not for its scope's own queued job. The test
    /// asks the jobs at their task boundaries and reads their answers.
    #[test]
    fn a_scope_job_learns_a_job_of_its_class_waits_past_its_scopes_part() {
        let _serial = POOL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
        #[derive(Default)]
        struct State {
            on: usize,
            asked: usize,
            answered: usize,
            left: usize,
            behind: bool,
            ended: bool,
        }
        let pool = test_pool(4);
        let state = Together::new(State::default());
        let ending = state.clone();
        let _end = AtEnd(move || ending.change(|s| s.ended = true));
        let (inner_pool, jobs) = (pool.clone(), state.clone());
        let lead = pool
            .submit(Class::Visible, move || {
                inner_pool.scope(|s| {
                    // on the three workers beside this one, and one queued behind them
                    for _ in 0..4 {
                        let state = jobs.clone();
                        s.spawn(move || {
                            let mut seen = state.change(|s| {
                                s.on += 1;
                                s.asked
                            });
                            loop {
                                {
                                    let s = state.until("a question", |s| s.asked != seen || s.ended);
                                    if s.ended {
                                        return;
                                    }
                                    seen = s.asked;
                                }
                                let leave = over_share_in_pool() && worker_wanted();
                                state.change(|s| {
                                    s.answered += 1;
                                    s.left += leave as usize;
                                    s.on -= leave as usize;
                                });
                                if leave {
                                    return;
                                }
                            }
                        });
                    }
                    // leaving this closure would run the queued one here
                    without_permit(|| drop(jobs.until("the test's end", |s| s.ended)));
                });
            })
            .unwrap();
        drop(state.until("the scope on three workers", |s| s.on == 3));
        let ask = || {
            let on = state.change(|s| {
                s.asked += 1;
                s.answered = 0;
                s.on
            });
            state.until("the jobs' answers", |s| s.answered >= on).left
        };
        assert_eq!(ask(), 0, "its own queued job asks for no worker");
        let starts = state.clone();
        let behind = pool.submit(Class::Visible, move || starts.change(|s| s.behind = true)).unwrap();
        assert!(ask() >= 1, "every worker stays on its scope while a job of its class waits");
        // the first to leave may start the scope's own queued job, so more than one left
        drop(state.until("the job behind", |s| s.behind));
        worker_join(behind).unwrap();
        state.change(|s| s.ended = true);
        worker_join(lead).unwrap();
        worker_join(pool.shutdown_pool(ShutdownMode::Drain)).unwrap();
    }

    /// A crew helper (a job taking task after task on a worker) learns at its task boundary that a
    /// higher-class job waits for a worker, though no permit waits: every worker is busy. Before
    /// it, an Interactive job waited 8 s behind 14 Visible helpers that checkpointed
    /// every few ms. This test checks that the pool's queue alone can make `worker_wanted` true.
    #[test]
    fn a_long_job_learns_a_higher_class_job_waits_for_its_worker() {
        let _serial = POOL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
        fn pool_wants_worker() -> bool {
            current_class().is_some_and(higher_queued_in_pool)
        }
        let pool = test_pool(1);
        let (started, on_worker) = mpsc::channel();
        let (queued, boundary) = mpsc::channel();
        let helper = pool
            .submit(Class::Visible, move || {
                let before = pool_wants_worker();
                started.send(()).unwrap();
                // its next task boundary comes after the Interactive job was queued
                boundary.recv().unwrap();
                (before, pool_wants_worker() && worker_wanted())
            })
            .unwrap();
        on_worker.recv().unwrap();
        let interactive = pool.submit(Class::Interactive, pool_wants_worker).unwrap();
        queued.send(()).unwrap();
        let (before, learned) = worker_join(helper).unwrap();
        assert!(!before, "nothing waited in the pool when the helper started");
        assert!(learned, "the helper learns at its task boundary that a higher-class job waits for its worker");
        assert!(!worker_join(interactive).unwrap(), "the Interactive job itself is not asked to leave");
        assert!(!worker_wanted(), "off a pool worker, with no job, nothing is wanted");
        worker_join(pool.shutdown_pool(ShutdownMode::Drain)).unwrap();
    }

    /// A fan-out ends when its items are done. A helper that finished its item and then
    /// waits at its checkpoint for a permit (the budget is full: a higher class holds it)
    /// borrows nothing, so the caller does not wait for it. Before, the caller slept until
    /// that helper was admitted again and found no index left: a cold load's texture encodes
    /// spent 13.8 s of 32.7 s in fan-outs waiting like that.
    #[test]
    fn a_fan_out_returns_when_its_items_are_done_not_when_its_helpers_are_admitted() {
        let _serial = POOL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
        let pool = test_pool(2);
        let child_pool = pool.clone();
        let job = pool
            .submit(Class::Background, move || {
                let caller = std::thread::current().id();
                let helper_in_item = AtomicBool::new(false);
                let helper_done = AtomicBool::new(false);
                let pressure_ready = AtomicBool::new(false);
                let ran = AtomicUsize::new(0);
                // Set once the budget is given back: the fan-out must have returned before.
                let released = Arc::new(AtomicBool::new(false));
                let pressure = Arc::new(Mutex::new(None));
                let watchdog = Mutex::new(None);
                child_pool.fan_out(2, |_| {
                    ran.fetch_add(1, Ordering::SeqCst);
                    if std::thread::current().id() == caller {
                        wait_for("the helper to take the other item", || helper_in_item.load(Ordering::Acquire));
                        // Fill the budget and let a higher class wait for a permit: the
                        // helper's next checkpoint gives its permit up and waits for one.
                        let budget = child_pool.budget();
                        let held = HeldTokens::new(budget, (budget.cores() as usize).saturating_sub(2), "fan-out occupant");
                        let (release, wait_release) = mpsc::channel::<()>();
                        let waiter_budget = budget.clone();
                        let waiter = std::thread::spawn(move || {
                            let permit = waiter_budget.admit(makepad_cpu_budget::VISIBLE, makepad_cpu_budget::APP, "fan-out waiter");
                            let _ = wait_release.recv();
                            waiter_budget.release(permit);
                        });
                        let permit = JOB.with(|j| j.borrow().as_ref().unwrap().permit.unwrap());
                        wait_for("a higher class waiting for a permit", || budget.should_yield(permit));
                        *pressure.lock().unwrap() = Some((held, release, waiter));
                        // A fan-out that waits for its helper's permit never returns by itself:
                        // give the budget back after a while, so that failure is an assert.
                        let (pressure, released) = (pressure.clone(), released.clone());
                        *watchdog.lock().unwrap() = Some(std::thread::spawn(move || {
                            let began = std::time::Instant::now();
                            while !released.load(Ordering::SeqCst) && began.elapsed() < std::time::Duration::from_secs(3) {
                                std::thread::sleep(std::time::Duration::from_millis(5));
                            }
                            if !released.swap(true, Ordering::SeqCst) {
                                drop(pressure.lock().unwrap().take());
                            }
                        }));
                        pressure_ready.store(true, Ordering::Release);
                        // The helper leaves its item under that pressure, before the caller is
                        // done: what is left for it is its checkpoint (the settle covers the
                        // few instructions between its item's end and the pool's own count).
                        wait_for("the helper to finish its item", || helper_done.load(Ordering::Acquire));
                        std::thread::sleep(std::time::Duration::from_millis(20));
                    } else {
                        helper_in_item.store(true, Ordering::Release);
                        // It keeps its permit through the item: giving it up here would make
                        // the item itself wait for the full budget.
                        wait_for("the budget to fill", || pressure_ready.load(Ordering::Acquire));
                        helper_done.store(true, Ordering::Release);
                    }
                });
                // Here the helper is out of its item, at its checkpoint or past it.
                let returned_before_release = !released.swap(true, Ordering::SeqCst);
                watchdog.into_inner().unwrap().expect("the caller ran an item").join().unwrap();
                // Dropping the sender releases the waiter's permit; the tokens go with it.
                if let Some((held, release, waiter)) = pressure.lock().unwrap().take() {
                    drop(held);
                    drop(release);
                    waiter.join().unwrap();
                }
                (ran.into_inner(), returned_before_release)
            })
            .unwrap();
        let (ran, returned_before_release) = worker_join(job).unwrap();
        assert_eq!(ran, 2, "both indices ran");
        assert!(returned_before_release, "the fan-out returned while the budget was still full");
        worker_join(pool.shutdown_pool(ShutdownMode::Drain)).unwrap();
    }

    /// The caller that waits for a helper inside its last item lends its token meanwhile and
    /// takes it back when the item ends: it is not admitted anew. With the budget full (a higher
    /// class took the token the caller lent to wait) the fan-out still returns at once.
    #[test]
    fn a_fan_out_caller_takes_its_own_token_back_instead_of_a_new_admission() {
        in_own_budget!(a_fan_out_caller_takes_its_own_token_back_instead_of_a_new_admission);
        let _serial = POOL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
        let pool = test_pool_on(2, process_test_budget());
        let child_pool = pool.clone();
        let job = pool
            .submit(Class::Background, move || {
                let caller = std::thread::current().id();
                let helper_in_item = AtomicBool::new(false);
                let caller_item_done = AtomicBool::new(false);
                let released = Arc::new(AtomicBool::new(false));
                let pressure = Arc::new(Mutex::new(None));
                let watchdog = Mutex::new(None);
                child_pool.fan_out(2, |_| {
                    if std::thread::current().id() == caller {
                        wait_for("the helper to take the other item", || helper_in_item.load(Ordering::Acquire));
                        // Every other permit is held and a higher class waits for one: it
                        // takes the token the caller lends when it sleeps for the helper.
                        let budget = child_pool.budget();
                        let held = HeldTokens::new(budget, (budget.cores() as usize).saturating_sub(2), "fan-out occupant");
                        let (release, wait_release) = mpsc::channel::<()>();
                        let waiter_budget = budget.clone();
                        let waiter = std::thread::spawn(move || {
                            let permit = waiter_budget.admit(makepad_cpu_budget::VISIBLE, makepad_cpu_budget::APP, "fan-out waiter");
                            let _ = wait_release.recv();
                            waiter_budget.release(permit);
                        });
                        let permit = JOB.with(|j| j.borrow().as_ref().unwrap().permit.unwrap());
                        wait_for("a higher class waiting for a permit", || budget.should_yield(permit));
                        *pressure.lock().unwrap() = Some((held, release, waiter));
                        let (pressure, released) = (pressure.clone(), released.clone());
                        *watchdog.lock().unwrap() = Some(std::thread::spawn(move || {
                            let began = std::time::Instant::now();
                            while !released.load(Ordering::SeqCst) && began.elapsed() < std::time::Duration::from_secs(3) {
                                std::thread::sleep(std::time::Duration::from_millis(5));
                            }
                            if !released.swap(true, Ordering::SeqCst) {
                                drop(pressure.lock().unwrap().take());
                            }
                        }));
                        caller_item_done.store(true, Ordering::Release);
                    } else {
                        helper_in_item.store(true, Ordering::Release);
                        // Stay in the item, permit held, until the caller sleeps for it (the
                        // settle covers the caller's way from its item into that sleep).
                        wait_for("the caller to finish its item", || caller_item_done.load(Ordering::Acquire));
                        std::thread::sleep(std::time::Duration::from_millis(20));
                    }
                });
                let returned_before_release = !released.swap(true, Ordering::SeqCst);
                let has_permit = JOB.with(|j| j.borrow().as_ref().unwrap().permit.is_some());
                // Its permit is a row of its own thread: asleep, the caller gives its token back.
                // A permit handed over by a helper stayed the helper's thread's, whose sleeps
                // lent the token the caller ran on; a checkpoint that yielded it counted it again.
                let lends_its_own = makepad_cpu_budget::lend();
                if lends_its_own {
                    makepad_cpu_budget::take_back();
                }
                watchdog.into_inner().unwrap().expect("the caller ran an item").join().unwrap();
                if let Some((held, release, waiter)) = pressure.lock().unwrap().take() {
                    drop(held);
                    drop(release);
                    waiter.join().unwrap();
                }
                (returned_before_release, has_permit, lends_its_own)
            })
            .unwrap();
        let (returned_before_release, has_permit, lends_its_own) = worker_join(job).unwrap();
        assert!(returned_before_release, "the caller came back while the budget was still full, without a new admission");
        assert!(has_permit, "the caller's job holds a permit again");
        assert!(lends_its_own, "the caller's permit is its own thread's: asleep, it lends its token");
        worker_join(pool.shutdown_pool(ShutdownMode::Drain)).unwrap();
    }

    #[test]
    fn fan_out_finishes_when_all_helpers_yield_to_visible_work() {
        let _serial = POOL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
        let helpers = 2;
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
                        let budget = child_pool.budget();
                        let held = HeldTokens::new(budget, (budget.cores() as usize).saturating_sub(helpers + 1), "batch occupant");
                        let (release, released) = mpsc::channel();
                        let waiter_budget = budget.clone();
                        let waiter = std::thread::spawn(move || {
                            let permit = waiter_budget.admit(makepad_cpu_budget::VISIBLE, makepad_cpu_budget::APP, "batch waiter");
                            let _ = released.recv();
                            waiter_budget.release(permit);
                        });
                        let permit = JOB.with(|j| j.borrow().as_ref().unwrap().permit.unwrap());
                        wait_for("higher class waiting for the batch", || budget.should_yield(permit));
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
                        drop(held);
                        release.send(()).unwrap();
                        waiter.join().unwrap();
                    } else if std::thread::current().id() != caller
                        && arrived.fetch_add(1, Ordering::SeqCst) < helpers
                    {
                        barrier.wait();
                        wait_for("the pressure on the batch", || pressure_ready.load(Ordering::Acquire));
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
    fn admission_waiters_leave_a_worker_for_higher_classes() {
        let _serial = POOL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
        let pool = test_pool(3);
        let held = HeldTokens::new(pool.budget(), pool.budget().cores() as usize, "admission gate");
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
        held.give_back_one();
        started.recv_timeout(NOTHING_MOVES).unwrap();
        worker_join(interactive).unwrap();
        drop(held);
        worker_join(visible).unwrap();
        worker_join(bg).unwrap();
        worker_join(pool.shutdown_pool(ShutdownMode::Drain)).unwrap();
    }

    /// The pool's one free worker waits for a Background permit when Visible work queues: it asks
    /// for a Visible permit instead and runs the Visible job while Background work has no permit,
    /// then waits for the Background job's own.
    #[test]
    fn a_background_admission_gives_way_to_visible_work_queued_behind_it() {
        let _serial = POOL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
        let pool = test_pool(1);
        // Every permit Background work may have, held: Visible work still has room.
        let held = HeldTokens::new(pool.budget(), pool.budget().background_limit() as usize, "background permits");
        let (bg_started, bg_began) = mpsc::channel();
        let bg = pool.submit(Class::Background, move || bg_started.send(()).unwrap()).unwrap();
        wait_for("the worker to wait for a Background permit", || {
            pool.inner.state.lock().unwrap().waiting[2]
        });
        let (visible_started, visible_began) = mpsc::channel();
        let visible = pool.submit(Class::Visible, move || visible_started.send(()).unwrap()).unwrap();
        visible_began
            .recv_timeout(NOTHING_MOVES)
            .expect("the Visible job runs while the Background job has no permit");
        worker_join(visible).unwrap();
        assert!(bg_began.try_recv().is_err(), "the Background job waits for its own permit");
        wait_for("the worker to wait for a Background permit again", || {
            pool.inner.state.lock().unwrap().waiting[2]
        });
        drop(held);
        bg_began.recv_timeout(NOTHING_MOVES).unwrap();
        worker_join(bg).unwrap();
        worker_join(pool.shutdown_pool(ShutdownMode::Drain)).unwrap();
    }

    #[test]
    fn scope_wait_releases_its_permit_for_work_that_unblocks_a_child() {
        let _serial = POOL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
        let pool = test_pool(3);
        // Two permits let the caller and its child start together.
        let held = HeldTokens::new(pool.budget(), pool.budget().cores() as usize - 2, "scope gate");
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
        drop(held);
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

    /// A wait as the CPU budget's report writes it (`human`), in seconds.
    fn report_seconds(text: &str) -> f64 {
        let (number, unit) = text.trim().split_once(' ').expect("a number and a unit");
        let number: f64 = number.parse().expect("a number");
        number
            * match unit {
                "ns" => 1e-9,
                "us" => 1e-6,
                "ms" => 1e-3,
                "s" => 1.0,
                other => panic!("unit {other}"),
            }
    }

    #[test]
    fn a_strand_jobs_wait_behind_an_earlier_job_is_reported_as_the_strands() {
        let _serial = POOL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
        let pool = test_pool(2);
        let strand = Strand::new(&pool, "behind its strand");
        let (started, began) = mpsc::channel();
        let (go, wait) = mpsc::channel();
        let first = strand.submit(Class::Visible, Ticket::none(), move |_| {
            started.send(()).unwrap();
            wait.recv().unwrap();
        });
        began.recv().unwrap();
        let second = strand.submit(Class::Visible, Ticket::none(), |_| ());
        // the second job's wait behind the first, however long the box takes: long enough to tell
        // from a worker picking a job up
        let held = std::time::Instant::now();
        wait_for("the second job's wait", || held.elapsed() >= Duration::from_millis(2));
        let waited = held.elapsed();
        go.send(()).unwrap();
        worker_join(first).unwrap();
        worker_join(second).unwrap();
        let report = pool.budget().report_string();
        let line = report
            .lines()
            .find_map(|l| l.strip_prefix("behind an earlier job on its strand: wait p50 / p95 / max  "))
            .unwrap_or_else(|| panic!("the report has no strand waits:\n{report}"));
        let max = line.split(" / ").nth(2).and_then(|m| m.split(',').next()).expect("a max");
        // `human` prints integer nanoseconds, or one decimal place in the larger unit.
        let rounding = match max.trim().split_once(' ').unwrap().1 {
            "ns" => 0.0,
            "us" => 0.05e-6,
            "ms" => 0.05e-3,
            "s" => 0.05,
            other => panic!("unit {other}"),
        };
        assert!(report_seconds(max) + rounding >= waited.as_secs_f64(), "the strand's longest wait is {max}, the job waited {waited:?} behind the first:\n{report}");
        worker_join(pool.shutdown_pool(ShutdownMode::Drain)).unwrap();
    }

    #[test]
    fn a_strand_job_runs_at_the_class_of_a_more_urgent_job_queued_behind_it() {
        let _serial = POOL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
        // Background's share is taken: a background job waits for a token, a visible one has room.
        let pool = test_pool(2);
        let held = HeldTokens::new(pool.budget(), pool.budget().background_limit() as usize, "background share");
        let strand = Strand::new(&pool, "raised");
        let (ran, order) = mpsc::channel();
        let first_ran = ran.clone();
        let first = strand.submit(Class::Background, Ticket::none(), move |_| first_ran.send("background").unwrap());
        wait_for("the background job to wait for a token", || pool.inner.state.lock().unwrap().waiting[2]);
        let second = strand.submit(Class::Visible, Ticket::none(), move |_| ran.send("visible").unwrap());
        assert_eq!(order.recv_timeout(NOTHING_MOVES), Ok("background"), "the background job ahead of a visible one runs at its class");
        assert_eq!(order.recv_timeout(NOTHING_MOVES), Ok("visible"));
        worker_join(first).unwrap();
        worker_join(second).unwrap();
        drop(held);
        worker_join(pool.shutdown_pool(ShutdownMode::Drain)).unwrap();
    }

    /// The job a raised wrapper runs keeps that class: it ran at its own from its first line, and
    /// parked at its first checkpoint for its own class's share.
    #[test]
    fn a_strand_job_admitted_for_a_more_urgent_one_behind_it_runs_at_that_class() {
        let _serial = POOL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
        let pool = test_pool(2);
        let held = HeldTokens::new(pool.budget(), pool.budget().background_limit() as usize, "background share");
        let strand = Strand::new(&pool, "raised");
        let (ran, order) = mpsc::channel();
        let first = strand.submit(Class::Background, Ticket::none(), move |_| ran.send(current_class()).unwrap());
        wait_for("the background job to wait for a token", || pool.inner.state.lock().unwrap().waiting[2]);
        let second = strand.submit(Class::Visible, Ticket::none(), |_| ());
        assert_eq!(order.recv_timeout(NOTHING_MOVES), Ok(Some(Class::Visible)));
        worker_join(first).unwrap();
        worker_join(second).unwrap();
        drop(held);
        worker_join(pool.shutdown_pool(ShutdownMode::Drain)).unwrap();
    }

    /// Why (user): a plate load showed "waiting for a free worker thread" with most cores busy,
    /// behind a background job on the model's strand that had parked at a checkpoint: only
    /// Background's share would let it go on, however urgent what waited behind it.
    #[test]
    fn a_strand_job_parked_at_a_checkpoint_goes_on_for_a_more_urgent_one_behind_it() {
        let _serial = POOL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
        let pool = test_pool(2);
        let strand = Strand::new(&pool, "raised while parked");
        let (started, began) = mpsc::channel();
        let (go, proceed) = mpsc::channel();
        let (ran, order) = mpsc::channel();
        let first_ran = ran.clone();
        let first = strand.submit(Class::Background, Ticket::none(), move |_| {
            started.send(()).unwrap();
            proceed.recv().unwrap();
            let _ = checkpoint();
            first_ran.send(("background job", current_class())).unwrap();
        });
        began.recv().unwrap();
        // Only the job's checkpoint can free a token for the person's work on this budget.
        let budget = pool.budget();
        let held = HeldTokens::new(budget, budget.cores() as usize - 1, "the rest of the machine");
        let (end, person_end) = mpsc::channel::<()>();
        let person_in = Arc::new(AtomicBool::new(false));
        let admitted = person_in.clone();
        let person_budget = budget.clone();
        let person = std::thread::spawn(move || {
            let permit = person_budget.admit(makepad_cpu_budget::INTERACTIVE, makepad_cpu_budget::APP, "a person");
            admitted.store(true, Ordering::Release);
            let _ = person_end.recv();
            person_budget.release(permit);
        });
        let waiting = |class| budget.tokens().waiting.iter().any(|w| w.class == class);
        wait_for("the person's work to wait", || {
            assert!(!person_in.load(Ordering::Acquire), "the person's work came in with every token taken");
            waiting(makepad_cpu_budget::INTERACTIVE)
        });
        go.send(()).unwrap();
        wait_for("the job to park for Background's share and the person to take its token", || {
            assert!(order.try_recv().is_err(), "the job ran on at its checkpoint with a person's work waiting");
            waiting(makepad_cpu_budget::BACKGROUND) && person_in.load(Ordering::Acquire)
        });
        let second = strand.submit(Class::Interactive, Ticket::none(), move |_| ran.send(("load", current_class())).unwrap());
        wait_for("the parked job to wait at Interactive", || {
            assert!(order.try_recv().is_err(), "the raised job ran before a token was free");
            waiting(makepad_cpu_budget::INTERACTIVE)
        });
        // One token comes free that only Interactive may take.
        held.give_back_one();
        assert_eq!(order.recv_timeout(NOTHING_MOVES), Ok(("background job", Some(Class::Interactive))), "the parked job must resume at Interactive");
        assert_eq!(order.recv_timeout(NOTHING_MOVES), Ok(("load", Some(Class::Interactive))));
        drop(end);
        person.join().unwrap();
        drop(held);
        worker_join(first).unwrap();
        worker_join(second).unwrap();
        worker_join(pool.shutdown_pool(ShutdownMode::Drain)).unwrap();
    }

    /// A job that has started and runs on takes the class at its next checkpoint: it holds its token
    /// through whatever a higher class leaves behind it.
    #[test]
    fn a_strand_job_on_its_way_takes_the_class_of_a_more_urgent_one_behind_it() {
        let _serial = POOL_TESTS.lock().unwrap_or_else(|p| p.into_inner());
        let pool = test_pool(2);
        let strand = Strand::new(&pool, "raised while it runs");
        let (started, began) = mpsc::channel();
        let budget = pool.budget().clone();
        let go = Arc::new(AtomicBool::new(false));
        let (ran, order) = mpsc::channel();
        let first_go = go.clone();
        let first = strand.submit(Class::Background, Ticket::none(), move |_| {
            started.send(current_class()).unwrap();
            wait_for("the test's go", || first_go.load(Ordering::Acquire));
            let _ = checkpoint();
            ran.send((current_class(), budget.tokens().holders.iter().any(|h| h.label == "raised while it runs" && h.class == makepad_cpu_budget::VISIBLE))).unwrap();
        });
        assert_eq!(began.recv().unwrap(), Some(Class::Background));
        let second = strand.submit(Class::Visible, Ticket::none(), |_| ());
        go.store(true, Ordering::Release);
        assert_eq!(order.recv_timeout(NOTHING_MOVES), Ok((Some(Class::Visible), true)));
        worker_join(first).unwrap();
        worker_join(second).unwrap();
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
