//! Eases: the curve that maps a tween's linear progress to its eased ratio.
//!
//! Two families live in one `Copy` enum and are evaluated directly (one
//! `match`, no lookup tables):
//! - the parity family reproduces Makepad's `animator::Ease::map` bit for bit
//!   (same operations in the same order, same snaps and clamps), so values
//!   written through the engine match what the Animator produced;
//! - the GSAP / CSS family follows GSAP 3's formulas (`back`, `elastic`,
//!   `expo`, `steps`) and CSS (`cubic-bezier`, `steps(n, jump-*)`) with a
//!   precise bezier solver.
//!
//! [`parse_gsap_ease`] maps GSAP ease strings (`"power2.out"`,
//! `"back.inOut(3)"`, `"steps(5)"`, ...) onto these variants.

use std::f64::consts::PI;

/// An ease: GSAP's `ease` value. `map(p)` turns linear progress `p` (0..1)
/// into the eased ratio (which may leave 0..1 for overshooting eases).
///
/// The default is [`Easing::OutQuad`], GSAP's default `"power1.out"`.
#[derive(Clone, Copy, Debug)]
pub enum Easing {
    // --- parity family: bit-identical to animator::Ease::map ---
    /// Identity clamped to 0..1: GSAP `"none"` / `"linear"` / `"power0"`.
    Linear,
    /// Jumps to the end at once: 1.0 for every input (Makepad `Ease::None`).
    Instant,
    /// Ignores progress and answers the constant, clamped to 0..1.
    Constant(f64),
    /// GSAP `power1.in` / `quad.in`.
    InQuad,
    /// GSAP `power1.out` / `quad.out`: the default ease.
    OutQuad,
    /// GSAP `power1.inOut` / `quad.inOut`.
    InOutQuad,
    /// GSAP `power2.in` / `cubic.in`.
    InCubic,
    /// GSAP `power2.out` / `cubic.out`.
    OutCubic,
    /// GSAP `power2.inOut` / `cubic.inOut`.
    InOutCubic,
    /// GSAP `power3.in` / `quart.in`.
    InQuart,
    /// GSAP `power3.out` / `quart.out`.
    OutQuart,
    /// GSAP `power3.inOut` / `quart.inOut`.
    InOutQuart,
    /// GSAP `power4.in` / `quint.in` / `strong.in`.
    InQuint,
    /// GSAP `power4.out` / `quint.out` / `strong.out`.
    OutQuint,
    /// GSAP `power4.inOut` / `quint.inOut` / `strong.inOut`.
    InOutQuint,
    /// GSAP `sine.in`.
    InSine,
    /// GSAP `sine.out`.
    OutSine,
    /// GSAP `sine.inOut`.
    InOutSine,
    /// Textbook exponential in, snapped to 0 below 0.001 (Makepad `Ease::InExp`;
    /// GSAP's `expo` is [`Easing::Expo`]).
    InExp,
    /// Textbook exponential out, snapped to 1 above 0.999.
    OutExp,
    /// Textbook exponential in-out with both snaps.
    InOutExp,
    /// GSAP `circ.in`.
    InCirc,
    /// GSAP `circ.out`.
    OutCirc,
    /// GSAP `circ.inOut`.
    InOutCirc,
    /// Makepad's elastic in (period 0.3, snapped ends); GSAP's is [`Easing::Elastic`].
    InElastic,
    /// Makepad's elastic out (the theme's `motion_ease_spring`).
    OutElastic,
    /// Makepad's elastic in-out (period 0.3 in both halves).
    InOutElastic,
    /// Back in with overshoot 1.70158 (GSAP `back.in`).
    InBack,
    /// Back out with overshoot 1.70158 (GSAP `back.out`).
    OutBack,
    /// Makepad's back in-out (overshoot scaled by 1.525 in the first half only).
    InOutBack,
    /// GSAP `bounce.in`.
    InBounce,
    /// GSAP `bounce.out` (the theme's `motion_ease_bounce`).
    OutBounce,
    /// GSAP `bounce.inOut`.
    InOutBounce,
    /// Makepad's stepped exponential decay; build it with [`Easing::exp_decay`],
    /// which precomputes `steps`.
    ExpDecay {
        /// First decay factor.
        d1: f64,
        /// Per-step multiplier of the decay factor.
        d2: f64,
        /// Number of steps until the remainder drops below 0.001.
        steps: u32,
        /// Step cap, at most 1000.
        max: u32,
    },
    /// Makepad's `Ease::Pow`; build it with [`Easing::pow`], which precomputes
    /// the constants.
    Pow {
        /// `-1 / max(begin², 1)`.
        a: f64,
        /// `1 + 1 / max(end², 1)`.
        b: f64,
        /// `((a - 1) * -b) / (a * (1 - b))`.
        base: f64,
    },
    /// Makepad's loose cubic-bezier solver (theme motion tokens, EaseEditor
    /// output): identical to `Ease::Bezier`. CSS strings use [`Easing::CubicBezier`].
    Bezier {
        /// First control point x.
        x1: f64,
        /// First control point y.
        y1: f64,
        /// Second control point x.
        x2: f64,
        /// Second control point y.
        y2: f64,
    },
    // --- GSAP / CSS family ---
    /// CSS `cubic-bezier(x1, y1, x2, y2)` solved precisely (Newton, then
    /// bisection, to 1e-12 in x). Build it with [`Easing::css`], which clamps x1 and x2.
    CubicBezier {
        /// First control point x, in 0..1.
        x1: f64,
        /// First control point y.
        y1: f64,
        /// Second control point x, in 0..1.
        x2: f64,
        /// Second control point y.
        y2: f64,
    },
    /// GSAP `back.in/out/inOut(overshoot)`; GSAP's default overshoot is 1.70158.
    Back {
        /// Which end the ease acts on.
        dir: EaseDir,
        /// How far past the end the curve swings.
        overshoot: f64,
    },
    /// GSAP `elastic.in/out/inOut(amplitude, period)`; GSAP's defaults are
    /// amplitude 1 and period 0.3 (in, out) or 0.45 (inOut).
    Elastic {
        /// Which end the ease acts on.
        dir: EaseDir,
        /// Swing height (values below 1 shorten the period instead).
        amplitude: f64,
        /// Oscillation period in progress units.
        period: f64,
    },
    /// GSAP `expo.in/out/inOut`: GSAP's blended exponential
    /// `2^(10(p-1))·p + p⁶·(1-p)`, exact at both ends, no snaps.
    Expo {
        /// Which end the ease acts on.
        dir: EaseDir,
    },
    /// CSS `steps(n, jump-*)`. GSAP's `steps(n)` has the same levels as
    /// `Steps { n: n + 1, jump: Jump::None }` but not the same last bit; it is
    /// [`Easing::GsapSteps`].
    Steps {
        /// Number of intervals.
        n: u32,
        /// Where the jumps happen.
        jump: Jump,
    },
    /// GSAP `steps(n)` / `steps(n, true)` (SteppedEase), bit-exact:
    /// `(floor(m · clamp(p, 0, 1 - 1e-8)) + s) · (1 / n)` with `m = n + 1`,
    /// `s = 0` (`n + 1` levels, the last reached at the end) or, with
    /// `start`, `m = n`, `s = 1` (the first jump at the start). GSAP multiplies
    /// by the reciprocal: `steps(5)(0.5) = 0.6000000000000001`.
    GsapSteps {
        /// Number of steps (0 counts as 1).
        n: u32,
        /// GSAP `immediateStart`: jump at the start instead of holding 0.
        start: bool,
    },
    /// Smoothstep `t²(3 - 2t)` on `t` clamped to 0..1.
    SmoothStep,
    /// Smootherstep `t³(t(6t - 15) + 10)` on `t` clamped to 0..1, result clamped.
    SmootherStep,
    /// A Rust function (GSAP custom ease function). Compared by address.
    Custom(CustomEase),
}

