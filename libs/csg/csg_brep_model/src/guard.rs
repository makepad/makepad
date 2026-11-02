//! Running kernel code so that it can't take the app down.
//!
//! The kernel recurses deeply and its algorithms carry invariants, so a call runs inside
//! [`guarded`], on a thread with a big stack (see [`stack_size`]). There a panic and a
//! stack limit come back as a [`BrepError`], and a cancel unwinds the work.

use makepad_csg_brep::limits;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Why a body could not be built.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BrepError {
    /// The kernel refused or failed; the message says what it was doing.
    Kernel(String),
    /// Bad input from the model.
    Input(String),
    /// A newer edit replaced this build.
    Cancelled,
    /// The kernel recursed past its stack limit, or panicked on an internal invariant.
    Internal(String),
}

impl std::fmt::Display for BrepError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BrepError::Kernel(m) | BrepError::Input(m) => f.write_str(m),
            BrepError::Cancelled => f.write_str("cancelled by a newer edit"),
            BrepError::Internal(m) => write!(f, "the kernel gave up: {m}"),
        }
    }
}

impl std::error::Error for BrepError {}

impl From<makepad_csg_brep::SolidError> for BrepError {
    fn from(e: makepad_csg_brep::SolidError) -> BrepError {
        BrepError::Kernel(e.to_string())
    }
}

/// The stack of a kernel worker thread. Only the pages it touches are ever committed.
pub const fn stack_size() -> usize {
    1 << 30
}

/// How far from the stack's end the kernel stops recursing.
const STACK_MARGIN: usize = 16 << 20;

/// Cancels the work of one [`guarded`] call from another thread.
#[derive(Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> CancelToken {
        CancelToken::default()
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed)
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
    fn flag(&self) -> Arc<AtomicBool> {
        self.0.clone()
    }
}

/// Runs `f` with the stack limit and `cancel` armed on this thread, whose stack is
/// `stack` bytes. A panic, a stack limit and a cancel become errors. Call it on a thread
/// spawned with that stack size.
pub fn guarded<T>(stack: usize, cancel: &CancelToken, f: impl FnOnce() -> Result<T, BrepError>) -> Result<T, BrepError> {
    let _armed = limits::arm(stack.saturating_sub(STACK_MARGIN).max(1), cancel.flag());
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(r) => r,
        Err(payload) => Err(from_panic(payload)),
    }
}

fn from_panic(payload: Box<dyn std::any::Any + Send>) -> BrepError {
    match payload.downcast::<limits::Unwound>() {
        Ok(u) => match *u {
            limits::Unwound::Cancelled => BrepError::Cancelled,
            limits::Unwound::Stack => BrepError::Internal("recursion too deep (stack limit)".into()),
        },
        Err(p) => {
            let message = p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()));
            BrepError::Internal(message.unwrap_or_else(|| "internal panic".into()))
        }
    }
}
