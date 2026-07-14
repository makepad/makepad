//! Shared kinetic-scroll ("fling") math used by the scrollable widgets (PortalList and
//! ScrollBar, and thus ScrollBars / ScrollXView / ScrollYView / ScrollXYView).
//!
//! Touch-drag flicks use the iOS `UIScrollView` momentum model: velocity decays as
//! `v *= DECEL_RATE^(elapsed_ms)`, and each frame's displacement is the integral of
//! that decay, so the motion is smooth and frame-rate-independent. Hard flicks decay
//! more slowly and open with a glide, approximating how far native scrollers carry
//! them (see `FLING_FAST_DECEL_*` / `FLING_GLIDE_*`). An exact port of Android's
//! `OverScroller` fling spline — the same curve Chrome uses on Android — is also
//! here behind [`USE_ANDROID_FLING_SPLINE`] for A/B testing.
//!
//! Trackpad scrolling follows the OS momentum stream exactly instead: each delta is
//! applied as it arrives, and the OS owns the deceleration and stops the stream when
//! the pad is touched.

use std::sync::OnceLock;

/// Whether touch flicks animate along Android's native fling spline instead of the
/// exponential decay. Currently off: the spline's high-speed tail runs very long
/// (a maximum fling coasts ~2.6s, boosted ones several times longer), which felt
/// too strong in testing. Set to `cfg!(target_os = "android")` to A/B the native curve.
const USE_ANDROID_FLING_SPLINE: bool = false;

/// Default per-ms decay for a touch-drag flick (the widgets' `fling_decel` field). For
/// reference, iOS `UIScrollViewDecelerationRateNormal` is 0.998; we run a little firmer.
pub const FLING_DECEL_RATE_PER_MS: f64 = 0.997;

/// Hard flicks decay more slowly than gentle ones, blending from the widget's base
/// rate at [`FLING_FAST_DECEL_START`] px/s up to [`FLING_FAST_DECEL_RATE_PER_MS`] at
/// [`FLING_FAST_DECEL_FULL`] px/s. Native scrollers carry hard flicks far: Android's
/// fling spline travels ~ v^1.7 (a max fling covers ~7,200dp over ~2.6s) and iOS
/// decays at 0.998/ms, while a flat exponential tuned for pleasant gentle scrolling
/// sheds a hard flick's speed several times faster than either. With these values a
/// maximum flick glides a near-native distance and duration, easing back into the
/// familiar base decay as it slows.
pub const FLING_FAST_DECEL_START: f64 = 2_000.0;
pub const FLING_FAST_DECEL_FULL: f64 = 8_000.0;
pub const FLING_FAST_DECEL_RATE_PER_MS: f64 = 0.9985;

/// A fast fling first glides — decaying at [`FLING_GLIDE_RATE_PER_MS`], i.e.
/// holding nearly constant speed — easing into the regular decay over its first
/// [`FLING_GLIDE_TIME`] seconds. Android's fling spline similarly front-loads
/// its travel; without this, deceleration bites the moment the finger lifts and
/// a hard flick never feels like it reaches its speed. Scaled by the same speed
/// blend as the fast decay, so gentle flicks don't float.
pub const FLING_GLIDE_TIME: f64 = 0.25;
pub const FLING_GLIDE_RATE_PER_MS: f64 = 0.9997;

// The Android fling model, ported from AOSP's `OverScroller.SplineOverScroller`.
// A fling launched at velocity v travels a fixed total distance over a fixed duration
// (distance grows ~ v^1.7, duration ~ v^0.7; a maximum-strength fling covers ~7,200dp
// in ~2.6s), animated along a spline that front-loads the travel and eases out long.
const ANDROID_SCROLL_FRICTION: f64 = 0.015;
/// ln(0.78) / ln(0.9): the fling loses 22% of its speed for every 10% of remaining time.
const ANDROID_DECELERATION_RATE: f64 = 2.3582018154259448;
const ANDROID_INFLEXION: f64 = 0.35;
/// Earth gravity (m/s²) × inches-per-meter × 160dpi × Android's look-and-feel factor.
/// Android computes this with the device's real ppi because it works in physical
/// pixels; our touch coordinates are logical (physical ÷ density), which cancels the
/// density out of the fling equations exactly, so the 160dpi baseline (1 logical px =
/// 1dp) reproduces the native curve on every screen.
const ANDROID_PHYSICAL_COEFF: f64 = 9.80665 * 39.37 * 160.0 * 0.84;

