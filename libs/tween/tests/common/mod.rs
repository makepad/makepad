//! Shared test helpers. The oracle below is a verbatim copy of Makepad's
//! `animator::Ease` and `Ease::map` (widgets/src/animator.rs), with only the
//! script derives and `#[live]` / `#[pick]` attributes removed: the parity
//! family of `Easing` must reproduce it bit for bit.
//!
//! Every test binary that declares `mod common;` uses a different subset of
//! these items, so unused ones are expected per binary.
#![allow(dead_code)]

use makepad_tween::Easing;
use std::f64::consts::PI;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Ease {
    Linear,
    None,
    Constant(f64),
    InQuad,
    OutQuad,
    InOutQuad,
    InCubic,
    OutCubic,
    InOutCubic,
    InQuart,
    OutQuart,
    InOutQuart,
    InQuint,
    OutQuint,
    InOutQuint,
    InSine,
    OutSine,
    InOutSine,
    InExp,
    OutExp,
    InOutExp,
    InCirc,
    OutCirc,
    InOutCirc,
    InElastic,
    OutElastic,
    InOutElastic,
    InBack,
    OutBack,
    InOutBack,
    InBounce,
    OutBounce,
    InOutBounce,
    ExpDecay {
        d1: f64,
        d2: f64,
        max: usize,
    },

    Pow {
        begin: f64,
        end: f64,
    },
    Bezier {
        cp0: f64,
        cp1: f64,
        cp2: f64,
        cp3: f64,
    },
}

