//! Leaves and fern fronds for gobos (B7). A canopy is a list of primitives
//! (leaf blades and stems) in s-units, built once from a seed, then binned
//! on a grid so a lookup visits only the few that can reach a point. A
//! point's transmission is the product of (1 - coverage) over them, so
//! overlapping soft edges darken each other the way two half-shadows do.
//!
//! A leaf is a lens: the shape between two circular arcs that meet in a tip
//! at each end. Its signed distance is exact (the idea of iq's vesica,
//! reimplemented). The lens is laid along a midrib that sags sideways as a
//! parabola; teeth nibble at its edge; a short stalk runs back from its base.
//!
//! **The penumbra keeps the light (D9, L:146).** A primitive's coverage is
//! B6's box filter of its sharp shape, taken as a strip: across it, the box
//! filter of its cross-section, whose half-thickness is the shape's own
//! averaged along it over the box. That keeps exactly the area of its shade
//! whatever the penumbra (it moves its shadow about and never loses or adds
//! any), so for leaves scattered at random the canopy passes the same share
//! of its light at any distance; and a part thinner or shorter than the
//! penumbra spreads its little shade thin. A soft edge on its distance
//! darkened a band twice the penumbra wide by half instead, so a canopy lost
//! a third of its light as it moved off, and the 14 tiny pinnules of a fern's
//! pinna multiplied to a black dot.

use makepad_draw::*;
use super::noise::hash_u32;
use makepad_tween::path::MotionPath;

/// The most leaves one gobo scatters, about as many primitives as stay local
/// in the bins (MAX_BINS): the smallest leaves at density 1 on a 60 x 45
/// degree light ask for twice as many. Past the cap the leaves grow instead,
/// so the canopy still covers what density asks (D9, L:148: at 900 leaves,
/// density 0.5 and 1 gave the same canopy from leaf size 0.5 down).
pub const MAX_LEAVES: usize = 40_000;
/// The most fronds one fern gobo grows, about as many primitives as
/// MAX_LEAVES (a frond is about 450). Past it the fronds grow instead.
pub const MAX_FRONDS: usize = 90;
/// How far past the light's rectangle the canopy reaches, in s-units: the
/// most a light draws past its box, half its soft band at softness 1 (half
/// its smaller half size: studio.rs's and B2's edge rule). Leaves are
/// scattered and binned over it, so a soft light's glow shows them too (D9,
/// L:147).
pub const GLOW: f32 = 0.5;
/// The most bins a side.
const MAX_BINS: f32 = 160.0;
/// Pinna pairs along a fern frond, and pinnule pairs along each pinna.
pub const PAIRS: usize = 14;
pub const LOBES: usize = 7;

/// Leaves per leaf-length squared at density 1. A leaf covers about 0.3 of
/// its length squared, so 8 of them leave about e^-2.4, 9 %, open.
const LEAVES_PER_AREA: f32 = 8.0;
/// A leaf's stalk: how far it runs back from the base, and its radius, in leaf lengths.
const STALK: f32 = 0.22;
const STALK_RADIUS: f32 = 0.012;
/// Teeth along a leaf's edge.
const TEETH: f32 = 14.0;
/// A hair over 1, so a primitive's padded circle never misses a point its
/// coverage reaches by rounding (`Prim::pad`).
const BIN_PAD_SLACK: f32 = 1.01;

/// A leaf blade: `origin` is its base, `axis` points to the tip (unit), `len`
/// its length, `width` its widest half-width over `len` (at mid-length),
/// `bend` how far its midrib sags sideways at mid-length (over `len`),
/// `serration` 0..1 the depth of its teeth.
#[derive(Clone, Copy, Debug)]
pub struct LeafPrim {
    pub origin: Vec2f,
    pub axis: Vec2f,
    pub len: f32,
    pub width: f32,
    pub bend: f32,
    pub serration: f32,
}

/// A stem: a capsule round the segment a-b.
#[derive(Clone, Copy, Debug)]
pub struct StemPrim {
    pub a: Vec2f,
    pub b: Vec2f,
    pub radius: f32,
}

#[derive(Clone, Copy, Debug)]
pub enum Prim {
    Leaf(LeafPrim),
    Stem(StemPrim),
}

impl Prim {
    /// The sharp shape's signed distance, negative inside.
    pub fn sdf(&self, p: Vec2f) -> f32 {
        match self {
            Prim::Leaf(leaf) => leaf_sdf(leaf, p),
            Prim::Stem(stem) => stem_sdf(stem, p),
        }
    }

    /// The share 0..=1 of the box `w` each way round `p` (the penumbra) that
    /// the primitive covers: B6's box filter of its sharp shape, taken as a
    /// strip (see the module notes). Exactly 0 past `pad` penumbrae from it.
    pub fn coverage(&self, p: Vec2f, w: f32) -> f32 {
        match self {
            Prim::Leaf(leaf) => leaf_coverage(leaf, p, w),
            Prim::Stem(stem) => stem_coverage(stem, p, w),
        }
    }

