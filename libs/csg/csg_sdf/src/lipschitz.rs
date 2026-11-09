//! How much faster than distance a field's value can change, and how far its values prove its
//! surface away ([`Clearance`]).
//!
//! A tree's pieces prove their slopes ([`crate::bounds::slopes_within`]); a Splash kernel
//! (`m.sdf`) may be any function, not a distance (a gyroid's `|g| / k` climbs up to about 1.9
//! times as fast as the distance to its sheet), and is measured on its own over its box
//! ([`measured`]): the steepest forward-difference gradients on a lattice, near its surface and
//! at any value. The tree's slopes compose round that measure as round any piece's.
//! Everything that culls by the value (the mesher, a coarse search, a march) reads its radius
//! from these so it never steps past a surface.

use crate::sdf::Sdf3;
use crate::shape::{SdfNode, SdfShape, P3};
use makepad_csg_math::Vec3d;
use std::sync::Arc;

/// Lattice cells per side.
const N: usize = 24;

/// How steep a field was measured over a box, as many times as a distance (each at least 1,
/// [`measured`]): outside its solid and inside, at the values within `near` of 0 and at any.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Measured {
    /// How far from 0 the near values reach: two lattice cells.
    pub near: f64,
    /// (near values, any value) outside the solid.
    pub outside: (f64, f64),
    /// (near values, any value) inside it.
    pub inside: (f64, f64),
}

impl Measured {
    /// How steep at the values in `w.0..=w.1` (0 where there are none): the near values' where
    /// the window stays within their reach on a side, else any value's.
    pub fn within(&self, w: (f64, f64)) -> f64 {
        if !(w.0 <= w.1) {
            return 0.0;
        }
        let outside = if w.1 < 0.0 { 0.0 } else if w.1 <= self.near { self.outside.0 } else { self.outside.1 };
        let inside = if w.0 > 0.0 { 0.0 } else if -w.0 <= self.near { self.inside.0 } else { self.inside.1 };
        outside.max(inside)
    }
}

/// The steepest gradient of `field` over `min..max` near its surface (within two lattice
/// cells of it) on either side, at least 1, with a tenth to spare ([`measured`]).
pub fn measure(field: &Arc<dyn Sdf3 + Send + Sync>, min: Vec3d, max: Vec3d) -> f64 {
    let m = measured(field, min, max);
    m.outside.0.max(m.inside.0)
}

/// How steep `field` is over `min..max` ([`Measured`]): the steepest forward-difference
/// gradient on a lattice of `N` cells a side, at the points whose values lie on each side and
/// within two cells of 0 or anywhere, each at least 1 and with a tenth to spare.
pub fn measured(field: &Arc<dyn Sdf3 + Send + Sync>, min: Vec3d, max: Vec3d) -> Measured {
    let size = max - min;
    let step = size.x.max(size.y).max(size.z).max(1e-9) / N as f64;
    let h = step / 16.0;
    let mut ps = Vec::with_capacity((N + 1).pow(3) * 4);
    for k in 0..=N {
        for j in 0..=N {
            for i in 0..=N {
                let p = Vec3d::new(min.x + size.x * i as f64 / N as f64, min.y + size.y * j as f64 / N as f64, min.z + size.z * k as f64 / N as f64);
                ps.extend([p, Vec3d::new(p.x + h, p.y, p.z), Vec3d::new(p.x, p.y + h, p.z), Vec3d::new(p.x, p.y, p.z + h)]);
            }
        }
    }
    let d = crate::surface_mesh::sample(field, ps);
    let near = 2.0 * step;
    let (mut outside, mut inside) = ((1.0f64, 1.0f64), (1.0f64, 1.0f64));
    for q in d.chunks_exact(4) {
        let g = ((q[1] - q[0]).powi(2) + (q[2] - q[0]).powi(2) + (q[3] - q[0]).powi(2)).sqrt() / h;
        if !g.is_finite() {
            continue;
        }
        let v = q[0];
        if v >= 0.0 {
            outside = (if v <= near { outside.0.max(g) } else { outside.0 }, outside.1.max(g));
        }
        if v <= 0.0 {
            inside = (if -v <= near { inside.0.max(g) } else { inside.0 }, inside.1.max(g));
        }
    }
    Measured { near, outside: (outside.0 * 1.1, outside.1 * 1.1), inside: (inside.0 * 1.1, inside.1 * 1.1) }
}

/// How far past its declared box a kernel is measured, in shares of the box's longest side.
const KERNEL_MARGIN: f64 = 0.25;

