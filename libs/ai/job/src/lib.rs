//! The one thing every AI model shares: a job surface the control plane can
//! drive uniformly — progress reporting and cancellation. Deliberately tiny
//! and dependency-free. Everything below this line (execution, tensors,
//! residency) is per-model, per-backend, and shares nothing but kernels.
//!
//! Cancellation is cooperative: models check [`JobCtx::cancelled`] between
//! denoise steps, AR frames, decode chunks, and loader tensors. A cancel
//! may therefore land one step late; it never tears down mid-kernel.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Progress/lifecycle events a running job emits. The service maps these
/// onto its HTTP job routes and chat tool streams.
#[derive(Clone, Debug)]
pub enum JobEvent {
    /// A named stage began ("load te", "load dit", "denoise", "decode", …).
    StageStart { stage: &'static str },
    /// Progress within the current stage. `total == 0` means indeterminate.
    /// Units are stage-specific (bytes for loads, steps for denoise loops,
    /// frames for AR decode) — `unit` names them for display.
    Progress { stage: &'static str, done: u64, total: u64, unit: &'static str },
    /// Optional intermediate artifact (preview image, partial audio…).
    Preview { stage: &'static str, mime: &'static str, bytes: Arc<Vec<u8>> },
}

/// Shared cancellation flag. Clone freely; `cancel()` from any thread.
#[derive(Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

/// Everything a model needs to be a good citizen: an event sink and a
/// cancel flag. Constructed by the service per job; passed by reference
/// through load and generate paths.
pub struct JobCtx {
    sink: Box<dyn Fn(JobEvent) + Send + Sync>,
    cancel: CancelToken,
}

impl JobCtx {
    pub fn new(sink: impl Fn(JobEvent) + Send + Sync + 'static, cancel: CancelToken) -> Self {
        Self { sink: Box::new(sink), cancel }
    }

    /// A context that drops all events and never cancels — for tests and
    /// CLI validate bins.
    pub fn ignore() -> Self {
        Self { sink: Box::new(|_| {}), cancel: CancelToken::new() }
    }

    pub fn emit(&self, event: JobEvent) {
        (self.sink)(event);
    }

    pub fn stage(&self, stage: &'static str) {
        self.emit(JobEvent::StageStart { stage });
    }

    pub fn progress(&self, stage: &'static str, done: u64, total: u64, unit: &'static str) {
        self.emit(JobEvent::Progress { stage, done, total, unit });
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    pub fn cancel_token(&self) -> CancelToken {
        self.cancel.clone()
    }
}

/// The uniform "job stopped early" error models return on cancellation, so
/// the service can distinguish a cancel from a failure without string
/// matching.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "job cancelled")
    }
}

impl std::error::Error for Cancelled {}
