//! How many presents a Wayland window may have awaiting their
//! `wl_surface::frame` callback before its next present is held back.
//!
//! One serializes callback → draw → commit. That is always safe, but a frame
//! that takes longer than the compositor's deadline then lands a whole refresh
//! late, every time, and a heavy scene runs at half the display rate. Two lets
//! the next frame be drawn while the previous one is still on its way through
//! the compositor. Whether two is safe depends on what happens to a second
//! present that arrives before the first was shown:
//!
//! - Vulkan presents through a FIFO swapchain. When the compositor offers
//!   `fifo-v1` and the driver uses it, the second present queues behind the
//!   first and nothing blocks, so two is always allowed. Otherwise Mesa's
//!   swapchain throttles inside `vkQueuePresentKHR` on the previous present's
//!   own frame callback, with no timeout, and a compositor withholds callbacks
//!   from an occluded window: the second present would hang the event loop
//!   until the window is shown again. Such a session stays at one.
//! - OpenGL (swap interval 0, see `OpenglCx::swap_interval`) has no queue: a
//!   second commit replaces the first. Two frames committed within one
//!   refresh cost twice the work for one picture, so a frame may start early
//!   only when its swap would land after the outstanding callback is due,
//!   which is the case exactly when frames are slow enough to need it.

use std::time::{Duration, Instant};

/// What presents for the running process, as far as pacing is concerned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PacedBackend {
    /// `fifo_presents_queue`: see `CxVulkan::fifo_presents_queue`.
    Vulkan {
        fifo_presents_queue: bool,
    },
    OpenGl,
}

pub(crate) struct FramePacer {
    /// `MAKEPAD_LINUX_LATENCY` (1 or 2) bypasses the policy.
    forced: Option<usize>,
    backend: PacedBackend,
    /// The compositor's own timestamps (milliseconds) of recent frame
    /// callbacks, as gaps. Their median is the refresh interval; the times at
    /// which this thread got round to dispatching them are too jittery.
    recent_gaps_ms: [u32; Self::GAP_SAMPLES],
    gap_samples: usize,
    last_callback_ms: Option<u32>,
    /// When the last callback was dispatched here: never earlier than it was
    /// sent, so predictions anchored on it err toward waiting.
    last_callback: Option<Instant>,
    /// CPU time of the last few paint cycles that presented (the app's draw
    /// plus the GL submission, without the driver's own wait inside the swap).
    recent_costs: [Duration; Self::COST_SAMPLES],
    cost_samples: usize,
}

impl FramePacer {
    /// Gaps outside this range are not one refresh: callbacks of two windows
    /// or of a replaced commit carry the same time, and an idle app gets none.
    const REFRESH_RANGE_MS: std::ops::RangeInclusive<u32> = 2..=50;
    const GAP_SAMPLES: usize = 9;

    const COST_SAMPLES: usize = 8;

    pub(crate) fn new() -> Self {
        Self {
            forced: match std::env::var("MAKEPAD_LINUX_LATENCY").ok().as_deref() {
                Some("1") => Some(1),
                Some("2") => Some(2),
                _ => None,
            },
            backend: PacedBackend::OpenGl,
            recent_gaps_ms: [0; Self::GAP_SAMPLES],
            gap_samples: 0,
            last_callback_ms: None,
            last_callback: None,
            recent_costs: [Duration::ZERO; Self::COST_SAMPLES],
            cost_samples: 0,
        }
    }

    pub(crate) fn set_backend(&mut self, backend: PacedBackend) {
        self.backend = backend;
    }

    /// A frame callback was dispatched at `now`; `compositor_ms` is the time
    /// the compositor stamped it with.
    pub(crate) fn callback_arrived(&mut self, now: Instant, compositor_ms: u32) {
        if let Some(last_ms) = self.last_callback_ms {
            let gap = compositor_ms.wrapping_sub(last_ms);
            if Self::REFRESH_RANGE_MS.contains(&gap) {
                self.recent_gaps_ms[self.gap_samples % Self::GAP_SAMPLES] = gap;
                self.gap_samples += 1;
            }
        }
        self.last_callback_ms = Some(compositor_ms);
        self.last_callback = Some(now);
    }

    /// The median recent callback gap. A run of skipped refreshes raises it,
    /// which only makes the pacer more willing to start the next frame early,
    /// the remedy for exactly that.
    fn refresh(&self) -> Option<Duration> {
        let count = self.gap_samples.min(Self::GAP_SAMPLES);
        if count < 3 {
            return None;
        }
        let mut gaps = self.recent_gaps_ms;
        let gaps = &mut gaps[..count];
        gaps.sort_unstable();
        Some(Duration::from_millis(gaps[count / 2] as u64))
    }

    /// One paint cycle that presented took `cost` of CPU time.
    pub(crate) fn frame_presented(&mut self, cost: Duration) {
        self.recent_costs[self.cost_samples % Self::COST_SAMPLES] = cost;
        self.cost_samples += 1;
    }

    /// The fastest recent frame. Predicting with it errs toward waiting for the
    /// callback: a frame that finishes sooner than predicted is the one that
    /// would be committed twice in a refresh.
    fn shortest_recent_cost(&self) -> Option<Duration> {
        self.recent_costs[..self.cost_samples.min(Self::COST_SAMPLES)]
            .iter()
            .copied()
            .min()
    }

    /// `refresh_ms=… cost_ms=…` for the `wl.pacer` trace.
    pub(crate) fn describe(&self) -> String {
        let ms = |value: Option<Duration>| value.map_or(-1.0, |value| value.as_secs_f64() * 1000.0);
        format!(
            "refresh_ms={:.2} cost_ms={:.2}",
            ms(self.refresh()),
            ms(self.shortest_recent_cost())
        )
    }

    /// Presents a window may have in flight right now. `has_fifo_v1` is whether
    /// the compositor advertises `wp_fifo_manager_v1`.
    pub(crate) fn frames_in_flight(&self, has_fifo_v1: bool, now: Instant) -> usize {
        if let Some(forced) = self.forced {
            return forced;
        }
        match self.backend {
            PacedBackend::Vulkan {
                fifo_presents_queue,
            } => {
                if has_fifo_v1 && fifo_presents_queue {
                    2
                } else {
                    1
                }
            }
            PacedBackend::OpenGl => {
                let (Some(refresh), Some(last_callback), Some(cost)) = (
                    self.refresh(),
                    self.last_callback,
                    self.shortest_recent_cost(),
                ) else {
                    return 1;
                };
                // A frame started now swaps at `now + cost`; the outstanding
                // callback is due one refresh after the last one.
                if now + cost >= last_callback + refresh {
                    2
                } else {
                    1
                }
            }
        }
    }
}
