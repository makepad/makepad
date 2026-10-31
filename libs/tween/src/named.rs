//! Named eases: the vocabulary Motion documents, their 3D scenes and the
//! kinetic kits write (`@ease_out_back`, `@hold`, `spring(k, d)`,
//! `bezier(x1, y1, x2, y2)`), with "did you mean" suggestions and the source
//! form. The curves are [`Easing`]'s: one implementation for the Animator,
//! the tween engine and the documents.

use crate::easing::{EaseDir, Easing};

/// The curve family of an eased segment.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Curve {
    Quad,
    Cubic,
    Quart,
    Quint,
    Sine,
    Expo,
    Circ,
    /// Overshoots (in: pulls back first; out: past the target and back).
    Back,
    Elastic,
    Bounce,
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum Ease {
    #[default]
    Linear,
    /// Keep the previous key's value until this key, then jump.
    Hold,
    In(Curve),
    Out(Curve),
    InOut(Curve),
    /// A spring from rest at the start value to the end value: stiffness
    /// `k` (per s², mass 1) and damping `d` (per s). `spring(170, 26)` is
    /// snappy without wobble, `spring(100, 10)` wobbles. The segment ends at
    /// the target whatever the spring has done by then, so give it time to
    /// settle (about `8 / d` seconds).
    Spring { k: f64, d: f64 },
    /// CSS `cubic-bezier(x1, y1, x2, y2)`: the curve from (0, 0) to (1, 1)
    /// with those two control points. The x values are clamped to 0..1
    /// (so time runs forward); the y values may overshoot.
    Bezier { x1: f64, y1: f64, x2: f64, y2: f64 },
    /// A rate shaper over another ease: there and back, a running start,
    /// a wiggle, squished into a sub-range, ... (see [`Shape`]). Unlike the
    /// other eases a shaped one need not end at 1 (`there_and_back` ends
    /// where it began); it is the value at 0 and at 1 exactly.
    Shaped(Shaped),
}

/// The eases a shaper can wrap: every ease but `Hold` and another shaper.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum Base {
    #[default]
    Linear,
    In(Curve),
    Out(Curve),
    InOut(Curve),
    Spring { k: f64, d: f64 },
    Bezier { x1: f64, y1: f64, x2: f64, y2: f64 },
}

/// The shapers. Each reshapes the progress of its base ease `b(u)`:
/// - `ThereAndBack`: `b` up the first half, back down the second (0 to 1 to 0).
/// - `ThereAndBackWithPause` (`a` = the share of the segment spent at the top, 1/3):
///   up, hold at 1, down.
/// - `RunningStart` (`a` = how far it pulls back first, -0.5): dips below 0
///   then runs on to 1.
/// - `Wiggle` (`a` = oscillations, 2): a sine of that many periods under a
///   there-and-back envelope, 0 at both ends.
/// - `Squish` (`a..b` = the part of the segment that moves, 0.25..0.75): the
///   ease runs inside the range, 0 before it and 1 after.
/// - `NotQuiteThere` (`a` = how far it gets, 0.7): `a * b(u)`.
/// - `Lingering` (`a` = the base progress at which it has arrived, 0.8): reaches
///   1 early and stays.
/// - `ExponentialDecay` (`a` = half-life in segment fractions, 0.1): fast then
///   asymptotic, rescaled to land exactly on 1.
/// - `DoubleSmooth`: two smooth steps in succession, resting at 0.5 between.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Shape {
    ThereAndBack,
    ThereAndBackWithPause,
    RunningStart,
    Wiggle,
    Squish,
    NotQuiteThere,
    Lingering,
    ExponentialDecay,
    DoubleSmooth,
}

const SHAPES: &[(&str, Shape)] = &[
    ("there_and_back", Shape::ThereAndBack),
    ("there_and_back_with_pause", Shape::ThereAndBackWithPause),
    ("running_start", Shape::RunningStart),
    ("wiggle", Shape::Wiggle),
    ("squish", Shape::Squish),
    ("not_quite_there", Shape::NotQuiteThere),
    ("lingering", Shape::Lingering),
    ("exponential_decay", Shape::ExponentialDecay),
    ("double_smooth", Shape::DoubleSmooth),
];

