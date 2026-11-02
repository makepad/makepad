//! Adapt Flow's work admission to the application's existing platform pool.
//! Long-lived engine loops use the platform spawner; evaluations/preparation
//! use the shared pool's Background class. Dropping a handle never joins a worker.

use makepad_flow::runtime::{HostWork, Job, JobLane, JobRefused, SpawnRefused, WorkBudget};
use makepad_widgets::{Cx, Class, TaskPool, ThreadOptions, ThreadSpawner};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

const LOOPS: usize = 4;
const JOBS: usize = 64;

struct Permit(Arc<AtomicUsize>);

impl Permit {
    fn take(counter: &Arc<AtomicUsize>, limit: usize) -> Option<Self> {
        counter.try_update(Ordering::AcqRel, Ordering::Acquire, |count| {
            (count < limit).then_some(count + 1)
        }).ok()?;
        Some(Self(counter.clone()))
    }
}

impl Drop for Permit {
    fn drop(&mut self) { self.0.fetch_sub(1, Ordering::AcqRel); }
}

pub struct AppWork {
    pool: TaskPool,
    spawner: ThreadSpawner,
    loops: Arc<AtomicUsize>,
    jobs: Arc<AtomicUsize>,
}

impl AppWork {
    pub fn new(cx: &Cx) -> Arc<Self> {
        Arc::new(Self {
            pool: cx.task_pool(), spawner: cx.thread_spawner(),
            loops: Arc::new(AtomicUsize::new(0)), jobs: Arc::new(AtomicUsize::new(0)),
        })
    }
}

impl HostWork for AppWork {
    fn submit_job(&self, _lane: JobLane, job: Job) -> Result<(), JobRefused> {
        let Some(permit) = Permit::take(&self.jobs, JOBS) else { return Err(JobRefused(job)) };
        // HostWork returns the original job if the application pool closed.
        let pending = Arc::new(std::sync::Mutex::new(Some(job)));
        let work = pending.clone();
        match self.pool.submit_named(Class::Background, "flow-studio", move || {
            let _permit = permit;
            let job = work.lock().unwrap().take().unwrap();
            job();
        }) {
            Ok(handle) => { handle.detach(); Ok(()) }
            Err(_) => Err(JobRefused(pending.lock().unwrap().take().unwrap())),
        }
    }

    fn spawn_loop(&self, name: &str, body: Job) -> Result<(), SpawnRefused> {
        let Some(permit) = Permit::take(&self.loops, LOOPS) else { return Err(SpawnRefused::Budget(body)) };
        self.spawner.spawn_worker(ThreadOptions { name: Some(Arc::from(name)), ..Default::default() }, move || {
            let _permit = permit;
            body();
        }).map(|handle| handle.detach()).map_err(|error| SpawnRefused::Os(std::io::Error::other(error)))
    }

    fn budget(&self) -> WorkBudget {
        WorkBudget { loops_free: LOOPS.saturating_sub(self.loops.load(Ordering::Acquire)), job_queue: JOBS }
    }
}
