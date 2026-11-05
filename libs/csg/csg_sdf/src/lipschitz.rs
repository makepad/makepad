//! A field's measured Lipschitz bound: how much faster than distance its value changes.
//!
//! A Splash kernel (`m.sdf`) may be any function, not a distance: a gyroid's `|g| / k`
//! climbs up to about 1.9 times as fast as the distance to its sheet. Everything that steps
//! by the value (the mesher's culling, the slicer's coarse search, a march) divides by this
//! bound so it never steps past a surface. Measured as the steepest forward-difference gradient
//! on a lattice over the bounds, near the surface.

use crate::sdf::Sdf3;
use crate::shape::{SdfNode, SdfShape, P3};
use makepad_csg_math::Vec3d;
use std::sync::Arc;

/// Lattice cells per side.
const N: usize = 24;

/// The steepest gradient of `field` over `min..max` near its surface (within two lattice
/// cells of it), at least 1, with a tenth to spare.
pub fn measure(field: &Arc<dyn Sdf3 + Send + Sync>, min: Vec3d, max: Vec3d) -> f64 {
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
    let mut l = 1.0f64;
    for q in d.chunks_exact(4) {
        if q[0].abs() > 2.0 * step {
            continue;
        }
        let g = ((q[1] - q[0]).powi(2) + (q[2] - q[0]).powi(2) + (q[3] - q[0]).powi(2)).sqrt() / h;
        if g.is_finite() {
            l = l.max(g);
        }
    }
    l * 1.1
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

/// How steep the ellipsoid's estimate is within `ball` of `p` (its own frame). Infinite if that may
/// reach its centre. In the terms of [`ellipsoid_slope`], the squared gradient is
/// `1 - 2 c a + c^2 |y - w|^2` with `a = w.y - 1`: Kantorovich's inequality puts `a` in
/// `0..=(T - 1)^2 / (4 T)`, and `|y - w|^2 = |y|^2 - 1 - 2 a` is under the peak of `|y|^2`, less 1. On
/// the surface `c` is 0 and the estimate is exactly as steep as a distance.
pub fn ellipsoid_slope_near(radii: P3, p: P3, ball: f64) -> f64 {
    let lo = radii[0].min(radii[1]).min(radii[2]);
    let hi = radii[0].max(radii[1]).max(radii[2]);
    let k = (0..3).map(|i| (p[i] / radii[i]) * (p[i] / radii[i])).sum::<f64>().sqrt();
    let (k0, k1) = (k - ball / lo, k + ball / lo);
    if k0 <= 0.0 {
        return f64::INFINITY;
    }
    let (c0, c1) = (1.0 - 1.0 / k0, 1.0 - 1.0 / k1);
    let t = (hi / lo) * (hi / lo);
    let (most_a, most_v) = ((t - 1.0) * (t - 1.0) / (4.0 * t), ellipsoid_slope(radii).powi(2) - 1.0);
    let c = c0.abs().max(c1.abs());
    (1.0 + 2.0 * (-c0).max(0.0) * most_a + c * c * most_v).sqrt()
}

impl SdfShape {
    /// The shape's Lipschitz bound: 1 for a field built from distances (the engine's own
    /// nodes keep it), measured over the bounds when a kernel is in it.
    pub fn lipschitz(&self) -> f64 {
        if !has_kernel(&self.root) {
            return 1.0;
        }
        let v = |p: [f64; 3]| Vec3d::new(p[0], p[1], p[2]);
        measure(&self.field(), v(self.min), v(self.max))
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
}