impl Shape {
    /// The shaper named `id` (`wiggle`), as an `@id` or a call.
    pub fn from_id(id: &str) -> Option<Shape> {
        SHAPES.iter().find(|(n, _)| *n == id).map(|(_, s)| *s)
    }

    pub fn id(self) -> &'static str {
        SHAPES.iter().find(|(_, s)| *s == self).map_or("", |(n, _)| n)
    }

    /// The parameters when none are written: (a, b).
    pub fn defaults(self) -> (f64, f64) {
        match self {
            Shape::ThereAndBack | Shape::DoubleSmooth => (0.0, 0.0),
            Shape::ThereAndBackWithPause => (1.0 / 3.0, 0.0),
            Shape::RunningStart => (-0.5, 0.0),
            Shape::Wiggle => (2.0, 0.0),
            Shape::Squish => (0.25, 0.75),
            Shape::NotQuiteThere => (0.7, 0.0),
            Shape::Lingering => (0.8, 0.0),
            Shape::ExponentialDecay => (0.1, 0.0),
        }
    }

    /// How many numbers the shaper takes after its base ease.
    pub fn params(self) -> usize {
        match self {
            Shape::ThereAndBack | Shape::DoubleSmooth => 0,
            Shape::Squish => 2,
            _ => 1,
        }
    }
}

/// A shaper, its parameters and the ease it reshapes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shaped {
    pub shape: Shape,
    pub base: Base,
    pub a: f64,
    pub b: f64,
}

impl Shaped {
    /// The shaper with its default parameters over `base`.
    pub fn new(shape: Shape, base: Base) -> Shaped {
        let (a, b) = shape.defaults();
        Shaped { shape, base, a, b }
    }

    /// The shaped value at `u` (0..1) of a segment `seconds` long.
    pub fn apply(self, u: f64, seconds: f64) -> f64 {
        let u = if u > 0.0 { u.min(1.0) } else { 0.0 };
        let base = |x: f64| Ease::from(self.base).apply(x, seconds);
        let smooth = |x: f64| {
            let x = x.clamp(0.0, 1.0);
            x * x * (3.0 - 2.0 * x)
        };
        let (a, b) = (self.a, self.b);
        match self.shape {
            Shape::ThereAndBack => base(1.0 - (2.0 * u - 1.0).abs()),
            Shape::ThereAndBackWithPause => {
                let side = ((1.0 - a.clamp(0.0, 1.0)) * 0.5).max(1e-9);
                base(if u < side {
                    u / side
                } else if u > 1.0 - side {
                    (1.0 - u) / side
                } else {
                    1.0
                })
            }
            Shape::RunningStart => {
                // Bernstein form of the control values 0, pull, pull, 1, 1, 1 over b(u).
                let x = base(u);
                let y = 1.0 - x;
                5.0 * y.powi(4) * x * a + 10.0 * y.powi(3) * x * x * a + 10.0 * y * y * x.powi(3) + 5.0 * y * x.powi(4) + x.powi(5)
            }
            Shape::Wiggle => {
                let envelope = base(1.0 - (2.0 * u - 1.0).abs());
                // sin(2 pi count u) is exactly 0 at both ends for a whole count only;
                // the envelope is 0 there, so the product is 0 at both ends always.
                envelope * (std::f64::consts::TAU * a * u).sin()
            }
            Shape::Squish => {
                let span = (b - a).max(1e-9);
                base(((u - a) / span).clamp(0.0, 1.0))
            }
            Shape::NotQuiteThere => a * base(u),
            Shape::Lingering => (base(u) / a.max(1e-9)).min(1.0),
            Shape::ExponentialDecay => {
                let k = std::f64::consts::LN_2 / a.max(1e-9);
                (1.0 - (-k * base(u)).exp()) / (1.0 - (-k).exp())
            }
            Shape::DoubleSmooth => {
                if u < 0.5 {
                    0.5 * base(smooth(2.0 * u))
                } else {
                    0.5 + 0.5 * base(smooth(2.0 * u - 1.0))
                }
            }
        }
    }