const SPLINE_SAMPLES: usize = 100;
const SPLINE_START_TENSION: f64 = 0.5;
const SPLINE_END_TENSION: f64 = 1.0;
const SPLINE_P1: f64 = SPLINE_START_TENSION * ANDROID_INFLEXION;
const SPLINE_P2: f64 = 1.0 - SPLINE_END_TENSION * (1.0 - ANDROID_INFLEXION);

/// Fraction of the fling's total distance covered at each hundredth of its duration:
/// `OverScroller`'s `SPLINE_POSITION` table, generated the same way (for each time
/// fraction, bisect for the Bézier parameter, then evaluate the position curve there).
fn spline_position_table() -> &'static [f64; SPLINE_SAMPLES + 1] {
    static TABLE: OnceLock<[f64; SPLINE_SAMPLES + 1]> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut table = [1.0; SPLINE_SAMPLES + 1];
        let mut x_min = 0.0;
        for i in 0..SPLINE_SAMPLES {
            let alpha = i as f64 / SPLINE_SAMPLES as f64;
            let mut x_max = 1.0;
            loop {
                let x = x_min + (x_max - x_min) / 2.0;
                let coef = 3.0 * x * (1.0 - x);
                let tx = coef * ((1.0 - x) * SPLINE_P1 + x * SPLINE_P2) + x * x * x;
                if (tx - alpha).abs() < 1e-5 {
                    table[i] = coef * ((1.0 - x) * SPLINE_START_TENSION + x * SPLINE_END_TENSION)
                        + x * x * x;
                    break;
                }
                if tx > alpha {
                    x_max = x;
                } else {
                    x_min = x;
                }
            }
        }
        table
    })
}

/// The total signed travel (logical px) and duration (seconds) of an Android-model
/// fling released at `velocity` px/s: `OverScroller`'s `getSplineFlingDistance` and
/// `getSplineFlingDuration`.
fn android_fling_target(velocity: f64) -> (f64, f64) {
    if velocity == 0.0 {
        return (0.0, 0.0);
    }
    let friction_coeff = ANDROID_SCROLL_FRICTION * ANDROID_PHYSICAL_COEFF;
    let l = (ANDROID_INFLEXION * velocity.abs() / friction_coeff).ln();
    let decel_minus_one = ANDROID_DECELERATION_RATE - 1.0;
    let duration = (l / decel_minus_one).exp();
    let distance = friction_coeff * (ANDROID_DECELERATION_RATE / decel_minus_one * l).exp();
    (distance.copysign(velocity), duration)
}

/// EMA weight for the newest inter-frame interval sample (see [`Fling::step`]).
const FLING_DT_EMA_ALPHA: f64 = 0.15;

/// The band around the EMA'd frame interval that a raw `dt` is clamped to,
/// so one late/early frame cannot produce a visible jump or stall.
const FLING_DT_BAND: (f64, f64) = (0.5, 1.5);

/// Upper bound on a single integration step (seconds): a long hitch produces a small
/// catch-up rather than a huge jump.
const FLING_MAX_DT: f64 = 0.1;

/// How much history (in seconds) the release-velocity estimate covers, like a native
/// `VelocityTracker` (~100ms). The window is time-based, not event-count-based: mice
/// can report at 500Hz+, where a fixed count of samples would span only a few
/// milliseconds and turn the estimate into amplified instantaneous noise, launching
/// maximum-speed flings from tiny drags.
pub const FLING_SAMPLE_MAX_AGE: f64 = 0.1;

/// Hard cap on retained samples, bounding memory at extreme event rates.
pub const FLING_SAMPLE_CAP: usize = 64;

/// The minimum time span (seconds) the retained samples must cover for a release
/// velocity to be meaningful. A press that moved for less time than this is a jerk,
/// not a flick; its instantaneous velocity says nothing about intent.
pub const FLING_MIN_SAMPLE_SPAN: f64 = 0.01;

