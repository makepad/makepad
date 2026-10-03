//! The process's kernel engine: one scheduler over one pool, and a compile
//! cache.
//!
//! An app installs the platform's `TaskPool` at start-up ([`install`]):
//! kernel jobs then run on its Heavy lane (the lane long work shares, so a
//! burst of kernels never queues in front of the light jobs), with no thread
//! of their own. Before that, or headless
//! (tests, tools, a level built without a window), the engine runs on the
//! compute crate's own pool. Results never depend on which: chunks are
//! fixed and combine in order.

use makepad_platform::thread::{Lane, TaskPool};
use makepad_platform::Cx;
use makepad_script_compute::admission::{JobBudget, Ledger, Origin};
use makepad_script_compute::kernel::{Kernel, Layout};
use makepad_script_compute::module::Module;
use makepad_script_compute::sched::{Executor, FnExecutor, Job, JobError, Priority, Scheduler, SchedulerConfig, ThreadExecutor};
use makepad_script_compute::{Backend, ShaderError};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

pub struct KernelEngine {
    exec: Arc<dyn Executor>,
    sched: Scheduler,
    task_pool: bool,
}

static ENGINE: OnceLock<KernelEngine> = OnceLock::new();

/// An executor on the platform pool's Heavy lane. `ui` is the UI thread:
/// the pool never fans out from it (that would block the UI on the pool's
/// queue), so a job run there runs on it alone.
pub fn task_pool_executor(pool: TaskPool, ui: std::thread::ThreadId) -> Arc<dyn Executor> {
    let workers = pool.heavy_workers();
    let (p1, p2) = (pool.clone(), pool);
    Arc::new(FnExecutor::new(
        workers,
        move |task: Arc<dyn Fn() + Send + Sync>| p1.submit_internal(Lane::Heavy, move || task()).is_ok(),
        move |n: usize, f: &(dyn Fn(usize) + Sync)| {
            if std::thread::current().id() == ui {
                (0..n).for_each(f);
            } else {
                p2.fan_out(Lane::Heavy, n, |w| f(w))
            }
        },
    ))
}

/// The ops the admitted jobs the device may hold can run, summed (the
/// admission ledger's `max_in_flight`), for the engine's trusted work: not
/// held back. Admission charges a job the least of its static worst case
/// and its budget, and a trusted pass's worst case (every loop at its cap,
/// every buffer access weighed as a miss) is far above what it runs, so a
/// world's build would otherwise wait on its own estimates. Untrusted work
/// keeps its own, smaller share (`max_untrusted_in_flight`), which this
/// does not change.
pub const TRUSTED_IN_FLIGHT: u64 = u64::MAX;

impl KernelEngine {
    fn new(exec: Arc<dyn Executor>, task_pool: bool) -> Self {
        let ledger = Ledger::device();
        let mut limits = ledger.limits();
        limits.max_in_flight = limits.max_in_flight.max(TRUSTED_IN_FLIGHT);
        ledger.set_limits(limits);
        let threads = exec.workers() + 1;
        let sched = Scheduler::new(exec.clone(), SchedulerConfig { threads, ..SchedulerConfig::default() });
        KernelEngine { exec, sched, task_pool }
    }

    pub fn executor(&self) -> &dyn Executor {
        &*self.exec
    }

    /// Threads one job may use (the caller included).
    pub fn threads(&self) -> usize {
        self.exec.workers() + 1
    }

    pub fn scheduler(&self) -> &Scheduler {
        &self.sched
    }

    /// Whether jobs run on the platform's `TaskPool`.
    pub fn on_task_pool(&self) -> bool {
        self.task_pool
    }

    /// Runs one trusted job to completion at `priority` and gives it back:
    /// on the calling thread with the workers' help for MustComplete (load
    /// gating: nothing queues in front of it), otherwise queued in its
    /// class and waited for. Trusted work runs to its end (its counted ops
    /// are not limited). Never call this on the UI thread.
    pub fn run(&self, mut job: Job, priority: Priority) -> Result<Job, (Job, JobError)> {
        let budget = JobBudget::UNLIMITED;
        if priority == Priority::MustComplete {
            return match self.sched.run_sync(&mut job, Origin::Host, budget) {
                Ok(()) => Ok(job),
                Err(e) => Err((job, e)),
            };
        }
        let handle = self.sched.submit(job, priority, Origin::Host, budget)?;
        handle.wait()
    }
}

/// Installs the app's `TaskPool` as the kernel engine's pool. Call once at
/// start-up on the UI thread, before the first kernel job; returns false
/// when the engine had already started on another pool (it keeps that one).
pub fn install(cx: &Cx) -> bool {
    let mut installed = false;
    ENGINE.get_or_init(|| {
        installed = true;
        KernelEngine::new(task_pool_executor(cx.task_pool(), std::thread::current().id()), true)
    });
    installed
}

/// The compute crate's process pool, as an owned executor.
struct SharedPool;

impl Executor for SharedPool {
    fn workers(&self) -> usize {
        ThreadExecutor::shared().workers()
    }
    fn spawn(&self, task: Arc<dyn Fn() + Send + Sync>) -> bool {
        ThreadExecutor::shared().spawn(task)
    }
    fn fan_out(&self, n: usize, f: &(dyn Fn(usize) + Sync)) {
        ThreadExecutor::shared().fan_out(n, f)
    }
}

/// The engine (the headless pool when no app installed one).
pub fn engine() -> &'static KernelEngine {
    ENGINE.get_or_init(|| {
        KernelEngine::new(Arc::new(SharedPool), false)
    })
}

/// Compiles a kernel once per process for each (source, layouts, modules):
/// kernels are immutable and shared, so a generator that runs per tile
/// compiles its passes on first use only.
pub fn compile(src: &str, layouts: &[Layout], modules: &[Module<'_>]) -> Result<Arc<Kernel>, Vec<ShaderError>> {
    static CACHE: OnceLock<Mutex<HashMap<u64, Arc<Kernel>>>> = OnceLock::new();
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    let mut eat = |b: &[u8]| {
        for x in b {
            h ^= *x as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        h = (h ^ 0xff).wrapping_mul(0x0000_0100_0000_01b3);
    };
    eat(src.as_bytes());
    eat(format!("{:?}", layouts).as_bytes());
    for m in modules {
        eat(m.path.as_bytes());
        eat(m.source.as_bytes());
    }
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(k) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(&h) {
        return Ok(k.clone());
    }
    let k = makepad_script_compute::kernel::compile_with_modules(src, layouts, Backend::Native, modules)?;
    cache.lock().unwrap_or_else(|e| e.into_inner()).insert(h, k.clone());
    Ok(k)
}