/// Which end of the motion an ease shapes: GSAP's `.in`, `.out`, `.inOut`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EaseDir {
    /// Slow start (`.in`).
    In,
    /// Slow end (`.out`).
    Out,
    /// Slow start and end (`.inOut`).
    InOut,
}

/// The CSS `steps()` jump term: where the value jumps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Jump {
    /// `jump-start`: the first jump happens at the start.
    Start,
    /// `jump-end` (CSS default): the last jump happens at the end.
    End,
    /// `jump-both`: jumps at both ends (n + 1 jumps).
    Both,
    /// `jump-none`: holds both end values (n - 1 jumps).
    None,
}

/// A custom ease function, compared by function address.
#[derive(Clone, Copy, Debug)]
pub struct CustomEase(pub fn(f64) -> f64);

impl PartialEq for CustomEase {
    fn eq(&self, o: &Self) -> bool {
        std::ptr::fn_addr_eq(self.0, o.0)
    }
}

impl PartialEq for Easing {
    fn eq(&self, o: &Self) -> bool {
        use Easing::*;
        match (self, o) {
            (Constant(a), Constant(b)) => a == b,
            (
                ExpDecay { d1, d2, steps, max },
                ExpDecay {
                    d1: e1,
                    d2: e2,
                    steps: s2,
                    max: m2,
                },
            ) => d1 == e1 && d2 == e2 && steps == s2 && max == m2,
            (
                Pow { a, b, base },
                Pow {
                    a: a2,
                    b: b2,
                    base: c2,
                },
            ) => a == a2 && b == b2 && base == c2,
            (
                Bezier { x1, y1, x2, y2 },
                Bezier {
                    x1: a,
                    y1: b,
                    x2: c,
                    y2: d,
                },
            )
            | (
                CubicBezier { x1, y1, x2, y2 },
                CubicBezier {
                    x1: a,
                    y1: b,
                    x2: c,
                    y2: d,
                },
            ) => x1 == a && y1 == b && x2 == c && y2 == d,
            (
                Back { dir, overshoot },
                Back {
                    dir: d2,
                    overshoot: o2,
                },
            ) => dir == d2 && overshoot == o2,
            (
                Elastic {
                    dir,
                    amplitude,
                    period,
                },
                Elastic {
                    dir: d2,
                    amplitude: a2,
                    period: p2,
                },
            ) => dir == d2 && amplitude == a2 && period == p2,
            (Expo { dir }, Expo { dir: d2 }) => dir == d2,
            (Steps { n, jump }, Steps { n: n2, jump: j2 }) => n == n2 && jump == j2,
            (GsapSteps { n, start }, GsapSteps { n: n2, start: s2 }) => n == n2 && start == s2,
            (Custom(a), Custom(b)) => a == b,
            // Every variant with a payload is matched above, so equal
            // discriminants here are equal payload-free variants.
            _ => std::mem::discriminant(self) == std::mem::discriminant(o),
        }
    }
}

