//! QuickTo: bit parity with pill_nav's UnitTween and the retargeting rules.

mod common;

use common::{to_easing, Ease, Rng, THEME_EASES};
use makepad_tween::{Easing, Lerp, QuickTo, Retime};

// ---- oracle: a verbatim copy of widgets/src/pill_nav.rs UnitTween ----

/// A number on its way to one end of 0..1 along one of the theme's easings:
/// the panel's grow and the pill's fade here, the line menu's reveal. The
/// clock runs straight and the ease shapes what is read off it, so a spring
/// or a bounce can carry the value past its end and back while the clock
/// still says how much of the run is left.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UnitTween {
    from: f64,
    to: f64,
    /// 0 when the run starts, 1 once it is over.
    t: f64,
    secs: f64,
    ease: Ease,
    /// The furthest the value has come toward `to` this run, held to 0..1.
    peak: f64,
}

impl Default for UnitTween {
    fn default() -> Self {
        Self::at(0.0)
    }
}

impl UnitTween {
    /// At rest on `value`.
    pub fn at(value: f64) -> Self {
        Self {
            from: value,
            to: value,
            t: 1.0,
            secs: 0.0,
            ease: Ease::Linear,
            peak: value.clamp(0.0, 1.0),
        }
    }

    /// Head for `to` along `ease`. A run from one end to the other takes
    /// `secs` and a run from part way takes that share of it, so turning back
    /// mid-run starts from where the value is and takes only as long as the
    /// way back is. Aiming where it already heads changes nothing, so a
    /// caller can aim on every frame.
    pub fn aim(&mut self, to: f64, secs: f64, ease: Ease) {
        if to == self.to {
            return;
        }
        let now = self.value();
        self.secs = secs.max(0.0) * (to - now).abs().min(1.0);
        self.from = now;
        self.to = to;
        self.ease = ease;
        self.peak = now.clamp(0.0, 1.0);
        self.t = 0.0;
        if self.secs <= 0.0 {
            self.settle();
        }
    }

    /// Advance the clock by `dt` seconds. Answers whether the run goes on.
    pub fn step(&mut self, dt: f64) -> bool {
        if self.t >= 1.0 {
            return false;
        }
        if self.secs <= 0.0 {
            self.settle();
            return false;
        }
        self.t = (self.t + dt / self.secs).min(1.0);
        if self.t >= 1.0 {
            self.settle();
            return false;
        }
        let now = self.value().clamp(0.0, 1.0);
        self.peak = if self.to >= self.from {
            self.peak.max(now)
        } else {
            self.peak.min(now)
        };
        true
    }

    /// End the run on its target now. For motion that has been switched off.
    pub fn settle(&mut self) {
        *self = Self::at(self.to);
    }

    /// Where the value is now, past either end while an overshooting ease
    /// carries it. Exactly the target once the run is over: a curve read at
    /// its last sample can land a hair off.
    pub fn value(&self) -> f64 {
        if self.t >= 1.0 {
            return self.to;
        }
        self.from + (self.to - self.from) * self.ease.map(self.t)
    }

    /// The furthest the value has come this run, held to 0..1 and never going
    /// back: what a fade reads, so it does not flicker while a bounce dips.
    pub fn reached(&self) -> f64 {
        self.peak
    }

    pub fn target(&self) -> f64 {
        self.to
    }

    /// Resting on `value`, with nothing of the run left.
    pub fn is_at(&self, value: f64) -> bool {
        self.t >= 1.0 && self.to == value
    }
}

// ---- end of oracle ----

/// The retiming pill_nav's UnitTween uses: a full run is one unit.
const UNIT: Retime = Retime::ByDistance { per_unit: 1.0 };

fn assert_same(oracle: &UnitTween, quick: &QuickTo<f64>, what: &str) {
    assert_eq!(
        quick.value().to_bits(),
        oracle.value().to_bits(),
        "{what}: value"
    );
    assert_eq!(quick.target(), oracle.target(), "{what}: target");
    assert_eq!(
        quick.is_settled() && quick.target() == 0.0,
        oracle.is_at(0.0),
        "{what}: at 0"
    );
    assert_eq!(
        quick.is_settled() && quick.target() == 1.0,
        oracle.is_at(1.0),
        "{what}: at 1"
    );
}

#[test]
fn quick_to_reproduces_unit_tween() {
    for (k, ease) in THEME_EASES.iter().enumerate() {
        for seed in 0..8u64 {
            let mut rng = Rng(seed * 1000 + k as u64);
            let mut oracle = UnitTween::at(0.0);
            let mut quick = QuickTo::at(0.0);
            let secs = [0.25, 0.4, 0.0, 1.0][(seed % 4) as usize];
            for frame in 0..1500 {
                let what = format!("{ease:?} seed {seed} frame {frame}");
                let roll = rng.next_f64();
                if roll < 0.04 {
                    let to = if rng.next_f64() < 0.5 { 0.0 } else { 1.0 };
                    oracle.aim(to, secs, *ease);
                    quick.aim(to, secs, to_easing(ease), UNIT);
                } else if roll < 0.06 {
                    let to = (rng.next_f64() * 8.0).floor() / 8.0;
                    oracle.aim(to, secs, *ease);
                    quick.aim(to, secs, to_easing(ease), UNIT);
                } else if roll < 0.065 {
                    oracle.settle();
                    quick.settle();
                }
                assert_same(&oracle, &quick, &what);
                let dt = if rng.next_f64() < 0.02 {
                    0.3
                } else {
                    rng.next_f64() * 0.034
                };
                assert_eq!(quick.step(dt), oracle.step(dt), "{what}: step");
                assert_same(&oracle, &quick, &what);
            }
        }
    }
}