    /// `@there_and_back` (over linear, default parameters), else the call form
    /// `wiggle(@ease_out, 3)` with the base ease and every parameter.
    pub fn source(self) -> String {
        let id = self.shape.id();
        let base = Ease::from(self.base);
        if self.base == Base::Linear && self.shape.defaults() == (self.a, self.b) {
            return format!("@{id}");
        }
        let n = fmt_num;
        match self.shape.params() {
            0 => format!("{id}({})", base.source()),
            1 => format!("{id}({}, {})", base.source(), n(self.a)),
            _ => format!("{id}({}, {}, {})", base.source(), n(self.a), n(self.b)),
        }
    }
}

impl From<Base> for Ease {
    fn from(base: Base) -> Ease {
        match base {
            Base::Linear => Ease::Linear,
            Base::In(c) => Ease::In(c),
            Base::Out(c) => Ease::Out(c),
            Base::InOut(c) => Ease::InOut(c),
            Base::Spring { k, d } => Ease::Spring { k, d },
            Base::Bezier { x1, y1, x2, y2 } => Ease::Bezier { x1, y1, x2, y2 },
        }
    }
}

const CURVES: &[(&str, Curve)] = &[
    ("quad", Curve::Quad),
    ("cubic", Curve::Cubic),
    ("quart", Curve::Quart),
    ("quint", Curve::Quint),
    ("sine", Curve::Sine),
    ("expo", Curve::Expo),
    ("circ", Curve::Circ),
    ("back", Curve::Back),
    ("elastic", Curve::Elastic),
    ("bounce", Curve::Bounce),
];

impl Ease {
    /// Every ease written as an `@id`, for messages and the cheat sheet.
    pub fn ids() -> Vec<String> {
        let mut ids = vec!["linear".to_string(), "hold".into(), "ease_in".into(), "ease_out".into(), "ease_in_out".into()];
        for (name, _) in CURVES {
            for dir in ["in", "out", "in_out"] {
                ids.push(format!("ease_{dir}_{name}"));
            }
        }
        ids
    }

    /// Every shaper written as an `@id` (over linear with default
    /// parameters: `@wiggle`); over another ease or with parameters a shaper is
    /// the call form `wiggle(@ease_out, 3)`.
    pub fn shaper_ids() -> Vec<String> {
        SHAPES.iter().map(|(n, _)| n.to_string()).collect()
    }

    /// The ease as a shaper's base: `None` for `Hold` and shapers.
    pub fn as_base(self) -> Option<Base> {
        Some(match self {
            Ease::Linear => Base::Linear,
            Ease::In(c) => Base::In(c),
            Ease::Out(c) => Base::Out(c),
            Ease::InOut(c) => Base::InOut(c),
            Ease::Spring { k, d } => Base::Spring { k, d },
            Ease::Bezier { x1, y1, x2, y2 } => Base::Bezier { x1, y1, x2, y2 },
            Ease::Hold | Ease::Shaped(_) => return None,
        })
    }

    /// `base.shaped(Shape::Wiggle)`: the shaper with default parameters over this ease.
    pub fn shaped(self, shape: Shape) -> Option<Ease> {
        Some(Ease::Shaped(Shaped::new(shape, self.as_base()?)))
    }

    /// `@linear`, `@hold`, `@ease_in` / `@ease_out` / `@ease_in_out`
    /// (quadratic, as in edits), and `@ease_{in,out,in_out}_{quad, cubic,
    /// quart, quint, sine, expo, circ, back, elastic, bounce}`. `in_out`
    /// may also be written `inout`.
    pub fn from_id(id: &str) -> Option<Ease> {
        match id {
            "linear" => return Some(Ease::Linear),
            "hold" | "step" => return Some(Ease::Hold),
            "ease_in" => return Some(Ease::In(Curve::Quad)),
            "ease_out" => return Some(Ease::Out(Curve::Quad)),
            "ease_in_out" | "ease_inout" => return Some(Ease::InOut(Curve::Quad)),
            _ => {}
        }
        // A shaper over linear with its default parameters: `@wiggle`.
        if let Some(shape) = Shape::from_id(id) {
            return Ease::Linear.shaped(shape);
        }
        let rest = id.strip_prefix("ease_")?;
        let (dir, name) = if let Some(name) = rest.strip_prefix("in_out_").or_else(|| rest.strip_prefix("inout_")) {
            (2, name)
        } else if let Some(name) = rest.strip_prefix("in_") {
            (0, name)
        } else if let Some(name) = rest.strip_prefix("out_") {
            (1, name)
        } else {
            return None;
        };
        let curve = CURVES.iter().find(|(n, _)| *n == name)?.1;
        Some(match dir {
            0 => Ease::In(curve),
            1 => Ease::Out(curve),
            _ => Ease::InOut(curve),
        })
    }