    /// Centre and radius of a circle holding the whole primitive. A leaf
    /// with its stalk runs from -0.23 to 1 along its axis, so every point of
    /// it is within 0.63 lengths of the point 0.39 along; sideways it reaches
    /// its half-width plus its sag, about mid-length.
    fn bounds(&self) -> (Vec2f, f32) {
        match self {
            Prim::Leaf(l) => {
                let lateral = l.width.abs() + l.bend.abs();
                let reach = 0.66f32.max((0.0121 + lateral * lateral).sqrt() + 0.03);
                (l.origin + l.axis * (0.39 * l.len), reach * l.len)
            }
            Prim::Stem(s) => ((s.a + s.b) * 0.5, (s.b - s.a).length() * 0.5 + s.radius),
        }
    }

    /// How many penumbrae past its sharp shape the coverage reaches, which
    /// the bins pad its bounding circle by. The coverage reaches one penumbra
    /// past the shape along it and one across it: sqrt 2. Across a leaf it
    /// is measured from the sagging midrib, whose slope (at most 4 x its sag
    /// of 0.12) adds that much again over the penumbra along it, so a sagging
    /// leaf reaches up to sqrt(1 + 1.48²) = 1.79.
    fn pad(&self) -> f32 {
        let across = match self {
            Prim::Leaf(l) => 1.0 + 4.0 * l.bend.abs(),
            Prim::Stem(_) => 1.0,
        };
        (1.0 + across * across).sqrt() * BIN_PAD_SLACK
    }
}

/// Distance from `p` to the segment a-b.
fn seg_dist(p: Vec2f, a: Vec2f, b: Vec2f) -> f32 {
    let pa = p - a;
    let ba = b - a;
    let h = ((pa.x * ba.x + pa.y * ba.y) / (ba.x * ba.x + ba.y * ba.y).max(1.0e-12)).max(0.0).min(1.0);
    (pa - ba * h).length()
}

/// Signed distance to a lens one unit long with its tips at (0, -0.5) and
/// (0, 0.5) and half-width `w` at the middle: the shape two circles of
/// radius r share when their centres sit at (-d, 0) and (d, 0). Exact:
/// outside the wedge at a tip the nearest point is the tip; elsewhere it is
/// on the far circle's arc.
fn sd_lens(x: f32, y: f32, w: f32) -> f32 {
    let (x, y) = (x.abs(), y.abs());
    let w = w.max(1.0e-3).min(0.5);
    // r - d = w and r² - d² = 0.25 give both.
    let r = 0.5 * (0.25 / w + w);
    let d = 0.5 * (0.25 / w - w);
    if (y - 0.5) * d > x * 0.5 {
        (x * x + (y - 0.5) * (y - 0.5)).sqrt()
    } else {
        ((x + d) * (x + d) + y * y).sqrt() - r
    }
}

/// Signed distance to a leaf, negative inside. Exact for a straight,
/// smooth leaf. The midrib's sag shears the plane sideways, which stretches
/// distances by at most 13 % at a sag of 0.12; the teeth are a sawtooth added
/// to the distance, deepest at mid-length and gone at both tips.
pub fn leaf_sdf(l: &LeafPrim, p: Vec2f) -> f32 {
    let rel = p - l.origin;
    // Leaf coordinates in lengths: x from the base (0) to the tip (1), y sideways.
    let x = (rel.x * l.axis.x + rel.y * l.axis.y) / l.len;
    let y = (rel.y * l.axis.x - rel.x * l.axis.y) / l.len;
    let t = x.max(0.0).min(1.0);
    // The midrib is a parabola sagging `bend` at mid-length.
    let mid = l.bend * 4.0 * t * (1.0 - t);
    let mut blade = sd_lens(y - mid, x - 0.5, l.width);
    if l.serration > 0.0 && x > 0.0 && x < 1.0 {
        // A sawtooth along the leaf, its sharp side toward the tip.
        let tooth = x * TEETH - (x * TEETH).floor();
        blade += l.serration * 0.3 * l.width * tooth * (4.0 * t * (1.0 - t)).sqrt();
    }
    // A short thin stalk behind the base, overlapping the blade a little.
    let stalk = seg_dist(vec2f(x, y), vec2f(-STALK, 0.0), vec2f(0.02, 0.0)) - STALK_RADIUS;
    blade.min(stalk) * l.len
}

pub fn stem_sdf(s: &StemPrim, p: Vec2f) -> f32 {
    seg_dist(p, s.a, s.b) - s.radius
}

/// The share 0..=1 of the box `w` each way round a point that a strip covers,
/// across it: the box filter of a cross-section `h` thick each way whose
/// middle lies `x` (>= 0) from the point. Its integral across is exactly
/// 2h, so a strip thinner than the box spreads its shade over it instead of
/// darkening the box's width by half. `h` is the strip's half-thickness
/// averaged along it over the box, which carries the box filter along it.
fn strip(x: f32, h: f32, w: f32) -> f32 {
    if !(h > 0.0) {
        return 0.0;
    }
    ((h.min(x + w) - (-h).max(x - w)).max(0.0) / (2.0 * w)).min(1.0)
}

/// How much of `a..b` lies in `lo..hi`.
fn overlap(a: f32, b: f32, lo: f32, hi: f32) -> f32 {
    (b.min(hi) - a.max(lo)).max(0.0)
}