/// The release velocity is measured over this most-recent slice (seconds) of the
/// gesture, so it reflects how fast the finger was moving at lift-off — the way
/// native velocity trackers weight recent motion — rather than the average of the
/// whole gesture, which under-reads a flick that accelerates toward the end.
pub const FLING_VELOCITY_SPAN: f64 = 0.04;

/// How recently a fling must have been caught for a same-direction re-flick to
/// add the caught speed back (the "fling boost" that lets repeated flicks build
/// up speed, as Chrome on Android does). Measured from the catching touch to the
/// lift that re-flicks; holding longer is a deliberate stop.
pub const FLING_BOOST_MAX_DWELL: f64 = 0.4;

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

/// How long (in seconds) after the last finger-driven scroll delta a press still counts
/// as settling that scroll rather than clicking a child. Callers must track the last
/// delta as an `Option` and treat `None` (nothing has ever scrolled this widget) as not
/// live: a plain `f64` defaulting to `0.0` reads as "scrolled at time zero", which
/// swallows every press in the first `FINGER_SCROLL_SETTLE_WINDOW` of an app's life,
/// since app clocks also start at zero.
pub const FINGER_SCROLL_SETTLE_WINDOW: f64 = 0.15;

/// Whether a press at `time` lands close enough after the finger scroll at
/// `last_finger_scroll_time` to be settling it rather than clicking through.
///
/// `None` means nothing has scrolled the widget yet, so no press can be settling one.
/// Negative gaps are rejected too: a press timestamped before the scroll it would be
/// settling means the two came from different clocks, and answering "yes" there would
/// swallow presses indefinitely.
pub fn press_settles_finger_scroll(last_finger_scroll_time: Option<f64>, time: f64) -> bool {
    last_finger_scroll_time
        .is_some_and(|at| (0.0..FINGER_SCROLL_SETTLE_WINDOW).contains(&(time - at)))
}

/// The rubber-band edge overscroll has two feels, chosen by input:
///
/// Trackpad/wheel input follows Chrome's model on macOS
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

/// A finger on the screen (touch or mouse drag of the content, and the flicks they
/// release) follows the iOS `UIScrollView` rubber band instead, which is much looser
/// than the trackpad one:
/// * a drag of `raw` px past the edge shows `raw·c·d / (raw·c + d)` of it, where `d`
///   is the viewport extent times [`RUBBER_BAND_TOUCH_RANGE`] — just over half the
///   finger travel at first, flattening so the stretch never exceeds that range
///   ([`RUBBER_BAND_TOUCH_COEFF`] is `c`; iOS uses the full viewport as the range,
///   which lets the content be pulled farther than feels right here);
/// * released (or reached by a flick), it springs back along a critically damped
///   spring `x(t) = (x0 + (v0 + λ·x0)·t)·e^(−λ·t)`, whose overshoot carries the
///   full remaining velocity ([`RUBBER_BAND_TOUCH_DECAY`] is `λ`, per second).
///
/// A flick into an edge overshoots by `v/(λ·e)` px and the spring settles in
/// `~7.5/λ` seconds, so raising `λ` makes the bounce both shorter and snappier
/// (iOS is closest to λ≈14; we run firmer so the overshoot stays modest).
///
/// [`RUBBER_BAND_TOUCH_RANGE`] × the viewport extent is the hard limit on all
/// displayed overscroll: the drag stretch flattens toward it, and the widgets
/// clamp the spring-back overshoot to it, so no bounce ever travels farther.
pub const RUBBER_BAND_TOUCH_COEFF: f64 = 0.55;
pub const RUBBER_BAND_TOUCH_DECAY: f64 = 20.0;
pub const RUBBER_BAND_TOUCH_RANGE: f64 = 0.35;

/// The displayed overscroll for `raw` px of finger travel past the edge (signed), in
/// a viewport `extent` px long. `touch` picks the iOS curve; trackpad input keeps the
/// stiffer linear stretch.
pub fn stretch_displayed(raw: f64, extent: f64, touch: bool) -> f64 {
    if touch {
        let range = (extent * RUBBER_BAND_TOUCH_RANGE).max(1.0);
        let c = RUBBER_BAND_TOUCH_COEFF;
        (raw.abs() * c * range / (raw.abs() * c + range)).copysign(raw)
    } else {
        raw / RUBBER_BAND_STRETCH_STIFFNESS
    }
}

