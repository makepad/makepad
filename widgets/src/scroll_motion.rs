//! Shared kinetic-scroll ("fling") math used by the scrollable widgets (PortalList and
//! ScrollBar, and thus ScrollBars / ScrollXView / ScrollYView / ScrollXYView).
//!
//! Touch-drag flicks use the iOS `UIScrollView` momentum model: velocity decays as
//! `v *= DECEL_RATE^(elapsed_ms)`, and each frame's displacement is the integral of that
//! decay, so the motion is smooth and frame-rate-independent. Trackpad scrolling follows
//! the OS momentum stream exactly instead: each delta is applied as it arrives, and the
//! OS owns the deceleration and stops the stream when the pad is touched.

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

/// How long (in seconds) after the last applied OS momentum delta a trackpad coast still
/// counts as live. Momentum events arrive at display refresh rate, so a stream silent for
/// this long has stopped reaching the widget: it ended, or the pointer or window routing
/// changed mid-coast without a final event.
pub const COAST_STREAM_TIMEOUT: f64 = 0.2;

/// How long (in seconds) after a trackpad touch stops live scroll motion its own
/// press still counts as that stop rather than a click. The touch and the press are
/// separate events (the press is the tap's click, delivered or synthesized at finger
/// lift), so this only bridges one tap's internal latency. It is single-use and armed
/// only when the touch interrupted real motion, so a stationary list never consumes
/// a press.
pub const CATCH_PRESS_WINDOW: f64 = 0.4;

/// A trackpad touch that catches a coast makes the OS end its momentum stream, but the
/// end event and the touch event can be delivered in either order. If the end arrives
/// first it clears the coasting state, so the touch handler must still count a stream
/// cut this recently (in seconds) as live motion. The two events come from the same
/// physical touch, so the real gap is a few milliseconds.
pub const MOMENTUM_CUT_TOUCH_WINDOW: f64 = 0.1;

/// The rubber-band edge bounce follows Chrome's model on macOS
/// (`cc/input/elastic_overscroll_controller_exponential.cc`):
/// * a bounce from momentum animates as `x(t) = (x0 + v0·t·A)·e^(−S·t/P)`,
///   so the overshoot is proportional to the velocity remaining at the edge;
/// * a finger-driven stretch displays the accumulated overscroll divided by `S`.
pub const RUBBER_BAND_STIFFNESS: f64 = 20.0;
pub const RUBBER_BAND_AMPLITUDE: f64 = 0.31;
pub const RUBBER_BAND_PERIOD: f64 = 1.6;

/// How much raw finger travel it takes to produce one pixel of displayed stretch.
/// Lower is more sensitive. Split from [`RUBBER_BAND_STIFFNESS`] (which also sets the
/// spring's decay rate) so the stretch feel can be tuned without changing the bounce.
pub const RUBBER_BAND_STRETCH_STIFFNESS: f64 = 12.0;

/// The lifecycle of the OS trackpad momentum stream for one scrollable widget.
///
/// The OS owns the deceleration: after fingers lift it streams `Momentum` deltas until the
/// coast fades out at rest or a trackpad touch cuts it. This enum is the single record of
/// where that stream stands, replacing a family of booleans and timestamps whose stale
/// combinations repeatedly misreported whether content was moving. Every transition is an
/// explicit state change, so evidence (a live coast, a fresh cut) is carried forward
/// instead of erased by whichever event happens to be delivered first.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum MomentumStream {
    /// No stream is expected or running.
    #[default]
    Idle,
    /// Fingers lifted from a scroll gesture at `since`; the OS stream may begin.
    Expected { since: f64 },
    /// Deltas are being applied: the content is moving while `scroll_state` stays
    /// `Stopped`. The stream can stop reaching the widget without a final event
    /// (pointer or window routing can change mid-coast), so liveness also requires
    /// the last delta to be recent (see [`MomentumStream::is_live`]).
    Live {
        /// Wall-clock time of the newest applied delta.
        last_delta_time: f64,
        /// Speed (px/s) of the newest applied delta; seeds the rubber-band bounce
        /// when the coast reaches an edge.
        velocity: f64,
        /// Sign of the applied deltas, so the draw can end the coast the instant
        /// it reaches the edge the coast was headed toward.
        direction: f64,
    },
    /// Pinned at a non-bouncing edge with the stream still running: deltas are
    /// consumed with no visible effect and presses are ordinary clicks. If content
    /// grows past the edge (e.g. pagination prepending items), the stream's next
    /// delta moves the content again, so the original flick continues naturally.
    /// `last_delta_time` is refreshed by each consumed delta; a parked stream whose
    /// deltas stop arriving (rerouted mid-coast, end event lost) expires instead of
    /// staying armed for some later, unrelated stream.
    Pinned { last_delta_time: f64 },
    /// The stream ended at `at` while still live (a touch cut it, or it faded on
    /// its final delta). The touch that cut it can be delivered after the end
    /// event, so the record lets that touch see the motion it stopped.
    Cut { at: f64 },
}

impl MomentumStream {
    /// Whether the stream is moving content at `time`: live, with a recent delta.
    pub fn is_live(&self, time: f64) -> bool {
        match self {
            Self::Live { last_delta_time, .. } => time - last_delta_time < COAST_STREAM_TIMEOUT,
            _ => false,
        }
    }

    /// Whether the stream was cut within the cut-to-touch pairing window before `time`.
    pub fn cut_near(&self, time: f64) -> bool {
        match self {
            Self::Cut { at } => time - at < MOMENTUM_CUT_TOUCH_WINDOW,
            _ => false,
        }
    }

    /// Whether an OS momentum delta arriving at `time` belongs to this widget's
    /// gesture and should be processed. `Cut`/`Idle` streams stay dead: a stray
    /// delta (e.g. from a stream a press already caught) must not restart motion.
    /// A `Pinned` stream must be receiving deltas continuously to stay armed.
    pub fn accepts_deltas(&self, time: f64) -> bool {
        match self {
            Self::Expected { .. } | Self::Live { .. } => true,
            Self::Pinned { last_delta_time } => {
                time - last_delta_time < COAST_STREAM_TIMEOUT
            }
            _ => false,
        }
    }

    /// The time base for a velocity estimate of the next delta, if one exists.
    pub fn prev_delta_time(&self) -> Option<f64> {
        match self {
            Self::Expected { since } => Some(*since),
            Self::Live { last_delta_time, .. } => Some(*last_delta_time),
            Self::Pinned { last_delta_time } => Some(*last_delta_time),
            _ => None,
        }
    }
}

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

    /// Whether this fling may overscroll into the pulldown bounce.
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
