// SDF primitives — based on Inigo Quilez's distance function reference
// https://iquilezles.org/articles/distfunctions/
//
// All shapes are centered at an optional `center` position.
// Shapes that have an axis use Y as the default axis.

use crate::sdf::Sdf3;
use makepad_csg_math::Vec3d;

// ============================================================
// Basic primitives
// ============================================================

/// Sphere SDF.
pub struct SdfSphere {
    pub center: Vec3d,
    pub radius: f64,
}

impl SdfSphere {
    pub fn new(center: Vec3d, radius: f64) -> Self {
        Self { center, radius }
    }
}

impl Sdf3 for SdfSphere {
    fn distance(&self, p: Vec3d) -> f64 {
        (p - self.center).length() - self.radius
    }
}

/// Axis-aligned box SDF (exact).
pub struct SdfBox {
    pub center: Vec3d,
    pub half_size: Vec3d,
}

impl SdfBox {
    pub fn new(center: Vec3d, size: Vec3d) -> Self {
        Self {
            center,
            half_size: size * 0.5,
        }
    }
}

impl Sdf3 for SdfBox {
    fn distance(&self, p: Vec3d) -> f64 {
        let q = (p - self.center).abs() - self.half_size;
        Vec3d::new(q.x.max(0.0), q.y.max(0.0), q.z.max(0.0)).length()
            + q.x.max(q.y.max(q.z)).min(0.0)
    }
}

/// Rounded box SDF (exact). Box with rounded edges.
pub struct SdfRoundedBox {
    pub center: Vec3d,
    pub half_size: Vec3d,
    pub radius: f64,
}

impl SdfRoundedBox {
    pub fn new(center: Vec3d, size: Vec3d, radius: f64) -> Self {
        Self {
            center,
            half_size: size * 0.5,
            radius,
        }
    }
}

impl Sdf3 for SdfRoundedBox {
    fn distance(&self, p: Vec3d) -> f64 {
        let q = (p - self.center).abs() - self.half_size + Vec3d::all(self.radius);
        Vec3d::new(q.x.max(0.0), q.y.max(0.0), q.z.max(0.0)).length()
            + q.x.max(q.y.max(q.z)).min(0.0)
            - self.radius
    }
}

/// Plane SDF (exact). Defined by normal and offset from origin.
pub struct SdfPlane {
    pub normal: Vec3d,
    pub offset: f64,
}

impl SdfPlane {
    pub fn new(normal: Vec3d, offset: f64) -> Self {
        Self {
            normal: normal.normalize(),
            offset,
        }
    }
}

impl Sdf3 for SdfPlane {
    fn distance(&self, p: Vec3d) -> f64 {
        p.dot(self.normal) + self.offset
    }
}

// ============================================================
// Capsule / pill shapes
// ============================================================

/// Capsule SDF (exact). Line segment with radius — the "pill" shape.
pub struct SdfCapsule {
    pub a: Vec3d,
    pub b: Vec3d,
    pub radius: f64,
}

impl SdfCapsule {
    pub fn new(a: Vec3d, b: Vec3d, radius: f64) -> Self {
        Self { a, b, radius }
    }
}

impl Sdf3 for SdfCapsule {
    fn distance(&self, p: Vec3d) -> f64 {
        let pa = p - self.a;
        let ba = self.b - self.a;
        let h = (pa.dot(ba) / ba.dot(ba)).clamp(0.0, 1.0);
        (pa - ba * h).length() - self.radius
    }
}

/// Capped cylinder SDF (exact). Vertical cylinder along Y axis.
pub struct SdfCylinder {
    pub center: Vec3d,
    pub radius: f64,
    pub half_height: f64,
}

impl SdfCylinder {
    pub fn new(center: Vec3d, radius: f64, height: f64) -> Self {
        Self {
            center,
            radius,
            half_height: height * 0.5,
        }
    }
}

impl Sdf3 for SdfCylinder {
    fn distance(&self, p: Vec3d) -> f64 {
        let d = p - self.center;
        let dx = (d.x * d.x + d.z * d.z).sqrt() - self.radius;
        let dy = d.y.abs() - self.half_height;
        (dx.max(0.0) * dx.max(0.0) + dy.max(0.0) * dy.max(0.0)).sqrt() + dx.max(dy).min(0.0)
    }
}

/// Rounded cylinder SDF (exact). Cylinder with rounded edges.
pub struct SdfRoundedCylinder {
    pub center: Vec3d,
    pub radius: f64,
    pub half_height: f64,
    pub rounding: f64,
}

impl SdfRoundedCylinder {
    pub fn new(center: Vec3d, radius: f64, height: f64, rounding: f64) -> Self {
        Self {
            center,
            radius,
            half_height: height * 0.5,
            rounding,
        }
    }
}

impl Sdf3 for SdfRoundedCylinder {
    fn distance(&self, p: Vec3d) -> f64 {
        let d = p - self.center;
        let dx = (d.x * d.x + d.z * d.z).sqrt() - self.radius + self.rounding;
        let dy = d.y.abs() - self.half_height + self.rounding;
        (dx.max(0.0) * dx.max(0.0) + dy.max(0.0) * dy.max(0.0)).sqrt() + dx.max(dy).min(0.0)
            - self.rounding
    }
}