/// Inverse of [`stretch_displayed`]: the raw finger travel that shows as `displayed`.
/// Round-tripping through these lets a widget keep only the displayed stretch as
/// state while the finger stretches and unwinds along the same curve.
pub fn stretch_raw(displayed: f64, extent: f64, touch: bool) -> f64 {
    if touch {
        let range = (extent * RUBBER_BAND_TOUCH_RANGE).max(1.0);
        let c = RUBBER_BAND_TOUCH_COEFF;
        let d = displayed.abs().min(range * 0.999);
        (d * range / (c * (range - d))).copysign(displayed)
    } else {
        displayed * RUBBER_BAND_STRETCH_STIFFNESS
    }
}

/// Soften a touch bounce's seed velocity so its overshoot approaches the headroom
/// left under `max_overscroll` asymptotically instead of slamming into it: a gentle
/// bounce keeps nearly its full velocity, while a huge flick's is compressed so the
/// spring peaks smoothly below the limit rather than being clipped flat against it.
pub fn soften_bounce_velocity(v0: f64, x0: f64, max_overscroll: f64, touch: bool) -> f64 {
    if !touch || v0 == 0.0 {
        return v0;
    }
    let headroom = (max_overscroll - x0.abs()).max(0.0);
    if headroom <= 0.0 {
        return 0.0;
    }
    // A spring entering the edge at v peaks at v/(λ·e).
    let peak = v0.abs() / (RUBBER_BAND_TOUCH_DECAY * std::f64::consts::E);
    v0 * headroom / (peak + headroom)
}

/// A clock for the bounce animations that advances by jitter-clamped frame steps —
/// the same smoothing [`Fling::step`] applies — so a late or early frame becomes a
/// slightly uneven step instead of a visible jerk in the spring's fast early motion.
#[derive(Clone, Copy, Debug, Default)]
pub struct FrameClock {
    /// Wall-clock time of the previous frame (0.0 = not yet started).
    last_time: f64,
    /// Running EMA (seconds) of the inter-frame interval.
    dt_ema: f64,
    /// Smoothed elapsed time (seconds) since the first frame.
    t: f64,
}

/// The bounce clock's first frame advances by this nominal step rather than zero.
/// The spring is always seeded mid-motion (a fling hitting the edge, a finger
/// lifting off a stretch), so its first frame must move like every other frame:
/// spending it measuring the frame interval would freeze the content for one frame
/// right at the hand-off — a visible hitch at fling speeds. Real frame deltas take
/// over from the second frame.
const FRAME_CLOCK_FIRST_STEP: f64 = 1.0 / 60.0;

impl FrameClock {
    /// Whether [`FrameClock::advance`] has ever run; the first frame advances by
    /// the nominal step (see [`FRAME_CLOCK_FIRST_STEP`]).
    pub fn not_started(&self) -> bool {
        self.last_time <= 0.0
    }

    /// Advance to wall-clock `now`, returning the smoothed elapsed time.
    pub fn advance(&mut self, now: f64) -> f64 {
        if self.last_time <= 0.0 {
            self.last_time = now;
            self.t = FRAME_CLOCK_FIRST_STEP;
            return self.t;
        }
        let raw_dt = (now - self.last_time).clamp(0.0, FLING_MAX_DT);
        self.last_time = now;
        if self.dt_ema <= 0.0 {
            self.dt_ema = raw_dt;
        } else {
            self.dt_ema =
                self.dt_ema * (1.0 - FLING_DT_EMA_ALPHA) + raw_dt * FLING_DT_EMA_ALPHA;
        }
        self.t += raw_dt.clamp(self.dt_ema * FLING_DT_BAND.0, self.dt_ema * FLING_DT_BAND.1);
        self.t
    }
}

/// The bounce-back animation, evaluated `t` seconds after release: returns the
/// displayed overscroll and whether the spring is past its peak (callers should only
/// settle after the peak, so an overshoot isn't cut off while still growing).
/// `x0` is the stretch at release and `v0` the velocity still carrying into the
/// overscroll, both signed the same way.
pub fn rubber_band_bounce(x0: f64, v0: f64, t: f64, touch: bool) -> (f64, bool) {
    let (x, lambda) = if touch {
        let lambda = RUBBER_BAND_TOUCH_DECAY;
        ((x0 + (v0 + lambda * x0) * t) * (-lambda * t).exp(), lambda)
    } else {
        let lambda = RUBBER_BAND_STIFFNESS / RUBBER_BAND_PERIOD;
        ((x0 + v0 * RUBBER_BAND_AMPLITUDE * t) * (-lambda * t).exp(), lambda)
    };
    (x, t * lambda > 1.0)
}

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

