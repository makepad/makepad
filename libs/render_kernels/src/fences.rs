//! Frame serials: a draw reads a slot in the frame that will be submitted
//! with `submitted() + 1`; the slot may be written again once `completed()`
//! has reached that serial.
//!
//! The fence-delay test flag holds completion back by whole frames, so the
//! slot lifecycle can be exercised deterministically (a GPU that runs
//! frames behind) without loading the machine: `MAKEPAD_KERNEL_FENCE_DELAY=n`
//! for an app run, [`ManualFences::with_delay`] in tests.

use makepad_platform::Cx;
use std::sync::atomic::{AtomicU64, Ordering};

/// The renderer's frame serials.
pub trait FrameFences {
    /// The last submitted frame.
    fn submitted(&self) -> u64;
    /// The greatest serial whose whole prefix has completed on the GPU.
    fn completed(&self) -> u64;
}

/// Frames the fence-delay test flag holds completion back (0: off).
pub fn fence_delay_flag() -> u64 {
    static DELAY: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *DELAY.get_or_init(|| std::env::var("MAKEPAD_KERNEL_FENCE_DELAY").ok().and_then(|v| v.parse().ok()).unwrap_or(0))
}

/// The platform's serials (`Cx::frame_submission_serial`,
/// `Cx::frame_completed_serial`), under the fence-delay test flag.
pub struct CxFences<'a> {
    cx: &'a Cx,
    delay: u64,
}

impl<'a> CxFences<'a> {
    pub fn new(cx: &'a Cx) -> Self {
        CxFences { cx, delay: fence_delay_flag() }
    }
}

impl FrameFences for CxFences<'_> {
    fn submitted(&self) -> u64 {
        self.cx.frame_submission_serial()
    }
    fn completed(&self) -> u64 {
        let done = self.cx.frame_completed_serial();
        done.min(self.submitted().saturating_sub(self.delay))
    }
}

/// Serials driven by hand (tests, headless renders): `submit()` a frame,
/// `complete(serial)` when its fence signals. With a delay, completion
/// never runs ahead of `submitted - delay`.
#[derive(Default)]
pub struct ManualFences {
    submitted: AtomicU64,
    completed: AtomicU64,
    delay: u64,
}

impl ManualFences {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_delay(delay: u64) -> Self {
        ManualFences { delay, ..Self::default() }
    }

    /// Submits the next frame; returns its serial.
    pub fn submit(&self) -> u64 {
        self.submitted.fetch_add(1, Ordering::AcqRel) + 1
    }

    /// The fence of `serial` signalled (every earlier one has too).
    pub fn complete(&self, serial: u64) {
        self.completed.fetch_max(serial, Ordering::AcqRel);
    }

    /// Completes every submitted frame.
    pub fn complete_all(&self) {
        self.complete(self.submitted.load(Ordering::Acquire));
    }
}

impl FrameFences for ManualFences {
    fn submitted(&self) -> u64 {
        self.submitted.load(Ordering::Acquire)
    }
    fn completed(&self) -> u64 {
        self.completed.load(Ordering::Acquire).min(self.submitted().saturating_sub(self.delay))
    }
}
