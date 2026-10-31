//! Bounded worker lanes for the window's blocking calls: HTTP requests to
//! the flow server, value downloads, package file writes.
//!
//! A lane is a fixed set of long-lived threads behind one bounded queue.
//! The UI thread only ever `try_send`s: a full queue keeps the job in the
//! lane's backlog and the next poll tick moves it on, so accepted work is
//! never dropped and the UI never waits. Nothing here spawns per request.

use makepad_flow_ui::values::FetchLane;
use std::collections::VecDeque;
use std::sync::mpsc::{sync_channel, SyncSender, TrySendError};

pub type Job = Box<dyn FnOnce() + Send>;

/// Most jobs a lane holds back on the UI side while its queue is full.
/// Far above what one poll tick can produce; past it the caller is told.
const BACKLOG_MAX: usize = 4_096;

#[derive(Default)]
pub struct WorkerLane {
    queue: Option<SyncSender<Job>>,
    backlog: VecDeque<Job>,
    refused: u64,
}

impl WorkerLane {
    /// Start `threads` long-lived workers behind a queue of `capacity`.
    pub fn start(&mut self, name: &str, threads: usize, capacity: usize) {
        let (sender, receiver) = sync_channel::<Job>(capacity.max(1));
        let receiver = std::sync::Arc::new(std::sync::Mutex::new(receiver));
        for index in 0..threads.max(1) {
            let receiver = receiver.clone();
            let spawned = std::thread::Builder::new().name(format!("{name}-{index}")).spawn(move || loop {
                // The lock is held only to take the next job; a job never
                // runs under it.
                let job = match receiver.lock() {
                    Ok(receiver) => receiver.recv(),
                    Err(_) => return,
                };
                match job {
                    Ok(job) => job(),
                    Err(_) => return,
                }
            });
            if let Err(error) = spawned {
                makepad_widgets::log!("flow: worker lane `{name}` thread {index}: {error}");
            }
        }
        self.queue = Some(sender);
    }

    /// Whether a job submitted now would be taken (a started lane whose
    /// backlog has room). A caller that must not hand over state it cannot
    /// get back asks this first.
    pub fn can_accept(&self) -> bool {
        self.queue.is_some() && self.backlog.len() < BACKLOG_MAX
    }

    /// Queue a job; a full queue keeps it for the next `pump`. Never blocks.
    /// Returns false when nothing can take it (no lane, or the backlog is
    /// past its bound); the job is then gone and the caller says so.
    pub fn submit(&mut self, job: Job) -> bool {
        let Some(queue) = self.queue.as_ref() else {
            return false;
        };
        if !self.backlog.is_empty() {
            return self.hold(job);
        }
        match queue.try_send(job) {
            Ok(()) => true,
            Err(TrySendError::Full(job)) => self.hold(job),
            Err(TrySendError::Disconnected(_)) => false,
        }
    }

    fn hold(&mut self, job: Job) -> bool {
        if self.backlog.len() >= BACKLOG_MAX {
            self.refused += 1;
            return false;
        }
        self.backlog.push_back(job);
        true
    }

    /// Move held jobs into the queue while there is room.
    pub fn pump(&mut self) {
        let Some(queue) = self.queue.as_ref() else {
            return;
        };
        while let Some(job) = self.backlog.pop_front() {
            match queue.try_send(job) {
                Ok(()) => {}
                Err(TrySendError::Full(job)) | Err(TrySendError::Disconnected(job)) => {
                    // Kept either way: a full queue drains, and a lane whose
                    // workers are gone still says how much it holds.
                    self.backlog.push_front(job);
                    return;
                }
            }
        }
    }

    /// Jobs refused since start because the backlog was full.
    pub fn refused(&self) -> u64 {
        self.refused
    }
}

impl FetchLane for WorkerLane {
    /// A value fetch takes the queue directly and is refused when full; the
    /// cache un-pends it and the next request asks again.
    fn run(&self, job: Job) -> bool {
        self.queue.as_ref().is_some_and(|queue| queue.try_send(job).is_ok())
    }
}
