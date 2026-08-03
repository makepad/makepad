//! L-system expansion and 3D turtle interpretation.
//!
//! Expansion is string rewriting with optional stochastic productions;
//! interpretation walks the result with a turtle carrying position,
//! orientation, radius and depth, emitting a branch skeleton.
//!
//! The skeleton is where both animation weights come from, derived in one
//! pass because they are the same quantity family: **growth** is normalised
//! arc length from the root (what sprouts last), **flex** is how freely a
//! point may bend (what moves most in wind). A leaf tip is both the last
//! thing to grow and the floppiest thing on the plant.

use crate::rng::GenRng;
use makepad_game_math as gm;
use makepad_math::*;

/// Hard ceiling on expanded string length. An L-system's growth is
/// exponential in iteration count, and these rules come from presets a
/// game (or an AI) can parameterise — so a bad `iterations` must produce a
/// small plant, not an out-of-memory.
pub const MAX_EXPANSION: usize = 200_000;

/// One production. `weight` allows stochastic rules: several productions
/// for the same symbol are chosen by weight, which is what makes two seeds
/// grow visibly different trees from identical rules.
#[derive(Clone, Debug)]
pub struct Rule {
    pub symbol: char,
    pub produces: &'static str,
    pub weight: f32,
}

#[derive(Clone, Debug)]
pub struct LSystem {
    pub axiom: &'static str,
    pub rules: &'static [Rule],
}

impl LSystem {
    /// Rewrite the axiom `iterations` times. Stops early (returning what it
    /// has) if the string would exceed [`MAX_EXPANSION`].
    pub fn expand(&self, iterations: usize, rng: &mut GenRng) -> String {
        let mut cur = self.axiom.to_string();
        for _ in 0..iterations {
            let mut next = String::with_capacity(cur.len() * 2);
            for ch in cur.chars() {
                match self.pick(ch, rng) {
                    Some(p) => next.push_str(p),
                    None => next.push(ch),
                }
                if next.len() > MAX_EXPANSION {
                    return next;
                }
            }
            cur = next;
        }
        cur
    }

    fn pick(&self, symbol: char, rng: &mut GenRng) -> Option<&'static str> {
        let mut total = 0.0;
        for r in self.rules {
            if r.symbol == symbol {
                total += r.weight;
            }
        }
        if total <= 0.0 {
            return None;
        }
        let mut pick = rng.f32() * total;
        for r in self.rules {
            if r.symbol == symbol {
                pick -= r.weight;
                if pick <= 0.0 {
                    return Some(r.produces);
                }
            }
        }
        // Float drift can leave `pick` marginally positive; fall back to the
        // last matching rule rather than silently dropping the symbol.
        self.rules
            .iter()
            .rev()
            .find(|r| r.symbol == symbol)
            .map(|r| r.produces)
    }
}

/// One emitted branch segment: a tapered cone from `a` to `b`.
#[derive(Clone, Copy, Debug)]
pub struct Segment {
    pub a: Vec3f,
    pub b: Vec3f,
    pub radius_a: f32,
    pub radius_b: f32,
    /// Arc length from the root at `a` and `b`, normalised later.
    pub arc_a: f32,
    pub arc_b: f32,
    pub depth: u32,
}

/// A leaf/frond attachment point with its orientation.
#[derive(Clone, Copy, Debug)]
pub struct Leaf {
    pub pos: Vec3f,
    pub heading: Vec3f,
    pub left: Vec3f,
    pub size: f32,
    pub arc: f32,
    pub depth: u32,
}

#[derive(Clone, Debug, Default)]
pub struct Skeleton {
    pub segments: Vec<Segment>,
    pub leaves: Vec<Leaf>,
    pub max_arc: f32,
    pub max_depth: u32,
}

impl Skeleton {
    /// Normalised generation order in [0, 1] for an arc length: the reveal
    /// threshold a vertex crosses when the plant grows.
    pub fn growth_at(&self, arc: f32) -> f32 {
        if self.max_arc <= 1.0e-6 {
            return 1.0;
        }
        (arc / self.max_arc).clamp(0.0, 1.0)
    }