impl Default for Easing {
    fn default() -> Self {
        Easing::OutQuad
    }
}

/// GSAP `yoyoEase`: the ease used on the backward (odd) iterations of a
/// yoyo tween.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum YoyoEase {
    /// GSAP `yoyoEase: true`: the tween's own ease, mirrored.
    Invert,
    /// GSAP `yoyoEase: "<ease>"`: that ease, mirrored.
    Ease(Easing),
}

/// The 25 named CSS curves (issue 786): `linear` plus the 24 classic Penner
/// approximations, as `(name, [x1, y1, x2, y2])`.
pub const CSS_PRESETS: [(&str, [f64; 4]); 25] = [
    ("linear", [0.0, 0.0, 1.0, 1.0]),
    ("ease_in_sine", [0.470, 0.000, 0.745, 0.715]),
    ("ease_out_sine", [0.390, 0.575, 0.565, 1.000]),
    ("ease_in_out_sine", [0.445, 0.050, 0.550, 0.950]),
    ("ease_in_quad", [0.550, 0.085, 0.680, 0.530]),
    ("ease_out_quad", [0.250, 0.460, 0.450, 0.940]),
    ("ease_in_out_quad", [0.455, 0.030, 0.515, 0.955]),
    ("ease_in_cubic", [0.550, 0.055, 0.675, 0.190]),
    ("ease_out_cubic", [0.215, 0.610, 0.355, 1.000]),
    ("ease_in_out_cubic", [0.645, 0.045, 0.355, 1.000]),
    ("ease_in_quart", [0.895, 0.030, 0.685, 0.220]),
    ("ease_out_quart", [0.165, 0.840, 0.440, 1.000]),
    ("ease_in_out_quart", [0.770, 0.000, 0.175, 1.000]),
    ("ease_in_quint", [0.755, 0.050, 0.855, 0.060]),
    ("ease_out_quint", [0.230, 1.000, 0.320, 1.000]),
    ("ease_in_out_quint", [0.860, 0.000, 0.070, 1.000]),
    ("ease_in_expo", [0.950, 0.050, 0.795, 0.035]),
    ("ease_out_expo", [0.190, 1.000, 0.220, 1.000]),
    ("ease_in_out_expo", [1.000, 0.000, 0.000, 1.000]),
    ("ease_in_circ", [0.600, 0.040, 0.980, 0.335]),
    ("ease_out_circ", [0.075, 0.820, 0.165, 1.000]),
    ("ease_in_out_circ", [0.785, 0.135, 0.150, 0.860]),
    ("ease_in_back", [0.600, -0.280, 0.735, 0.045]),
    ("ease_out_back", [0.175, 0.885, 0.320, 1.275]),
    ("ease_in_out_back", [0.680, -0.550, 0.265, 1.550]),
];

