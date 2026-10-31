//! The app-wide clock policy: GSAP's `gsap.ticker` (lag smoothing, global
//! time scale, pause) as plain `Copy` data with no platform dependency.

/// GSAP `ticker.lagSmoothing(threshold, adjusted)`: a frame delta above
/// `threshold` counts as `adjusted`, so a stall does not make motion jump.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LagSmoothing {
    /// Deltas above this (seconds) are replaced.
    pub threshold: f64,
    /// What they are replaced with (seconds).
    pub adjusted: f64,
}

impl LagSmoothing {
    /// Caps every delta at `max` seconds.
    pub const fn clamp(max: f64) -> Self {
        Self {
            threshold: max,
            adjusted: max,
        }
    }

    /// GSAP's default (`lagSmoothing(500, 33)`): above 500 ms, count 33 ms.
    pub const GSAP: Self = Self {
        threshold: 0.5,
        adjusted: 0.033,
    };

    /// No smoothing (GSAP `lagSmoothing(0)`).
    pub const OFF: Self = Self {
        threshold: f64::INFINITY,
        adjusted: 0.0,
    };

    /// The smoothed delta for a raw one (negative raw deltas count as 0).
    #[inline]
    pub fn filter(self, raw: f64) -> f64 {
        let r = raw.max(0.0);
        if r > self.threshold {
            self.adjusted
        } else {
            r
        }
    }
}

/// App-wide playback controls for every tween host: GSAP's `gsap.ticker` and
/// `gsap.globalTimeline` (time scale, pause), plus the reduced-motion flag.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TweenTicker {
    /// Global time scale (GSAP `globalTimeline.timeScale()`), 1.0.
    pub time_scale: f64,
    /// Global pause (GSAP `globalTimeline.pause()`), false.
    pub paused: bool,
    /// Lag smoothing, `LagSmoothing::clamp(0.1)`.
    pub lag: LagSmoothing,
    /// Reduced motion requested (finish animations instead of playing them).
    pub reduced_motion: bool,
    /// Bumped by every change so hosts that hold still can re-arm.
    pub epoch: u64,
}

impl Default for TweenTicker {
    fn default() -> Self {
        Self {
            time_scale: 1.0,
            paused: false,
            lag: LagSmoothing::clamp(0.1),
            reduced_motion: false,
            epoch: 0,
        }
    }
}

/// How one clock turns raw frame deltas into animation time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClockPolicy {
    /// Lag smoothing for this clock; `None` uses the ticker's.
    pub lag: Option<LagSmoothing>,
    /// The delta of the first frame after the clock (re)starts.
    pub first_dt: f64,
    /// Whether the ticker's pause and time scale apply.
    pub follow_ticker: bool,
}

impl Default for ClockPolicy {
    fn default() -> Self {
        Self {
            lag: None,
            first_dt: 0.0,
            follow_ticker: true,
        }
    }
}

impl TweenTicker {
    /// The animation delta for a raw frame delta (`None` on the first frame
    /// after the clock was armed): lag-smoothed, then paused or scaled.
    pub fn dt(&self, raw: Option<f64>, p: ClockPolicy) -> f64 {
        let t = if p.follow_ticker {
            *self
        } else {
            TweenTicker::default()
        };
        let d = match raw {
            None => p.first_dt,
            Some(r) => p.lag.unwrap_or(t.lag).filter(r),
        };
        if t.paused {
            0.0
        } else {
            d * t.time_scale
        }
    }
}
