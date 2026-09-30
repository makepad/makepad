//! Time on a fixed grid (KERNELS.md §3.6.1).
//!
//! A sim steps by a rational `dt` and lands on arbitrary frame and shutter
//! times through an explicit mapping: `step(t) = floor(t / dt)`, the pose
//! between two steps is interpolated by `alpha = (t - step(t)·dt) / dt`.
//! Both are integer arithmetic: a time is a whole number of ticks at
//! [`TICKS_PER_SECOND`], which every common frame rate (24, 25, 30, 48, 50,
//! 60, 120, 240 and the 1001 rates), the motion-blur sub-frames of the
//! render graph's sets (4·3^l up to 324, at 90°, 180° and 360° shutters)
//! and every step of 1/n s for n dividing it land on exactly. Any other
//! time is snapped to the nearest tick (under 2 ns away), the same way on
//! every call, so a float frame time (`frame / fps` computed in f64)
//! recovers its exact tick.

/// Ticks per second: 2^8 · 3^5 · 5^4 · 7.
pub const TICKS_PER_SECOND: i64 = 272_160_000;

/// A point in sim time: whole ticks (negative before the start).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SimTime {
    ticks: i64,
}

impl SimTime {
    pub const ZERO: SimTime = SimTime { ticks: 0 };

    pub const fn from_ticks(ticks: i64) -> Self {
        Self { ticks }
    }

    pub const fn ticks(self) -> i64 {
        self.ticks
    }

    /// Seconds, snapped to the nearest tick. Non-finite times are 0; times
    /// beyond ±10^9 s saturate.
    pub fn from_secs(secs: f64) -> Self {
        if !secs.is_finite() {
            return Self::ZERO;
        }
        let t = (secs * TICKS_PER_SECOND as f64).round();
        let lim = 1.0e9 * TICKS_PER_SECOND as f64;
        Self { ticks: t.clamp(-lim, lim) as i64 }
    }

    /// `num / den` seconds, exact when `den` divides [`TICKS_PER_SECOND`]
    /// times `num`'s factors, rounded to the nearest tick otherwise.
    pub fn from_ratio(num: i64, den: u64) -> Self {
        let den = den.max(1) as i128;
        let n = num as i128 * TICKS_PER_SECOND as i128;
        Self { ticks: div_round(n, den) as i64 }
    }

    /// The start of frame `frame` at `fps_num / fps_den` frames per second.
    pub fn frame(frame: i64, fps_num: u32, fps_den: u32) -> Self {
        Self::from_ratio_i128(frame as i128 * fps_den.max(1) as i128, fps_num.max(1) as i128)
    }

    /// Sub-frame `k` of `count` over a shutter open `shutter_deg` of the
    /// frame (the render graph's canonical time: the midpoint of the k-th
    /// slice, `frame + ((k + 0.5)/count - 0.5) · shutter/360` frames).
    pub fn subframe(frame: i64, fps_num: u32, fps_den: u32, k: u32, count: u32, shutter_deg: u32) -> Self {
        let count = count.max(1) as i128;
        // In units of 1 / (2 · count · 360) frames.
        let u = (2 * k as i128 + 1 - count) * shutter_deg as i128;
        let num = (frame as i128 * 2 * count * 360 + u) * fps_den.max(1) as i128;
        let den = 2 * count * 360 * fps_num.max(1) as i128;
        Self::from_ratio_i128(num, den)
    }

    fn from_ratio_i128(num: i128, den: i128) -> Self {
        Self { ticks: div_round(num * TICKS_PER_SECOND as i128, den) as i64 }
    }

    pub fn as_secs(self) -> f64 {
        self.ticks as f64 / TICKS_PER_SECOND as f64
    }

    pub fn saturating_sub(self, o: SimTime) -> SimTime {
        SimTime { ticks: self.ticks.saturating_sub(o.ticks) }
    }

    pub fn saturating_add(self, o: SimTime) -> SimTime {
        SimTime { ticks: self.ticks.saturating_add(o.ticks) }
    }
}

/// Rounds `n / d` (d > 0) to the nearest integer, halves away from zero.
fn div_round(n: i128, d: i128) -> i128 {
    if n >= 0 {
        (n + d / 2) / d
    } else {
        -((-n + d / 2) / d)
    }
}

/// The step length, `num / den` seconds: exact, never a float.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct StepDt {
    num: u32,
    den: u32,
}

/// Why a step length was refused.
#[derive(Clone, Debug, PartialEq)]
pub enum DtError {
    Zero,
    /// Shorter than 1/10000 s or longer than 1/10 s.
    OutOfRange(f64),
    /// Not a whole number of ticks (its denominator shares no factor with
    /// the tick rate), so steps would drift against frame times.
    OffGrid(u32, u32),
}

impl std::fmt::Display for DtError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DtError::Zero => write!(f, "the step must be longer than 0"),
            DtError::OutOfRange(s) => write!(f, "a step of {s} s is outside 1/10000 .. 1/10 s"),
            DtError::OffGrid(n, d) => write!(f, "a step of {n}/{d} s does not divide the time grid; use 1/n with n dividing 272160000 (60, 120, 240, 480, 1000 ...)"),
        }
    }
}

impl StepDt {
    pub fn new(num: u32, den: u32) -> Result<Self, DtError> {
        if num == 0 || den == 0 {
            return Err(DtError::Zero);
        }
        let g = gcd(num as u64, den as u64) as u32;
        let (num, den) = (num / g, den / g);
        let secs = num as f64 / den as f64;
        if !(1.0 / 10000.0..=0.1).contains(&secs) {
            return Err(DtError::OutOfRange(secs));
        }
        if (TICKS_PER_SECOND as i128 * num as i128) % den as i128 != 0 {
            return Err(DtError::OffGrid(num, den));
        }
        Ok(Self { num, den })
    }