    /// Wind flex weight in [0, 1].
    ///
    /// Two factors, multiplied: distance from the root (a tip swings, the
    /// base cannot) and thinness (a twig bends where a trunk does not).
    /// `radius` is relative to the trunk's base radius.
    pub fn flex_at(&self, arc: f32, radius_ratio: f32) -> f32 {
        let along = self.growth_at(arc);
        // Squared so the lower trunk stays genuinely stiff instead of
        // drifting; a linear ramp makes the whole tree wobble.
        let thin = (1.0 - radius_ratio.clamp(0.0, 1.0)).powi(2);
        (along * along * 0.65 + thin * 0.35).clamp(0.0, 1.0)
    }
}

/// Turtle state. Orientation is an explicit orthonormal frame rather than
/// Euler angles, so repeated rotations cannot gimbal-lock.
#[derive(Clone, Copy, Debug)]
struct Turtle {
    pos: Vec3f,
    heading: Vec3f,
    left: Vec3f,
    up: Vec3f,
    radius: f32,
    arc: f32,
    depth: u32,
    step: f32,
}

fn norm(v: Vec3f) -> Vec3f {
    let l = (v.x * v.x + v.y * v.y + v.z * v.z).sqrt();
    if l > 1.0e-8 {
        vec3f(v.x / l, v.y / l, v.z / l)
    } else {
        vec3f(0.0, 1.0, 0.0)
    }
}

fn cross(a: Vec3f, b: Vec3f) -> Vec3f {
    vec3f(
        a.y * b.z - a.z * b.y,
        a.z * b.x - a.x * b.z,
        a.x * b.y - a.y * b.x,
    )
}

/// Rodrigues rotation of `v` about unit `axis`. Uses the deterministic
/// sin/cos so two devices build byte-identical trees from one seed.
fn rotate(v: Vec3f, axis: Vec3f, angle: f32) -> Vec3f {
    let (s, c) = gm::sincos(angle);
    let d = axis.x * v.x + axis.y * v.y + axis.z * v.z;
    let cr = cross(axis, v);
    vec3f(
        v.x * c + cr.x * s + axis.x * d * (1.0 - c),
        v.y * c + cr.y * s + axis.y * d * (1.0 - c),
        v.z * c + cr.z * s + axis.z * d * (1.0 - c),
    )
}

/// Parameters for turtle interpretation.
#[derive(Clone, Copy, Debug)]
pub struct TurtleParams {
    pub step: f32,
    pub angle: f32,
    /// Radius multiplier applied at `!`.
    pub taper: f32,
    pub start_radius: f32,
    /// Random angle jitter per turn, in radians.
    pub jitter: f32,
    /// Multiplier applied to `step` on each `[` (branches shorten).
    pub step_falloff: f32,
    pub leaf_size: f32,
}

impl Default for TurtleParams {
    fn default() -> Self {
        Self {
            step: 1.0,
            angle: 0.42,
            taper: 0.72,
            start_radius: 0.12,
            jitter: 0.12,
            step_falloff: 0.82,
            leaf_size: 0.35,
        }
    }
}