/// The box a kernel is measured over (its declared box grown by [`KERNEL_MARGIN`]) and its
/// measure there ([`measured`]), once per kernel and box in the process: a kernel is the same
/// function wherever its tree places it. A Splash kernel is known by its key (its whole program);
/// any other by its key and its field, which the record holds so no other field takes its place.
/// None: a kernel with no box, or one that reads a grid (each region crops its grid to the piece it
/// reads, and a grid read's own samples bound its tree).
pub(crate) fn kernel_measured(k: &crate::shape::SdfKernel) -> Option<(crate::bounds::Bounds, Measured)> {
    let b = k.bounds?;
    if k.solid.is_some() || k.splash.as_ref().is_some_and(|s| s.grid().is_some()) {
        return None;
    }
    type Field = Option<Arc<dyn Sdf3 + Send + Sync>>;
    type Seen = std::sync::Mutex<std::collections::HashMap<(u64, usize, [u64; 6]), (crate::bounds::Bounds, Measured, Field)>>;
    static SEEN: std::sync::OnceLock<Seen> = std::sync::OnceLock::new();
    let field = if k.splash.is_some() { 0 } else { Arc::as_ptr(&k.field) as *const u8 as usize };
    let key = (k.key, field, [b.0[0], b.0[1], b.0[2], b.1[0], b.1[1], b.1[2]].map(f64::to_bits));
    let seen = SEEN.get_or_init(Default::default);
    if let Some(&(grown, m, _)) = seen.lock().unwrap().get(&key) {
        return Some((grown, m));
    }
    let size = (0..3).map(|i| b.1[i] - b.0[i]).fold(0.0f64, f64::max);
    if !(size > 0.0) {
        return None;
    }
    let grown = crate::bounds::grow(b, KERNEL_MARGIN * size);
    let v = |p: P3| Vec3d::new(p[0], p[1], p[2]);
    let m = measured(&k.field, v(grown.0), v(grown.1));
    let mut seen = seen.lock().unwrap();
    if seen.len() >= 4096 {
        seen.clear();
    }
    seen.insert(key, (grown, m, k.splash.is_none().then(|| k.field.clone())));
    Some((grown, m))
}

/// How steep the ellipsoid's estimate `k0 (k0 - 1) / k1` is wherever `k0 >= 1/2`, from its radii.
/// Exact: the proof below reaches it. Closer to the centre there is no bound at all: the value
/// jumps there with the direction a point leaves in, so a user of this leaves out the ball of
/// `max(radii) / 2` about the centre, which holds all of `k0 < 1/2`.
///
/// With `D = diag(1 / r)`, `k0 = |D p|`, `u = D p / k0` and `g = |D u|`, the value is
/// `(k0 - 1) / g`. Its gradient is `(1 + c) w - c y`, with `c = 1 - 1 / k0`, the unit vector
/// `w = D u / g` and `y = D^3 u / g^3`. Over `k0 >= 1/2`, `c` runs over `[-1, 1)`. The squared norm
/// is convex in `c`, so it peaks at an end: `|y|` at `c = -1`, and at `c = 1` the less
/// `|2 w - y|^2 = |y|^2 - 4 (w.y - 1)`, as `w.y >= 1`. With the weights `u_i^2 / (r_i^2 g^2)`, which
/// sum to 1, and `X` the squared radius under them, `|y|^2 = E[1 / X^2] E[X]^2`. `1 / X^2` is convex,
/// so for a given mean the most sits on the two extreme radii; with `T = (r_max / r_min)^2` and `s`
/// the weight on the largest that is `(1 + s (T - 1))^2 (1 - s + s / T^2)`, whose only stationary
/// point in `0..1` is `s = (2 T + 1) / (3 T + 3)`.
pub fn ellipsoid_slope(radii: P3) -> f64 {
    let lo = radii[0].min(radii[1]).min(radii[2]);
    let hi = radii[0].max(radii[1]).max(radii[2]);
    let t = (hi / lo) * (hi / lo);
    let s = (2.0 * t + 1.0) / (3.0 * t + 3.0);
    (1.0 + s * (t - 1.0)) * (1.0 - s + s / (t * t)).sqrt()
}

