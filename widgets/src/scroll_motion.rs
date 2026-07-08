//! Shared kinetic-scroll ("fling") math used by the scrollable widgets (PortalList and
//! ScrollBar, and thus ScrollBars / ScrollXView / ScrollYView / ScrollXYView).
//!
//! Both touch-drag flicks and the trackpad deceleration tail use the iOS `UIScrollView`
//! momentum model: velocity decays as `v *= DECEL_RATE^(elapsed_ms)`, and each frame's
//! displacement is the integral of that decay, so the motion is smooth and frame-rate-
//! independent. Trackpad scrolling applies the OS momentum directly while fast, then hands off
//! to a gentler self-decaying tail once it slows (see [`Fling::new_trackpad_tail`]).

use std::sync::OnceLock;

/// Diagnostic toggle: when the `MAKEPAD_RAW_TRACKPAD_MOMENTUM` env var is set (to anything
/// other than empty / `0` / `false`), trackpad momentum is applied exactly as the OS delivers
/// it, bypassing the self-decaying tail. Use it to A/B the smoothed deceleration against raw
/// native momentum — raw follows the OS's short, choppy tail; the smoothed path hands off to a
/// gentler, longer glide once the flick slows.
pub fn raw_os_momentum() -> bool {
    static RAW: OnceLock<bool> = OnceLock::new();
    *RAW.get_or_init(|| {
        std::env::var("MAKEPAD_RAW_TRACKPAD_MOMENTUM")
            .map(|v| !v.is_empty() && v != "0" && v != "false")
            .unwrap_or(false)
    })
}

/// Default per-ms decay for a touch-drag flick (the widgets' `fling_decel` field). For
/// reference, iOS `UIScrollViewDecelerationRateNormal` is 0.998; we run a little firmer.
pub const FLING_DECEL_RATE_PER_MS: f64 = 0.997;

/// EMA weight for the newest inter-frame interval sample (see [`Fling::step`]).
const FLING_DT_EMA_ALPHA: f64 = 0.15;

/// The band around the EMA'd frame interval that a raw `dt` is clamped to,
/// so one late/early frame cannot produce a visible jump or stall.
const FLING_DT_BAND: (f64, f64) = (0.5, 1.5);

/// Upper bound on a single integration step (seconds): a long hitch produces a small
/// catch-up rather than a huge jump.
const FLING_MAX_DT: f64 = 0.1;

/// How many of the most recent position samples are retained for velocity estimation,
/// like a native `VelocityTracker`.
pub const FLING_SAMPLE_WINDOW: usize = 4;

/// The minimum total travel (pixels) across the retained samples for a release to count as a
/// fling. Filters out taps and micro-jitters.
pub const FLING_MIN_TOTAL_DELTA: f64 = 10.0;

/// Converts the per-frame `flick_scroll_minimum` / `flick_scroll_maximum` widget parameters
/// (defined at a nominal 60 fps) into pixels-per-second velocities, so the same DSL values
/// keep their meaning under the time-based model.
pub const PER_FRAME_TO_PER_SECOND: f64 = 60.0;

/// Default trackpad fast→tail handoff speed, in per-event pixels (the widgets'
/// `fling_smoothing_cutoff_speed` field).
pub const FLING_MOMENTUM_SMOOTH_BELOW: f64 = 35.0;

/// Default per-ms decay for the trackpad deceleration tail (the widgets' `fling_tail_decel`
/// field). Gentler than [`FLING_DECEL_RATE_PER_MS`] for a longer, smoother glide.
pub const FLING_MOMENTUM_TAIL_DECEL_PER_MS: f64 = 0.996;

/// One position sample along the scroll axis: a finger/mouse position for drag scrolling, or
/// the accumulated applied scroll delta for trackpad gestures. Its derivative is the scroll
/// velocity in pixels per second.
#[derive(Clone, Copy, Debug)]
pub struct ScrollSample {
    pub abs: f64,
    pub time: f64,
}

/// Append a sample, retaining only the most recent [`FLING_SAMPLE_WINDOW`] of them.
pub fn push_sample(samples: &mut Vec<ScrollSample>, abs: f64, time: f64) {
    samples.push(ScrollSample { abs, time });
    if samples.len() > FLING_SAMPLE_WINDOW {
        samples.remove(0);
    }
}

/// Estimate the release velocity (pixels/second) and total travel (pixels) across the
/// retained samples, like a native `VelocityTracker`: oldest→newest over their time span.
///
/// Returns `(release_velocity, total_delta)`. A release should become a fling only if
/// `total_delta.abs() > FLING_MIN_TOTAL_DELTA` and the velocity exceeds the widget's
/// minimum; otherwise the lift is a stop, not a flick.
pub fn estimate_release_velocity(samples: &[ScrollSample]) -> (f64, f64) {
    let mut total_delta = 0.0;
    for w in samples.windows(2) {
        total_delta += w[1].abs - w[0].abs;
    }
    let release_velocity = if let (Some(first), Some(last)) = (samples.first(), samples.last()) {
        let dt = last.time - first.time;
        if dt > 0.0001 {
            (last.abs - first.abs) / dt
        } else {
            0.0
        }
    } else {
        0.0
    };
    (release_velocity, total_delta)
}