    /// The nearest ease names to a misspelt one (for "did you mean").
    pub fn suggest(id: &str) -> Vec<String> {
        let mut scored: Vec<(usize, String)> = Ease::ids().into_iter().chain(SHAPES.iter().map(|(n, _)| n.to_string())).map(|name| (edit_distance(id, &name), name)).collect();
        scored.sort();
        scored.into_iter().filter(|(d, _)| *d <= 4).take(3).map(|(_, name)| name).collect()
    }

    /// The eased fraction at `u` (0..1) of a segment `seconds` long:
    /// exactly 0 at (and before) 0 and 1 at (and after) 1.
    pub fn apply(self, u: f64, seconds: f64) -> f64 {
        if let Ease::Shaped(shaped) = self {
            return shaped.apply(u, seconds);
        }
        if !(u > 0.0) {
            return 0.0;
        }
        if u >= 1.0 {
            return 1.0;
        }
        self.easing(seconds).map_or(u, |easing| easing.map(u))
    }

    /// The curve as a tween ease, for a segment `seconds` long; `None` for a
    /// shaper (which is not one tween curve: use [`Ease::apply`]).
    pub fn easing(self, seconds: f64) -> Option<Easing> {
        Some(match self {
            Ease::Linear => Easing::Linear,
            Ease::Hold => Easing::Hold,
            Ease::In(curve) => curve.easing(EaseDir::In),
            Ease::Out(curve) => curve.easing(EaseDir::Out),
            Ease::InOut(curve) => curve.easing(EaseDir::InOut),
            Ease::Spring { k, d } => Easing::Spring { stiffness: k, damping: d, seconds },
            Ease::Bezier { x1, y1, x2, y2 } => Easing::css(x1, y1, x2, y2),
            Ease::Shaped(_) => return None,
        })
    }

    /// The ease as it is written in a document (`@ease_out_cubic`,
    /// `spring(170, 26)`, `bezier(0.7, 0, 0.3, 1)`): loading this gives the
    /// same ease back, exactly.
    pub fn source(self) -> String {
        let n = fmt_num;
        let named = |dir: &str, curve: Curve| {
            let name = CURVES.iter().find(|(_, c)| *c == curve).map_or("quad", |(n, _)| n);
            format!("@ease_{dir}_{name}")
        };
        match self {
            Ease::Linear => "@linear".into(),
            Ease::Hold => "@hold".into(),
            Ease::In(curve) => named("in", curve),
            Ease::Out(curve) => named("out", curve),
            Ease::InOut(curve) => named("in_out", curve),
            Ease::Spring { k, d } => format!("spring({}, {})", n(k), n(d)),
            Ease::Bezier { x1, y1, x2, y2 } => format!("bezier({}, {}, {}, {})", n(x1), n(y1), n(x2), n(y2)),
            Ease::Shaped(shaped) => shaped.source(),
        }
    }
}