/// The most `|grad|^2 - 1` of the ellipsoid's estimate reaches where `c = 1 - 1 / k0` is `c`, over
/// every direction: exact, and at least 0 (along an axis it is a distance). In the terms of
/// [`ellipsoid_slope`], `|grad|^2 = 1 - 2 c a + c^2 |y - w|^2` with `a = w.y - 1`, which with
/// `m = E[X]` is `1 + E[phi(X)] + c^2 + 2 c` for `phi(X) = c^2 m^2 / X^2 - 2 c (c + 1) m / X`. For a
/// given `m`, the most `E[phi]` over weights on the three squared radii is on the upper side of
/// their points' hull, a chord between two of them; so the most over all is the most of each chord
/// over its span, a cubic in `m` (its ends and where its slope is 0).
fn ellipsoid_excess(radii: P3, c: f64) -> f64 {
    let mut x = radii.map(|r| r * r);
    x.sort_by(f64::total_cmp);
    let x = x.map(|v| v / x[0]);
    let mut most = 0.0f64;
    for (i, j) in [(0, 1), (0, 2), (1, 2)] {
        let (xi, xj) = (x[i], x[j]);
        if !(xj > xi) {
            continue;
        }
        let (ui, uj, d) = (1.0 / xi, 1.0 / xj, xj - xi);
        let phi = |m: f64, u: f64| c * c * m * m * u * u - 2.0 * c * (c + 1.0) * m * u;
        let chord = |m: f64| (phi(m, ui) * (xj - m) + phi(m, uj) * (m - xi)) / d + c * c + 2.0 * c;
        // the chord as `a3 m^3 + a2 m^2 + a1 m + a0`: its slope's roots
        let a3 = c * c * (uj * uj - ui * ui) / d;
        let a2 = (c * c * (ui * ui * xj - uj * uj * xi) - 2.0 * c * (c + 1.0) * (uj - ui)) / d;
        let a1 = -2.0 * c * (c + 1.0) * (ui * xj - uj * xi) / d;
        let (qa, qb, qc) = (3.0 * a3, 2.0 * a2, a1);
        let mut at = [xi, xj, xi, xi];
        if qa != 0.0 {
            let disc = qb * qb - 4.0 * qa * qc;
            if disc >= 0.0 {
                let q = -0.5 * (qb + disc.sqrt().copysign(qb));
                if q != 0.0 {
                    at[2] = q / qa;
                    at[3] = qc / q;
                }
            }
        } else if qb != 0.0 {
            at[2] = -qc / qb;
        }
        for m in at {
            if m >= xi && m <= xj {
                most = most.max(chord(m));
            }
        }
    }
    // a hair's room for the rounding
    most * (1.0 + 1e-12)
}

/// How steep the ellipsoid's estimate is where `k0` lies in `k.0..=k.1`: [`ellipsoid_excess`] is
/// convex in `c` (each direction's is), so its most is at an end. Infinite unless `k.0 > 0`: at its
/// centre there is no bound. On the surface `c` is 0 and the estimate is exactly as steep as a
/// distance.
pub fn ellipsoid_slope_k(radii: P3, k: (f64, f64)) -> f64 {
    if !(k.0 > 0.0) {
        return f64::INFINITY;
    }
    let (c0, c1) = (1.0 - 1.0 / k.0, 1.0 - 1.0 / k.1);
    (1.0 + ellipsoid_excess(radii, c0).max(ellipsoid_excess(radii, c1))).sqrt()
}

/// How steep the ellipsoid's estimate is within `ball` of `p` (its own frame): over the `k0` the
/// ball spans ([`ellipsoid_slope_k`]; `k0` moves at most `1 / min(r)` per unit), infinite if it
/// may reach the centre.
pub fn ellipsoid_slope_near(radii: P3, p: P3, ball: f64) -> f64 {
    let lo = radii[0].min(radii[1]).min(radii[2]);
    let k = (0..3).map(|i| (p[i] / radii[i]) * (p[i] / radii[i])).sum::<f64>().sqrt();
    ellipsoid_slope_k(radii, (k - ball / lo, k + ball / lo))
}

/// The most `G = E[1 / X^2] E[X] - 1 / E[X]` reaches over weights on the squared radii `X` (see
/// [`ellipsoid_slope_within`]). For a given mean `m = E[X]` the convex `1 / X^2` has the most mean
/// on the two extreme radii, so with `m` in units of `min(r)^2`, `1..=T`,
/// `G min(r)^2 = m (T^2 + T + 1 - m (T + 1)) / T^2 - 1 / m`, which is concave in `m`, 0 at both
/// ends. Bisection brackets the peak where its slope changes sign, and the tangent at the
/// bracket's low end bounds it from above.
fn ellipsoid_spread(radii: P3) -> f64 {
    let lo = radii[0].min(radii[1]).min(radii[2]);
    let hi = radii[0].max(radii[1]).max(radii[2]);
    let t = (hi / lo) * (hi / lo);
    if !(t > 1.0) {
        return 0.0;
    }
    let g = |m: f64| m * (t * t + t + 1.0 - m * (t + 1.0)) / (t * t) - 1.0 / m;
    let dg = |m: f64| (t * t + t + 1.0 - 2.0 * m * (t + 1.0)) / (t * t) + 1.0 / (m * m);
    let (mut a, mut b) = (1.0, t);
    for _ in 0..64 {
        let mid = 0.5 * (a + b);
        if dg(mid) > 0.0 {
            a = mid;
        } else {
            b = mid;
        }
    }
    (g(a) + dg(a).max(0.0) * (b - a)) / (lo * lo)
}