/// The mean over `a..b` (in leaf lengths from the base; the blade runs from 0
/// to 1) of the lens's half-width, in leaf lengths: the circles' arc
/// sqrt(r² - u²) - d, u from the middle, integrated exactly, and 0 past the
/// tips. Over a window a thousandth of a length or less, its value there.
fn lens_mean(width: f32, a: f32, b: f32) -> f32 {
    let w = width.max(1.0e-3).min(0.5);
    // As sd_lens: r - d = w and r² - d² = 0.25, so r >= 0.5.
    let r = 0.5 * (0.25 / w + w);
    let d = 0.5 * (0.25 / w - w);
    if !(b - a > 1.0e-3) {
        let u = 0.5 * (a + b) - 0.5;
        return if u.abs() < 0.5 { (r * r - u * u).max(0.0).sqrt() - d } else { 0.0 };
    }
    let integral = |x: f32| {
        let u = (x - 0.5).max(-0.5).min(0.5);
        0.5 * (u * (r * r - u * u).max(0.0).sqrt() + r * r * (u / r).max(-1.0).min(1.0).asin()) - d * u
    };
    ((integral(b) - integral(a)) / (b - a)).max(0.0)
}

/// The mean over `a..b` (leaf lengths from the base) of how deep a leaf's
/// teeth bite into its edge, in leaf lengths: `leaf_sdf`'s sawtooth, the
/// sawtooth averaged exactly over the window and its sqrt(4t(1 - t))
/// envelope taken at the window's middle (its teeth are a few per cent of
/// the blade, so that is close), and nothing past the tips.
fn teeth_mean(l: &LeafPrim, a: f32, b: f32) -> f32 {
    if !(l.serration > 0.0) {
        return 0.0;
    }
    let (lo, hi) = (a.max(0.0), b.min(1.0));
    if !(hi > lo) {
        return 0.0;
    }
    let t = (0.5 * (a + b)).max(0.0).min(1.0);
    let depth = l.serration * 0.3 * l.width * (4.0 * t * (1.0 - t)).sqrt();
    // The integral of the sawtooth frac(TEETH x) from 0 to x.
    let saw = |x: f32| {
        let u = x * TEETH;
        let f = u - u.floor();
        (u.floor() + f * f) / (2.0 * TEETH)
    };
    let mean = if b - a > 1.0e-4 {
        (saw(hi) - saw(lo)) / (b - a)
    } else {
        let x = 0.5 * (lo + hi) * TEETH;
        x - x.floor()
    };
    depth * mean
}

/// A leaf's coverage of the box `w` each way round `p` (`Prim::coverage`):
/// its blade a strip along the sagging midrib, as thick as the lens less the
/// bite of its teeth (as in `leaf_sdf`, whose teeth nibble the edge), and its
/// stalk a strip of its own; in leaf lengths throughout.
pub fn leaf_coverage(l: &LeafPrim, p: Vec2f, w: f32) -> f32 {
    let len = l.len.max(1.0e-6);
    let rel = p - l.origin;
    let x = (rel.x * l.axis.x + rel.y * l.axis.y) / len;
    let y = (rel.y * l.axis.x - rel.x * l.axis.y) / len;
    let wl = w.max(1.0e-6) / len;
    let t = x.max(0.0).min(1.0);
    let mid = l.bend * 4.0 * t * (1.0 - t);
    let across = (y - mid).abs();
    // Where the box cannot reach the blade (no thicker than its widest, nor
    // past its tips) `strip` gives 0: skip its integrals there.
    let blade = if across < l.width.max(1.0e-3).min(0.5) + wl && x + wl > 0.0 && x - wl < 1.0 {
        // The teeth thin the blade by their mean bite over the box: sharp,
        // the toothed edge itself; far off, half their depth, so their shade
        // keeps its area too.
        let thick = (lens_mean(l.width, x - wl, x + wl) - teeth_mean(l, x - wl, x + wl)).max(0.0);
        strip(across, thick, wl)
    } else {
        0.0
    };
    // The stalk behind the base, overlapping the blade a little, as in leaf_sdf.
    let stalk = if y.abs() < STALK_RADIUS + wl {
        strip(y.abs(), STALK_RADIUS * overlap(x - wl, x + wl, -STALK, 0.02) / (2.0 * wl), wl)
    } else {
        0.0
    };
    1.0 - (1.0 - blade) * (1.0 - stalk)
}

/// A stem's coverage of the box `w` each way round `p` (`Prim::coverage`): a
/// strip `radius` thick each way along the segment a-b.
pub fn stem_coverage(s: &StemPrim, p: Vec2f, w: f32) -> f32 {
    let w = w.max(1.0e-6);
    let ab = s.b - s.a;
    let length = ab.length();
    let rel = p - s.a;
    let (along, across) = if length > 1.0e-9 {
        let d = ab * (1.0 / length);
        (rel.x * d.x + rel.y * d.y, (rel.x * d.y - rel.y * d.x).abs())
    } else {
        (0.0, rel.length())
    };
    strip(across, s.radius * overlap(along - w, along + w, 0.0, length) / (2.0 * w), w)
}

/// A small deterministic generator over A2's integer hash: the same seed
/// gives the same canopy on every machine.
struct Rng(u32);

impl Rng {
    fn new(seed: u32, salt: u32) -> Rng {
        Rng(hash_u32(seed ^ hash_u32(salt)))
    }

    /// The next value in [0, 1).
    fn f(&mut self) -> f32 {
        self.0 = hash_u32(self.0.wrapping_add(0x9e37_79b9));
        (self.0 >> 8) as f32 * (1.0 / 16_777_216.0)
    }

    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.f()
    }
}