impl Easing {
    /// The eased ratio at linear progress `t`.
    #[inline]
    pub fn map(&self, t: f64) -> f64 {
        match *self {
            Self::Linear => t.max(0.0).min(1.0),
            Self::Instant => 1.0,
            Self::Constant(c) => c.max(0.0).min(1.0),
            Self::InQuad => t * t,
            Self::OutQuad => t * (2.0 - t),
            Self::InOutQuad => {
                let t = t * 2.0;
                if t < 1. {
                    0.5 * t * t
                } else {
                    let t = t - 1.;
                    -0.5 * (t * (t - 2.) - 1.)
                }
            }
            Self::InCubic => t * t * t,
            Self::OutCubic => {
                let t2 = t - 1.0;
                t2 * t2 * t2 + 1.0
            }
            Self::InOutCubic => {
                let t = t * 2.0;
                if t < 1. {
                    0.5 * t * t * t
                } else {
                    let t = t - 2.;
                    1. / 2. * (t * t * t + 2.)
                }
            }
            Self::InQuart => t * t * t * t,
            Self::OutQuart => {
                let t = t - 1.;
                -(t * t * t * t - 1.)
            }
            Self::InOutQuart => {
                let t = t * 2.0;
                if t < 1. {
                    0.5 * t * t * t * t
                } else {
                    let t = t - 2.;
                    -0.5 * (t * t * t * t - 2.)
                }
            }
            Self::InQuint => t * t * t * t * t,
            Self::OutQuint => {
                let t = t - 1.;
                t * t * t * t * t + 1.
            }
            Self::InOutQuint => {
                let t = t * 2.0;
                if t < 1. {
                    0.5 * t * t * t * t * t
                } else {
                    let t = t - 2.;
                    0.5 * (t * t * t * t * t + 2.)
                }
            }
            Self::InSine => -(t * PI * 0.5).cos() + 1.,
            Self::OutSine => (t * PI * 0.5).sin(),
            Self::InOutSine => -0.5 * ((t * PI).cos() - 1.),
            Self::InExp => {
                if t < 0.001 {
                    0.
                } else {
                    2.0f64.powf(10. * (t - 1.))
                }
            }
            Self::OutExp => {
                if t > 0.999 {
                    1.
                } else {
                    -(2.0f64.powf(-10. * t)) + 1.
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
                    0.5 * 2.0f64.powf(10. * (t - 1.))
                } else {
                    let t = t - 1.;
                    0.5 * (-(2.0f64.powf(-10. * t)) + 2.)
                }
            }
            Self::InCirc => -((1. - t * t).sqrt() - 1.),
            Self::OutCirc => {
                let t = t - 1.;
                (1. - t * t).sqrt()
            }
            Self::InOutCirc => {
                let t = t * 2.;
                if t < 1. {
                    -0.5 * ((1. - t * t).sqrt() - 1.)
                } else {
                    let t = t - 2.;
                    0.5 * ((1. - t * t).sqrt() + 1.)
                }
            }
            Self::InElastic => {
                let p = 0.3;
                let s = p / 4.0;
                if t < 0.001 {
                    return 0.;
                }
                if t > 0.999 {
                    return 1.;
                }
                let t = t - 1.0;
                -(2.0f64.powf(10.0 * t) * ((t - s) * (2.0 * PI) / p).sin())
            }
            Self::OutElastic => {
                let p = 0.3;
                let s = p / 4.0;
                if t < 0.001 {
                    return 0.;
                }
                if t > 0.999 {
                    return 1.;
                }
                2.0f64.powf(-10.0 * t) * ((t - s) * (2.0 * PI) / p).sin() + 1.0
            }
            Self::InOutElastic => {
                let p = 0.3;
                let s = p / 4.0;
                if t < 0.001 {
                    return 0.;
                }
                if t > 0.999 {
                    return 1.;
                }
                let t = t * 2.0;
                if t < 1. {
                    let t = t - 1.0;
                    -0.5 * (2.0f64.powf(10.0 * t) * ((t - s) * (2.0 * PI) / p).sin())
                } else {
                    let t = t - 1.0;
                    0.5 * 2.0f64.powf(-10.0 * t) * ((t - s) * (2.0 * PI) / p).sin() + 1.0
                }
            }
            Self::InBack => {
                let s = 1.70158;
                t * t * ((s + 1.) * t - s)
            }
            Self::OutBack => {
                let s = 1.70158;
                let t = t - 1.;
                t * t * ((s + 1.) * t + s) + 1.
            }
            Self::InOutBack => {
                let s = 1.70158;
                let t = t * 2.0;
                if t < 1. {
                    let s = s * 1.525;
                    0.5 * (t * t * ((s + 1.) * t - s))
                } else {
                    let t = t - 2.;
                    0.5 * (t * t * ((s + 1.) * t + s) + 2.)
                }
            }
            Self::InBounce => 1.0 - out_bounce(1.0 - t),
            Self::OutBounce => out_bounce(t),
            Self::InOutBounce => {
                if t < 0.5 {
                    (1.0 - out_bounce(1.0 - t * 2.)) * 0.5
                } else {
                    out_bounce(t * 2. - 1.) * 0.5 + 0.5
                }
            }
            Self::ExpDecay { d1, d2, steps, max } => exp_decay_map(d1, d2, steps, max, t),
            Self::Pow { a, b, base } => {
                if t < 0. {
                    return 0.;
                }
                if t > 1. {
                    return 1.;
                }
                let t2 = base.powf(t);
                (-a * b + b * a * t2) / (a * t2 - b)
            }
            Self::Bezier { x1, y1, x2, y2 } => parity_bezier(x1, y1, x2, y2, t),
            Self::CubicBezier { x1, y1, x2, y2 } => precise_bezier(x1, y1, x2, y2, t),
            Self::Back { dir, overshoot } => {
                let out = |p: f64| back_out(overshoot, p);
                from_out(dir, t.clamp(0.0, 1.0), out)
            }
            Self::Elastic {
                dir,
                amplitude,
                period,
            } => {
                let p1 = amplitude.max(1.0);
                let p2 = period / amplitude.min(1.0);
                let p3 = p2 / (2.0 * PI) * (1.0 / p1).asin();
                let w = 2.0 * PI / p2;
                let out = |p: f64| {
                    if p == 1.0 {
                        1.0
                    } else {
                        p1 * 2.0f64.powf(-10.0 * p) * ((p - p3) * w).sin() + 1.0
                    }
                };
                from_out(dir, t.clamp(0.0, 1.0), out)
            }
            Self::Expo { dir } => {
                let p = t.clamp(0.0, 1.0);
                match dir {
                    EaseDir::In => expo_in(p),
                    EaseDir::Out => 1.0 - expo_in(1.0 - p),
                    EaseDir::InOut => {
                        if p < 0.5 {
                            expo_in(p * 2.0) / 2.0
                        } else {
                            1.0 - expo_in((1.0 - p) * 2.0) / 2.0
                        }
                    }
                }
            }
            Self::Steps { n, jump } => steps(n, jump, t),
            Self::GsapSteps { n, start } => gsap_steps(n, start, t),
            Self::SmoothStep => {
                let t = t.clamp(0.0, 1.0);
                t * t * (3.0 - 2.0 * t)
            }
            Self::SmootherStep => {
                let t = t.clamp(0.0, 1.0);
                (t * t * t * (t * (t * 6.0 - 15.0) + 10.0)).clamp(0.0, 1.0)
            }
            Self::Custom(f) => (f.0)(t),
        }
    }

