//! A hub generation on the app's jobs board (`makepad_jobs`): the status
//! bar's strip and its list show where it runs, what it is doing and how far,
//! and the person's cancel there stops it on its node and frees the model's
//! device memory.
//!
//! The app polls its [`GenJob`] where it already does (a worker thread) and
//! passes every poll through [`BoardGen::poll`]:
//!
//! ```text
//! let mut tracked = BoardGen::start("Generating mesh from photo.png");
//! job.start(Domain::Mesh, req, want)?;
//! loop {
//!     match tracked.poll(&mut job) {
//!         JobPoll::Done(out) => { tracked.done_with_next("Place it"); break }
//!         JobPoll::Failed(why) => { tracked.fail(why); break }
//!         _ => sleep(..),
//!     }
//! }
//! ```
//!
//! A cancel reaches the node at the next poll: the worker sees the job's
//! flag there, sends the freeing cancel (`POST /job/<id>/cancel
//! {"free":true}`) and the poll answers `Failed("cancelled")`, which
//! [`BoardGen::fail`] ends as a cancel. On the node the backend unwinds at
//! its next step boundary and the worker unloads the model once it has.

use crate::job::{GenJob, JobPoll};
use makepad_jobs::{JobHandle, JobSpec};

/// What the board calls a node before the job has one.
const ROUTING: &str = "AI hub";

pub struct BoardGen {
    handle: Option<JobHandle>,
    cancel_sent: bool,
}

impl BoardGen {
    /// Put a generation on the board, titled in words.
    pub fn start(title: impl Into<String>) -> Self {
        let handle = JobSpec::new(title).machine(ROUTING).stage("finding a node").start();
        Self { handle: Some(handle), cancel_sent: false }
    }

    /// Track a generation on a job the caller already put on the board (it
    /// reported the steps before the hub's: reading a picture, finding nodes).
    pub fn with_handle(handle: JobHandle) -> Self {
        Self { handle: Some(handle), cancel_sent: false }
    }

    /// The board's job, for a caller that reports more itself.
    pub fn handle(&self) -> Option<&JobHandle> {
        self.handle.as_ref()
    }

    /// Whether the person cancelled it.
    pub fn is_cancelled(&self) -> bool {
        self.handle.as_ref().is_some_and(|h| h.is_cancelled())
    }

    /// Poll `job` once: a cancel goes to the node first, the answer's node,
    /// stage and progress go on the board, and the answer comes back.
    pub fn poll(&mut self, job: &mut GenJob) -> JobPoll {
        let Some(handle) = &self.handle else { return job.poll() };
        if handle.is_cancelled() && !self.cancel_sent {
            self.cancel_sent = true;
            job.cancel_freeing();
            return JobPoll::Failed("cancelled".to_string());
        }
        let poll = job.poll();
        if let Some(node) = job.node() {
            handle.set_machine(node);
        }
        if let JobPoll::Progress { permille, stage } = &poll {
            let fraction = (*permille > 0).then(|| *permille as f32 / 1000.0);
            handle.stage(stage.clone(), None, fraction);
        }
        poll
    }

    /// Finished; nothing more needed from the person.
    pub fn done(mut self) {
        if let Some(h) = self.handle.take() {
            h.done();
        }
    }

    /// Finished, and the result waits for the person: `next` names the step.
    pub fn done_with_next(mut self, next: impl Into<String>) {
        if let Some(h) = self.handle.take() {
            h.done_with_next(next);
        }
    }

    /// Failed with `reason`, or stopped when the person cancelled it.
    pub fn fail(mut self, reason: impl Into<String>) {
        if let Some(h) = self.handle.take() {
            if h.is_cancelled() {
                h.cancelled();
            } else {
                h.fail(reason, true);
            }
        }
    }
}
