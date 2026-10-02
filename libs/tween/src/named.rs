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
        let mut scored: Vec<(usize, String)> = Ease::ids().into_iter().map(|name| (edit_distance(id, &name), name)).collect();
        scored.sort();
        scored.into_iter().filter(|(d, _)| *d <= 4).take(3).map(|(_, name)| name).collect()
    }

    /// The eased fraction at `u` (0..1) of a segment `seconds` long:
    /// exactly 0 at (and before) 0 and 1 at (and after) 1.
    pub fn apply(self, u: f64, seconds: f64) -> f64 {
        if !(u > 0.0) {
            return 0.0;
        }
        if u >= 1.0 {
            return 1.0;
        }
        self.easing(seconds).map(u)
    }

    /// The curve as a tween ease, for a segment `seconds` long.
    pub fn easing(self, seconds: f64) -> Easing {
        match self {
            Ease::Linear => Easing::Linear,
            Ease::Hold => Easing::Hold,
            Ease::In(curve) => curve.easing(EaseDir::In),
            Ease::Out(curve) => curve.easing(EaseDir::Out),
            Ease::InOut(curve) => curve.easing(EaseDir::InOut),
            Ease::Spring { k, d } => Easing::Spring { stiffness: k, damping: d, seconds },
            Ease::Bezier { x1, y1, x2, y2 } => Easing::css(x1, y1, x2, y2),
        }
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
}