/// Append a sample, retaining the last [`FLING_SAMPLE_MAX_AGE`] seconds of history
/// (capped at [`FLING_SAMPLE_CAP`] entries).
pub fn push_sample(samples: &mut Vec<ScrollSample>, abs: f64, time: f64) {
    samples.push(ScrollSample { abs, time });
    while samples.len() > FLING_SAMPLE_CAP
        || (samples.len() > 1 && time - samples[0].time > FLING_SAMPLE_MAX_AGE)
    {
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
    // Velocity comes from the last FLING_VELOCITY_SPAN of the gesture (falling
    // back to the oldest sample when the gesture is shorter), so it reflects the
    // speed at lift-off rather than the whole-gesture average.
    let release_velocity = if let Some(last) = samples.last() {
        let start = samples
            .iter()
            .find(|s| last.time - s.time <= FLING_VELOCITY_SPAN)
            .unwrap_or(last);
        let (start, dt) = if last.time - start.time > FLING_MIN_SAMPLE_SPAN {
            (start, last.time - start.time)
        } else if let Some(first) = samples.first() {
            (first, last.time - first.time)
        } else {
            (last, 0.0)
        };
        if dt > FLING_MIN_SAMPLE_SPAN {
            (last.abs - start.abs) / dt
        } else {
            0.0
        }
    } else {
        0.0
    };
    (release_velocity, total_delta)
}

/// One kinetic-scroll animation along a single scroll axis, stepped per frame so the
/// motion is smooth and frame-rate-independent. The curve is the platform's native one:
/// Android's fling spline, or exponentially decaying velocity everywhere else.
///
/// The `decay_rate_per_ms` is supplied by the caller (a widget `#[live]` field), so the
/// exponential feel is configurable per widget; the Android spline ignores it, since the
/// native curve is fully determined by the release velocity. Drive it once per animation
/// frame with [`Fling::step`], apply the returned displacement, and stop when
/// [`Fling::is_active`] returns false.
#[derive(Clone, Copy, Debug)]
pub struct Fling {
    /// Current velocity in pixels per second.
    pub velocity: f64,
    /// Per-millisecond velocity decay factor applied each step (exponential model only).
    decay_rate_per_ms: f64,
    /// Whether this fling may overscroll into the pulldown bounce (touch-drag) or clips at the
    /// edges (trackpad tail).
    overscroll: bool,
    /// Wall-clock time of the previous step (0.0 = not yet started).
    last_time: f64,
    /// Total animated time (seconds) since launch, driving the Android spline lookup.
    age: f64,
    /// Running EMA (seconds) of the inter-frame interval. The step is driven off a `dt`
    /// clamped to a tight band around this, so frame-delivery jitter (a late or early frame)
    /// does not turn into an uneven jump/stall in the motion.
    dt_ema: f64,
    /// Total signed travel of an Android-model fling (see [`android_fling_target`]).
    spline_distance: f64,
    /// Total duration (seconds) of an Android-model fling.
    spline_duration: f64,
    /// Travel already handed out by previous steps of an Android-model fling.
    spline_emitted: f64,
}

impl Default for Fling {
    fn default() -> Self {
        Self::new(0.0, FLING_DECEL_RATE_PER_MS)
    }
}

impl Fling {
    /// A touch-drag flick released at `velocity` px/s; may overscroll.
    pub fn new(velocity: f64, decay_rate_per_ms: f64) -> Self {
        let (spline_distance, spline_duration) = if USE_ANDROID_FLING_SPLINE {
            android_fling_target(velocity)
        } else {
            (0.0, 0.0)
        };
        Self {
            velocity,
            decay_rate_per_ms,
            overscroll: true,
            last_time: 0.0,
            dt_ema: 0.0,
            age: 0.0,
            spline_distance,
            spline_duration,
            spline_emitted: 0.0,
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
        if USE_ANDROID_FLING_SPLINE {
            self.age += dt;
            return Some(self.step_android_spline());
        }
        // Hard flicks carry farther: blend the decay rate toward the fast-fling
        // rate as speed rises (see FLING_FAST_DECEL_*), so gentle scrolling keeps
        // the base feel while a hard flick glides the way native scrollers do.
        let fast_rate = FLING_FAST_DECEL_RATE_PER_MS.max(self.decay_rate_per_ms);
        let speed_blend = ((self.velocity.abs() - FLING_FAST_DECEL_START)
            / (FLING_FAST_DECEL_FULL - FLING_FAST_DECEL_START))
            .clamp(0.0, 1.0);
        let rate = self.decay_rate_per_ms + (fast_rate - self.decay_rate_per_ms) * speed_blend;
        // A fast fling's opening stretch glides at nearly constant speed, easing
        // into the regular decay over FLING_GLIDE_TIME (see the constants above).
        let glide_rate = FLING_GLIDE_RATE_PER_MS.max(rate);
        let glide_blend = (1.0 - self.age / FLING_GLIDE_TIME).clamp(0.0, 1.0) * speed_blend;
        let rate = rate + (glide_rate - rate) * glide_blend;
        self.age += dt;
        let factor = rate.powf(dt * 1000.0);
        // v(t) = v0 * e^(-λt); displacement over dt = v0 * (1 - factor) / λ.
        let lambda = -rate.ln() * 1000.0;
        let displacement = self.velocity * (1.0 - factor) / lambda;
        self.velocity *= factor;
        Some(displacement)
    }

    /// One frame of the Android-model fling (`OverScroller.update`): the position comes
    /// straight from the spline table, and `velocity` is refreshed to the curve's current
    /// slope so callers that read it (edge bounce seeding, catching a fling to boost the
    /// next flick) see the true current speed.
    fn step_android_spline(&mut self) -> f64 {
        if self.spline_duration <= 0.0 || self.age >= self.spline_duration {
            self.velocity = 0.0;
            let rest = self.spline_distance - self.spline_emitted;
            self.spline_emitted = self.spline_distance;
            return rest;
        }
        let table = spline_position_table();
        let t = self.age / self.spline_duration;
        let index = ((SPLINE_SAMPLES as f64 * t) as usize).min(SPLINE_SAMPLES - 1);
        let t_inf = index as f64 / SPLINE_SAMPLES as f64;
        let t_sup = (index + 1) as f64 / SPLINE_SAMPLES as f64;
        let velocity_coef = (table[index + 1] - table[index]) / (t_sup - t_inf);
        let distance_coef = table[index] + (t - t_inf) * velocity_coef;
        let target = distance_coef * self.spline_distance;
        let displacement = target - self.spline_emitted;
        self.spline_emitted = target;
        self.velocity = velocity_coef * self.spline_distance / self.spline_duration;
        displacement
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_widget_that_never_scrolled_never_settles_a_press() {
        // Regression: this used to be a plain `f64` defaulting to 0.0, which read as
        // "finger-scrolled at time zero". App clocks also start at zero, so every press
        // in the first 150ms of an app's life was swallowed as a scroll-stop instead of
        // reaching the widget under it.
        assert!(!press_settles_finger_scroll(None, 0.0));
        assert!(!press_settles_finger_scroll(None, 0.05));
        assert!(!press_settles_finger_scroll(None, 10_000.0));
    }

    #[test]
    fn a_press_settles_only_a_scroll_inside_the_window() {
        assert!(press_settles_finger_scroll(Some(10.0), 10.0));
        assert!(press_settles_finger_scroll(
            Some(10.0),
            10.0 + FINGER_SCROLL_SETTLE_WINDOW / 2.0
        ));
        assert!(!press_settles_finger_scroll(
            Some(10.0),
            10.0 + FINGER_SCROLL_SETTLE_WINDOW
        ));
        assert!(!press_settles_finger_scroll(Some(10.0), 11.0));
    }

    #[test]
    fn a_press_older_than_the_scroll_does_not_settle_it() {
        // A negative gap means the press and the scroll were stamped from different
        // clocks. Treating that as "settling" would swallow presses indefinitely.
        assert!(!press_settles_finger_scroll(Some(1_784_000_000.0), 5.0));
    }
}