// ============================================================
// Cones
// ============================================================

/// Capped cone SDF (exact). Cone frustum between two radii along Y.
pub struct SdfCappedCone {
    pub center: Vec3d,
    pub height: f64,
    pub r1: f64,
    pub r2: f64,
}

impl SdfCappedCone {
    pub fn new(center: Vec3d, height: f64, r1: f64, r2: f64) -> Self {
        Self {
            center,
            height,
            r1,
            r2,
        }
    }
}

impl Sdf3 for SdfCappedCone {
    fn distance(&self, p: Vec3d) -> f64 {
        let d = p - self.center;
        let q = (d.x * d.x + d.z * d.z).sqrt();
        let k1x = self.r2;
        let k1y = self.height;
        let k2x = self.r2 - self.r1;
        let k2y = 2.0 * self.height;
        let cax = q - (if d.y < 0.0 { self.r1 } else { self.r2 }).min(q);
        let cay = (d.y - if d.y < 0.0 { 0.0 } else { self.height }).abs();
        let k = k2x * k2x + k2y * k2y;
        let dot_val = (q - self.r1) * k2x + (d.y) * k2y;
        let t = (dot_val / k).clamp(0.0, 1.0);
        let cbx = q - self.r1 - k2x * t;
        let cby = d.y - k2y * t;
        let s = if cbx < 0.0 && cay - k1y < 0.0 {
            -1.0
        } else {
            1.0
        };
        (cax * cax + cay * cay).min(cbx * cbx + cby * cby).sqrt() * s
    }
}

/// Rounded cone SDF (exact). Cone with spherical caps of different radii.
pub struct SdfRoundedCone {
    pub a: Vec3d,
    pub b: Vec3d,
    pub r1: f64,
    pub r2: f64,
}

impl SdfRoundedCone {
    pub fn new(a: Vec3d, b: Vec3d, r1: f64, r2: f64) -> Self {
        Self { a, b, r1, r2 }
    }
}

impl Sdf3 for SdfRoundedCone {
    fn distance(&self, p: Vec3d) -> f64 {
        let ba = self.b - self.a;
        let l2 = ba.dot(ba);
        let rr = self.r1 - self.r2;
        let a2 = l2 - rr * rr;
        let il2 = 1.0 / l2;

        let pa = p - self.a;
        let y = pa.dot(ba);
        let z = y - l2;
        let x2 = (pa * l2 - ba * y).length_sq();
        let y2 = y * y * l2;
        let z2 = z * z * l2;

        let k = rr.signum() * rr * rr * x2;
        if k.signum() * (z2 * a2 - k) < 0.0 {
            return (x2 + z2).sqrt() * il2.sqrt() - self.r2;
        }
        if k.signum() * (y2 * a2 - k) < 0.0 {
            return (x2 + y2).sqrt() * il2.sqrt() - self.r1;
        }
        ((x2 * a2 * il2).sqrt() + y * rr) * il2.sqrt() - self.r1
    }
}

// ============================================================
// Torus variants
// ============================================================

/// Torus SDF (exact). In XZ plane, centered.
pub struct SdfTorus {
    pub center: Vec3d,
    pub major_radius: f64,
    pub minor_radius: f64,
}

impl SdfTorus {
    pub fn new(center: Vec3d, major_radius: f64, minor_radius: f64) -> Self {
        Self {
            center,
            major_radius,
            minor_radius,
        }
    }
}

impl Sdf3 for SdfTorus {
    fn distance(&self, p: Vec3d) -> f64 {
        let d = p - self.center;
        let q_x = (d.x * d.x + d.z * d.z).sqrt() - self.major_radius;
        (q_x * q_x + d.y * d.y).sqrt() - self.minor_radius
    }
}

// ============================================================
// Platonic / polyhedra
// ============================================================

/// Octahedron SDF (exact).
pub struct SdfOctahedron {
    pub center: Vec3d,
    pub size: f64,
}

impl SdfOctahedron {
    pub fn new(center: Vec3d, size: f64) -> Self {
        Self { center, size }
    }
}

impl Sdf3 for SdfOctahedron {
    fn distance(&self, p: Vec3d) -> f64 {
        let p = (p - self.center).abs();
        let m = p.x + p.y + p.z - self.size;
        let (qx, qy, qz) = if 3.0 * p.x < m {
            (p.x, p.y, p.z)
        } else if 3.0 * p.y < m {
            (p.y, p.z, p.x)
        } else if 3.0 * p.z < m {
            (p.z, p.x, p.y)
        } else {
            return m * 0.57735027;
        };
        let k = (0.5 * (qz - qy + self.size)).clamp(0.0, self.size);
        Vec3d::new(qx, qy - self.size + k, qz - k).length()
    }
}

/// Ellipsoid SDF (approximate bound).
pub struct SdfEllipsoid {
    pub center: Vec3d,
    pub radii: Vec3d,
}

impl SdfEllipsoid {
    pub fn new(center: Vec3d, radii: Vec3d) -> Self {
        Self { center, radii }
    }
}