/// How steep the ellipsoid's estimate is where its value lies in `w.0..=w.1` and `k0` in
/// `k.0..=k.1` (a region's; `0..=INFINITY` anywhere), from its radii; 0 where it has no such value.
///
/// The estimate is `v = (k0 - 1) / g` ([`ellipsoid_slope`]), and `k0 / k1 = 1 / g` lies between the
/// least and the most radius, so a value window bounds `k0` ([`ellipsoid_slope_k`] over that), and
/// outside the surface tighter: there `c = (k0 - 1) / k0 <= k0 - 1 = v g` and `a >= 0` leave
/// `|grad|^2 <= 1 + c^2 (|y|^2 - 1) <= 1 + v^2 g^2 (|y|^2 - 1)`, and `g^2 = 1 / E[X]` makes
/// `g^2 (|y|^2 - 1) = E[1 / X^2] E[X] - 1 / E[X]`, at most [`ellipsoid_spread`]'s `G`. So outside
/// it is at most `sqrt(1 + v^2 G)` as steep as a distance: about 1 near the surface, as steep as
/// its full bound only some `1 / sqrt(G)` out (about the radius of its sharpest curve).
///
/// Outside, the estimate is also `(k0 - 1) / |grad k0|`, and `k0` is convex: the tangent plane
/// `k0(p) + grad k0 . (q - p) <= k0(q) <= 1` puts every solid point `q` at least that far from
/// `p`, so it never overstates the distance. Inside, the ellipsoid holds its copy at `k0` grown by
/// `(1 - k0) min(r)`, and `|v| <= (1 - k0) max(r)`: it overstates the depth at most
/// `max(r) / min(r)` times.
pub fn ellipsoid_slope_within(radii: P3, k: (f64, f64), w: (f64, f64)) -> f64 {
    let lo = radii[0].min(radii[1]).min(radii[2]);
    let hi = radii[0].max(radii[1]).max(radii[2]);
    // the values `k0` allows: `k0 - 1` times between the least and the most radius
    let low = |k: f64| (k - 1.0) * if k >= 1.0 { lo } else { hi };
    let high = |k: f64| (k - 1.0) * if k >= 1.0 { hi } else { lo };
    let (a, b) = (w.0.max(low(k.0)), w.1.min(high(k.1)));
    if !(a <= b) {
        return 0.0;
    }
    let spread = ellipsoid_spread(radii);
    // the values `a..=b` on one side of the surface: the `k0` they allow
    let part = |a: f64, b: f64| {
        let ka = k.0.max(1.0 + a / if a >= 0.0 { hi } else { lo });
        let kb = k.1.min(1.0 + b / if b >= 0.0 { lo } else { hi });
        let by_k = ellipsoid_slope_k(radii, (ka, kb));
        if a >= 0.0 && spread > 0.0 {
            by_k.min((1.0 + b * b * spread).sqrt())
        } else if a >= 0.0 {
            1.0
        } else {
            by_k
        }
    };
    if a >= 0.0 || b <= 0.0 {
        part(a, b)
    } else {
        part(a, 0.0).max(part(0.0, b))
    }
}

/// The reaches from 0, as shares of a shape's box, of the values a [`Clearance`] holds a slope
/// for, besides any value's: down to where a thin ellipsoid's estimate, unbounded deep inside it,
/// is proven near its surface.
const REACHES: [f64; 8] = [1.0, 0.25, 1.0 / 16.0, 1.0 / 64.0, 1.0 / 256.0, 1.0 / 1024.0, 1.0 / 4096.0, 1.0 / 16384.0];

/// How far a field's values prove its surface away. Along a line from a point of value `v` the
/// value reaches 0 only through the values between, so the line runs at least `min(|v|, w) / s`
/// before the surface, `s` how steep the field can be at the values within `w` of 0 on `v`'s
/// side: for a few reaches `w`, and for any value. A cull at one radius reads one of them
/// ([`Clearance::at`]); one at many radii, the best ([`Clearance::radius`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Clearance {
    /// (reach, slope outside, slope inside): the slopes at least 1, infinite where none is proven.
    rungs: [(f64, f64, f64); REACHES.len() + 1],
}

impl Clearance {
    /// The slope `l` (at least 1) at every value: a distance's at 1.
    pub fn uniform(l: f64) -> Clearance {
        let l = l.max(1.0);
        Clearance { rungs: [(f64::INFINITY, l, l); REACHES.len() + 1] }
    }

    /// The radius of the ball about a point of value `v` that holds none of the surface (none for
    /// a value that is no number).
    pub fn radius(&self, v: f64) -> f64 {
        self.radius_past(v, 0.0)
    }

    /// The radius of the ball about a point of value `v` over which the value stays past
    /// `level` (at least 0) on `v`'s side: it falls to there only through the values between.
    pub fn radius_past(&self, v: f64, level: f64) -> f64 {
        let a = v.abs();
        if a.is_nan() {
            return 0.0;
        }
        self.rungs.iter().map(|&(w, o, i)| (a.min(w) - level) / if v > 0.0 { o } else { i }).fold(0.0, f64::max)
    }