fn rotate2(v: Vec2f, angle: f32) -> Vec2f {
    let (s, c) = angle.sin_cos();
    vec2f(v.x * c - v.y * s, v.x * s + v.y * c)
}

/// Pinna length along a frond (u 0..1 of its length): short at the base,
/// longest about a third of the way up, tapering to nothing at the tip.
fn pinna_envelope(u: f32) -> f32 {
    smoothstep(0.0, 0.3, u) * (1.0 - u).max(0.0).powf(0.8)
}

/// Primitives binned over the light's rectangle.
#[derive(Clone, Debug)]
pub struct Foliage {
    prims: Vec<Prim>,
    bins: Vec<Vec<u32>>,
    nx: usize,
    ny: usize,
    min: Vec2f,
    cell: Vec2f,
    soft: f32,
}

impl Foliage {
    /// Leaves scattered over the light's rectangle `half` (s-units) and its
    /// glow (GLOW past it).
    /// - `density` 0..=1 sets how many: 8 per leaf-length squared at 1,
    ///   which leaves about a tenth open.
    /// - `scale` sets their size: a leaf is 0.35·scale long.
    /// - `seed` sets where they fall and how they turn.
    /// - `soft` is the penumbra half-width.
    ///
    /// Leaves are drawn in a fixed order, so for one seed a higher density
    /// only adds leaves, up to MAX_LEAVES; past it the same leaves grow so
    /// that they cover what density asks.
    pub fn leaves(half: Vec2f, density: f32, scale: f32, seed: u32, soft: f32) -> Foliage {
        let reach = vec2f(half.x + GLOW, half.y + GLOW);
        let mut len0 = 0.35 * scale.max(0.1).min(10.0);
        // Scatter a leaf length past the glow too, so leaves cross its edge.
        let wanted = density.max(0.0).min(1.0) * LEAVES_PER_AREA * 4.0 * (reach.x + len0) * (reach.y + len0) / (len0 * len0);
        let n = if wanted > MAX_LEAVES as f32 {
            len0 *= (wanted / MAX_LEAVES as f32).sqrt();
            MAX_LEAVES
        } else {
            wanted.round() as usize
        };
        let ex = reach.x + len0;
        let ey = reach.y + len0;
        let mut rng = Rng::new(seed, 0x1ea5);
        let mut prims = Vec::with_capacity(n);
        for _ in 0..n {
            let origin = vec2f(rng.range(-ex, ex), rng.range(-ey, ey));
            let angle = rng.range(0.0, std::f32::consts::TAU);
            prims.push(Prim::Leaf(LeafPrim {
                origin,
                axis: vec2f(angle.cos(), angle.sin()),
                len: len0 * rng.range(0.7, 1.3),
                width: rng.range(0.17, 0.28),
                bend: rng.range(-0.12, 0.12),
                serration: rng.range(0.0, 0.35),
            }));
        }
        Foliage::from_prims(prims, reach, soft)
    }