impl Ease {
    pub fn map(&self, t: f64) -> f64 {
        match self {
            Self::ExpDecay { d1, d2, max } => {
                // there must be a closed form for this
                if t > 0.999 {
                    return 1.0;
                }

                // first we count the number of steps we'd need to decay
                let mut di = *d1;
                let mut dt = 1.0;
                let max_steps = (*max).min(1000);
                let mut steps = 0;
                // for most of the settings we use this takes max 15 steps or so
                while dt > 0.001 && steps < max_steps {
                    steps = steps + 1;
                    dt = dt * di;
                    di *= d2;
                }
                // then we know how to find the step, and lerp it
                let step = t * (steps as f64);
                let mut di = *d1;
                let mut dt = 1.0;
                let max_steps = max_steps as f64;
                let mut steps = 0.0;
                while dt > 0.001 && steps < max_steps {
                    steps += 1.0;
                    if steps >= step {
                        // right step
                        let fac = steps - step;
                        return 1.0 - (dt * fac + (dt * di) * (1.0 - fac));
                    }
                    dt = dt * di;
                    di *= d2;
                }
                1.0
            }
            Self::Linear => {
                return t.max(0.0).min(1.0);
            }
            Self::Constant(t) => {
                return t.max(0.0).min(1.0);
            }
            Self::None => {
                return 1.0;
            }
            Self::Pow { begin, end } => {
                if t < 0. {
                    return 0.;
                }
                if t > 1. {
                    return 1.;
                }
                let a = -1. / (begin * begin).max(1.0);
                let b = 1. + 1. / (end * end).max(1.0);
                let t2 = (((a - 1.) * -b) / (a * (1. - b))).powf(t);
                return (-a * b + b * a * t2) / (a * t2 - b);
            }

            Self::InQuad => {
                return t * t;
            }
            Self::OutQuad => {
                return t * (2.0 - t);
            }
            Self::InOutQuad => {
                let t = t * 2.0;
                if t < 1. {
                    return 0.5 * t * t;
                } else {
                    let t = t - 1.;
                    return -0.5 * (t * (t - 2.) - 1.);
                }
            }
            Self::InCubic => {
                return t * t * t;
            }
            Self::OutCubic => {
                let t2 = t - 1.0;
                return t2 * t2 * t2 + 1.0;
            }
            Self::InOutCubic => {
                let t = t * 2.0;
                if t < 1. {
                    return 0.5 * t * t * t;
                } else {
                    let t = t - 2.;
                    return 1. / 2. * (t * t * t + 2.);
                }
            }
            Self::InQuart => return t * t * t * t,
            Self::OutQuart => {
                let t = t - 1.;
                return -(t * t * t * t - 1.);
            }
            Self::InOutQuart => {
                let t = t * 2.0;
                if t < 1. {
                    return 0.5 * t * t * t * t;
                } else {
                    let t = t - 2.;
                    return -0.5 * (t * t * t * t - 2.);
                }
            }
            Self::InQuint => {
                return t * t * t * t * t;
            }
            Self::OutQuint => {
                let t = t - 1.;
                return t * t * t * t * t + 1.;
            }
            Self::InOutQuint => {
                let t = t * 2.0;
                if t < 1. {
                    return 0.5 * t * t * t * t * t;
                } else {
                    let t = t - 2.;
                    return 0.5 * (t * t * t * t * t + 2.);
                }
            }
            Self::InSine => {
                return -(t * PI * 0.5).cos() + 1.;
            }
            Self::OutSine => {
                return (t * PI * 0.5).sin();
            }
            Self::InOutSine => {
                return -0.5 * ((t * PI).cos() - 1.);
            }
            Self::InExp => {
                if t < 0.001 {
                    return 0.;
                } else {
                    return 2.0f64.powf(10. * (t - 1.));
                }
            }
            Self::OutExp => {
                if t > 0.999 {
                    return 1.;
                } else {
                    return -(2.0f64.powf(-10. * t)) + 1.;
                }
            }
            Self::InOutExp => {
                if t < 0.001 {
                    return 0.;
                }
                if t > 0.999 {
                    return 1.;
                }
                let t = t * 2.0;
                if t < 1. {
                    return 0.5 * 2.0f64.powf(10. * (t - 1.));
                } else {
                    let t = t - 1.;
                    return 0.5 * (-(2.0f64.powf(-10. * t)) + 2.);
                }
            }
            Self::InCirc => {
                return -((1. - t * t).sqrt() - 1.);
            }
            Self::OutCirc => {
                let t = t - 1.;
                return (1. - t * t).sqrt();
            }
            Self::InOutCirc => {
                let t = t * 2.;
                if t < 1. {
                    return -0.5 * ((1. - t * t).sqrt() - 1.);
                } else {
                    let t = t - 2.;
                    return 0.5 * ((1. - t * t).sqrt() + 1.);
                }
            }
            Self::InElastic => {
                let p = 0.3;
                let s = p / 4.0; // c = 1.0, b = 0.0, d = 1.0
                if t < 0.001 {
                    return 0.;
                }
                if t > 0.999 {
                    return 1.;
                }
                let t = t - 1.0;
                return -(2.0f64.powf(10.0 * t) * ((t - s) * (2.0 * PI) / p).sin());
            }
            Self::OutElastic => {
                let p = 0.3;
                let s = p / 4.0; // c = 1.0, b = 0.0, d = 1.0

                if t < 0.001 {
                    return 0.;
                }
                if t > 0.999 {
                    return 1.;
                }
                return 2.0f64.powf(-10.0 * t) * ((t - s) * (2.0 * PI) / p).sin() + 1.0;
            }
            Self::InOutElastic => {
                let p = 0.3;
                let s = p / 4.0; // c = 1.0, b = 0.0, d = 1.0
                if t < 0.001 {
                    return 0.;
                }
                if t > 0.999 {
                    return 1.;
                }
                let t = t * 2.0;
                if t < 1. {
                    let t = t - 1.0;
                    return -0.5 * (2.0f64.powf(10.0 * t) * ((t - s) * (2.0 * PI) / p).sin());
                } else {
                    let t = t - 1.0;
                    return 0.5 * 2.0f64.powf(-10.0 * t) * ((t - s) * (2.0 * PI) / p).sin() + 1.0;
                }
            }
            Self::InBack => {
                let s = 1.70158;
                return t * t * ((s + 1.) * t - s);
            }
            Self::OutBack => {
                let s = 1.70158;
                let t = t - 1.;
                return t * t * ((s + 1.) * t + s) + 1.;
            }
            Self::InOutBack => {
                let s = 1.70158;
                let t = t * 2.0;
                if t < 1. {
                    let s = s * 1.525;
                    return 0.5 * (t * t * ((s + 1.) * t - s));
                } else {
                    let t = t - 2.;
                    return 0.5 * (t * t * ((s + 1.) * t + s) + 2.);
                }
            }
            Self::InBounce => {
                return 1.0 - Self::OutBounce.map(1.0 - t);
            }
            Self::OutBounce => {
                if t < (1. / 2.75) {
                    return 7.5625 * t * t;
                }
                if t < (2. / 2.75) {
                    let t = t - (1.5 / 2.75);
                    return 7.5625 * t * t + 0.75;
                }
                if t < (2.5 / 2.75) {
                    let t = t - (2.25 / 2.75);
                    return 7.5625 * t * t + 0.9375;
                }
                let t = t - (2.625 / 2.75);
                return 7.5625 * t * t + 0.984375;
            }
            Self::InOutBounce => {
                if t < 0.5 {
                    return Self::InBounce.map(t * 2.) * 0.5;
                } else {
                    return Self::OutBounce.map(t * 2. - 1.) * 0.5 + 0.5;
                }
            }
            Self::Bezier { cp0, cp1, cp2, cp3 } => {
                if t < 0. {
                    return 0.;
                }
                if t > 1. {
                    return 1.;
                }

                if (cp0 - cp1).abs() < 0.001 && (cp2 - cp3).abs() < 0.001 {
                    return t;
                }

                let epsilon = 1.0 / 200.0 * t;
                let cx = 3.0 * cp0;
                let bx = 3.0 * (cp2 - cp0) - cx;
                let ax = 1.0 - cx - bx;
                let cy = 3.0 * cp1;
                let by = 3.0 * (cp3 - cp1) - cy;
                let ay = 1.0 - cy - by;
                let mut u = t;

                for _i in 0..6 {
                    let x = ((ax * u + bx) * u + cx) * u - t;
                    if x.abs() < epsilon {
                        return ((ay * u + by) * u + cy) * u;
                    }
                    let d = (3.0 * ax * u + 2.0 * bx) * u + cx;
                    if d.abs() < 1e-6 {
                        break;
                    }
                    u = u - x / d;
                }

                if t > 1. {
                    return (ay + by) + cy;
                }
                if t < 0. {
                    return 0.0;
                }

                let mut w = 0.0;
                let mut v = 1.0;
                u = t;
                for _i in 0..8 {
                    let x = ((ax * u + bx) * u + cx) * u;
                    if (x - t).abs() < epsilon {
                        return ((ay * u + by) * u + cy) * u;
                    }

                    if t > x {
                        w = u;
                    } else {
                        v = u;
                    }
                    u = (v - w) * 0.5 + w;
                }

                return ((ay * u + by) * u + cy) * u;
            }
        }
    }
}