impl Curve {
    /// The family in one direction as a tween ease. `out` mirrors `in` and
    /// `in_out` joins its halves: Back and Elastic are tween's GSAP forms
    /// with the classic constants (overshoot 1.70158; amplitude 1, period
    /// 0.3), which are exactly that.
    fn easing(self, dir: EaseDir) -> Easing {
        use EaseDir::*;
        match (self, dir) {
            (Curve::Quad, In) => Easing::InQuad,
            (Curve::Quad, Out) => Easing::OutQuad,
            (Curve::Quad, InOut) => Easing::InOutQuad,
            (Curve::Cubic, In) => Easing::InCubic,
            (Curve::Cubic, Out) => Easing::OutCubic,
            (Curve::Cubic, InOut) => Easing::InOutCubic,
            (Curve::Quart, In) => Easing::InQuart,
            (Curve::Quart, Out) => Easing::OutQuart,
            (Curve::Quart, InOut) => Easing::InOutQuart,
            (Curve::Quint, In) => Easing::InQuint,
            (Curve::Quint, Out) => Easing::OutQuint,
            (Curve::Quint, InOut) => Easing::InOutQuint,
            (Curve::Sine, In) => Easing::InSine,
            (Curve::Sine, Out) => Easing::OutSine,
            (Curve::Sine, InOut) => Easing::InOutSine,
            (Curve::Expo, dir) => Easing::ExpoClassic { dir },
            (Curve::Circ, In) => Easing::InCirc,
            (Curve::Circ, Out) => Easing::OutCirc,
            (Curve::Circ, InOut) => Easing::InOutCirc,
            (Curve::Back, dir) => Easing::Back { dir, overshoot: 1.70158 },
            (Curve::Elastic, dir) => Easing::Elastic { dir, amplitude: 1.0, period: 0.3 },
            (Curve::Bounce, In) => Easing::InBounce,
            (Curve::Bounce, Out) => Easing::OutBounce,
            (Curve::Bounce, InOut) => Easing::InOutBounce,
        }
    }
}

