// CSG execution helpers. Standalone clients may use the legacy helping pool.
// Platform-owned jobs enter with_serial: every recursive helper then runs on
// that worker and never initializes or submits into an independent pool.

use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[derive(Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self { Self::default() }
    /// A token raised with a flag its owner already keeps (a job's cancel flag).
    pub fn of_flag(flag: Arc<AtomicBool>) -> Self { Self(flag) }
    pub fn cancel(&self) { self.0.store(true, Ordering::Release); }
    pub fn is_cancelled(&self) -> bool { self.0.load(Ordering::Acquire) }
}

thread_local! {
    static CURRENT_CANCEL: RefCell<Option<CancelToken>> = const { RefCell::new(None) };
    static SERIAL_EXECUTION: Cell<bool> = const { Cell::new(false) };
}

#[cfg(feature = "threads")]
fn inherited_cancel() -> Option<CancelToken> {
    CURRENT_CANCEL.with(|slot| slot.borrow().clone())
}

struct CancelScope(Option<CancelToken>);
impl Drop for CancelScope {
    fn drop(&mut self) {
        CURRENT_CANCEL.with(|slot| { slot.replace(self.0.take()); });
    }
}

fn run_with_inherited_cancel<R>(cancel: Option<CancelToken>, f: impl FnOnce() -> R) -> R {
    let _scope = CancelScope(CURRENT_CANCEL.with(|slot| slot.replace(cancel)));
    f()
}

/// Install cancellation for this worker, restoring its previous token even
/// when an operation unwinds. This contract is identical without `threads`.
pub fn with_cancel<R>(token: &CancelToken, f: impl FnOnce() -> R) -> R {
    run_with_inherited_cancel(Some(token.clone()), f)
}

#[inline]
pub fn cancelled() -> bool {
    CURRENT_CANCEL.with(|slot| slot.borrow().as_ref().is_some_and(CancelToken::is_cancelled))
}

struct SerialScope(bool);
impl Drop for SerialScope {
    fn drop(&mut self) { SERIAL_EXECUTION.with(|slot| slot.set(self.0)); }
}

/// Keep all nested CSG work on the calling worker. Must be entered on an
/// existing executor worker, never around heavy evaluation on the UI.
pub fn with_serial<R>(f: impl FnOnce() -> R) -> R {
    let _scope = SerialScope(SERIAL_EXECUTION.with(|slot| slot.replace(true)));
    f()
}

#[cfg(any(feature = "threads", test))]
fn serial_execution() -> bool { SERIAL_EXECUTION.with(Cell::get) }

#[cfg(feature = "threads")]
mod inner {
    use std::collections::VecDeque;
    use std::sync::{mpsc, Arc, Condvar, Mutex, OnceLock};
    use std::thread;

    type Job = Box<dyn FnOnce() + Send>;

    use super::{inherited_cancel, run_with_inherited_cancel, serial_execution};

    struct Jobs {
        queue: VecDeque<Job>,
        /// [`shutdown`] ran: the workers end once the queue is empty, new work runs on the caller.
        stopped: bool,
        /// callers parked for a result ([`receive_helping`])
        waiting: usize,
    }

    struct Queue {
        jobs: Mutex<Jobs>,
        /// the workers park on it for the next job
        ready: Condvar,
        /// a caller parks on it for its result or a job to help with ([`receive_helping`])
        helpers: Condvar,
    }

    impl Queue {
        /// Queues `job`; hands it back when the pool has shut down.
        fn push(&self, job: Job) -> Result<(), Job> {
            let mut jobs = self.jobs.lock().unwrap();
            if jobs.stopped {
                return Err(job);
            }
            jobs.queue.push_back(job);
            self.ready.notify_one();
            // a parked caller helps where every worker is busy: a worker waiting on its own nested
            // work waits on jobs only a caller may be free to run
            if jobs.waiting > 0 {
                self.helpers.notify_one();
            }
            Ok(())
        }

        /// A job ended: the callers parked for a result look again.
        fn finished(&self) {
            if self.jobs.lock().unwrap().waiting > 0 {
                self.helpers.notify_all();
            }
        }

        /// The next job; `None` once the pool shut down and the queue is drained.
        fn pop(&self) -> Option<Job> {
            let mut jobs = self.jobs.lock().unwrap();
            loop {
                if let Some(job) = jobs.queue.pop_front() {
                    return Some(job);
                }
                if jobs.stopped {
                    return None;
                }
                jobs = self.ready.wait(jobs).unwrap();
            }
        }

        fn stop(&self) {
            self.jobs.lock().unwrap().stopped = true;
            self.ready.notify_all();
            self.helpers.notify_all();
        }

        fn stopped(&self) -> bool {
            self.jobs.lock().unwrap().stopped
        }
    }

    struct Pool {
        queue: Arc<Queue>,
        size: usize,
    }

    impl Pool {
        fn new(size: usize) -> Pool {
            let queue = Arc::new(Queue {
                jobs: Mutex::new(Jobs { queue: VecDeque::new(), stopped: false, waiting: 0 }),
                ready: Condvar::new(),
                helpers: Condvar::new(),
            });
            for index in 0..size {
                let queue = queue.clone();
                thread::Builder::new()
                    .name(format!("makepad-csg-{index}"))
                    .spawn(move || {
                        while let Some(job) = queue.pop() {
                            // A job's panic ends the job, never the worker: the pool keeps its size.
                            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job));
                        }
                    })
                    .expect("spawn CSG worker");
            }
            Pool { queue, size }
        }

        fn submit(&self, f: impl FnOnce() + Send + 'static) {
            let cancel = inherited_cancel();
            let queue = self.queue.clone();
            let job: Job = Box::new(move || {
                // (on unwind too: a caller parked for its result looks again)
                let _finished = Finished(&queue);
                run_with_inherited_cancel(cancel, f)
            });
            if let Err(job) = self.queue.push(job) {
                job();
            }
        }
    }

    struct Finished<'a>(&'a Queue);

    impl Drop for Finished<'_> {
        fn drop(&mut self) {
            self.0.finished();
        }
    }

    static POOL: OnceLock<Pool> = OnceLock::new();

    /// The pool: a long-lived pool's cores less two workers, the machine's unless its run was given
    /// fewer ([`makepad_cpu_budget::pool_cores`]: `MAKEPAD_POOL_CORES`, else `MAKEPAD_CPU_CORES`), as
    /// the platform's app pool sizes. It took the machine's alone: 14 workers in a test given 4
    /// cores of a 16-core box, beside the app's pool and the JIT's workers each the box's size, and
    /// the box stood at load 57 (11-18).
    fn get_pool() -> &'static Pool {
        POOL.get_or_init(|| Pool::new((makepad_cpu_budget::pool_cores() as usize).saturating_sub(2).max(1)))
    }

    /// Ends the pool's workers once the queued work is done (the app is shutting down: under
    /// `makepad run` its domain waits for every thread it started). Later work runs on the caller.
    pub fn shutdown() {
        if let Some(pool) = POOL.get() {
            pool.queue.stop();
        }
    }

    /// No pool to hand work to: serial, or the pool shut down.
    fn inline() -> bool {
        serial_execution() || POOL.get().is_some_and(|p| p.queue.stopped())
    }

    pub fn thread_count() -> usize {
        if inline() { 1 } else { get_pool().size }
    }

    /// Queue one top-level task on the shared pool.
    pub fn spawn(f: impl FnOnce() + Send + 'static) {
        if inline() { f() } else { get_pool().submit(f); }
    }

    /// Times a caller looked for its result ([`receive_helping`]), for the test that it parks.
    #[cfg(test)]
    pub(super) static LOOKS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    /// A caller's wait on a job a worker already runs, `ms` long: nothing queued to help with.
    #[cfg(test)]
    pub(super) fn wait_on_a_running_job(ms: u64) -> usize {
        let (started_tx, started) = mpsc::channel();
        let (tx, rx) = mpsc::channel();
        get_pool().submit(move || {
            let _ = started_tx.send(());
            thread::sleep(std::time::Duration::from_millis(ms));
            let _ = tx.send(1);
        });
        started.recv().unwrap();
        receive_helping(rx)
    }

    /// A caller's wait for its result. It runs the queued jobs meanwhile (a worker waiting on its
    /// own nested work could wait on jobs no free worker is left to run) and parks while there are
    /// none, until a job ends or another is queued. It spun on `yield_now` before: every waiting
    /// caller stayed runnable and took a core from the work it waited on, on every machine (a live
    /// Fab test had all 14 of its CSG workers on CPU, 11-18).
    fn receive_helping<R>(rx: mpsc::Receiver<R>) -> R {
        let queue = &get_pool().queue;
        let take = |rx: &mpsc::Receiver<R>| match rx.try_recv() {
            Ok(value) => Some(value),
            Err(mpsc::TryRecvError::Disconnected) => panic!("CSG pool task failed"),
            Err(mpsc::TryRecvError::Empty) => None,
        };
        loop {
            #[cfg(test)]
            LOOKS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if let Some(value) = take(&rx) {
                return value;
            }
            let mut jobs = queue.jobs.lock().unwrap();
            if let Some(job) = jobs.queue.pop_front() {
                drop(jobs);
                job();
                continue;
            }
            // (looked at again under the lock: a job that ends after this says so to a parked caller)
            if let Some(value) = take(&rx) {
                return value;
            }
            jobs.waiting += 1;
            let mut jobs = queue.helpers.wait(jobs).unwrap();
            jobs.waiting -= 1;
        }
    }

    pub fn parallel_do2<A, B, FA, FB>(fa: FA, fb: FB) -> (A, B)
    where
        A: Send + 'static,
        B: Send + 'static,
        FA: FnOnce() -> A + Send + 'static,
        FB: FnOnce() -> B + Send + 'static,
    {
        if inline() || get_pool().size < 2 {
            return (fa(), fb());
        }
        let (tx_a, rx_a) = mpsc::channel();
        let (tx_b, rx_b) = mpsc::channel();
        get_pool().submit(move || {
            let _ = tx_a.send(std::panic::catch_unwind(std::panic::AssertUnwindSafe(fa)));
        });
        get_pool().submit(move || {
            let _ = tx_b.send(std::panic::catch_unwind(std::panic::AssertUnwindSafe(fb)));
        });
        (
            receive_helping(rx_a).unwrap_or_else(|panic| std::panic::resume_unwind(panic)),
            receive_helping(rx_b).unwrap_or_else(|panic| std::panic::resume_unwind(panic)),
        )
    }

    pub fn parallel_do8<R, F>(tasks: [F; 8]) -> [R; 8]
    where
        R: Send + 'static,
        F: FnOnce() -> R + Send + 'static,
    {
        let mut out = parallel_for(tasks.into_iter().collect());
        std::array::from_fn(|_| out.remove(0))
    }

    pub fn parallel_for<R, F>(tasks: Vec<F>) -> Vec<R>
    where
        R: Send + 'static,
        F: FnOnce() -> R + Send + 'static,
    {
        if inline() || get_pool().size < 2 || tasks.len() < 2 {
            return tasks.into_iter().map(|f| f()).collect();
        }
        let mut receivers = Vec::with_capacity(tasks.len());
        for task in tasks {
            let (tx, rx) = mpsc::channel();
            receivers.push(rx);
            get_pool().submit(move || {
                let _ = tx.send(std::panic::catch_unwind(std::panic::AssertUnwindSafe(task)));
            });
        }
        receivers.into_iter().map(|rx| {
            receive_helping(rx).unwrap_or_else(|panic| std::panic::resume_unwind(panic))
        }).collect()
    }

    pub fn parallel_map<T, R, F>(data: &[T], f: F) -> Vec<R>
    where
        T: Send + Sync + Copy + 'static,
        R: Send + 'static,
        F: Fn(&[T]) -> Vec<R> + Send + Clone + 'static,
    {
        let n = data.len();
        if n == 0 {
            return Vec::new();
        }
        let chunks = thread_count().min(n);
        if chunks <= 1 {
            return f(data);
        }
        let chunk_size = (n + chunks - 1) / chunks;
        let shared: Arc<[T]> = data.to_vec().into();
        let tasks = (0..chunks)
            .filter_map(|i| {
                let start = i * chunk_size;
                let end = (start + chunk_size).min(n);
                (start < end).then(|| {
                    let shared = shared.clone();
                    let f = f.clone();
                    move || f(&shared[start..end])
                })
            })
            .collect();
        parallel_for(tasks).into_iter().flatten().collect()
    }
}

