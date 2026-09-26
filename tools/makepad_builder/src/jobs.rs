//! Installing components side by side.
//!
//! [`run`] starts one thread per component (Build tools, Windows SDK, Rust,
//! CUDA), one shared unpack pool sized to the machine and the download
//! threads (see `fetch`). Components hand CPU work to the pool with
//! [`spawn`] and downloads to `fetch`; neither starts threads of its own, so
//! the cores are never oversubscribed however many components run.
//!
//! Progress is per thread (see `progress`): each component thread and every
//! task on its behalf forwards its events over one channel to the thread
//! that called [`run`], which hands them to its own hook (the work page).
use crate::{fetch, progress, timing};
use std::{
    cell::RefCell,
    collections::VecDeque,
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Condvar, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

/// One component to install: its row label and its work.
pub struct Job<'a> {
    pub label: &'static str,
    pub run: Box<dyn FnOnce() -> Result<(), String> + Send + 'a>,
}
impl<'a> Job<'a> {
    pub fn new(label: &'static str, run: impl FnOnce() -> Result<(), String> + Send + 'a) -> Self {
        Job { label, run: Box::new(run) }
    }
}

/// What the threads of one [`run`] share.
pub struct Env {
    pool: Arc<Pool>,
    pub(crate) fetch: Arc<fetch::Fetcher>,
    pub(crate) claims: crate::extract::Claims,
    cancel: AtomicBool,
}
thread_local! {
    static ENV: RefCell<Option<Arc<Env>>> = const { RefCell::new(None) };
}
pub(crate) fn env() -> Option<Arc<Env>> {
    ENV.with(|e| e.borrow().clone())
}
/// True once another component failed: start nothing new, stop cleanly.
pub fn cancelled() -> bool {
    env().is_some_and(|e| e.cancel.load(Ordering::Relaxed))
}
pub fn check_cancelled() -> Result<(), String> {
    if cancelled() { Err(CANCELLED.into()) } else { Ok(()) }
}
const CANCELLED: &str = "stopped: another component failed";

/// Everything a thread's work depends on, to continue it on another thread.
#[derive(Clone)]
pub struct Ctx {
    progress: progress::Ctx,
    timing: String,
    env: Option<Arc<Env>>,
}
impl Ctx {
    pub fn current() -> Ctx {
        Ctx { progress: progress::Ctx::current(), timing: timing::current(), env: env() }
    }
    pub fn enter<R>(&self, f: impl FnOnce() -> R) -> R {
        let previous = ENV.with(|e| e.replace(self.env.clone()));
        struct Restore(Option<Arc<Env>>);
        impl Drop for Restore {
            fn drop(&mut self) {
                ENV.with(|e| *e.borrow_mut() = self.0.take());
            }
        }
        let _restore = Restore(previous);
        let _timing = timing::adopt(&self.timing);
        self.progress.enter(f)
    }
}

/// The result of work running elsewhere.
pub struct Pending<R> {
    rx: mpsc::Receiver<Result<R, String>>,
}
impl<R> Pending<R> {
    pub fn channel() -> (mpsc::Sender<Result<R, String>>, Pending<R>) {
        let (tx, rx) = mpsc::channel();
        (tx, Pending { rx })
    }
    pub fn ready(value: Result<R, String>) -> Pending<R> {
        let (tx, pending) = Pending::channel();
        let _ = tx.send(value);
        pending
    }
    /// Wait for the result. A pool thread runs queued tasks meanwhile, so
    /// tasks may wait for tasks they spawned without starving the pool.
    pub fn wait(self) -> Result<R, String> {
        let pool = if IN_POOL.with(|p| p.get()) { env().map(|e| e.pool.clone()) } else { None };
        let Some(pool) = pool else {
            return self.rx.recv().unwrap_or_else(|_| Err("a worker stopped without a result".into()));
        };
        loop {
            match self.rx.try_recv() {
                Ok(result) => return result,
                Err(mpsc::TryRecvError::Disconnected) => return Err("a worker stopped without a result".into()),
                Err(mpsc::TryRecvError::Empty) => {}
            }
            match pool.try_pop() {
                Some(task) => task(),
                None => match self.rx.recv_timeout(Duration::from_millis(5)) {
                    Ok(result) => return result,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => return Err("a worker stopped without a result".into()),
                },
            }
        }
    }
}
/// Wait for all, returning the first error after every one has finished
/// (nothing may still use what the caller borrowed to them).
pub fn wait_all<R>(pending: impl IntoIterator<Item = Pending<R>>) -> Result<Vec<R>, String> {
    let mut out = Vec::new();
    let mut error = None;
    for p in pending {
        match p.wait() {
            Ok(r) => out.push(r),
            Err(e) => {
                if error.is_none() || error.as_deref() == Some(CANCELLED) {
                    error = Some(e);
                }
            }
        }
    }
    match error {
        Some(e) => Err(e),
        None => Ok(out),
    }
}

