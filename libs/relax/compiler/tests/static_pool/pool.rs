// The program of domain_tests.rs's static_task_pool_relaunches_in_place_without_live_threads: an app
// that keeps a task pool's handle in a `static OnceLock`, as a Makepad app keeps its Cx pool, and
// ends its workers on stop. The pool has the shape of makepad-platform's (named workers that run
// each job on a CPU budget permit and park while idle; a runtime whose close closes every pool it
// made, statics' included), trimmed to what the app drives. The test puts the CPU budget's source
// (makepad-cpu-budget) before this as `mod makepad_cpu_budget`.

mod pool {
    use crate::makepad_cpu_budget;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex, OnceLock, Weak};

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Class {
        Background,
        Visible,
    }
    impl Class {
        /// The budget's class, which is also the queue's index: the more urgent class comes first.
        fn budget(self) -> u8 {
            match self {
                Self::Visible => makepad_cpu_budget::VISIBLE,
                Self::Background => makepad_cpu_budget::BACKGROUND,
            }
        }
    }

    #[derive(Debug)]
    pub enum SubmitError {
        Closed,
    }
    #[derive(Debug)]
    pub enum SpawnError {
        RuntimeClosed,
        Backend,
    }

    /// The runtime's threads: closing it closes every pool made through it.
    #[derive(Clone)]
    pub struct ThreadSpawner {
        open: Arc<AtomicBool>,
        pools: Arc<Mutex<Vec<Weak<PoolInner>>>>,
    }
    impl ThreadSpawner {
        pub fn for_current_thread() -> Self {
            Self { open: Arc::new(AtomicBool::new(true)), pools: Arc::default() }
        }
        fn register_pool(&self, pool: &Arc<PoolInner>) -> Result<(), SpawnError> {
            let mut pools = self.pools.lock().unwrap();
            if !self.open.load(Ordering::Acquire) {
                return Err(SpawnError::RuntimeClosed);
            }
            pools.retain(|pool| pool.strong_count() != 0);
            pools.push(Arc::downgrade(pool));
            Ok(())
        }
        fn close_runtime(&self) {
            let pools = {
                let mut pools = self.pools.lock().unwrap();
                self.open.store(false, Ordering::Release);
                std::mem::take(&mut *pools)
            };
            for pool in pools.into_iter().filter_map(|pool| pool.upgrade()) {
                pool.close();
            }
        }
    }

    struct Job {
        class: Class,
        run: Box<dyn FnOnce() + Send>,
    }
    #[derive(Default)]
    struct PoolState {
        closed: bool,
        queues: [VecDeque<Job>; 3],
    }
    struct Worker {
        thread: OnceLock<std::thread::Thread>,
        idle: AtomicBool,
    }
    struct PoolInner {
        state: Mutex<PoolState>,
        workers: Vec<Worker>,
    }
    impl PoolInner {
        /// Takes no new job and drops the queued ones; each worker ends after its running job.
        fn close(&self) {
            let discarded = {
                let mut state = self.state.lock().unwrap();
                state.closed = true;
                std::mem::take(&mut state.queues)
            };
            drop(discarded);
            self.unpark_all();
        }
        fn unpark_all(&self) {
            for thread in self.workers.iter().filter_map(|worker| worker.thread.get()) {
                thread.unpark();
            }
        }
    }

    fn worker(inner: Arc<PoolInner>, index: usize) {
        let me = &inner.workers[index];
        let _ = me.thread.set(std::thread::current());
        loop {
            me.idle.store(true, Ordering::SeqCst);
            let job = {
                let mut state = inner.state.lock().unwrap();
                match state.queues.iter_mut().find_map(VecDeque::pop_front) {
                    Some(job) => job,
                    None if state.closed => break,
                    None => {
                        drop(state);
                        std::thread::park();
                        continue;
                    }
                }
            };
            me.idle.store(false, Ordering::SeqCst);
            let permit = makepad_cpu_budget::admit(job.class.budget(), makepad_cpu_budget::APP, "static-pool");
            (job.run)();
            makepad_cpu_budget::release(permit);
        }
    }

    /// The pool's only handle: its drop closes the pool.
    pub struct TaskPool {
        inner: Arc<PoolInner>,
    }
    impl Drop for TaskPool {
        fn drop(&mut self) {
            self.inner.close();
        }
    }
    impl TaskPool {
        pub fn create(spawner: ThreadSpawner, workers: usize) -> Result<Self, SpawnError> {
            let inner = Arc::new(PoolInner {
                state: Mutex::default(),
                workers: (0..workers)
                    .map(|_| Worker { thread: OnceLock::new(), idle: AtomicBool::new(false) })
                    .collect(),
            });
            spawner.register_pool(&inner)?;
            let pool = Self { inner: inner.clone() };
            for index in 0..workers {
                let inner = inner.clone();
                std::thread::Builder::new()
                    .name(format!("pool-{index}"))
                    .spawn(move || worker(inner, index))
                    .map_err(|_| SpawnError::Backend)?;
            }
            Ok(pool)
        }
        pub fn submit(&self, class: Class, f: impl FnOnce() + Send + 'static) -> Result<(), SubmitError> {
            {
                let mut state = self.inner.state.lock().unwrap();
                if state.closed {
                    return Err(SubmitError::Closed);
                }
                state.queues[class.budget() as usize].push_back(Job { class, run: Box::new(f) });
            }
            self.inner.unpark_all();
            Ok(())
        }
        pub fn is_open(&self) -> bool {
            !self.inner.state.lock().unwrap().closed
        }
        /// Whether every worker has started and waits for a job.
        pub fn idle(&self) -> bool {
            self.inner.workers.iter().all(|worker| worker.idle.load(Ordering::SeqCst))
        }
    }

    /// A hosted app ends: close every pool of its runtime, those held in statics included.
    pub fn stop_workers(spawner: &ThreadSpawner) {
        spawner.close_runtime();
    }
}

use pool::{Class, TaskPool, ThreadSpawner};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

static POOL: OnceLock<TaskPool> = OnceLock::new();
static RAN: AtomicU64 = AtomicU64::new(0);

/// `p` points at the test's four flags: [0] the app runs with its pool, [1] the test lets it stop,
/// [2] the stop closed the pool.
fn static_pool_app(p: *mut u64) {
    let flags = unsafe { &*(p as *const [AtomicU64; 4]) };
    assert!(POOL.get().is_none(), "relaunch must start from its own static image");
    assert_eq!(RAN.load(Ordering::SeqCst), 0, "relaunch must start from its own static image");
    let spawner = ThreadSpawner::for_current_thread();
    let pool = POOL.get_or_init(|| {
        let pool = TaskPool::create(spawner.clone(), 3).unwrap();
        while !pool.idle() {
            std::thread::yield_now();
        }
        pool
    });
    // a job on a worker, on a permit of this run's budget
    pool.submit(Class::Visible, || {
        RAN.fetch_add(1, Ordering::SeqCst);
    })
    .unwrap();
    while RAN.load(Ordering::SeqCst) == 0 {
        std::thread::yield_now();
    }
    flags[0].store(1, Ordering::SeqCst);
    while flags[1].load(Ordering::SeqCst) == 0 {
        std::thread::yield_now();
    }
    pool::stop_workers(&spawner);
    assert!(!pool.is_open());
    assert!(pool.submit(Class::Background, || ()).is_err());
    assert!(POOL.get().is_some(), "the pool handle remains in the static");
    flags[2].store(1, Ordering::SeqCst);
}