#[cfg(not(feature = "threads"))]
mod inner {
    pub fn shutdown() {}
    pub fn thread_count() -> usize { 1 }
    pub fn spawn(f: impl FnOnce() + Send + 'static) { f() }
    pub fn parallel_do2<A, B, FA, FB>(fa: FA, fb: FB) -> (A, B)
    where A: Send + 'static, B: Send + 'static, FA: FnOnce() -> A + Send + 'static, FB: FnOnce() -> B + Send + 'static { (fa(), fb()) }
    pub fn parallel_do8<R, F>(tasks: [F; 8]) -> [R; 8]
    where R: Send + 'static, F: FnOnce() -> R + Send + 'static { tasks.map(|f| f()) }
    pub fn parallel_for<R, F>(tasks: Vec<F>) -> Vec<R>
    where R: Send + 'static, F: FnOnce() -> R + Send + 'static { tasks.into_iter().map(|f| f()).collect() }
    pub fn parallel_map<T, R, F>(data: &[T], f: F) -> Vec<R>
    where T: Send + Sync + Copy + 'static, R: Send + 'static, F: Fn(&[T]) -> Vec<R> + Send + Clone + 'static { f(data) }
}

pub use inner::{
    parallel_do2, parallel_do8, parallel_for, parallel_map, shutdown, spawn, thread_count,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_panicking_task_leaves_the_pool_at_full_size() {
        if thread_count() < 2 {
            return;
        }
        std::panic::set_hook(Box::new(|_| {}));
        let failed_for = std::panic::catch_unwind(|| parallel_for(
            (0..2 * thread_count())
                .map(|i| move || if i % 2 == 0 { panic!("pool task {i}") } else { i })
                .collect::<Vec<_>>(),
        ));
        let failed_do2 = std::panic::catch_unwind(|| parallel_do2(|| panic!("pool task a"), || 1));

        let task_count = thread_count() + 1;
        let arrived = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let results = parallel_for((0..task_count).map(|i| {
            let arrived = Arc::clone(&arrived);
            move || {
                arrived.fetch_add(1, Ordering::SeqCst);
                while arrived.load(Ordering::SeqCst) != task_count {
                    for _ in 0..16 {
                        std::hint::spin_loop();
                    }
                    std::thread::yield_now();
                }
                i
            }
        }).collect());
        // All queued panicking tasks have finished once the rendezvous completes.
        let _ = std::panic::take_hook();

        assert!(
            failed_for.err().is_some_and(|panic| {
                panic.downcast_ref::<String>().is_some_and(|message| message.starts_with("pool task"))
            }),
            "a task's own panic reaches the caller",
        );
        assert!(
            failed_do2.err().is_some_and(|panic| {
                panic.downcast_ref::<&str>().is_some_and(|message| *message == "pool task a")
            }),
            "a task's own panic reaches the caller",
        );
        assert_eq!(
            results, (0..task_count).collect::<Vec<_>>(),
            "after panics every worker is still there",
        );
    }

    /// A caller waiting on its result parks while no job is queued: it spun on `yield_now` before,
    /// runnable the whole wait, which took a core from the work it waited on (11-18).
    #[cfg(feature = "threads")]
    #[test]
    fn a_waiting_caller_parks_until_its_result() {
        let looks = || super::inner::LOOKS.load(Ordering::Relaxed);
        let before = looks();
        assert_eq!(super::inner::wait_on_a_running_job(50), 1);
        let looked = looks() - before;
        // (other tests' callers share the count: a few looks each; a spinning one looked
        // hundreds of thousands of times in 50 ms)
        assert!(looked < 1_000, "the caller looked {looked} times in a 50 ms wait");
    }

    #[test]
    fn serial_scope_keeps_nested_helpers_on_the_calling_worker() {
        let owner = std::thread::current().id();
        with_serial(|| {
            assert_eq!(thread_count(), 1);
            let check = move || {
                assert_eq!(std::thread::current().id(), owner);
                assert_eq!(thread_count(), 1);
                parallel_do2(|| 3, || 4)
            };
            assert_eq!(parallel_do2(check, check), ((3, 4), (3, 4)));
            assert_eq!(parallel_for(vec![check, check]).len(), 2);
            assert_eq!(parallel_map(&[1, 2, 3], |values| values.to_vec()), [1, 2, 3]);
            spawn(move || assert_eq!(std::thread::current().id(), owner));
        });
    }

    #[test]
    fn cancellation_and_serial_scopes_restore_after_unwind() {
        assert!(!serial_execution());
        let outer = CancelToken::new();
        let inner = CancelToken::new();
        inner.cancel();
        with_cancel(&outer, || {
            let failed = std::panic::catch_unwind(|| with_serial(|| with_cancel(&inner, || {
                assert!(cancelled());
                panic!("operation failed");
            })));
            assert!(failed.is_err());
            assert!(!serial_execution());
            assert!(!cancelled());
            outer.cancel();
            with_serial(|| parallel_do2(|| assert!(cancelled()), || assert!(cancelled())));
        });
        assert!(!cancelled());
    }
}