type Task = Box<dyn FnOnce() + Send>;
/// The unpack pool: decompressing and writing, one thread per core less one
/// (the download threads and the terminal stay responsive).
pub(crate) struct Pool {
    queue: Mutex<(VecDeque<Task>, bool)>,
    ready: Condvar,
}
thread_local! {
    static IN_POOL: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
impl Pool {
    fn push(&self, task: Task) {
        self.queue.lock().unwrap_or_else(|e| e.into_inner()).0.push_back(task);
        self.ready.notify_one();
    }
    fn try_pop(&self) -> Option<Task> {
        self.queue.lock().unwrap_or_else(|e| e.into_inner()).0.pop_front()
    }
    fn close(&self) {
        self.queue.lock().unwrap_or_else(|e| e.into_inner()).1 = true;
        self.ready.notify_all();
    }
    fn work(&self) {
        IN_POOL.with(|p| p.set(true));
        loop {
            let task = {
                let mut queue = self.queue.lock().unwrap_or_else(|e| e.into_inner());
                loop {
                    if let Some(task) = queue.0.pop_front() {
                        break Some(task);
                    }
                    if queue.1 {
                        break None;
                    }
                    queue = self.ready.wait(queue).unwrap_or_else(|e| e.into_inner());
                }
            };
            match task {
                Some(task) => task(),
                None => return,
            }
        }
    }
}
/// Pool size: `MAKEPAD_BUILDER_UNPACK_THREADS`, else one per logical core
/// less one.
pub fn pool_threads() -> usize {
    std::env::var("MAKEPAD_BUILDER_UNPACK_THREADS").ok().and_then(|s| s.parse().ok()).filter(|n| *n > 0)
        .unwrap_or_else(|| thread::available_parallelism().map_or(4, |n| n.get()).saturating_sub(1).max(1))
}

/// Run `f` on the unpack pool, in this thread's context. Without a pool
/// (outside [`run`]) it runs right here.
pub fn spawn<R: Send + 'static>(f: impl FnOnce() -> Result<R, String> + Send + 'static) -> Pending<R> {
    let Some(env) = env() else {
        return Pending::ready(f());
    };
    let ctx = Ctx::current();
    let (tx, pending) = Pending::channel();
    env.pool.push(Box::new(move || {
        let result = ctx.enter(|| catch_unwind(AssertUnwindSafe(f)));
        let _ = tx.send(result.unwrap_or_else(|panic| Err(panic_message(panic))));
    }));
    pending
}
pub(crate) fn panic_message(panic: Box<dyn std::any::Any + Send>) -> String {
    let text = panic.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| panic.downcast_ref::<String>().cloned());
    format!("internal error: {}", text.unwrap_or_else(|| "worker panicked".into()))
}

