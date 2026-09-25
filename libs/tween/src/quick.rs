//! `QuickTo`: a single retargetable tween with no engine (GSAP `gsap.quickTo`).
//!
//! It holds one value moving from `from` to `to` along an ease; calling
//! [`QuickTo::aim`] again mid-run restarts from wherever the value is now.
//! It is `Copy`, never allocates and never rounds, which makes it the
//! building block for small widget-local motions.

use crate::easing::Easing;

/// A value `QuickTo` can move: interpolation plus a distance for
/// [`Retime::ByDistance`].
pub trait Lerp: Copy + PartialEq {
    /// `a` moved `t` of the way to `b` (`t` may leave 0..1 when eased).
    fn lerp(a: Self, b: Self, t: f64) -> Self;
    /// How far apart `a` and `b` are.
    fn distance(a: Self, b: Self) -> f64;
}

impl Lerp for f64 {
    #[inline]
    fn lerp(a: Self, b: Self, t: f64) -> Self {
        a + (b - a) * t
    }

    #[inline]
    fn distance(a: Self, b: Self) -> f64 {
        (b - a).abs()
    }
}

impl<const N: usize> Lerp for [f64; N] {
    #[inline]
    fn lerp(a: Self, b: Self, t: f64) -> Self {
        let mut out = a;
        for (o, (x, y)) in out.iter_mut().zip(a.iter().zip(b.iter())) {
            *o = x + (y - x) * t;
        }
        out
    }

    /// The largest per-lane distance.
    #[inline]
    fn distance(a: Self, b: Self) -> f64 {
        a.iter()
            .zip(b.iter())
            .fold(0.0, |m, (x, y)| m.max((y - x).abs()))
    }
}

/// How a retarget sets the run's duration.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Retime {
    /// Always the full duration (GSAP `quickTo`).
    Full,
    /// The duration scaled by the distance left, `min(distance / per_unit, 1)`:
    /// turning back halfway takes half as long.
    ByDistance {
        /// The distance that takes the full duration.
        per_unit: f64,
    },
}

/// One value on its way to a target: GSAP `gsap.quickTo(target, prop)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct QuickTo<V: Lerp> {
    from: V,
    to: V,
    t: f64,
    secs: f64,
    ease: Easing,
}

impl<V: Lerp + Default> Default for QuickTo<V> {
    /// At rest on `V::default()` (for `#[rust]` widget fields).
    fn default() -> Self {
        Self::at(V::default())
    }
}

impl<V: Lerp> QuickTo<V> {
    /// At rest on `v`.
    pub fn at(v: V) -> Self {
        Self {
            from: v,
            to: v,
            t: 1.0,
            secs: 0.0,
            ease: Easing::Linear,
        }
    }

    /// Heads for `to` from the current value. Aiming at the current target
    /// changes nothing and answers `false`, so a caller may aim every frame.
    /// A run of zero length (or zero `secs`) lands at once.
    pub fn aim(&mut self, to: V, secs: f64, ease: Easing, r: Retime) -> bool {
        if to == self.to {
            return false;
        }
        let now = self.value();
        let scale = match r {
            Retime::Full => 1.0,
            Retime::ByDistance { per_unit } => (V::distance(now, to) / per_unit).min(1.0),
        };
        self.start(now, to, secs.max(0.0) * scale, ease);
        true
    }

    /// Always starts a new run from the current value, even towards the
    /// same target.
    pub fn restart(&mut self, to: V, secs: f64, ease: Easing) {
        let now = self.value();
        self.start(now, to, secs.max(0.0), ease);
    }

    /// Starts a new run from an explicit value (GSAP `fromTo`).
    pub fn retarget_from(&mut self, from: V, to: V, secs: f64, ease: Easing) {
        self.start(from, to, secs.max(0.0), ease);
    }

    fn start(&mut self, from: V, to: V, secs: f64, ease: Easing) {
        self.from = from;
        self.to = to;
        self.secs = secs;
        self.ease = ease;
        self.t = if secs <= 0.0 { 1.0 } else { 0.0 };
    }

    /// Advances the run by `dt` seconds. Answers whether it goes on; the
    /// step that lands answers `false` and leaves the value exactly on target.
    pub fn step(&mut self, dt: f64) -> bool {
        if self.t >= 1.0 {
            false
        } else if self.secs <= 0.0 {
            self.t = 1.0;
            false
        } else {
            self.t = (self.t + dt / self.secs).min(1.0);
            self.t < 1.0
        }
    }

    /// The current value: exactly the target once the run is over, otherwise
    /// `lerp(from, to, ease(t))` (so an overshooting ease leaves the range).
    pub fn value(&self) -> V {
        if self.t >= 1.0 {
            self.to
        } else {
            V::lerp(self.from, self.to, self.ease.map(self.t))
        }
    }

    /// Ends the run on its target now.
    pub fn settle(&mut self) {
        self.t = 1.0;
    }

    /// Where the current run started.
    pub fn from_value(&self) -> V {
        self.from
    }

    /// Where the current run ends.
    pub fn target(&self) -> V {
        self.to
    }

    /// Linear progress of the run, 0..=1.
    pub fn progress(&self) -> f64 {
        self.t
    }

    /// The run's duration in seconds.
    pub fn duration(&self) -> f64 {
        self.secs
    }

    /// The run's ease.
    pub fn ease(&self) -> Easing {
        self.ease
    }

    /// Whether the run is over.
    pub fn is_settled(&self) -> bool {
        self.t >= 1.0
    }
}
