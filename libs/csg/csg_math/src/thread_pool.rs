// Shared helping thread pool for CSG evaluation and polygonal booleans.
//
// There is deliberately ONE pool. LocalGen jobs enter it through `spawn`,
// and recursive boolean subtrees use the parallel helpers below. A worker
// waiting for children helps run queued work, so nesting cannot deadlock even
// when every worker is occupied by a top-level generation job.

#[cfg(feature = "threads")]
mod inner {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{mpsc, Arc, Condvar, Mutex, OnceLock};
    use std::thread;

    type Job = Box<dyn FnOnce() + Send>;

    #[derive(Clone, Default)]
    pub struct CancelToken(Arc<AtomicBool>);

    impl CancelToken {
        pub fn new() -> Self {
            Self::default()
        }

        pub fn cancel(&self) {
            self.0.store(true, Ordering::Relaxed);
        }

        pub fn is_cancelled(&self) -> bool {
            self.0.load(Ordering::Relaxed)
        }
    }

    thread_local! {
        static CURRENT_CANCEL: RefCell<Option<CancelToken>> = const { RefCell::new(None) };
    }

    fn inherited_cancel() -> Option<CancelToken> {
        CURRENT_CANCEL.with(|slot| slot.borrow().clone())
    }

    fn run_with_inherited_cancel(cancel: Option<CancelToken>, f: impl FnOnce()) {
        CURRENT_CANCEL.with(|slot| {
            let previous = slot.replace(cancel);
            f();
            slot.replace(previous);
        });
    }

    pub fn with_cancel<R>(token: &CancelToken, f: impl FnOnce() -> R) -> R {
        CURRENT_CANCEL.with(|slot| {
            let previous = slot.replace(Some(token.clone()));
            let out = f();
            slot.replace(previous);
            out
        })
    }

    #[inline]
    pub fn cancelled() -> bool {
        CURRENT_CANCEL.with(|slot| {
            slot.borrow().as_ref().is_some_and(CancelToken::is_cancelled)
        })
    }

    struct Queue {
        jobs: Mutex<VecDeque<Job>>,
        ready: Condvar,
    }

    impl Queue {
        fn push(&self, job: Job) {
            self.jobs.lock().unwrap().push_back(job);
            self.ready.notify_one();
        }

        fn try_pop(&self) -> Option<Job> {
            self.jobs.lock().unwrap().pop_front()
        }

        fn pop(&self) -> Job {
            let mut jobs = self.jobs.lock().unwrap();
            loop {
                if let Some(job) = jobs.pop_front() {
                    return job;
                }
                jobs = self.ready.wait(jobs).unwrap();
            }
        }
    }

    struct Pool {
        queue: Arc<Queue>,
        size: usize,
    }

    impl Pool {
        fn new(size: usize) -> Pool {
            let queue = Arc::new(Queue {
                jobs: Mutex::new(VecDeque::new()),
                ready: Condvar::new(),
            });
            for index in 0..size {
                let queue = queue.clone();
                thread::Builder::new()
                    .name(format!("makepad-csg-{index}"))
                    .spawn(move || loop {
                        queue.pop()();
                    })
                    .expect("spawn CSG worker");
            }
            Pool { queue, size }
        }

        fn submit(&self, f: impl FnOnce() + Send + 'static) {
            let cancel = inherited_cancel();
            self.queue.push(Box::new(move || run_with_inherited_cancel(cancel, f)));
        }

        fn help_one(&self) -> bool {
            match self.queue.try_pop() {
                Some(job) => {
                    job();
                    true
                }
                None => false,
            }
        }
    }

    static POOL: OnceLock<Pool> = OnceLock::new();

    fn get_pool() -> &'static Pool {
        POOL.get_or_init(|| {
            let cpus = thread::available_parallelism().map(|p| p.get()).unwrap_or(1);
            Pool::new(cpus.saturating_sub(2).max(1))
        })
    }

    pub fn thread_count() -> usize {
        get_pool().size
    }

    /// Queue one top-level task on the shared pool.
    pub fn spawn(f: impl FnOnce() + Send + 'static) {
        get_pool().submit(f);
    }

    fn receive_helping<R>(rx: mpsc::Receiver<R>) -> R {
        let pool = get_pool();
        loop {
            match rx.try_recv() {
                Ok(value) => return value,
                Err(mpsc::TryRecvError::Disconnected) => panic!("CSG pool task failed"),
                Err(mpsc::TryRecvError::Empty) => {
                    if !pool.help_one() {
                        thread::yield_now();
                    }
                }
            }
        }
    }

    pub fn parallel_do2<A, B, FA, FB>(fa: FA, fb: FB) -> (A, B)
    where
        A: Send + 'static,
        B: Send + 'static,
        FA: FnOnce() -> A + Send + 'static,
        FB: FnOnce() -> B + Send + 'static,
    {
        if get_pool().size < 2 {
            return (fa(), fb());
        }
        let (tx_a, rx_a) = mpsc::channel();
        let (tx_b, rx_b) = mpsc::channel();
        get_pool().submit(move || {
            let _ = tx_a.send(fa());
        });
        get_pool().submit(move || {
            let _ = tx_b.send(fb());
        });
        (receive_helping(rx_a), receive_helping(rx_b))
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
        if get_pool().size < 2 || tasks.len() < 2 {
            return tasks.into_iter().map(|f| f()).collect();
        }
        let mut receivers = Vec::with_capacity(tasks.len());
        for task in tasks {
            let (tx, rx) = mpsc::channel();
            receivers.push(rx);
            get_pool().submit(move || {
                let _ = tx.send(task());
            });
        }
        receivers.into_iter().map(receive_helping).collect()
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
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    #[derive(Clone, Default)]
    pub struct CancelToken(Arc<AtomicBool>);
    impl CancelToken {
        pub fn new() -> Self { Self::default() }
        pub fn cancel(&self) { self.0.store(true, Ordering::Relaxed); }
        pub fn is_cancelled(&self) -> bool { self.0.load(Ordering::Relaxed) }
    }
    pub fn with_cancel<R>(_: &CancelToken, f: impl FnOnce() -> R) -> R { f() }
    pub fn cancelled() -> bool { false }
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
    cancelled, parallel_do2, parallel_do8, parallel_for, parallel_map, spawn, thread_count,
    with_cancel, CancelToken,
};