    /// The nearest `1/n` step to `secs` (a document writes `step: 1 / 240`,
    /// which the VM hands over as a float).
    pub fn from_secs(secs: f64) -> Result<Self, DtError> {
        if !(secs > 0.0) || !secs.is_finite() {
            return Err(DtError::Zero);
        }
        let n = (1.0 / secs).round();
        if (1.0 / n - secs).abs() > secs * 1e-4 {
            return Err(DtError::OffGrid(1, n as u32));
        }
        Self::new(1, n.clamp(1.0, u32::MAX as f64) as u32)
    }

    pub fn num(self) -> u32 {
        self.num
    }

    pub fn den(self) -> u32 {
        self.den
    }

    /// Ticks per step (exact by construction).
    pub fn ticks(self) -> i64 {
        (TICKS_PER_SECOND as i128 * self.num as i128 / self.den as i128) as i64
    }

    pub fn secs_f32(self) -> f32 {
        (self.num as f64 / self.den as f64) as f32
    }

    pub fn secs(self) -> f64 {
        self.num as f64 / self.den as f64
    }

    /// `floor(t / dt)`, 0 before the start (negative time is the initial
    /// state).
    pub fn step_floor(self, t: SimTime) -> u64 {
        if t.ticks <= 0 {
            return 0;
        }
        (t.ticks / self.ticks()) as u64
    }

    /// `ceil(t / dt)`: the first step at or after `t` (spawn times).
    pub fn step_ceil(self, t: SimTime) -> u64 {
        if t.ticks <= 0 {
            return 0;
        }
        let d = self.ticks();
        ((t.ticks + d - 1) / d) as u64
    }

    /// Where `t` lies between `step_floor(t)` and the step after (0..1).
    pub fn alpha(self, t: SimTime) -> f32 {
        if t.ticks <= 0 {
            return 0.0;
        }
        let d = self.ticks();
        (t.ticks % d) as f32 / d as f32
    }

    /// The time of step `step`.
    pub fn time_of(self, step: u64) -> SimTime {
        SimTime { ticks: (step as i64).saturating_mul(self.ticks()) }
    }

    /// Whole steps in `t` (rounded up), for durations.
    pub fn steps_in(self, t: SimTime) -> u64 {
        self.step_ceil(t)
    }

    /// Whole steps in a duration a kernel wrote as f32 seconds (a record's
    /// `delay`, `life`), rounded up, with a thousandth of a step of slack
    /// for the f32's rounding (0.05 · 4 is not quite 0.2).
    pub fn steps_in_f32(self, secs: f32) -> u64 {
        if !(secs > 0.0) {
            return 0;
        }
        (secs as f64 / self.secs() - 1e-3).ceil().max(0.0) as u64
    }
}

fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a.max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_times_land_on_ticks() {
        for (num, den) in [(24, 1), (25, 1), (30, 1), (60, 1), (120, 1), (30000, 1001), (24000, 1001), (60000, 1001)] {
            for f in [0i64, 1, 7, 1001, 123_457] {
                let exact = SimTime::frame(f, num, den);
                let secs = f as f64 * den as f64 / num as f64;
                assert_eq!(SimTime::from_secs(secs), exact, "{f} @ {num}/{den}");
                // Exact: frame · den · TPS is a multiple of num.
                assert_eq!(exact.ticks() as i128 * num as i128, f as i128 * den as i128 * TICKS_PER_SECOND as i128);
            }
        }
    }

    #[test]
    fn subframes_are_exact_and_centred() {
        // 4 sub-frames of a 180° shutter at 30 fps around frame 10.
        let f = SimTime::frame(10, 30, 1);
        let open = SimTime::from_ratio(1, 60);
        let s: Vec<SimTime> = (0..4).map(|k| SimTime::subframe(10, 30, 1, k, 4, 180)).collect();
        assert_eq!(s[0], f.saturating_sub(SimTime::from_ticks(open.ticks() * 3 / 8)));
        assert_eq!(s[3], f.saturating_add(SimTime::from_ticks(open.ticks() * 3 / 8)));
        // The deepest adaptive set, 324 sub-frames, stays on the grid.
        for k in 0..324 {
            let t = SimTime::subframe(3, 24, 1, k, 324, 180);
            let back = SimTime::from_secs(t.as_secs());
            assert_eq!(t, back);
        }
    }

    #[test]
    fn steps_floor_ceil_and_alpha() {
        let dt = StepDt::new(1, 240).unwrap();
        assert_eq!(dt.step_floor(SimTime::from_secs(-1.0)), 0);
        assert_eq!(dt.alpha(SimTime::from_secs(-1.0)), 0.0);
        assert_eq!(dt.step_floor(SimTime::from_ratio(1, 240)), 1);
        assert_eq!(dt.step_ceil(SimTime::from_ratio(1, 240)), 1);
        assert_eq!(dt.step_floor(SimTime::from_ratio(1, 30)), 8);
        let t = SimTime::from_ratio(1, 480);
        assert_eq!(dt.step_floor(t), 0);
        assert_eq!(dt.step_ceil(t), 1);
        assert_eq!(dt.alpha(t), 0.5);
        // A beat at 120 bpm (0.5 s) is step 120 exactly.
        assert_eq!(dt.step_ceil(SimTime::from_secs(0.5 * 16.0)), 1920);
        assert!(StepDt::new(1, 1008).is_ok());
        assert!(matches!(StepDt::new(1, 11), Err(DtError::OffGrid(1, 11))));
        assert!(matches!(StepDt::new(1, 5), Err(DtError::OutOfRange(_))));
        assert_eq!(StepDt::from_secs(1.0 / 240.0).unwrap(), dt);
    }
}
