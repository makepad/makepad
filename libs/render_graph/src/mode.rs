//! Time for a rendered frame: [`RenderMode`] (realtime or locked), the
//! canonical sub-frame times of locked-time motion blur, and the adaptive
//! sub-frame schedule (KERNELS.md §3.4.2, PDOOM-PARITY R1-R3).
//!
//! Locked time never passes a float time around: a frame is an integer
//! index at a rational frame rate, a sub-frame is an integer index of an
//! integer count, and `t` is derived from them. Every system evaluated for
//! one sub-frame (document, kernels, stepper, renderer) gets the same `t`.
//!
//! **Adaptive motion blur.** Sub-frames come in nested ternary sets: 4
//! evenly spread over the shutter, then each step splits every interval in
//! three, adding a sub-frame either side of each old one (4, 12, 36, 108,
//! 324). Every prefix of 4·3^l offsets is evenly spread and centred on the
//! frame time, and so is each step's new set, so comparing the new set's
//! average with the old one's measures sampling error, not a shift in time.
//! After each step the host measures the displayed-value difference (worst
//! block, in 8-bit levels, see [`crate::accum`]); half of it is the
//! remaining error estimate, and the frame stops once that is below `tol`
//! or at `max`. The count depends only on rendered pixels, so it is
//! deterministic on one device.

/// A frame rate as a ratio (30000/1001 is NTSC 29.97).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Rational {
    pub num: u32,
    pub den: u32,
}

impl Rational {
    pub const fn new(num: u32, den: u32) -> Self {
        Self { num, den }
    }

    /// A whole frame rate (`fps(60)`).
    pub const fn fps(n: u32) -> Self {
        Self { num: n, den: 1 }
    }

    /// Frames per second as a float.
    pub fn as_f64(self) -> f64 {
        self.num as f64 / self.den.max(1) as f64
    }

    /// The start of frame `frame` in seconds, `frame · den / num` (exact
    /// for whole-second multiples).
    pub fn time_of(self, frame: u64) -> f64 {
        (frame as f64 * self.den.max(1) as f64) / self.num.max(1) as f64
    }

    /// The duration of one frame in seconds.
    pub fn frame_duration(self) -> f64 {
        self.den.max(1) as f64 / self.num.max(1) as f64
    }
}

/// How a frame's time is defined.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RenderMode {
    /// Interactive: time advances by the wall clock; history (auto
    /// exposure, TAA, fast GI) is allowed.
    Realtime { dt: f32 },
    /// Canonical time: frame `frame` at `fps`; `subframe` is `(k, S)` while
    /// rendering motion-blur sub-frame `k` of `S`; `seed` fixes every
    /// per-frame random choice. Every stateful subsystem must have a
    /// locked-time rule ([`crate::locked`]).
    LockedTime { frame: u64, fps: Rational, subframe: Option<(u32, u32)>, seed: u64 },
}

impl RenderMode {
    pub fn is_locked(&self) -> bool {
        matches!(self, RenderMode::LockedTime { .. })
    }

    /// The frame's own time (the shutter's centre). Realtime has none of
    /// its own: the host's clock is the time.
    pub fn frame_time(&self) -> Option<f64> {
        match *self {
            RenderMode::Realtime { .. } => None,
            RenderMode::LockedTime { frame, fps, .. } => Some(fps.time_of(frame)),
        }
    }

    /// The canonical time of this (sub-)frame: the frame time plus the
    /// sub-frame's midpoint offset over the open shutter,
    /// `t = frame/fps + ((k + 0.5)/S - 0.5) · shutter_deg/360 / fps`.
    pub fn canonical_t(&self, shutter_deg: f32) -> Option<f64> {
        match *self {
            RenderMode::Realtime { .. } => None,
            RenderMode::LockedTime { frame, fps, subframe, .. } => {
                let t = fps.time_of(frame);
                let u = match subframe {
                    Some((k, s)) if s > 0 => (k as f64 + 0.5) / s as f64 - 0.5,
                    _ => 0.0,
                };
                Some(t + u * shutter_open(shutter_deg, fps))
            }
        }
    }
}

