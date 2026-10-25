//! Host-supplied work.
//!
//! The engine never creates a thread of its own. A host hands it a
//! [`HostWork`]: a bounded pool for finite jobs and an admission gate for the
//! few long-lived loops the engine needs (its state worker and its run
//! lanes). A UI host implements this over its task pool; [`StdHostWork`] is
//! the std-only implementation for headless hosts.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};

pub type Job = Box<dyn FnOnce() + Send + 'static>;

/// Which of the host's bounded lanes a finite job belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobLane {
    /// Evaluating a flow source (Splash VM, instruction and heap limited).
    Evaluate,
    /// CPU preparation for a provider (decode, mesh, preview).
    Prepare,
}

/// The lane was full (or the host is shutting down). The job comes back to
/// the caller; nothing ran.
pub struct JobRefused(pub Job);

pub enum SpawnRefused {
    /// The host's loop budget is spent. The body comes back.
    Budget(Job),
    /// The operating system refused the thread.
    Os(std::io::Error),
}

impl std::fmt::Debug for SpawnRefused {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Budget(_) => write!(formatter, "SpawnRefused::Budget"),
            Self::Os(error) => write!(formatter, "SpawnRefused::Os({error})"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorkBudget {
    /// Long-lived loops still admissible.
    pub loops_free: usize,
    /// Capacity of one finite-job lane.
    pub job_queue: usize,
}

pub trait HostWork: Send + Sync {
    /// Queue a finite job on an already-running bounded pool. Never blocks
    /// and never creates a thread.
    fn submit_job(&self, lane: JobLane, job: Job) -> Result<(), JobRefused>;

    /// Admit one long-lived loop. `body` returns when its owner shuts it
    /// down (its channels disconnect).
    fn spawn_loop(&self, name: &str, body: Job) -> Result<(), SpawnRefused>;

    fn budget(&self) -> WorkBudget;
}

/// Std-only [`HostWork`]: a fixed pool of worker threads created once, fed
/// by one bounded queue, plus a counted allowance of long-lived loops.
pub struct StdHostWork {
    jobs: SyncSender<Job>,
    job_queue: usize,
    loops_max: usize,
    loops_live: Arc<AtomicUsize>,
}

impl StdHostWork {
    /// `workers` pool threads (at least one), a queue of `job_queue` jobs,
    /// and at most `loops_max` long-lived loops.
    pub fn new(workers: usize, job_queue: usize, loops_max: usize) -> std::io::Result<Arc<Self>> {
        let (jobs, queue) = sync_channel::<Job>(job_queue.max(1));
        // Pool workers share the receiving end; the lock is only ever taken
        // by pool workers, never by a UI or realtime thread.
        let queue: Arc<Mutex<Receiver<Job>>> = Arc::new(Mutex::new(queue));
        for index in 0..workers.max(1) {
            let queue = queue.clone();
            std::thread::Builder::new()
                .name(format!("flow-work-{index}"))
                .spawn(move || loop {
                    let job = match queue.lock() {
                        Ok(queue) => queue.recv(),
                        Err(_) => return,
                    };
                    match job {
                        Ok(job) => {
                            // A panicking job must not take the pool worker
                            // with it.
                            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job));
                        }
                        Err(_) => return,
                    }
                })?;
        }
        Ok(Arc::new(Self {
            jobs,
            job_queue: job_queue.max(1),
            loops_max,
            loops_live: Arc::new(AtomicUsize::new(0)),
        }))
    }
}

impl HostWork for StdHostWork {
    fn submit_job(&self, _lane: JobLane, job: Job) -> Result<(), JobRefused> {
        match self.jobs.try_send(job) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(job)) | Err(TrySendError::Disconnected(job)) => Err(JobRefused(job)),
        }
    }

    fn spawn_loop(&self, name: &str, body: Job) -> Result<(), SpawnRefused> {
        let live = self.loops_live.clone();
        let max = self.loops_max;
        if live
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |count| (count < max).then_some(count + 1))
            .is_err()
        {
            return Err(SpawnRefused::Budget(body));
        }
        let exit = live.clone();
        let spawned = std::thread::Builder::new().name(name.to_string()).spawn(move || {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(body));
            exit.fetch_sub(1, Ordering::SeqCst);
        });
        match spawned {
            Ok(_) => Ok(()),
            Err(error) => {
                live.fetch_sub(1, Ordering::SeqCst);
                Err(SpawnRefused::Os(error))
            }
        }
    }

    fn budget(&self) -> WorkBudget {
        WorkBudget {
            loops_free: self.loops_max.saturating_sub(self.loops_live.load(Ordering::SeqCst)),
            job_queue: self.job_queue,
        }
    }
}