    /// Fern fronds rising from below the light and arching over it.
    /// - Each frond has a rachis: a smooth curve through four points (`MotionPath::through`).
    /// - Along it hang `PAIRS` pinnae a side, placed by arc length and sweeping toward the tip (level one).
    /// - Each pinna carries `LOBES` pinnules a side (level two).
    /// - A frond is 1.6·scale long. The frond count follows `density` and how many frond-lengths the light
    ///   holds, up to MAX_FRONDS; past it the fronds grow, so a fern covers a long light or a small frond size
    ///   as densely as a square one (D9, L:148: held at 24 fronds and 6 frond-lengths, it thinned out).
    pub fn fern(half: Vec2f, density: f32, scale: f32, seed: u32, soft: f32) -> Foliage {
        let mut len = 1.6 * scale.max(0.1).min(10.0);
        let fit = ((2.0 * half.x) * (2.0 * half.y) / (len * len)).max(1.0);
        let wanted = (1.0 + density.max(0.0).min(1.0) * 4.0) * fit;
        let fronds = if wanted > MAX_FRONDS as f32 {
            len *= (wanted / MAX_FRONDS as f32).sqrt();
            MAX_FRONDS
        } else {
            wanted.round().max(1.0) as usize
        };
        // Fronds shorter than the light stand at random heights, so small ones fill it.
        let room = (2.0 * half.y - 0.9 * len).max(0.0);
        let spread_x = if room > 0.0 { 0.9 } else { 0.3 };
        let mut rng = Rng::new(seed, 0xfe24);
        let mut prims = Vec::new();
        for f in 0..fronds {
            // Fanned across the clump, each a little off its slot; radians off vertical.
            let fan = (f as f32 + 0.5) / fronds as f32 - 0.5;
            let lean = (fan * 1.6 + rng.range(-0.15, 0.15)).max(-1.2).min(1.2);
            let dir = vec2f(lean.sin(), lean.cos());
            let right = vec2f(dir.y, -dir.x);
            let base = vec2f(rng.range(-1.0, 1.0) * half.x * spread_x, -half.y - 0.1 * len + rng.f() * room);
            // The frond arches toward the side it leans to.
            let toward = if lean < -0.05 { -1.0 } else { 1.0 };
            let arch = toward * rng.range(0.1, 0.35) * len;
            let points: Vec<[f64; 2]> = [0.0f32, 0.34, 0.68, 1.0]
                .iter()
                .map(|&u| {
                    let p = base + dir * (u * len) + right * (arch * u * u);
                    [p.x as f64, p.y as f64]
                })
                .collect();
            let Ok(rachis) = MotionPath::through(&points, 1.0) else { continue };
            let total = rachis.length() as f32;
            if !(total > 1.0e-4) {
                continue;
            }
            // The rachis: thin capsules along the flattened curve, tapering to the tip.
            let mut line: Vec<Vec2f> = Vec::new();
            rachis.flatten(8, |p, _| line.push(vec2f(p[0] as f32, p[1] as f32)));
            let segs = line.len().saturating_sub(1).max(1);
            for k in 0..line.len().saturating_sub(1) {
                let taper = 1.0 - 0.7 * k as f32 / segs as f32;
                prims.push(Prim::Stem(StemPrim { a: line[k], b: line[k + 1], radius: 0.012 * len * taper }));
            }
            // Level one: pinnae, alternating sides, 62 degrees off the rachis toward the tip.
            for k in 0..2 * PAIRS {
                let u = 0.12 + 0.86 * (k as f32 + 0.5) / (2 * PAIRS) as f32;
                let at = rachis.sample_at_length((u * total) as f64);
                let pos = vec2f(at.pos[0] as f32, at.pos[1] as f32);
                let tangent = vec2f(at.tangent[0] as f32, at.tangent[1] as f32).normalize();
                let side = if k % 2 == 0 { 1.0 } else { -1.0 };
                let axis = rotate2(tangent, side * 62f32.to_radians());
                let plen = 0.30 * len * pinna_envelope(u);
                prims.push(Prim::Stem(StemPrim { a: pos, b: pos + axis * plen, radius: 0.004 * len }));
                // Level two: pinnules along the pinna, alternating, shrinking toward its tip.
                for j in 0..2 * LOBES {
                    let v = (j as f32 + 0.5) / (2 * LOBES) as f32;
                    let lobe_side = if j % 2 == 0 { 1.0 } else { -1.0 };
                    prims.push(Prim::Leaf(LeafPrim {
                        origin: pos + axis * (plen * v * 0.92),
                        axis: rotate2(axis, lobe_side * 50f32.to_radians()),
                        len: (plen * 0.26 * (1.0 - 0.65 * v)).max(1.0e-4),
                        width: 0.32,
                        bend: 0.0,
                        serration: 0.0,
                    }));
                }
            }
        }
        Foliage::from_prims(prims, vec2f(half.x + GLOW, half.y + GLOW), soft)
    }

    /// Bins `prims` over the rectangle `half`: the light's rectangle and its
    /// glow, past which no light draws, so points outside are never asked. A
    /// primitive goes into every cell its bounding circle, padded by `pad`
    /// penumbrae, touches.
    fn from_prims(prims: Vec<Prim>, half: Vec2f, soft: f32) -> Foliage {
        let soft = soft.max(1.0e-5);
        let half = vec2f(half.x.max(1.0e-4), half.y.max(1.0e-4));
        let min = vec2f(-half.x, -half.y);
        let size = half * 2.0;
        // About 0.75 cells a side per sqrt(primitive), at most MAX_BINS a side.
        let target = ((prims.len() as f32).sqrt() * 0.75).ceil().max(1.0).min(MAX_BINS);
        let (nx, ny) = if size.x >= size.y {
            (target as usize, ((target * size.y / size.x).ceil() as usize).max(1))
        } else {
            (((target * size.x / size.y).ceil() as usize).max(1), target as usize)
        };
        let cell = vec2f(size.x / nx as f32, size.y / ny as f32);
        let mut bins = vec![Vec::new(); nx * ny];
        for (i, prim) in prims.iter().enumerate() {
            let (c, r) = prim.bounds();
            let r = r + prim.pad() * soft;
            let x0 = ((c.x - r - min.x) / cell.x).floor();
            let x1 = ((c.x + r - min.x) / cell.x).floor();
            let y0 = ((c.y - r - min.y) / cell.y).floor();
            let y1 = ((c.y + r - min.y) / cell.y).floor();
            // Written so a NaN bound is skipped too.
            if !(x1 >= 0.0 && y1 >= 0.0 && x0 < nx as f32 && y0 < ny as f32) {
                continue;
            }
            for by in (y0.max(0.0) as usize)..=(y1.min(ny as f32 - 1.0) as usize) {
                for bx in (x0.max(0.0) as usize)..=(x1.min(nx as f32 - 1.0) as usize) {
                    bins[by * nx + bx].push(i as u32);
                }
            }
        }
        Foliage { prims, bins, nx, ny, min, cell, soft }
    }

    /// Coverage 0..=1 of one primitive at `p` (`Prim::coverage`): exactly 0
    /// once `p` is `Prim::pad` penumbrae outside it.
    fn coverage(&self, prim: &Prim, p: Vec2f) -> f32 {
        prim.coverage(p, self.soft)
    }

    /// The product of (1 - coverage) over `prims`, in order. Below 1e-4
    /// nothing more can show through: it is 0, and the walk stops early.
    fn through(&self, prims: impl Iterator<Item = usize>, p: Vec2f) -> f32 {
        let mut t = 1.0f32;
        for i in prims {
            t *= 1.0 - self.coverage(&self.prims[i], p);
            if t <= 1.0e-4 {
                return 0.0;
            }
        }
        t
    }