/// The `Easing` that stands for an oracle `Ease` (the future
/// `From<&Ease> for Easing` of the widgets adapter, test-local here).
pub fn to_easing(e: &Ease) -> Easing {
    match *e {
        Ease::Linear => Easing::Linear,
        Ease::None => Easing::Instant,
        Ease::Constant(c) => Easing::Constant(c),
        Ease::InQuad => Easing::InQuad,
        Ease::OutQuad => Easing::OutQuad,
        Ease::InOutQuad => Easing::InOutQuad,
        Ease::InCubic => Easing::InCubic,
        Ease::OutCubic => Easing::OutCubic,
        Ease::InOutCubic => Easing::InOutCubic,
        Ease::InQuart => Easing::InQuart,
        Ease::OutQuart => Easing::OutQuart,
        Ease::InOutQuart => Easing::InOutQuart,
        Ease::InQuint => Easing::InQuint,
        Ease::OutQuint => Easing::OutQuint,
        Ease::InOutQuint => Easing::InOutQuint,
        Ease::InSine => Easing::InSine,
        Ease::OutSine => Easing::OutSine,
        Ease::InOutSine => Easing::InOutSine,
        Ease::InExp => Easing::InExp,
        Ease::OutExp => Easing::OutExp,
        Ease::InOutExp => Easing::InOutExp,
        Ease::InCirc => Easing::InCirc,
        Ease::OutCirc => Easing::OutCirc,
        Ease::InOutCirc => Easing::InOutCirc,
        Ease::InElastic => Easing::InElastic,
        Ease::OutElastic => Easing::OutElastic,
        Ease::InOutElastic => Easing::InOutElastic,
        Ease::InBack => Easing::InBack,
        Ease::OutBack => Easing::OutBack,
        Ease::InOutBack => Easing::InOutBack,
        Ease::InBounce => Easing::InBounce,
        Ease::OutBounce => Easing::OutBounce,
        Ease::InOutBounce => Easing::InOutBounce,
        Ease::ExpDecay { d1, d2, max } => Easing::exp_decay(d1, d2, max),
        Ease::Pow { begin, end } => Easing::pow(begin, end),
        Ease::Bezier { cp0, cp1, cp2, cp3 } => Easing::Bezier {
            x1: cp0,
            y1: cp1,
            x2: cp2,
            y2: cp3,
        },
    }
}