/// Interpret an expanded L-system string into a branch skeleton.
///
/// Symbols: `F` forward+draw, `f` forward without drawing, `+`/`-` yaw,
/// `&`/`^` pitch, `\`/`/` roll, `|` turn around, `[`/`]` push/pop,
/// `!` taper, `L` emit leaf.
pub fn interpret(s: &str, p: TurtleParams, rng: &mut GenRng) -> Skeleton {
    let mut sk = Skeleton::default();
    let mut t = Turtle {
        pos: Vec3f::default(),
        heading: vec3f(0.0, 1.0, 0.0),
        left: vec3f(-1.0, 0.0, 0.0),
        up: vec3f(0.0, 0.0, 1.0),
        radius: p.start_radius,
        arc: 0.0,
        depth: 0,
        step: p.step,
    };
    let mut stack: Vec<Turtle> = Vec::new();

    for ch in s.chars() {
        let a = p.angle + rng.jitter(p.jitter);
        match ch {
            'F' => {
                let next_r = t.radius * p.taper.max(0.05);
                let b = vec3f(
                    t.pos.x + t.heading.x * t.step,
                    t.pos.y + t.heading.y * t.step,
                    t.pos.z + t.heading.z * t.step,
                );
                let arc_b = t.arc + t.step;
                sk.segments.push(Segment {
                    a: t.pos,
                    b,
                    radius_a: t.radius,
                    radius_b: next_r,
                    arc_a: t.arc,
                    arc_b,
                    depth: t.depth,
                });
                t.pos = b;
                t.arc = arc_b;
                sk.max_arc = sk.max_arc.max(arc_b);
                sk.max_depth = sk.max_depth.max(t.depth);
            }
            'f' => {
                t.pos = vec3f(
                    t.pos.x + t.heading.x * t.step,
                    t.pos.y + t.heading.y * t.step,
                    t.pos.z + t.heading.z * t.step,
                );
                t.arc += t.step;
                sk.max_arc = sk.max_arc.max(t.arc);
            }
            '+' | '-' => {
                let s = if ch == '+' { a } else { -a };
                t.heading = norm(rotate(t.heading, t.up, s));
                t.left = norm(cross(t.up, t.heading));
            }
            '&' | '^' => {
                let s = if ch == '&' { a } else { -a };
                t.heading = norm(rotate(t.heading, t.left, s));
                t.up = norm(cross(t.heading, t.left));
            }
            '\\' | '/' => {
                let s = if ch == '\\' { a } else { -a };
                t.left = norm(rotate(t.left, t.heading, s));
                t.up = norm(cross(t.heading, t.left));
            }
            '|' => {
                t.heading = vec3f(-t.heading.x, -t.heading.y, -t.heading.z);
                t.left = norm(cross(t.up, t.heading));
            }
            '!' => t.radius *= p.taper,
            '[' => {
                stack.push(t);
                t.depth += 1;
                t.step *= p.step_falloff;
                t.radius *= p.taper;
            }
            ']' => {
                if let Some(prev) = stack.pop() {
                    t = prev;
                }
            }
            'L' => sk.leaves.push(Leaf {
                pos: t.pos,
                heading: t.heading,
                left: t.left,
                size: p.leaf_size,
                arc: t.arc,
                depth: t.depth,
            }),
            _ => {}
        }
    }
    if sk.max_arc <= 0.0 {
        sk.max_arc = 1.0;
    }
    sk
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALGAE: LSystem = LSystem {
        axiom: "A",
        rules: &[
            Rule {
                symbol: 'A',
                produces: "AB",
                weight: 1.0,
            },
            Rule {
                symbol: 'B',
                produces: "A",
                weight: 1.0,
            },
        ],
    };

    #[test]
    fn expansion_matches_the_known_sequence() {
        // Lindenmayer's original algae example: A, AB, ABA, ABAAB, ABAABABA
        let mut r = GenRng::new(1);
        assert_eq!(ALGAE.expand(0, &mut r), "A");
        assert_eq!(ALGAE.expand(1, &mut r), "AB");
        assert_eq!(ALGAE.expand(2, &mut r), "ABA");
        assert_eq!(ALGAE.expand(3, &mut r), "ABAAB");
        assert_eq!(ALGAE.expand(4, &mut r), "ABAABABA");
    }

    #[test]
    fn expansion_is_bounded() {
        // 40 iterations of a doubling rule would be 2^40 characters.
        let mut r = GenRng::new(1);
        let out = ALGAE.expand(40, &mut r);
        assert!(
            out.len() <= MAX_EXPANSION + 8,
            "unbounded expansion: {}",
            out.len()
        );
    }

    #[test]
    fn unknown_symbols_pass_through() {
        let mut r = GenRng::new(1);
        let sys = LSystem {
            axiom: "X[+F]",
            rules: &[Rule {
                symbol: 'X',
                produces: "F",
                weight: 1.0,
            }],
        };
        assert_eq!(sys.expand(1, &mut r), "F[+F]");
    }

    #[test]
    fn a_straight_trunk_is_vertical_and_the_right_height() {
        let mut r = GenRng::new(5);
        let p = TurtleParams {
            step: 2.0,
            jitter: 0.0,
            taper: 1.0,
            step_falloff: 1.0,
            ..Default::default()
        };
        let sk = interpret("FFF", p, &mut r);
        assert_eq!(sk.segments.len(), 3);
        let top = sk.segments.last().unwrap().b;
        assert!(top.x.abs() < 1.0e-5 && top.z.abs() < 1.0e-5, "not vertical");
        assert!((top.y - 6.0).abs() < 1.0e-5, "height {} != 6", top.y);
        assert!((sk.max_arc - 6.0).abs() < 1.0e-5);
    }

    #[test]
    fn brackets_restore_the_turtle() {
        let mut r = GenRng::new(5);
        let p = TurtleParams {
            step: 1.0,
            jitter: 0.0,
            angle: 0.5,
            taper: 1.0,
            step_falloff: 1.0,
            ..Default::default()
        };
        // The branch must not move where the trunk continues from.
        let sk = interpret("F[+F]F", p, &mut r);
        assert_eq!(sk.segments.len(), 3);
        let trunk_top = sk.segments[2].b;
        assert!(trunk_top.x.abs() < 1.0e-5 && trunk_top.z.abs() < 1.0e-5);
        assert!((trunk_top.y - 2.0).abs() < 1.0e-5);
        // The bracketed branch did leave the trunk axis.
        assert!(sk.segments[1].b.x.abs() > 0.1 || sk.segments[1].b.z.abs() > 0.1);
    }

    #[test]
    fn depth_increases_inside_brackets() {
        let mut r = GenRng::new(5);
        let sk = interpret("F[F[F]]", TurtleParams::default(), &mut r);
        assert_eq!(sk.max_depth, 2);
    }

    #[test]
    fn growth_and_flex_run_root_to_tip() {
        let mut r = GenRng::new(9);
        let p = TurtleParams {
            jitter: 0.0,
            ..Default::default()
        };
        let sk = interpret("FFFF", p, &mut r);
        assert_eq!(sk.growth_at(0.0), 0.0);
        assert!((sk.growth_at(sk.max_arc) - 1.0).abs() < 1.0e-6);
        // The base must be stiffer than the tip, or the trunk detaches.
        let base = sk.flex_at(0.0, 1.0);
        let tip = sk.flex_at(sk.max_arc, 0.05);
        assert!(base < 0.05, "trunk base flex {base} should be ~0");
        assert!(tip > 0.8, "tip flex {tip} should be high");
    }

    #[test]
    fn rotations_keep_the_frame_orthonormal() {
        let mut r = GenRng::new(11);
        let sk = interpret(
            "F+F&F/F+F&F/F-F^F\\F",
            TurtleParams {
                jitter: 0.0,
                ..Default::default()
            },
            &mut r,
        );
        // Every segment must have finite, sane endpoints — a degenerate
        // frame shows up as NaN or a runaway coordinate.
        for s in &sk.segments {
            for v in [s.a, s.b] {
                assert!(v.x.is_finite() && v.y.is_finite() && v.z.is_finite());
                assert!(v.x.abs() < 1000.0 && v.y.abs() < 1000.0 && v.z.abs() < 1000.0);
            }
        }
    }

    #[test]
    fn same_seed_same_skeleton() {
        let sys = LSystem {
            axiom: "F",
            rules: &[
                Rule {
                    symbol: 'F',
                    produces: "F[+F]F",
                    weight: 1.0,
                },
                Rule {
                    symbol: 'F',
                    produces: "F[-F]F",
                    weight: 1.0,
                },
            ],
        };
        let build = |seed: u64| {
            let mut r = GenRng::new(seed);
            let s = sys.expand(4, &mut r);
            let sk = interpret(&s, TurtleParams::default(), &mut r);
            (s, sk.segments.len(), sk.max_arc)
        };
        assert_eq!(build(42), build(42));
        assert_ne!(build(42).0, build(43).0, "stochastic rules ignored the seed");
    }
}