/// How long the shutter is open for one frame, in seconds.
pub fn shutter_open(shutter_deg: f32, fps: Rational) -> f64 {
    (shutter_deg.max(0.0) as f64 / 360.0) * fps.frame_duration()
}

/// Where in the shutter frame-point post is read (shake, zoom, flash,
/// fade, invert, HUD: PDOOM-PARITY R3): 1/8 of the open shutter after the
/// frame time. Every adaptive set holds a sub-frame there (4 sub-frames:
/// the third).
pub const FRAME_POINT_U: f32 = 0.125;

/// Shutter offsets (-0.5..0.5) of an adaptive run's sub-frames in rendering
/// order, for `steps` refinements after the first 4: 4 evenly spread, then
/// each step adds a sub-frame either side of each old one.
pub fn ternary_offsets(steps: u32) -> Vec<f32> {
    let mut u: Vec<f32> = (0..4).map(|i| (i as f32 + 0.5) / 4.0 - 0.5).collect();
    let mut n = 4usize;
    for _ in 0..steps {
        for m in 0..n {
            let a = (3 * m) as f64;
            let d = (3 * n) as f64;
            u.push(((a + 0.5) / d - 0.5) as f32);
            u.push(((a + 2.5) / d - 0.5) as f32);
        }
        n *= 3;
    }
    u
}

/// The supersampling tap sub-frame `k` uses when taps cycle across the
/// shutter (every set a multiple of 4): rotated by k/4 so a tap does not
/// always land in the same part of the shutter (PDOOM-PARITY R2).
pub fn ss_tap(k: u32) -> u32 {
    (k + (k >> 2)) % 4
}

/// Adaptive sub-frame counts for one frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AdaptiveSampling {
    /// The fewest sub-frames (rounded to 4·3^l).
    pub min: u32,
    /// The most sub-frames (rounded to the nearest 4·3^l; a shot may cap
    /// it).
    pub max: u32,
    /// Stop once the estimated remaining error is below this many 8-bit
    /// display levels everywhere (worst 2x2-logical-pixel block).
    pub tol: f32,
}

impl Default for AdaptiveSampling {
    fn default() -> Self {
        Self { min: 4, max: 324, tol: 3.0 }
    }
}

/// The sub-frame count of one frame: a fixed count, or adaptive.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Sampling {
    Fixed(u32),
    Adaptive(AdaptiveSampling),
}

/// Which running sum a sub-frame adds into (see [`Schedule`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sum {
    /// The converged sum so far.
    Base,
    /// This step's new sub-frames, merged into `Base` after the measure.
    New,
}

/// What the host does next for this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Next {
    /// Render sub-frame `k` at shutter offset `u` (-0.5..0.5) and add it
    /// into `into`. `tap` is the supersampling tap (or None when taps do
    /// not cycle).
    Render { k: u32, u: f32, into: Sum, tap: Option<u32> },
    /// Measure how far the display moves when the `2n` sub-frames summed in
    /// `New` join the `n` in `Base`, then call [`Schedule::measured`].
    Measure { n: u32 },
    /// Every sub-frame is in `Base` (after the last merge): average by `n`.
    Done { n: u32 },
}

/// The sub-frame schedule of one frame, as a state machine the host steps
/// (one sub-frame per paint; a measure waits for a readback).
#[derive(Clone, Debug)]
pub struct Schedule {
    offsets: Vec<f32>,
    adaptive: Option<AdaptiveSampling>,
    /// Sub-frames converged in `Base`.
    n: u32,
    /// The next sub-frame to render.
    k: u32,
    /// The last level: `4·3^hi` sub-frames at most.
    hi: u32,
    level: u32,
    cycle: bool,
    waiting: bool,
    done: bool,
    /// Estimated error after each step (levels), for logs.
    pub errors: Vec<f32>,
}

fn lg3_quarter(x: u32) -> f64 {
    (x.max(1) as f64 / 4.0).ln() / 3f64.ln()
}