/// Every parity arm without a payload.
pub const PLAIN_ARMS: [Ease; 32] = [
    Ease::Linear,
    Ease::None,
    Ease::InQuad,
    Ease::OutQuad,
    Ease::InOutQuad,
    Ease::InCubic,
    Ease::OutCubic,
    Ease::InOutCubic,
    Ease::InQuart,
    Ease::OutQuart,
    Ease::InOutQuart,
    Ease::InQuint,
    Ease::OutQuint,
    Ease::InOutQuint,
    Ease::InSine,
    Ease::OutSine,
    Ease::InOutSine,
    Ease::InExp,
    Ease::OutExp,
    Ease::InOutExp,
    Ease::InCirc,
    Ease::OutCirc,
    Ease::InOutCirc,
    Ease::InElastic,
    Ease::OutElastic,
    Ease::InOutElastic,
    Ease::InBack,
    Ease::OutBack,
    Ease::InOutBack,
    Ease::InBounce,
    Ease::OutBounce,
    Ease::InOutBounce,
];

/// The eight theme motion eases (widgets/src/theme_desktop_dark.rs).
pub const THEME_EASES: [Ease; 8] = [
    Ease::Bezier {
        cp0: 0.2,
        cp1: 0.0,
        cp2: 0.0,
        cp3: 1.0,
    },
    Ease::Bezier {
        cp0: 0.0,
        cp1: 0.0,
        cp2: 0.0,
        cp3: 1.0,
    },
    Ease::Bezier {
        cp0: 0.3,
        cp1: 0.0,
        cp2: 1.0,
        cp3: 1.0,
    },
    Ease::Bezier {
        cp0: 0.05,
        cp1: 0.7,
        cp2: 0.1,
        cp3: 1.0,
    },
    Ease::Bezier {
        cp0: 0.3,
        cp1: 0.0,
        cp2: 0.8,
        cp3: 0.15,
    },
    Ease::Linear,
    Ease::OutElastic,
    Ease::OutBounce,
];

/// A small deterministic generator for seeded test inputs.
pub struct Rng(pub u64);

impl Rng {
    /// The next value in [0, 1).
    pub fn next_f64(&mut self) -> f64 {
        self.0 = makepad_tween::splitmix64(self.0);
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
}