    /// Makepad's `ExpDecay { d1, d2, max }`, with the step count (the first
    /// loop of `Ease::map`) counted once here instead of on every evaluation.
    pub fn exp_decay(d1: f64, d2: f64, max: usize) -> Easing {
        let max_steps = max.min(1000);
        let mut di = d1;
        let mut dt = 1.0;
        let mut steps = 0usize;
        while dt > 0.001 && steps < max_steps {
            steps += 1;
            dt *= di;
            di *= d2;
        }
        Easing::ExpDecay {
            d1,
            d2,
            steps: steps as u32,
            max: max_steps as u32,
        }
    }

    /// Makepad's `Pow { begin, end }` with its constants precomputed (the same
    /// float operations as `Ease::map`, so the results are identical).
    pub fn pow(begin: f64, end: f64) -> Easing {
        let a = -1. / (begin * begin).max(1.0);
        let b = 1. + 1. / (end * end).max(1.0);
        let base = ((a - 1.) * -b) / (a * (1. - b));
        Easing::Pow { a, b, base }
    }

    /// CSS `cubic-bezier(x1, y1, x2, y2)`, solved precisely. x1 and x2 are
    /// clamped to 0..1 (as CSS requires), which keeps the curve a function of x.
    pub fn css(x1: f64, y1: f64, x2: f64, y2: f64) -> Easing {
        Easing::CubicBezier {
            x1: x1.clamp(0.0, 1.0),
            y1,
            x2: x2.clamp(0.0, 1.0),
            y2,
        }
    }