    /// The fraction 0..=1 of light passing at `p` (s-units). Points outside
    /// the binned rectangle (past the glow, where no light draws) read the
    /// nearest edge cell.
    pub fn transmission(&self, p: Vec2f) -> f32 {
        let fx = (p.x - self.min.x) / self.cell.x;
        let fy = (p.y - self.min.y) / self.cell.y;
        if !(fx.is_finite() && fy.is_finite()) {
            return 1.0;
        }
        let bx = (fx.max(0.0) as usize).min(self.nx - 1);
        let by = (fy.max(0.0) as usize).min(self.ny - 1);
        self.through(self.bins[by * self.nx + bx].iter().map(|&i| i as usize), p)
    }

    /// `transmission` over every primitive, without the bins. A primitive
    /// the bins leave out has no coverage at the point, so the two agree
    /// bit for bit (the tests check it).
    pub fn transmission_brute(&self, p: Vec2f) -> f32 {
        self.through(0..self.prims.len(), p)
    }

    pub fn len(&self) -> usize {
        self.prims.len()
    }

    pub fn prims(&self) -> &[Prim] {
        &self.prims
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hdri::gobo::penumbra;

    fn one_leaf(soft: f32) -> Foliage {
        let leaf = LeafPrim { origin: vec2f(-0.5, 0.0), axis: vec2f(1.0, 0.0), len: 1.0, width: 0.25, bend: 0.0, serration: 0.0 };
        Foliage::from_prims(vec![Prim::Leaf(leaf)], vec2f(1.0, 1.0), soft)
    }

    /// Mean transmission over the rectangle `half` (s-units), 80 x 80 samples.
    fn mean_open(f: &Foliage, half: Vec2f) -> f32 {
        let n = 80;
        let mut sum = 0.0f64;
        for j in 0..n {
            for i in 0..n {
                let p = vec2f(
                    ((i as f32 + 0.5) / n as f32 * 2.0 - 1.0) * half.x,
                    ((j as f32 + 0.5) / n as f32 * 2.0 - 1.0) * half.y,
                );
                sum += f.transmission(p) as f64;
            }
        }
        (sum / (n * n) as f64) as f32
    }

    #[test]
    fn a_straight_leaf_has_an_exact_distance() {
        // Pointing up (+y) from the origin, two units long, half a unit wide
        // either side at its middle: a lens between two arcs.
        let leaf = LeafPrim { origin: vec2f(0.0, 0.0), axis: vec2f(0.0, 1.0), len: 2.0, width: 0.25, bend: 0.0, serration: 0.0 };
        assert!((leaf_sdf(&leaf, vec2f(0.0, 1.0)) + 0.5).abs() < 1.0e-4, "the middle is half a unit inside");
        assert!((leaf_sdf(&leaf, vec2f(0.6, 1.0)) - 0.1).abs() < 1.0e-4, "beside the widest point");
        assert!((leaf_sdf(&leaf, vec2f(-0.6, 1.0)) - 0.1).abs() < 1.0e-4, "and on the other side");
        assert!((leaf_sdf(&leaf, vec2f(0.0, 2.3)) - 0.3).abs() < 1.0e-4, "past the tip");
    }

    #[test]
    fn a_leaf_is_negative_inside_and_positive_outside() {
        // The same leaf with its midrib sagging 0.1 of its length and teeth.
        let leaf = LeafPrim { origin: vec2f(0.0, 0.0), axis: vec2f(0.0, 1.0), len: 2.0, width: 0.25, bend: 0.1, serration: 0.3 };
        // On the midrib 40% of the way up. The leaf's side axis is (-1, 0), so
        // a sideways offset `mid` (in lengths) sits at x = -mid * len.
        let x = 0.4;
        let mid = 0.1 * 4.0 * x * (1.0 - x);
        assert!(leaf_sdf(&leaf, vec2f(-mid * 2.0, x * 2.0)) < 0.0, "on the midrib");
        assert!(leaf_sdf(&leaf, vec2f(1.5, 1.0)) > 0.0, "well off to the side");
        assert!(leaf_sdf(&leaf, vec2f(0.0, 2.6)) > 0.0, "past the tip");
        assert!(leaf_sdf(&leaf, vec2f(0.0, -0.2)) < 0.0, "on the stalk");
        let stem = StemPrim { a: vec2f(0.0, 0.0), b: vec2f(1.0, 0.0), radius: 0.1 };
        assert!((stem_sdf(&stem, vec2f(0.5, 0.3)) - 0.2).abs() < 1.0e-6);
        assert!((stem_sdf(&stem, vec2f(1.3, 0.0)) - 0.2).abs() < 1.0e-6, "round at its ends");
    }

    #[test]
    fn the_bins_give_the_brute_force_answer() {
        // Over the rectangle and the glow a soft light draws past it (half its
        // soft band, at most half an s-unit: D9, L:147), sharp and far off.
        let half = vec2f(1.5, 1.0);
        let glow = 0.5;
        for soft in [0.02f32, penumbra(1.0)] {
            for f in [Foliage::leaves(half, 0.6, 1.0, 3, soft), Foliage::fern(half, 0.5, 1.0, 3, soft), Foliage::leaves(half, 0.8, 0.5, 4, soft)] {
                assert!(f.len() > 10);
                for j in 0..34 {
                    for i in 0..48 {
                        let p = vec2f((half.x + glow) * (-1.0 + 2.0 * (i as f32 + 0.5) / 48.0), (half.y + glow) * (-1.0 + 2.0 * (j as f32 + 0.5) / 34.0));
                        assert_eq!(f.transmission(p), f.transmission_brute(p), "soft {soft} at {p:?}");
                    }
                }
            }
        }
    }

    /// D9 (L:147): a soft light draws past its rectangle, up to half an
    /// s-unit at softness 1, and its leaves must be there too: the canopy in
    /// that glow is as thick as over the light.
    #[test]
    fn leaves_fill_a_soft_light_s_glow() {
        let half = vec2f(1.33, 1.0);
        let f = Foliage::leaves(half, 0.5, 1.0, 7, 0.01);
        // The open share over a band, sampled on a fine grid.
        let open_in = |inside: bool| {
            let (mut sum, mut n) = (0.0f64, 0);
            for j in 0..120 {
                for i in 0..150 {
                    let p = vec2f((half.x + 0.5) * (-1.0 + 2.0 * (i as f32 + 0.5) / 150.0), (half.y + 0.5) * (-1.0 + 2.0 * (j as f32 + 0.5) / 120.0));
                    // The outer part of the glow: 0.3 to 0.5 s-units past an edge.
                    let past = (p.x.abs() - half.x).max(p.y.abs() - half.y);
                    if (inside && past < 0.0) || (!inside && past > 0.3) {
                        sum += f.transmission(p) as f64;
                        n += 1;
                    }
                }
            }
            (sum / n as f64) as f32
        };
        let (over, glow) = (open_in(true), open_in(false));
        assert!((glow - over).abs() < 0.15, "the glow is {glow} open, the light {over}");
    }

    /// D9 (L:146): a leaf or a stem keeps the area of its shade however far
    /// off it hangs, as B6's box filter keeps a window's: the blur moves its
    /// shadow about and loses none, and a part thinner than the penumbra
    /// spreads thin instead of darkening a band twice its penumbra wide.
    #[test]
    fn a_blurred_part_keeps_its_area() {
        let parts = [
            ("leaf", Prim::Leaf(LeafPrim { origin: vec2f(-0.15, 0.0), axis: vec2f(1.0, 0.0), len: 0.3, width: 0.22, bend: 0.08, serration: 0.3 })),
            ("small leaf", Prim::Leaf(LeafPrim { origin: vec2f(-0.02, 0.0), axis: vec2f(0.6, 0.8), len: 0.04, width: 0.32, bend: 0.0, serration: 0.0 })),
            ("stem", Prim::Stem(StemPrim { a: vec2f(-0.3, -0.1), b: vec2f(0.3, 0.1), radius: 0.006 })),
        ];
        for (name, prim) in parts {
            // ∫ (1 - transmission) dA over the square round the part, at the sharpest penumbra and two softer ones.
            let shade = |soft: f32| {
                let f = Foliage::from_prims(vec![prim], vec2f(1.0, 1.0), soft);
                let n = 600;
                let cell = 1.2 / n as f32;
                let mut sum = 0.0f64;
                for j in 0..n {
                    for i in 0..n {
                        let p = vec2f(-0.6 + (i as f32 + 0.5) * cell, -0.6 + (j as f32 + 0.5) * cell);
                        sum += (1.0 - f.transmission(p)) as f64;
                    }
                }
                sum * (cell * cell) as f64
            };
            let sharp = shade(penumbra(0.0));
            assert!(sharp > 0.0, "{name}");
            for distance in [0.3f32, 1.0] {
                let soft = shade(penumbra(distance));
                assert!((soft / sharp - 1.0).abs() < 0.03, "{name} at distance {distance}: its shade is {soft}, sharp {sharp}");
            }
        }
    }

    /// D9 (L:146): tiny leaflets far smaller than the penumbra (a fern's
    /// pinnules near its tip) spread their little shade over the blur instead
    /// of multiplying their half-shadows to a black dot.
    #[test]
    fn tiny_leaflets_never_go_black() {
        let soft = penumbra(1.0);
        // A pinna's worth of leaflets a hundredth long, packed into a spot.
        let leaflets: Vec<Prim> = (0..2 * LOBES)
            .map(|j| {
                let a = j as f32 * 0.9;
                Prim::Leaf(LeafPrim { origin: vec2f(0.002 * j as f32, 0.0), axis: vec2f(a.cos(), a.sin()), len: 0.01, width: 0.32, bend: 0.0, serration: 0.0 })
            })
            .collect();
        let f = Foliage::from_prims(leaflets.clone(), vec2f(1.0, 1.0), soft);
        let darkest = (0..41 * 41)
            .map(|k| f.transmission(vec2f(-0.1 + 0.005 * (k % 41) as f32, -0.1 + 0.005 * (k / 41) as f32)))
            .fold(1.0f32, f32::min);
        // Their whole area is about 14 x 0.46 x 0.01², 0.1 % of the penumbra's box (2 x 0.122)².
        assert!(darkest > 0.95, "blurred, the leaflets shade at most a few per cent: {darkest}");
        // Against the light they are sharp, and dark where they lie.
        let sharp = Foliage::from_prims(leaflets, vec2f(1.0, 1.0), penumbra(0.0));
        let darkest = (0..100 * 100)
            .map(|k| sharp.transmission(vec2f(-0.012 + 0.0005 * (k % 100) as f32, -0.012 + 0.0005 * (k / 100) as f32)))
            .fold(1.0f32, f32::min);
        assert!(darkest < 0.05, "{darkest}");
    }

    /// D9 (L:148): Density acts at every leaf size. Capped at 900 leaves, a
    /// 60 x 45 light gave the same canopy at density 0.5 and 1 from leaf size
    /// 0.5 down, and size 0.1 stayed 81 % open; now the canopy covers what
    /// density asks whatever the leaves' size, and a fern does not thin out
    /// on a long light or at a small size either.
    #[test]
    fn density_acts_at_every_leaf_size() {
        // The 60 x 45 degree light in s-units.
        let light = vec2f(30f32.to_radians().tan() / 22.5f32.to_radians().tan(), 1.0);
        for scale in [0.1f32, 0.3, 1.0] {
            let open = |density: f32| mean_open(&Foliage::leaves(light, density, scale, 5, 0.01), light);
            let (half_on, full) = (open(0.5), open(1.0));
            assert!(full < 0.15, "leaf size {scale}: density 1 is a nearly closed canopy: {full}");
            assert!(half_on > full + 0.12, "leaf size {scale}: density 0.5 leaves {half_on} open, density 1 {full}");
        }
        // Fern: density still acts at a small frond size, and a long light
        // (120 x 20 degrees) is as covered as the 60 x 45 one.
        let fern = |half: Vec2f, density: f32, scale: f32| mean_open(&Foliage::fern(half, density, scale, 2, 0.01), half);
        assert!(fern(light, 0.75, 0.3) > fern(light, 1.0, 0.3) + 0.02, "{} against {}", fern(light, 0.75, 0.3), fern(light, 1.0, 0.3));
        let long = vec2f(60f32.to_radians().tan() / 10f32.to_radians().tan(), 1.0);
        let (wide, square) = (fern(long, 1.0, 1.0), fern(light, 1.0, 1.0));
        assert!((wide - square).abs() < 0.1, "a long light's fern is {wide} open, the 60 x 45 one's {square}");
    }

    #[test]
    fn leaves_are_fixed_by_their_seed() {
        let half = vec2f(1.0, 1.0);
        let a = Foliage::leaves(half, 0.5, 1.0, 11, 0.01);
        let b = Foliage::leaves(half, 0.5, 1.0, 11, 0.01);
        let c = Foliage::leaves(half, 0.5, 1.0, 12, 0.01);
        let mut differs = false;
        for j in 0..30 {
            for i in 0..30 {
                let p = vec2f(-1.0 + (i as f32 + 0.5) / 15.0, -1.0 + (j as f32 + 0.5) / 15.0);
                assert_eq!(a.transmission(p), b.transmission(p));
                differs |= a.transmission(p) != c.transmission(p);
            }
        }
        assert!(differs, "another seed scatters the leaves elsewhere");
    }

    #[test]
    fn density_sets_how_much_is_covered() {
        let half = vec2f(1.0, 1.0);
        assert_eq!(Foliage::leaves(half, 0.0, 1.0, 5, 0.01).len(), 0);
        // One seed draws the same leaves in the same order, so a higher density
        // only adds leaves: the open share must fall strictly.
        let m: Vec<f32> = [0.0f32, 0.2, 0.5, 1.0].iter().map(|&d| mean_open(&Foliage::leaves(half, d, 1.0, 5, 0.01), half)).collect();
        assert_eq!(m[0], 1.0);
        for w in m.windows(2) {
            assert!(w[0] > w[1], "{m:?}");
        }
        assert!(m[3] < 0.3, "density 1 is a nearly closed canopy: {m:?}");
        assert!(Foliage::leaves(vec2f(8.0, 1.0), 1.0, 0.3, 5, 0.01).len() <= MAX_LEAVES);
    }

    #[test]
    fn the_penumbra_widens_with_distance() {
        // Samples across the leaf's two side edges that are neither open nor dark.
        let grey = |soft: f32| {
            let f = one_leaf(soft);
            (0..400)
                .filter(|i| {
                    let t = f.transmission(vec2f(0.0, -0.4 + 0.8 * *i as f32 / 399.0));
                    t > 0.05 && t < 0.95
                })
                .count()
        };
        let sharp = grey(penumbra(0.0));
        let soft = grey(penumbra(1.0));
        assert!(soft > 4 * sharp.max(1), "sharp {sharp}, soft {soft}");
    }

    #[test]
    fn a_fern_has_pinnae_and_pinnules() {
        let half = vec2f(1.0, 1.0);
        let f = Foliage::fern(half, 0.0, 1.0, 9, 0.01);
        let leaflets = f.prims().iter().filter(|p| matches!(p, Prim::Leaf(_))).count();
        let stems = f.prims().iter().filter(|p| matches!(p, Prim::Stem(_))).count();
        // Every frond carries 2·PAIRS pinnae of 2·LOBES pinnules each (level two),
        // and every pinna its own midrib (level one) besides the rachis.
        assert!(leaflets > 0 && leaflets % (2 * PAIRS * 2 * LOBES) == 0, "{leaflets} pinnules");
        assert!(stems > leaflets / (2 * LOBES), "{stems} stems");
        let open = mean_open(&f, half);
        assert!(open > 0.3 && open < 0.99, "{open}");
    }
}