impl Schedule {
    pub fn new(sampling: Sampling) -> Self {
        match sampling {
            Sampling::Fixed(n) => {
                let n = n.max(1);
                let offsets = (0..n).map(|k| (k as f32 + 0.5) / n as f32 - 0.5).collect();
                Self { offsets, adaptive: None, n, k: 0, hi: 0, level: 0, cycle: n % 4 == 0, waiting: false, done: false, errors: Vec::new() }
            }
            Sampling::Adaptive(a) => {
                let lo = lg3_quarter(a.min.max(4)).round().max(0.0) as u32;
                let hi = lg3_quarter(a.max.max(4)).round().max(lo as f64) as u32;
                let n = 4 * 3u32.pow(lo);
                Self { offsets: ternary_offsets(hi), adaptive: Some(a), n, k: 0, hi, level: lo, cycle: true, waiting: false, done: false, errors: Vec::new() }
            }
        }
    }

    /// Shutter offsets in rendering order (all that may be rendered).
    pub fn offsets(&self) -> &[f32] {
        &self.offsets
    }

    /// The sub-frame whose shutter offset is nearest [`FRAME_POINT_U`]
    /// among the first `n` (ties: the later one): frame-point post reads
    /// its evaluation.
    pub fn frame_point(offsets: &[f32]) -> usize {
        let mut best = 0;
        let mut nearest = f32::INFINITY;
        for (k, &u) in offsets.iter().enumerate() {
            let d = (u - FRAME_POINT_U).abs();
            if d < nearest - 1e-7 || (d < nearest + 1e-7 && u > FRAME_POINT_U) {
                nearest = d;
                best = k;
            }
        }
        best
    }

    pub fn next(&mut self) -> Next {
        if self.done {
            return Next::Done { n: self.n };
        }
        if self.waiting {
            return Next::Measure { n: self.n };
        }
        // The first set goes into Base; each later step's 2n into New.
        let k = self.k;
        let tap = self.cycle.then(|| ss_tap(k));
        if k < self.n {
            self.k += 1;
            return Next::Render { k, u: self.offsets[k as usize], into: Sum::Base, tap };
        }
        if self.adaptive.is_none() || self.level >= self.hi {
            self.done = true;
            return Next::Done { n: self.n };
        }
        if k < 3 * self.n {
            self.k += 1;
            return Next::Render { k, u: self.offsets[k as usize], into: Sum::New, tap };
        }
        self.waiting = true;
        Next::Measure { n: self.n }
    }

    /// The measured display difference (8-bit levels, worst block) between
    /// the `n` sub-frames in `Base` and the `2n` in `New`. The host has
    /// merged `New` into `Base` (or does so now). Returns whether the frame
    /// is done.
    pub fn measured(&mut self, diff_levels: f32) -> bool {
        let Some(a) = self.adaptive else { return true };
        self.waiting = false;
        // 2/3 of the gap is how far the merged average moved; half of that
        // is the error estimate (e·(1/3 + 1/9 + …)).
        let err = if diff_levels.is_finite() { diff_levels * (2.0 / 3.0) / 2.0 } else { f32::INFINITY };
        self.errors.push(err);
        self.n *= 3;
        self.level += 1;
        if err < a.tol || self.level >= self.hi {
            self.done = true;
        }
        self.done
    }

    /// Sub-frames converged so far.
    pub fn count(&self) -> u32 {
        self.n
    }