    /// GSAP `power<n>.<dir>`: 0 is linear, 1 quad, 2 cubic, 3 quart, 4 quint
    /// (the parity arms). Powers above 4 answer quint, like GSAP's `strong`.
    pub fn power(n: u8, dir: EaseDir) -> Easing {
        use EaseDir::*;
        match (n, dir) {
            (0, _) => Easing::Linear,
            (1, In) => Easing::InQuad,
            (1, Out) => Easing::OutQuad,
            (1, InOut) => Easing::InOutQuad,
            (2, In) => Easing::InCubic,
            (2, Out) => Easing::OutCubic,
            (2, InOut) => Easing::InOutCubic,
            (3, In) => Easing::InQuart,
            (3, Out) => Easing::OutQuart,
            (3, InOut) => Easing::InOutQuart,
            (_, In) => Easing::InQuint,
            (_, Out) => Easing::OutQuint,
            (_, InOut) => Easing::InOutQuint,
        }
    }

    /// A named CSS curve from [`CSS_PRESETS`] as a parity [`Easing::Bezier`]
    /// (equal to Makepad's `Ease::Bezier` with the same points).
    pub fn css_preset(name: &str) -> Option<Easing> {
        CSS_PRESETS
            .iter()
            .find(|(n, _)| *n == name)
            .map(|&(_, [x1, y1, x2, y2])| Easing::Bezier { x1, y1, x2, y2 })
    }

    /// The cubic-bezier control points of this ease, when it has some: a
    /// bezier's own points, `[0, 0, 1, 1]` for linear, and for a Penner parity
    /// arm (sine, quad .. quint, exp, circ, back) its [`CSS_PRESETS`] row.
    /// Elastic, bounce, steps and the non-curve variants answer `None`.
    pub fn css_points(&self) -> Option<[f64; 4]> {
        let row = |i: usize| Some(CSS_PRESETS[i].1);
        match *self {
            Self::Bezier { x1, y1, x2, y2 } | Self::CubicBezier { x1, y1, x2, y2 } => {
                Some([x1, y1, x2, y2])
            }
            Self::Linear => row(0),
            Self::InSine => row(1),
            Self::OutSine => row(2),
            Self::InOutSine => row(3),
            Self::InQuad => row(4),
            Self::OutQuad => row(5),
            Self::InOutQuad => row(6),
            Self::InCubic => row(7),
            Self::OutCubic => row(8),
            Self::InOutCubic => row(9),
            Self::InQuart => row(10),
            Self::OutQuart => row(11),
            Self::InOutQuart => row(12),
            Self::InQuint => row(13),
            Self::OutQuint => row(14),
            Self::InOutQuint => row(15),
            Self::InExp => row(16),
            Self::OutExp => row(17),
            Self::InOutExp => row(18),
            Self::InCirc => row(19),
            Self::OutCirc => row(20),
            Self::InOutCirc => row(21),
            Self::InBack => row(22),
            Self::OutBack => row(23),
            Self::InOutBack => row(24),
            _ => None,
        }
    }
}

/// Makepad's bounce out (7.5625 / 2.75 piecewise parabola).
#[inline]
fn out_bounce(t: f64) -> f64 {
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
    7.5625 * t * t + 0.984375
}

/// The second loop of Makepad's `ExpDecay` (the first one is `steps`).
fn exp_decay_map(d1: f64, d2: f64, steps: u32, max: u32, t: f64) -> f64 {
    if t > 0.999 {
        return 1.0;
    }
    let step = t * (steps as f64);
    let mut di = d1;
    let mut dt = 1.0;
    let max_steps = max as f64;
    let mut steps = 0.0;
    while dt > 0.001 && steps < max_steps {
        steps += 1.0;
        if steps >= step {
            let fac = steps - step;
            return 1.0 - (dt * fac + (dt * di) * (1.0 - fac));
        }
        dt *= di;
        di *= d2;
    }
    1.0
}