/// A number as a document writes it: integers without a fraction.
fn fmt_num(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

/// Levenshtein distance by characters (for "did you mean").
pub fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cur = row[j + 1];
            row[j + 1] = (prev + (ca != *cb) as usize).min(row[j] + 1).min(cur + 1);
            prev = cur;
        }
    }
    row[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_named_ease_starts_at_0_ends_at_1_and_parses_back() {
        for id in Ease::ids() {
            let ease = Ease::from_id(&id).unwrap_or_else(|| panic!("{id}"));
            assert_eq!(ease.apply(0.0, 1.0), 0.0, "{id}");
            assert_eq!(ease.apply(1.0, 1.0), 1.0, "{id}");
            if ease != Ease::Hold {
                for i in 1..20 {
                    let v = ease.apply(i as f64 / 20.0, 1.0);
                    assert!(v.is_finite() && v > -0.5 && v < 1.5, "{id} at {i}: {v}");
                }
            }
        }
        assert_eq!(Ease::from_id("ease_out_back"), Some(Ease::Out(Curve::Back)));
        assert_eq!(Ease::from_id("ease_in_out_cubic"), Some(Ease::InOut(Curve::Cubic)));
        assert_eq!(Ease::from_id("ease_inout_expo"), Some(Ease::InOut(Curve::Expo)));
        assert_eq!(Ease::from_id("ease_sideways"), None);
        assert_eq!(Ease::suggest("ease_out_bak")[0], "ease_out_back");
    }

    #[test]
    fn curves_have_their_shape() {
        let back = Ease::Out(Curve::Back);
        assert!((0..100).any(|i| back.apply(i as f64 / 100.0, 1.0) > 1.0), "out-back overshoots");
        assert!(Ease::In(Curve::Cubic).apply(0.5, 1.0) < Ease::In(Curve::Quad).apply(0.5, 1.0));
        assert_eq!(Ease::InOut(Curve::Quint).apply(0.5, 1.0), 0.5);
        assert_eq!(Ease::Hold.apply(0.99, 1.0), 0.0);
    }

    #[test]
    fn beziers_match_css_cubic_bezier() {
        // CSS `ease` at the midpoint (the browsers' value), linear, and a
        // symmetric in-out.
        let ease = Ease::Bezier { x1: 0.25, y1: 0.1, x2: 0.25, y2: 1.0 };
        assert!((ease.apply(0.5, 1.0) - 0.802_403_387_7).abs() < 1e-8, "{}", ease.apply(0.5, 1.0));
        for k in 0..=20 {
            let u = k as f64 / 20.0;
            assert!((Ease::Bezier { x1: 0.0, y1: 0.0, x2: 1.0, y2: 1.0 }.apply(u, 1.0) - u).abs() < 1e-12);
        }
        let io = Ease::Bezier { x1: 0.42, y1: 0.0, x2: 0.58, y2: 1.0 };
        assert!((io.apply(0.5, 1.0) - 0.5).abs() < 1e-12);
        // Exact on the curve: for every parameter s, the ease at x(s) is y(s),
        // including overshooting and flat-ended handles.
        for (x1, y1, x2, y2) in [(0.7, 0.0, 0.3, 1.0), (0.34, 1.56, 0.64, 1.0), (0.0, 0.0, 0.0, 1.0), (1.0, 0.0, 1.0, 1.0), (0.5, -0.6, 0.2, 1.4)] {
            let b = |p1: f64, p2: f64, s: f64| 3.0 * (1.0 - s).powi(2) * s * p1 + 3.0 * (1.0 - s) * s * s * p2 + s * s * s;
            for k in 1..100 {
                let s = k as f64 / 100.0;
                let (x, y) = (b(x1, x2, s), b(y1, y2, s));
                let got = Ease::Bezier { x1, y1, x2, y2 }.apply(x, 1.0);
                assert!((got - y).abs() < 1e-9, "bezier({x1}, {y1}, {x2}, {y2}) at x {x}: {got} vs {y}");
            }
        }
        // Deterministic: the same value every time; ends pinned.
        assert_eq!(ease.apply(0.3, 1.0).to_bits(), ease.apply(0.3, 1.0).to_bits());
        assert_eq!((ease.apply(0.0, 1.0), ease.apply(1.0, 1.0)), (0.0, 1.0));
    }

    #[test]
    fn springs_settle_and_wobble_by_damping() {
        let stiff = Ease::Spring { k: 170.0, d: 26.0 };
        assert!((stiff.apply(0.999, 2.0) - 1.0).abs() < 1e-3);
        let wobbly = Ease::Spring { k: 100.0, d: 4.0 };
        assert!((0..200).any(|i| wobbly.apply(i as f64 / 200.0, 2.0) > 1.2));
        let over = Ease::Spring { k: 100.0, d: 40.0 };
        let mut last = 0.0;
        for i in 1..100 {
            let v = over.apply(i as f64 / 100.0, 2.0);
            assert!(v >= last - 1e-12 && v <= 1.0 + 1e-9, "overdamped never overshoots");
            last = v;
        }
        // The same spring over a longer segment is further along at the same fraction.
        assert!(stiff.apply(0.1, 2.0) > stiff.apply(0.1, 0.5));
    }

    fn shaped(id: &str) -> Ease {
        Ease::from_id(id).unwrap_or_else(|| panic!("{id}"))
    }

    #[test]
    fn shapers_parse_by_id_over_any_ease_and_write_back() {
        for id in Ease::shaper_ids() {
            let ease = shaped(&id);
            assert!(matches!(ease, Ease::Shaped(_)), "{id}");
            assert_eq!(Ease::from_id(ease.source().trim_start_matches('@')), Some(ease));
        }
        assert_eq!(Ease::from_id("wiggle_of_ease_out"), None, "no generated ids: the call form reshapes another ease");
        assert_eq!(Ease::suggest("there_and_bak")[0], "there_and_back");
        let spring = Ease::Spring { k: 170.0, d: 26.0 }.shaped(Shape::NotQuiteThere).unwrap();
        assert_eq!(spring.source(), "not_quite_there(spring(170, 26), 0.7)");
        let custom = Ease::Shaped(Shaped { a: 3.0, ..Shaped::new(Shape::Wiggle, Base::Out(Curve::Cubic)) });
        assert_eq!(custom.source(), "wiggle(@ease_out_cubic, 3)");
    }

    #[test]
    fn shaper_endpoints() {
        let end = |id: &str| (shaped(id).apply(0.0, 1.0), shaped(id).apply(1.0, 1.0));
        for id in ["there_and_back", "there_and_back_with_pause", "wiggle"] {
            assert_eq!(end(id), (0.0, 0.0), "{id}");
        }
        for id in ["running_start", "squish", "lingering", "exponential_decay", "double_smooth"] {
            let (a, b) = end(id);
            assert!(a.abs() < 1e-12 && (b - 1.0).abs() < 1e-12, "{id}: {a} {b}");
        }
        assert_eq!(end("not_quite_there"), (0.0, 0.7));
        // Out-of-range progress is the end values.
        assert_eq!(shaped("not_quite_there").apply(5.0, 1.0), 0.7);
        assert_eq!(shaped("there_and_back").apply(-1.0, 1.0), 0.0);
    }

    #[test]
    fn shapers_have_their_shape() {
        let tab = shaped("there_and_back");
        assert_eq!(tab.apply(0.5, 1.0), 1.0);
        assert!((tab.apply(0.25, 1.0) - tab.apply(0.75, 1.0)).abs() < 1e-12, "symmetric");
        let over = Ease::In(Curve::Cubic).shaped(Shape::ThereAndBack).unwrap();
        assert!((over.apply(0.25, 1.0) - 0.125).abs() < 1e-12, "the base eases the way up");
        // Pause: top held across the middle third.
        let pause = shaped("there_and_back_with_pause");
        assert_eq!((pause.apply(0.34, 1.0), pause.apply(0.5, 1.0), pause.apply(0.66, 1.0)), (1.0, 1.0, 1.0));
        assert!(pause.apply(0.1, 1.0) < 1.0);
        // Running start dips below 0 first, then arrives at 1 without overshoot.
        let run = shaped("running_start");
        assert!(run.apply(0.2, 1.0) < -0.05);
        assert!((0..=100).all(|i| run.apply(i as f64 / 100.0, 1.0) <= 1.0 + 1e-12));
        // Wiggle crosses zero and swings both ways.
        let wig = shaped("wiggle");
        let samples: Vec<f64> = (0..=200).map(|i| wig.apply(i as f64 / 200.0, 1.0)).collect();
        assert!(samples.iter().any(|v| *v > 0.5) && samples.iter().any(|v| *v < -0.5));
        // Squish: nothing before, done after.
        let sq = shaped("squish");
        assert_eq!((sq.apply(0.25, 1.0), sq.apply(0.5, 1.0), sq.apply(0.75, 1.0)), (0.0, 0.5, 1.0));
        // Lingering: arrived at the 0.8 mark and stays.
        let ling = shaped("lingering");
        assert!((ling.apply(0.8, 1.0) - 1.0).abs() < 1e-12 && ling.apply(0.9, 1.0) == 1.0);
        // Exponential decay is front-loaded.
        assert!(shaped("exponential_decay").apply(0.2, 1.0) > 0.7);
        // Double smooth rests at 0.5 in the middle (zero slope each side).
        let dbl = shaped("double_smooth");
        assert!((dbl.apply(0.5, 1.0) - 0.5).abs() < 1e-12);
        assert!((dbl.apply(0.49, 1.0) - dbl.apply(0.51, 1.0)).abs() < 3e-3);
    }

    #[test]
    fn monotonic_and_continuous_where_expected() {
        for id in ["running_start", "squish", "lingering", "exponential_decay", "double_smooth", "not_quite_there"] {
            let ease = shaped(id);
            let mut last = ease.apply(0.0, 1.0);
            for i in 1..=1000 {
                let v = ease.apply(i as f64 / 1000.0, 1.0);
                if !id.starts_with("running_start") {
                    assert!(v >= last - 1e-12, "{id} dips at {i}: {last} -> {v}");
                }
                assert!((v - last).abs() < 0.08, "{id} jumps at {i}: {last} -> {v}");
                last = v;
            }
        }
        for id in ["there_and_back", "there_and_back_with_pause", "wiggle", "running_start"] {
            let ease = shaped(id);
            let mut last = ease.apply(0.0, 1.0);
            for i in 1..=1000 {
                let v = ease.apply(i as f64 / 1000.0, 1.0);
                assert!((v - last).abs() < 0.1, "{id} jumps at {i}");
                last = v;
            }
        }
    }
}