    /// The same field with its values and distances `k` times as large.
    pub fn scaled(&self, k: f64) -> Clearance {
        Clearance { rungs: self.rungs.map(|(w, o, i)| (w * k, o, i)) }
    }

    /// Whether every value is its own radius, as a distance's.
    pub fn is_distance(&self) -> bool {
        self.rungs.iter().any(|&(w, o, i)| w == f64::INFINITY && o <= 1.0 && i <= 1.0)
    }

    /// For a cull at radius `r`, `(s, w)`: a ball of radius at most `r` is clear where
    /// `min(|v|, w) >= s r`. The least slope (both sides) whose reach holds `s r`; where none
    /// does, the widest proven on both sides, whose reach then proves no ball of `r`; where no
    /// reach is proven on both sides, the least slope either proves with a reach of 0, which
    /// proves no ball at all.
    pub fn at(&self, r: f64) -> (f64, f64) {
        let both = self.rungs.iter().map(|&(w, o, i)| (o.max(i), w)).filter(|(s, _)| s.is_finite());
        let held = both.clone().filter(|&(s, w)| r * s <= w).min_by(|a, b| a.0.total_cmp(&b.0));
        held.or_else(|| both.max_by(|a, b| a.1.total_cmp(&b.1))).unwrap_or_else(|| {
            let least = self.rungs.iter().map(|&(_, o, i)| o.min(i)).fold(f64::INFINITY, f64::min);
            (if least.is_finite() { least } else { 1.0 }, 0.0)
        })
    }
}

impl SdfShape {
    /// How far the shape's values prove its surface away, for points within its box's size of
    /// the box (a line from one to the solid stays there): what its pieces prove there at each
    /// reach ([`crate::bounds::slopes_within`]: an ellipsoid's estimate is about a distance near
    /// its surface and unbounded only deep inside, where its values then prove nothing; a
    /// kernel's as it measured round its own box, near its surface), and at any value where they
    /// prove no slope on either side (a kernel's values far from its box, a sculpt),
    /// [`SdfShape::lipschitz`]'s measure.
    pub fn clearance(&self) -> Clearance {
        let size = (0..3).map(|k| self.max[k] - self.min[k]).fold(0.0f64, f64::max);
        let mut c = self.clearance_in(&crate::bounds::grow((self.min, self.max), size));
        let any = &mut c.rungs[REACHES.len()];
        if any.1.is_infinite() && any.2.is_infinite() {
            let l = self.lipschitz().max(1.0);
            (any.1, any.2) = (l, l);
        }
        c
    }

    /// How far the shape's values prove its surface away for points in `region`: what its
    /// pieces prove there at each reach (the reaches its box's size sets, as
    /// [`SdfShape::clearance`]'s), infinite where they prove none. Nothing is measured.
    pub fn clearance_in(&self, region: &crate::bounds::Bounds) -> Clearance {
        let size = (0..3).map(|k| self.max[k] - self.min[k]).fold(0.0f64, f64::max);
        let mut rungs = [(f64::INFINITY, f64::INFINITY, f64::INFINITY); REACHES.len() + 1];
        for (r, w) in rungs.iter_mut().zip(REACHES.iter().map(|f| f * size).chain([f64::INFINITY])) {
            let s = crate::bounds::slopes_within(&self.root, region, w);
            *r = (w, s.outside.max(1.0), s.inside.max(1.0));
        }
        Clearance { rungs }
    }

    /// The shape's Lipschitz bound over its box, on both sides of its surface: its pieces' own
    /// ([`crate::bounds::slopes`]: 1 for a distance, more for an ellipsoid's estimate off its
    /// surface, a gyroid or a deform's stretch), measured over the box where a piece has none there
    /// (a kernel, a sculpt, an ellipsoid's centre).
    pub fn lipschitz(&self) -> f64 {
        let v = |p: [f64; 3]| Vec3d::new(p[0], p[1], p[2]);
        let s = crate::bounds::slopes(&self.root, &(self.min, self.max));
        let both = s.outside.max(s.inside);
        if both.is_finite() {
            both.max(1.0)
        } else {
            measure(&self.field(), v(self.min), v(self.max))
        }
    }
}

/// Whether a grid-read kernel is in the tree ([`crate::SdfKernel::grid_read`]).
pub(crate) fn has_grid_read(n: &SdfNode) -> bool {
    match n {
        SdfNode::Kernel(k) => k.solid.is_some(),
        SdfNode::Combine { of, .. } => of.iter().any(has_grid_read),
        SdfNode::Material { within, of, .. } => has_grid_read(of) || within.as_deref().is_some_and(has_grid_read),
        SdfNode::Shell { of, .. } | SdfNode::Round { of, .. } | SdfNode::Transform { of, .. } | SdfNode::Instance { of, .. } | SdfNode::Bone { of, .. } | SdfNode::Id { of, .. } | SdfNode::Deform { of, .. } | SdfNode::Sculpt { of, .. } | SdfNode::Texture { of, .. } => has_grid_read(of),
        _ => false,
    }
}