/// Makepad's `Ease::Bezier` solver: six Newton steps with a loose epsilon
/// (`t / 200`), then eight bisection steps.
fn parity_bezier(x1: f64, y1: f64, x2: f64, y2: f64, t: f64) -> f64 {
    if t < 0. {
        return 0.;
    }
    if t > 1. {
        return 1.;
    }
    if (x1 - y1).abs() < 0.001 && (x2 - y2).abs() < 0.001 {
        return t;
    }
    let epsilon = 1.0 / 200.0 * t;
    let cx = 3.0 * x1;
    let bx = 3.0 * (x2 - x1) - cx;
    let ax = 1.0 - cx - bx;
    let cy = 3.0 * y1;
    let by = 3.0 * (y2 - y1) - cy;
    let ay = 1.0 - cy - by;
    let mut u = t;
    for _ in 0..6 {
        let x = ((ax * u + bx) * u + cx) * u - t;
        if x.abs() < epsilon {
            return ((ay * u + by) * u + cy) * u;
        }
        let d = (3.0 * ax * u + 2.0 * bx) * u + cx;
        if d.abs() < 1e-6 {
            break;
        }
        u -= x / d;
    }
    let mut w = 0.0;
    let mut v = 1.0;
    u = t;
    for _ in 0..8 {
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
    ((ay * u + by) * u + cy) * u
}

/// How close x(u) must come to t in the precise solver. 1e-12 rather than
/// 1e-9: where the curve is steep an x error is multiplied by dy/dx (1e-9 in x
/// put y 1.4e-9 off the exact CSS curve), and Newton converges quadratically,
/// so the tighter bound costs at most one more step.
const PRECISE_EPS: f64 = 1e-12;

/// The precise CSS cubic-bezier solver: Newton (up to 8 steps) on x, then
/// bisection (up to 64 steps) when Newton stalls, to |x(u) - t| < 1e-12.
fn precise_bezier(x1: f64, y1: f64, x2: f64, y2: f64, t: f64) -> f64 {
    if t <= 0.0 {
        return 0.0;
    }
    if t >= 1.0 {
        return 1.0;
    }
    if x1 == y1 && x2 == y2 {
        return t;
    }
    let cx = 3.0 * x1;
    let bx = 3.0 * (x2 - x1) - cx;
    let ax = 1.0 - cx - bx;
    let cy = 3.0 * y1;
    let by = 3.0 * (y2 - y1) - cy;
    let ay = 1.0 - cy - by;
    let x_of = |u: f64| ((ax * u + bx) * u + cx) * u;
    let y_of = |u: f64| ((ay * u + by) * u + cy) * u;
    let mut u = t;
    for _ in 0..8 {
        let x = x_of(u) - t;
        if x.abs() < PRECISE_EPS {
            return y_of(u);
        }
        let d = (3.0 * ax * u + 2.0 * bx) * u + cx;
        if d.abs() < 1e-7 {
            break;
        }
        u -= x / d;
    }
    let (mut lo, mut hi) = (0.0, 1.0);
    u = t;
    for _ in 0..64 {
        let x = x_of(u);
        if (x - t).abs() < PRECISE_EPS {
            break;
        }
        if x < t {
            lo = u;
        } else {
            hi = u;
        }
        u = (lo + hi) * 0.5;
    }
    y_of(u)
}

/// GSAP `back` out: `(p-1)²((s+1)(p-1) + s) + 1`, and 0 at p == 0.
#[inline]
fn back_out(s: f64, p: f64) -> f64 {
    if p == 0.0 {
        return 0.0;
    }
    let q = p - 1.0;
    q * q * ((s + 1.0) * q + s) + 1.0
}

/// GSAP's derivation of `in` and `inOut` from an `out` curve (`_configBack`,
/// `_configElastic`, `_easeInOutFromOut`).
#[inline]
fn from_out(dir: EaseDir, p: f64, out: impl Fn(f64) -> f64) -> f64 {
    match dir {
        EaseDir::Out => out(p),
        EaseDir::In => 1.0 - out(1.0 - p),
        EaseDir::InOut => {
            if p < 0.5 {
                (1.0 - out(1.0 - p * 2.0)) / 2.0
            } else {
                0.5 + out((p - 0.5) * 2.0) / 2.0
            }
        }
    }
}

/// GSAP 3's blended `expo.in`: `2^(10(p-1))·p + p⁶·(1-p)` (exactly 0 at 0 and
/// 1 at 1, without the textbook curve's snaps).
#[inline]
fn expo_in(p: f64) -> f64 {
    2.0f64.powf(10.0 * (p - 1.0)) * p + p * p * p * p * p * p * (1.0 - p)
}

/// CSS `steps(n, jump)`. `n` below 1 counts as 1; `jump-none` with a single
/// interval holds the start value until the end.
fn steps(n: u32, jump: Jump, p: f64) -> f64 {
    if p >= 1.0 {
        return 1.0;
    }
    if p < 0.0 {
        return 0.0;
    }
    let k = n.max(1) as f64;
    let f = (p * k).floor();
    match jump {
        Jump::End => f / k,
        Jump::Start => (f + 1.0).min(k) / k,
        Jump::Both => (f + 1.0) / (k + 1.0),
        Jump::None => {
            if k < 2.0 {
                0.0
            } else {
                (f / (k - 1.0)).min(1.0)
            }
        }
    }
}

/// GSAP's SteppedEase with its exact float operations (multiply by `1 / n`).
fn gsap_steps(n: u32, start: bool, p: f64) -> f64 {
    let n = n.max(1) as f64;
    let (m, s) = if start { (n, 1.0) } else { (n + 1.0, 0.0) };
    ((m * p.clamp(0.0, 1.0 - 1e-8)).floor() + s) * (1.0 / n)
}

/// Parses a GSAP ease string into an [`Easing`] (for a script layer).
///
/// Accepted: `"none"`, `"linear"`, `"power0[.*]"` (linear);
/// `"power1".."power4"`, `"quad"`, `"cubic"`, `"quart"`, `"quint"`, `"strong"`
/// with `.in`, `.out` or `.inOut` (a bare name means `.out`); `"sine.*"`,
/// `"circ.*"`, `"bounce.*"`, `"expo.*"`; `"back.*(overshoot)"`;
/// `"elastic.*(amplitude, period)"`; `"steps(n)"` and `"steps(n, true)"`
/// ([`Easing::GsapSteps`]); `"cubic-bezier(x1, y1, x2, y2)"`.
/// Whitespace around names and arguments is ignored. Anything else is `None`.
pub fn parse_gsap_ease(s: &str) -> Option<Easing> {
    let s = s.trim();
    let (head, args) = match s.find('(') {
        Some(open) => {
            let inner = s[open + 1..].strip_suffix(')')?;
            (s[..open].trim(), Some(inner))
        }
        None => (s, None),
    };
    // Up to four numeric arguments; `true`/`false` read as 1/0 (steps).
    let mut vals = [0.0f64; 4];
    let mut count = 0;
    if let Some(inner) = args {
        if !inner.trim().is_empty() {
            for part in inner.split(',') {
                if count == vals.len() {
                    return None;
                }
                let part = part.trim();
                vals[count] = match part {
                    "true" => 1.0,
                    "false" => 0.0,
                    _ => part.parse::<f64>().ok().filter(|v| v.is_finite())?,
                };
                count += 1;
            }
        }
    }
    let arg = |i: usize| if i < count { Some(vals[i]) } else { None };

    if head == "cubic-bezier" {
        return (count == 4).then(|| Easing::css(vals[0], vals[1], vals[2], vals[3]));
    }
    if head == "steps" {
        let n = arg(0)?;
        if count > 2 || n < 1.0 || n.fract() != 0.0 || n > u32::MAX as f64 - 1.0 {
            return None;
        }
        return Some(Easing::GsapSteps {
            n: n as u32,
            start: arg(1).unwrap_or(0.0) != 0.0,
        });
    }

    let (family, dir) = match head.split_once('.') {
        Some((f, d)) => {
            let dir = match d {
                "in" => EaseDir::In,
                "out" => EaseDir::Out,
                "inOut" => EaseDir::InOut,
                _ => return None,
            };
            (f, dir)
        }
        None => (head, EaseDir::Out),
    };
    let no_args = count == 0;
    use EaseDir::*;
    Some(match family {
        "none" | "linear" | "power0" if no_args => Easing::Linear,
        "power1" | "quad" if no_args => Easing::power(1, dir),
        "power2" | "cubic" if no_args => Easing::power(2, dir),
        "power3" | "quart" if no_args => Easing::power(3, dir),
        "power4" | "quint" | "strong" if no_args => Easing::power(4, dir),
        "sine" if no_args => match dir {
            In => Easing::InSine,
            Out => Easing::OutSine,
            InOut => Easing::InOutSine,
        },
        "circ" if no_args => match dir {
            In => Easing::InCirc,
            Out => Easing::OutCirc,
            InOut => Easing::InOutCirc,
        },
        "bounce" if no_args => match dir {
            In => Easing::InBounce,
            Out => Easing::OutBounce,
            InOut => Easing::InOutBounce,
        },
        "expo" if no_args => Easing::Expo { dir },
        "back" if count <= 1 => Easing::Back {
            dir,
            overshoot: arg(0).unwrap_or(1.70158),
        },
        "elastic" if count <= 2 => {
            let default_period = if dir == InOut { 0.45 } else { 0.3 };
            // GSAP `period || default`: 0 also takes the default.
            let period = arg(1).filter(|p| *p != 0.0).unwrap_or(default_period);
            Easing::Elastic {
                dir,
                amplitude: arg(0).unwrap_or(1.0),
                period,
            }
        }
        _ => return None,
    })
}