    pub fn is_done(&self) -> bool {
        self.done
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_time_is_derived_from_integers() {
        let m = RenderMode::LockedTime { frame: 90, fps: Rational::fps(60), subframe: None, seed: 0 };
        assert_eq!(m.canonical_t(180.0), Some(1.5));
        // Sub-frame 0 of 4 at 180 degrees: the shutter is 1/120 s open,
        // the first midpoint is 3/8 of it before the centre.
        let m = RenderMode::LockedTime { frame: 90, fps: Rational::fps(60), subframe: Some((0, 4)), seed: 0 };
        let t = m.canonical_t(180.0).unwrap();
        assert!((t - (1.5 - 0.375 / 120.0)).abs() < 1e-12);
        let ntsc = RenderMode::LockedTime { frame: 30000, fps: Rational::new(30000, 1001), subframe: None, seed: 0 };
        assert_eq!(ntsc.frame_time(), Some(1001.0));
        assert_eq!(RenderMode::Realtime { dt: 0.016 }.canonical_t(180.0), None);
    }

    #[test]
    fn ternary_sets_nest_and_stay_centred() {
        let u = ternary_offsets(4);
        assert_eq!(u.len(), 324);
        // Every prefix of 4·3^l offsets is the evenly spread midpoint set
        // (k + 0.5)/S - 0.5, and so is each step's new 2n.
        for (l, s) in [4usize, 12, 36, 108, 324].into_iter().enumerate() {
            let mut prefix: Vec<f64> = u[..s].iter().map(|&x| x as f64).collect();
            prefix.sort_by(|a, b| a.partial_cmp(b).unwrap());
            for (k, x) in prefix.iter().enumerate() {
                assert!((x - ((k as f64 + 0.5) / s as f64 - 0.5)).abs() < 1e-6, "level {l} k {k}");
            }
            let mean: f64 = prefix.iter().sum::<f64>() / s as f64;
            assert!(mean.abs() < 1e-6);
            if l > 0 {
                let fresh: f64 = u[s / 3..s].iter().map(|&x| x as f64).sum::<f64>() / (2 * s / 3) as f64;
                assert!(fresh.abs() < 1e-6, "the new set is centred too");
            }
        }
        // The frame point is in every set: the third of 4 at +1/8.
        assert_eq!(Schedule::frame_point(&u[..4]), 2);
        assert_eq!(u[Schedule::frame_point(&u[..12])], 0.125);
    }

    #[test]
    fn a_still_frame_stops_after_one_step() {
        let mut s = Schedule::new(Sampling::Adaptive(AdaptiveSampling::default()));
        let mut rendered = Vec::new();
        loop {
            match s.next() {
                Next::Render { k, into, .. } => rendered.push((k, into)),
                Next::Measure { n } => {
                    assert_eq!(n, 4);
                    s.measured(0.0);
                }
                Next::Done { n } => {
                    assert_eq!(n, 12);
                    break;
                }
            }
        }
        assert_eq!(rendered.len(), 12);
        assert!(rendered[..4].iter().all(|r| r.1 == Sum::Base));
        assert!(rendered[4..].iter().all(|r| r.1 == Sum::New));
    }

    #[test]
    fn a_whip_goes_to_the_cap() {
        let mut s = Schedule::new(Sampling::Adaptive(AdaptiveSampling { min: 4, max: 324, tol: 3.0 }));
        let mut count = 0;
        let mut measures = 0;
        loop {
            match s.next() {
                Next::Render { k, .. } => {
                    assert_eq!(k, count);
                    count += 1;
                }
                Next::Measure { .. } => {
                    measures += 1;
                    s.measured(40.0);
                }
                Next::Done { n } => {
                    assert_eq!(n, 324);
                    break;
                }
            }
        }
        assert_eq!(count, 324);
        assert_eq!(measures, 4);
        // A per-shot cap of 50 rounds to 36.
        let mut s = Schedule::new(Sampling::Adaptive(AdaptiveSampling { min: 4, max: 50, tol: 3.0 }));
        let n = loop {
            match s.next() {
                Next::Measure { .. } => {
                    s.measured(f32::NAN);
                }
                Next::Done { n } => break n,
                _ => {}
            }
        };
        assert_eq!(n, 36);
    }

    #[test]
    fn fixed_counts_render_midpoints_once() {
        let mut s = Schedule::new(Sampling::Fixed(3));
        let mut u = Vec::new();
        loop {
            match s.next() {
                Next::Render { u: x, tap, .. } => {
                    assert_eq!(tap, None);
                    u.push(x);
                }
                Next::Done { n } => {
                    assert_eq!(n, 3);
                    break;
                }
                Next::Measure { .. } => panic!("fixed counts do not measure"),
            }
        }
        assert_eq!(u.len(), 3);
        for (a, b) in u.iter().zip([-1.0f32 / 3.0, 0.0, 1.0 / 3.0]) {
            assert!((a - b).abs() < 1e-6);
        }
        let one = Schedule::new(Sampling::Fixed(1));
        assert_eq!(one.offsets(), &[0.0]);
    }

    #[test]
    fn taps_cycle_through_all_four() {
        let taps: Vec<u32> = (0..8).map(ss_tap).collect();
        assert_eq!(taps, vec![0, 1, 2, 3, 1, 2, 3, 0]);
    }
}