pub(crate) fn has_kernel(n: &SdfNode) -> bool {
    match n {
        SdfNode::Kernel(_) => true,
        SdfNode::Combine { of, .. } => of.iter().any(has_kernel),
        SdfNode::Material { within, of, .. } => has_kernel(of) || within.as_deref().is_some_and(has_kernel),
        SdfNode::Shell { of, .. } | SdfNode::Round { of, .. } | SdfNode::Transform { of, .. } | SdfNode::Instance { of, .. } | SdfNode::Bone { of, .. } | SdfNode::Deform { of, .. } | SdfNode::Id { of, .. } | SdfNode::Sculpt { of, .. } => has_kernel(of),
        // its own bound keeps it 1-Lipschitz: its pattern kernel is no distance
        SdfNode::Texture { of, .. } => has_kernel(of),
        _ => false,
    }
}

/// Whether a sculpt is in the tree ([`SdfNode::Sculpt`]): a piece whose slope nothing proves
/// ([`crate::bounds::slopes_within`]), which only a measure of its values bounds.
pub fn has_sculpt(n: &SdfNode) -> bool {
    match n {
        SdfNode::Sculpt { .. } => true,
        SdfNode::Combine { of, .. } => of.iter().any(has_sculpt),
        SdfNode::Material { within, of, .. } => has_sculpt(of) || within.as_deref().is_some_and(has_sculpt),
        SdfNode::Shell { of, .. } | SdfNode::Round { of, .. } | SdfNode::Transform { of, .. } | SdfNode::Instance { of, .. } | SdfNode::Bone { of, .. } | SdfNode::Deform { of, .. } | SdfNode::Id { of, .. } | SdfNode::Texture { of, .. } => has_sculpt(of),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shape::distance;

    #[test]
    fn an_ellipsoid_is_never_steeper_than_its_radii_say() {
        for r in [[21.0, 25.2, 21.0], [10.0, 20.0, 30.0], [5.0, 5.0, 25.0], [30.0, 30.0, 31.0], [4.0, 8.0, 8.0]] {
            let node = SdfNode::Ellipsoid { radii: r };
            let bound = ellipsoid_slope(r);
            let (mut most, mut seed) = (0.0f64, 12345u64);
            let mut next = || {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                ((seed >> 33) as f64 / (1u64 << 31) as f64) * 2.0 - 1.0
            };
            for i in 0..200_000 {
                // over directions bent toward each axis, where the peak is narrow, and from k0 = 1/2 out
                let bend = (next() + 1.0) * 1.1;
                let u: [f64; 3] = std::array::from_fn(|a| next() * r[a].powf(bend));
                let k = [0.5, 0.5, 0.6, 1.0, 3.0][i % 5];
                let m = (0..3).map(|a| (u[a] / r[a]).powi(2)).sum::<f64>().sqrt();
                let p = Vec3d::new(u[0] * k / m, u[1] * k / m, u[2] * k / m);
                let h = 1.0e-6;
                let d = |e: Vec3d| (distance(&node, p + e) - distance(&node, p - e)) / (2.0 * h);
                let g = [d(Vec3d::new(h, 0.0, 0.0)), d(Vec3d::new(0.0, h, 0.0)), d(Vec3d::new(0.0, 0.0, h))];
                most = most.max((g[0] * g[0] + g[1] * g[1] + g[2] * g[2]).sqrt());
            }
            assert!(most <= bound * (1.0 + 1.0e-6), "{r:?}: {most} over {bound}");
            assert!(most >= bound * 0.995, "{r:?}: {most} is well under {bound}: the bound is loose");
        }
    }

    #[test]
    fn an_ellipsoid_near_its_surface_is_as_steep_as_a_distance() {
        let r = [21.0, 25.2, 21.0];
        let node = SdfNode::Ellipsoid { radii: r };
        let near = |p: [f64; 3], ball: f64| ellipsoid_slope_near(r, p, ball);
        assert!(near([21.0, 0.0, 0.0], 1.0) < 1.01, "{}", near([21.0, 0.0, 0.0], 1.0));
        assert!(near([0.0, 1.0, 0.0], 2.0).is_infinite());
        assert!(near([21.0, 0.0, 0.0], 25.0).is_infinite());
        // never under what the field does within the ball
        let (mut seed, mut next) = (777u64, 0.0f64);
        let mut draw = || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            next = ((seed >> 33) as f64 / (1u64 << 31) as f64) * 2.0 - 1.0;
            next
        };
        for _ in 0..200_000 {
            let bend = (draw() + 1.0) * 1.1;
            let u: [f64; 3] = std::array::from_fn(|a| draw() * r[a].powf(bend));
            let m = (0..3).map(|a| (u[a] / r[a]).powi(2)).sum::<f64>().sqrt();
            let k = 0.3 + (draw() + 1.0) * 1.5;
            let p: [f64; 3] = std::array::from_fn(|a| u[a] * k / m);
            let ball = (draw() + 1.0) * 2.0;
            let bound = near(p, ball + 1.0e-5);
            let at = Vec3d::new(p[0], p[1], p[2]);
            let h = 1.0e-6;
            let d = |e: Vec3d| (distance(&node, at + e) - distance(&node, at - e)) / (2.0 * h);
            let g = [d(Vec3d::new(h, 0.0, 0.0)), d(Vec3d::new(0.0, h, 0.0)), d(Vec3d::new(0.0, 0.0, h))];
            let here = (g[0] * g[0] + g[1] * g[1] + g[2] * g[2]).sqrt();
            assert!(here <= bound * (1.0 + 1.0e-6), "{p:?} ball {ball}: {here} over {bound}");
        }
    }

    /// The estimate's value and its gradient's length at `p`, from their formulas (no differences).
    fn estimate(r: P3, p: [f64; 3]) -> (f64, f64) {
        let k0 = (0..3).map(|i| (p[i] / r[i]).powi(2)).sum::<f64>().sqrt();
        let k1 = (0..3).map(|i| (p[i] / (r[i] * r[i])).powi(2)).sum::<f64>().sqrt();
        let g: [f64; 3] = std::array::from_fn(|i| ((2.0 * k0 - 1.0) * p[i] / (r[i] * r[i] * k0) * k1 - k0 * (k0 - 1.0) * p[i] / (r[i].powi(4) * k1)) / (k1 * k1));
        (k0 * (k0 - 1.0) / k1, (g[0] * g[0] + g[1] * g[1] + g[2] * g[2]).sqrt())
    }

    /// The estimate is never steeper than its window says: radii up to 100 to 1, points from a
    /// ten-thousandth of the size off the surface out to a hundred times it, outside over `0..=v`
    /// and inside over `v..=0` at the point's own value `v`, in directions bent toward each axis and
    /// within a hair of the long and the short ones. The bound is met to the rounding of the
    /// gradient's formula alone, and outside it is reached, not loose.
    #[test]
    fn an_ellipsoid_is_never_steeper_than_its_window_says() {
        let (mut seed, mut checked) = (99u64, [0usize; 2]);
        let mut next = || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (seed >> 11) as f64 / (1u64 << 53) as f64
        };
        for r in [[100.0f64, 1.0, 1.0], [1.0, 100.0, 100.0], [100.0, 10.0, 1.0], [50.0, 1.0, 50.0], [3.0, 0.2, 2.0], [8.5, 3.2, 13.0], [36.7, 10.0, 80.0], [4.0, 3.0, 3.5], [1.0, 1.0, 1.0001]] {
            let size = r[0].max(r[1]).max(r[2]);
            let (mut most_out, mut most_in) = (0.0f64, 0.0f64);
            for i in 0..120_000 {
                // a direction on the surface
                let u: [f64; 3] = if i % 4 == 0 {
                    let axis = (next() * 3.0) as usize % 3;
                    let hair = 10f64.powf(-8.0 + 7.0 * next());
                    std::array::from_fn(|a| if a == axis { if next() < 0.5 { -r[a] } else { r[a] } } else { (2.0 * next() - 1.0) * hair * size })
                } else {
                    let bend = 3.0 * next();
                    std::array::from_fn(|a| (2.0 * next() - 1.0) * r[a].powf(bend))
                };
                let m = (0..3).map(|a| (u[a] / r[a]).powi(2)).sum::<f64>().sqrt();
                let on: [f64; 3] = std::array::from_fn(|a| u[a] / m);
                let len = (on[0] * on[0] + on[1] * on[1] + on[2] * on[2]).sqrt();
                if i % 2 == 0 {
                    let d = size * 10f64.powf(-4.0 + 6.0 * next());
                    let p: [f64; 3] = std::array::from_fn(|a| on[a] * (1.0 + d / len));
                    let (v, g) = estimate(r, p);
                    if !(v > 0.0) {
                        continue;
                    }
                    let bound = ellipsoid_slope_within(r, (0.0, f64::INFINITY), (0.0, v));
                    assert!(g <= bound * (1.0 + 1e-12), "{r:?} at {p:?}: {g} over {bound} outside");
                    most_out = most_out.max(g / bound);
                    checked[0] += 1;
                } else {
                    let k = 0.02 + 0.98 * next().sqrt();
                    let p: [f64; 3] = std::array::from_fn(|a| on[a] * k);
                    let (v, g) = estimate(r, p);
                    let bound = ellipsoid_slope_within(r, (0.0, f64::INFINITY), (v, 0.0));
                    if !(v < 0.0 && bound.is_finite()) {
                        continue;
                    }
                    assert!(g <= bound * (1.0 + 1e-12), "{r:?} at {p:?}: {g} over {bound} inside");
                    most_in = most_in.max(g / bound);
                    checked[1] += 1;
                }
            }
            assert!(most_out >= 0.99, "{r:?}: {most_out} of the bound outside at most: loose");
            assert!(most_in > 0.0, "{r:?}: no point inside where the window bounds it");
        }
        assert!(checked[0] > 500_000 && checked[1] > 20_000, "{checked:?} points outside and inside");
    }

    /// Two and a half times a capsule's distance, in metres: a kernel steeper than a distance.
    struct SteepLeg;
    impl Sdf3 for SteepLeg {
        fn distance(&self, p: Vec3d) -> f64 {
            let y = p.y.clamp(0.003, 0.027);
            2.5 * ((p.x - 0.006).hypot(p.z).hypot(p.y - y) - 0.002)
        }
    }

    /// A steep kernel's leg in metres, framed a thousand times up and blended between ellipsoids,
    /// as a figure's: its slopes hold round its own measure, so the tree proves rungs near its
    /// surface instead of measuring the whole of it, and every one holds over a dense sample (the
    /// rung at any value is the whole tree's measure, not a proof).
    #[test]
    fn a_kernel_in_a_tree_is_proven_round_its_own_measure() {
        let at = |p: P3, scale: f64, of: SdfNode| SdfNode::Transform { at: p, rotate: [0.0; 3], scale, of: Box::new(of) };
        let leg = SdfNode::Kernel(crate::shape::SdfKernel { key: 0x1e9, field: Arc::new(SteepLeg), splash: None, bounds: Some(([0.003, 0.0, -0.003], [0.009, 0.03, 0.003])), solid: None });
        let root = SdfNode::Combine {
            op: crate::shape::SdfOp::Union,
            blend: 1.0,
            of: vec![at([0.0, 45.0, 0.0], 1.0, SdfNode::Ellipsoid { radii: [20.0, 40.0, 15.0] }), at([0.0; 3], 1000.0, leg), at([6.0, 2.0, 0.0], 1.0, SdfNode::Ellipsoid { radii: [8.0, 5.0, 10.0] })],
        };
        let (min, max) = ([-21.0, -4.0, -16.0], [21.0, 86.0, 16.0]);
        let shape = SdfShape { root, min, max, materials: Vec::new() };
        let c = shape.clearance();
        let (s, w) = c.at(1.0);
        assert!(s.is_finite() && s >= 2.5 && w > 0.0, "{c:?}: ({s}, {w}) at 1");
        let proven: Vec<_> = c.rungs.iter().copied().filter(|r| r.1.is_finite() && r.2.is_finite()).collect();
        assert!(!proven.is_empty() && proven.iter().all(|r| r.1 >= 2.5 && r.1 <= 3.0), "{c:?}");
        // every proven rung holds at the values within its reach on each side
        let n = 64;
        let size: Vec<f64> = (0..3).map(|k| max[k] - min[k]).collect();
        let (mut seed, mut checked) = (0x5eed_u64, 0usize);
        let mut next = || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (seed >> 11) as f64 / (1u64 << 53) as f64
        };
        let h = 1e-5;
        for idx in 0..n * n * n {
            let cell = [idx % n, idx / n % n, idx / (n * n)];
            let p = Vec3d::new(
                min[0] - 0.1 * size[0] + 1.2 * size[0] * (cell[0] as f64 + next()) / n as f64,
                min[1] - 0.1 * size[1] + 1.2 * size[1] * (cell[1] as f64 + next()) / n as f64,
                min[2] - 0.1 * size[2] + 1.2 * size[2] * (cell[2] as f64 + next()) / n as f64,
            );
            let v = crate::shape::distance(&shape.root, p);
            let d = |e: Vec3d| (crate::shape::distance(&shape.root, p + e) - crate::shape::distance(&shape.root, p - e)) / (2.0 * h);
            let g = Vec3d::new(d(Vec3d::new(h, 0.0, 0.0)), d(Vec3d::new(0.0, h, 0.0)), d(Vec3d::new(0.0, 0.0, h)));
            let g = (g.x * g.x + g.y * g.y + g.z * g.z).sqrt();
            for &(w, o, i) in c.rungs.iter() {
                let bound = if v >= 0.0 { o } else { i };
                if w.is_finite() && bound.is_finite() && v.abs() <= w {
                    assert!(g <= bound * (1.0 + 1e-3), "{g} over {bound} at {p:?} (value {v}, reach {w})");
                    checked += 1;
                }
            }
        }
        assert!(checked > 20_000, "{checked} checks");
    }
}