/// One kinetic-scroll animation along a single scroll axis: a velocity that decays
/// exponentially, integrated per frame so the motion is smooth and frame-rate-independent.
///
/// - A touch-drag flick ([`Fling::new`]) may overscroll into the pulldown bounce.
/// - A trackpad deceleration tail ([`Fling::new_trackpad_tail`]) takes over once the OS momentum
///   slows past the widget's handoff threshold, and clips at the edges instead.
///
/// The `decay_rate_per_ms` is supplied by the caller (a widget `#[live]` field), so the feel is
/// configurable per widget. Drive it once per animation frame with [`Fling::step`], apply the
/// returned displacement, and stop when [`Fling::is_active`] returns false.
#[derive(Clone, Copy, Debug)]
pub struct Fling {
    /// Current velocity in pixels per second.
    pub velocity: f64,
    /// Per-millisecond velocity decay factor applied each step.
    decay_rate_per_ms: f64,
    /// Whether this fling may overscroll into the pulldown bounce (touch-drag) or clips at the
    /// edges (trackpad tail).
    overscroll: bool,
    /// Wall-clock time of the previous step (0.0 = not yet started).
    last_time: f64,
    /// Running EMA (seconds) of the inter-frame interval. The step is driven off a `dt`
    /// clamped to a tight band around this, so frame-delivery jitter (a late or early frame)
    /// does not turn into an uneven jump/stall in the motion.
    dt_ema: f64,
}

impl Default for Fling {
    fn default() -> Self {
        Self::new(0.0, FLING_DECEL_RATE_PER_MS)
    }
}

impl Fling {
    /// A touch-drag flick decaying at `decay_rate_per_ms`; may overscroll.
    pub fn new(velocity: f64, decay_rate_per_ms: f64) -> Self {
        Self {
            velocity,
            decay_rate_per_ms,
            overscroll: true,
            last_time: 0.0,
            dt_ema: 0.0,
        }
    }

    /// A trackpad deceleration tail decaying at `decay_rate_per_ms`; clips at the edges.
    pub fn new_trackpad_tail(velocity: f64, decay_rate_per_ms: f64) -> Self {
        Self {
            velocity,
            decay_rate_per_ms,
            overscroll: false,
            last_time: 0.0,
            dt_ema: 0.0,
        }
    }

    /// Whether this fling may overscroll into the pulldown bounce (touch) rather than clip (trackpad).
    pub fn allows_overscroll(&self) -> bool {
        self.overscroll
    }

    /// Whether this fling should keep animating (still above the minimum speed).
    pub fn is_active(&self, min_velocity: f64) -> bool {
        self.velocity.abs() > min_velocity
    }

    /// Advance the fling to wall-clock time `now` (the NextFrame event time).
    ///
    /// Returns `None` on the first frame, which only establishes the time base. Afterwards
    /// returns `Some(displacement)` in pixels, with `velocity` decayed for the next step.
    ///
    /// Frame delivery is not perfectly vsync-uniform (e.g. Windows `Present(1,0)` can return
    /// early or span more than one vblank), so the raw inter-frame dt jitters. We track an EMA
    /// of the interval and clamp the dt used to a tight band around it, so a single late or
    /// early frame can't produce a visible jump or stall.
    pub fn step(&mut self, now: f64) -> Option<f64> {
        if self.last_time <= 0.0 {
            self.last_time = now;
            self.dt_ema = 0.0;
            return None;
        }
        let raw_dt = (now - self.last_time).clamp(0.0, FLING_MAX_DT);
        self.last_time = now;
        if self.dt_ema <= 0.0 {
            self.dt_ema = raw_dt;
        } else {
            self.dt_ema =
                self.dt_ema * (1.0 - FLING_DT_EMA_ALPHA) + raw_dt * FLING_DT_EMA_ALPHA;
        }
        let dt = raw_dt.clamp(self.dt_ema * FLING_DT_BAND.0, self.dt_ema * FLING_DT_BAND.1);
        let factor = self.decay_rate_per_ms.powf(dt * 1000.0);
        // v(t) = v0 * e^(-λt); displacement over dt = v0 * (1 - factor) / λ.
        let lambda = -self.decay_rate_per_ms.ln() * 1000.0;
        let displacement = self.velocity * (1.0 - factor) / lambda;
        self.velocity *= factor;
        Some(displacement)
    }
}