enum Event {
    Progress(progress::Progress),
    Done(&'static str, Result<(), String>, Duration),
}

/// Install `jobs` side by side and wait for all of them. Their progress
/// reaches this thread's hook; each finished one is reported as a "Ready"
/// or "Failed" event under its label.
///
/// A real failure stops the others at their next payload (downloads land
/// in the cache and unpacked payloads stay whole); the error names the
/// component. A compiler that Windows security software still holds does
/// not stop the others, and is returned (unchanged, so the caller can offer
/// the retry) only when nothing else failed.
pub fn run(jobs: Vec<Job<'_>>) -> Result<(), String> {
    if jobs.is_empty() {
        return Ok(());
    }
    let pool = Arc::new(Pool { queue: Mutex::new((VecDeque::new(), false)), ready: Condvar::new() });
    let fetch = Arc::new(fetch::Fetcher::new());
    let env = Arc::new(Env { pool: pool.clone(), fetch: fetch.clone(), claims: Default::default(), cancel: AtomicBool::new(false) });
    let (tx, rx) = mpsc::channel::<Event>();
    let labels: Vec<&'static str> = jobs.iter().map(|j| j.label).collect();
    fetch.expect(&labels);
    let mut results: Vec<(&'static str, Result<(), String>)> = Vec::new();
    thread::scope(|s| {
        for _ in 0..pool_threads() {
            let pool = pool.clone();
            s.spawn(move || pool.work());
        }
        for _ in 0..fetch.threads() {
            let fetch = fetch.clone();
            s.spawn(move || fetch.work());
        }
        for job in jobs {
            let tx = tx.clone();
            let env = env.clone();
            s.spawn(move || {
                let forward = tx.clone();
                let label = job.label;
                let hook: progress::Forward = Arc::new(move |mut p: progress::Progress| {
                    p.row = label.into();
                    let _ = forward.send(Event::Progress(p));
                });
                let start = Instant::now();
                let ctx = Ctx { progress: progress::Ctx::default(), timing: String::new(), env: Some(env.clone()) };
                let result = ctx.enter(|| {
                    progress::forward(hook, || {
                        let _timing = timing::component(label);
                        catch_unwind(AssertUnwindSafe(job.run)).unwrap_or_else(|panic| Err(panic_message(panic)))
                    })
                });
                env.fetch.ready(label);
                if let Err(error) = &result {
                    if !crate::rustc::needs_compiler_retry(error) && error != CANCELLED {
                        env.cancel.store(true, Ordering::Relaxed);
                    }
                }
                let _ = tx.send(Event::Done(label, result, start.elapsed()));
            });
        }
        drop(tx);
        let mut open = labels.len();
        while open > 0 {
            let Ok(event) = rx.recv() else { break };
            match event {
                Event::Progress(p) => progress::deliver(p),
                Event::Done(label, result, took) => {
                    open -= 1;
                    let (stage, detail) = match &result {
                        Ok(()) => (DONE, format!("{:.1}", took.as_secs_f64())),
                        Err(e) if e == CANCELLED => (STOPPED, e.clone()),
                        Err(e) if crate::rustc::needs_compiler_retry(e) => (WAITING, e.clone()),
                        Err(e) => (FAILED, e.clone()),
                    };
                    progress::deliver(finished(label, stage, &detail));
                    results.push((label, result));
                }
            }
        }
        // Component threads are done; the pool and downloads have no more work.
        pool.close();
        fetch.close();
    });
    for line in fetch.report() {
        timing::note(line);
    }
    let (overlaps, examples) = env.claims.overlaps();
    timing::note(format!("timing: unpack pool {} threads, {} connections, {overlaps} paths written by more than one payload",
        pool_threads(), fetch.threads()));
    for example in examples {
        timing::note(format!("timing:   twice: {example}"));
    }
    let mut retry = None;
    let mut stopped = None;
    for label in &labels {
        let Some((_, result)) = results.iter().find(|(l, _)| l == label) else { continue };
        match result {
            Ok(()) => {}
            Err(e) if e == CANCELLED => stopped = stopped.or(Some(format!("{label}: {e}"))),
            Err(e) if crate::rustc::needs_compiler_retry(e) => retry = retry.or(Some(e.clone())),
            Err(e) => return Err(format!("{label}: {e}")),
        }
    }
    match (retry, stopped) {
        (Some(e), _) => Err(e),
        (None, Some(e)) => Err(e),
        (None, None) => Ok(()),
    }
}

/// Stages of the event that closes a component's row: done (its detail is
/// the seconds it took), failed, stopped for another's failure, or waiting
/// for security software (the error, for the retry).
pub const DONE: &str = "Component done";
pub const FAILED: &str = "Component failed";
pub const STOPPED: &str = "Component stopped";
pub const WAITING: &str = "Component waiting";
fn finished(label: &str, stage: &str, detail: &str) -> progress::Progress {
    progress::Progress {
        stage: stage.into(),
        detail: detail.into(),
        loaded: 0,
        total: 0,
        frac: 1.0,
        unit: progress::Unit::None,
        package: progress::Package::default(),
        overall: None,
        row: label.into(),
    }
}