impl Sdf3 for SdfEllipsoid {
    fn distance(&self, p: Vec3d) -> f64 {
        let d = p - self.center;
        let r = self.radii;
        let k0 = Vec3d::new(d.x / r.x, d.y / r.y, d.z / r.z).length();
        let k1 = Vec3d::new(d.x / (r.x * r.x), d.y / (r.y * r.y), d.z / (r.z * r.z)).length();
        if k1 > 1e-30 {
            k0 * (k0 - 1.0) / k1
        } else {
            0.0
        }
    }
}

/// Triangular prism SDF (approximate bound). Along Z axis.
pub struct SdfTriPrism {
    pub center: Vec3d,
    pub h: f64,
    pub depth: f64,
}

impl SdfTriPrism {
    pub fn new(center: Vec3d, h: f64, depth: f64) -> Self {
        Self { center, h, depth }
    }
}

impl Sdf3 for SdfTriPrism {
    fn distance(&self, p: Vec3d) -> f64 {
        let q = (p - self.center).abs();
        (q.z - self.depth).max(
            (q.x * 0.866025 + (p - self.center).y * 0.5).max(-(p - self.center).y) - self.h * 0.5,
        )
    }
}

/// Hexagonal prism SDF (approximate bound). Along Z axis.
pub struct SdfHexPrism {
    pub center: Vec3d,
    pub h: f64,
    pub depth: f64,
}

impl SdfHexPrism {
    pub fn new(center: Vec3d, h: f64, depth: f64) -> Self {
        Self { center, h, depth }
    }
}

impl Sdf3 for SdfHexPrism {
    fn distance(&self, p: Vec3d) -> f64 {
        let d = p - self.center;
        let q = d.abs();
        let hex_d = (q.x * 0.866025 + q.z * 0.5).max(q.z) - self.h;
        hex_d.max(q.y - self.depth)
    }
}

// ============================================================
// Transforms
// ============================================================

/// Translate an SDF.
pub struct SdfTranslate<S: Sdf3> {
    pub inner: S,
    pub offset: Vec3d,
}

impl<S: Sdf3> Sdf3 for SdfTranslate<S> {
    fn distance(&self, p: Vec3d) -> f64 {
        self.inner.distance(p - self.offset)
    }
}

/// Scale an SDF uniformly.
pub struct SdfScale<S: Sdf3> {
    pub inner: S,
    pub factor: f64,
}

impl<S: Sdf3> Sdf3 for SdfScale<S> {
    fn distance(&self, p: Vec3d) -> f64 {
        self.inner.distance(p / self.factor) * self.factor
    }
}

/// Round any SDF — expands the surface outward by `radius`.
pub struct SdfRound<S: Sdf3> {
    pub inner: S,
    pub radius: f64,
}

impl<S: Sdf3> SdfRound<S> {
    pub fn new(inner: S, radius: f64) -> Self {
        Self { inner, radius }
    }
}

impl<S: Sdf3> Sdf3 for SdfRound<S> {
    fn distance(&self, p: Vec3d) -> f64 {
        self.inner.distance(p) - self.radius
    }
}

/// Shell (hollow) any SDF — makes it a thin shell of given thickness.
pub struct SdfOnion<S: Sdf3> {
    pub inner: S,
    pub thickness: f64,
}

impl<S: Sdf3> SdfOnion<S> {
    pub fn new(inner: S, thickness: f64) -> Self {
        Self { inner, thickness }
    }
}

impl<S: Sdf3> Sdf3 for SdfOnion<S> {
    fn distance(&self, p: Vec3d) -> f64 {
        self.inner.distance(p).abs() - self.thickness
    }
}

// ============================================================
// Distance perturbation / warping
// ============================================================

/// Warp an SDF with a distance perturbation callback.
///
/// The callback receives `(center, p, distance)` and returns a modified distance.
/// `center` is the center/origin of the underlying shape (for shapes that have one),
/// passed through as `warp_center`. `p` is the grid-space sample point.
///
/// Example — wavy surface:
/// ```ignore
/// SdfWarp::new(
///     SdfSphere::new(Vec3d::ZERO, 2.0),
///     Vec3d::ZERO,
///     |_center, p, d| d + 0.1 * (p.x * 8.0).sin() * (p.y * 8.0).sin()
/// )
/// ```
pub struct SdfWarp<S: Sdf3, F: Fn(Vec3d, Vec3d, f64) -> f64> {
    pub inner: S,
    pub warp_center: Vec3d,
    pub warp_fn: F,
}

impl<S: Sdf3, F: Fn(Vec3d, Vec3d, f64) -> f64> SdfWarp<S, F> {
    pub fn new(inner: S, warp_center: Vec3d, warp_fn: F) -> Self {
        Self {
            inner,
            warp_center,
            warp_fn,
        }
    }
}

impl<S: Sdf3, F: Fn(Vec3d, Vec3d, f64) -> f64> Sdf3 for SdfWarp<S, F> {
    fn distance(&self, p: Vec3d) -> f64 {
        let d = self.inner.distance(p);
        (self.warp_fn)(self.warp_center, p, d)
    }
}