#[test]
fn same_target_is_a_noop() {
    let mut q = QuickTo::at(0.0);
    assert!(q.aim(1.0, 0.5, Easing::OutQuad, Retime::Full));
    q.step(0.1);
    let before = q;
    assert!(!q.aim(1.0, 2.0, Easing::Linear, Retime::Full));
    assert_eq!(q, before);
}

#[test]
fn by_distance_retime() {
    let mut q = QuickTo::at(0.0);
    assert!(q.aim(0.5, 0.4, Easing::Linear, UNIT));
    assert_eq!(q.duration(), 0.2);
    let mut full = QuickTo::at(0.0);
    full.aim(0.5, 0.4, Easing::Linear, Retime::Full);
    assert_eq!(full.duration(), 0.4);
    // Distances beyond one unit take the full duration.
    let mut far = QuickTo::at(0.0);
    far.aim(3.0, 0.4, Easing::Linear, UNIT);
    assert_eq!(far.duration(), 0.4);
    // Arrays use the largest lane distance.
    let mut rect = QuickTo::at([0.0, 0.0, 10.0, 10.0]);
    rect.aim(
        [5.0, 0.0, 10.0, 30.0],
        1.0,
        Easing::Linear,
        Retime::ByDistance { per_unit: 40.0 },
    );
    assert_eq!(rect.duration(), 0.5);
}

#[test]
fn landing_step_answers_false_and_is_exact() {
    for ease in [
        Easing::OutBack,
        Easing::OutElastic,
        Easing::css(0.3, 0.0, 0.8, 0.15),
        Easing::InOutBounce,
    ] {
        let mut q = QuickTo::at(3.0);
        q.aim(7.1, 0.25, ease, Retime::Full);
        let mut frames = 0;
        // 1/64 s frames divide 0.25 s exactly: 15 frames run on, the 16th lands.
        while q.step(1.0 / 64.0) {
            frames += 1;
            assert!(!q.is_settled());
        }
        assert_eq!(frames, 15, "{ease:?}");
        assert!(q.is_settled());
        assert_eq!(q.value().to_bits(), 7.1f64.to_bits());
        assert_eq!(q.progress(), 1.0);
        assert!(!q.step(1.0 / 60.0), "a settled run stays settled");
    }
    // Zero seconds lands at once.
    let mut q = QuickTo::at(0.0);
    assert!(q.aim(1.0, 0.0, Easing::OutQuad, Retime::Full));
    assert!(q.is_settled());
    assert_eq!(q.value(), 1.0);
    assert!(!q.step(0.016));
}

#[test]
fn restart_always_runs_from_the_visible_value() {
    // wm's StyleTween: smoothstep over 0.65 s, restarted on every selection.
    let mut w = QuickTo::at([1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
    let mut target = [0.0; 8];
    target[3] = 1.0;
    w.restart(target, 0.65, Easing::SmoothStep);
    w.step(0.2);
    let seen = w.value();
    target = [0.0; 8];
    target[5] = 1.0;
    w.restart(target, 0.65, Easing::SmoothStep);
    w.step(0.0);
    assert_eq!(w.value().map(f64::to_bits), seen.map(f64::to_bits));
    assert_eq!(w.from_value(), seen);
    // Restarting towards the same target still runs again.
    w.restart(target, 0.65, Easing::SmoothStep);
    assert_eq!(w.progress(), 0.0);
    assert_eq!(w.ease(), Easing::SmoothStep);
}

#[test]
fn retarget_from() {
    let mut q = QuickTo::at(5.0);
    q.retarget_from(10.0, 20.0, 1.0, Easing::Linear);
    assert_eq!(q.from_value(), 10.0);
    assert_eq!(q.value(), 10.0);
    q.step(0.25);
    assert_eq!(q.value(), 12.5);
    q.settle();
    assert_eq!(q.value(), 20.0);
    assert_eq!(q.from_value(), 10.0, "settle keeps the run's ends");
}

#[test]
fn value_does_not_force_the_start() {
    // Like UnitTween, the ease is read at t = 0 too (an ease that does not
    // start at 0 shows it at once).
    let mut q = QuickTo::at(0.0);
    q.aim(10.0, 1.0, Easing::Constant(0.5), Retime::Full);
    assert_eq!(q.value(), 5.0);
}

#[test]
fn lerp_arrays_per_lane() {
    assert_eq!(
        <[f64; 3]>::lerp([0.0, 10.0, -4.0], [10.0, 20.0, 4.0], 0.5),
        [5.0, 15.0, 0.0]
    );
    assert_eq!(
        <[f64; 3]>::distance([0.0, 10.0, -4.0], [1.0, 20.0, 4.0]),
        10.0
    );
    assert_eq!(f64::lerp(2.0, 4.0, 0.25), 2.5);
    assert_eq!(f64::distance(4.0, 2.0), 2.0);
}
