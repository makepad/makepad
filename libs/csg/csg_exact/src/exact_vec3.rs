// Exact 3D vector using expansion arithmetic.
// Each coordinate is an Expansion (exact real number).
// Used for computing exact intersection points in corefinement.

use crate::expansion::Expansion;
use makepad_csg_math::Vec3d;

/// A 3D vector with exact (expansion) coordinates.
#[derive(Clone, Debug)]
pub struct ExactVec3 {
    pub x: Expansion,
    pub y: Expansion,
    pub z: Expansion,
}

impl ExactVec3 {
    /// Create from three f64 values.
    pub fn from_f64(x: f64, y: f64, z: f64) -> ExactVec3 {
        ExactVec3 {
            x: Expansion::from_f64(x),
            y: Expansion::from_f64(y),
            z: Expansion::from_f64(z),
        }
    }

    /// Create from a Vec3d.
    pub fn from_vec3d(v: Vec3d) -> ExactVec3 {
        ExactVec3::from_f64(v.x, v.y, v.z)
    }

    /// Convert to approximate Vec3d.
    pub fn to_vec3d(&self) -> Vec3d {
        Vec3d::new(self.x.estimate(), self.y.estimate(), self.z.estimate())
    }

    /// Add two exact vectors.
    pub fn add(&self, other: &ExactVec3) -> ExactVec3 {
        ExactVec3 {
            x: self.x.add(&other.x),
            y: self.y.add(&other.y),
            z: self.z.add(&other.z),
        }
    }

    /// Subtract two exact vectors.
    pub fn sub(&self, other: &ExactVec3) -> ExactVec3 {
        ExactVec3 {
            x: self.x.sub(&other.x),
            y: self.y.sub(&other.y),
            z: self.z.sub(&other.z),
        }
    }

    /// Scale by an exact scalar.
    pub fn scale(&self, s: &Expansion) -> ExactVec3 {
        ExactVec3 {
            x: self.x.mul(s),
            y: self.y.mul(s),
            z: self.z.mul(s),
        }
    }

    /// Scale by an f64 scalar.
    pub fn scale_f64(&self, s: f64) -> ExactVec3 {
        ExactVec3 {
            x: self.x.mul_f64(s),
            y: self.y.mul_f64(s),
            z: self.z.mul_f64(s),
        }
    }

    /// Exact dot product.
    pub fn dot(&self, other: &ExactVec3) -> Expansion {
        let xx = self.x.mul(&other.x);
        let yy = self.y.mul(&other.y);
        let zz = self.z.mul(&other.z);
        xx.add(&yy).add(&zz)
    }

    /// Exact cross product.
    pub fn cross(&self, other: &ExactVec3) -> ExactVec3 {
        ExactVec3 {
            x: self.y.mul(&other.z).sub(&self.z.mul(&other.y)),
            y: self.z.mul(&other.x).sub(&self.x.mul(&other.z)),
            z: self.x.mul(&other.y).sub(&self.y.mul(&other.x)),
        }
    }

    /// Linear interpolation: self + t * (other - self)
    /// where t is represented as num/den (exact rational).
    pub fn lerp_rational(&self, other: &ExactVec3, num: &Expansion, den: &Expansion) -> ExactVec3 {
        // result = self * den + (other - self) * num, then divide by den
        // We return the non-divided version and the denominator separately
        // For now, evaluate to f64 at the end.
        let diff = other.sub(self);
        let offset = diff.scale(num);
        let base = self.scale(den);
        let numerator = base.add(&offset);
        // Divide each component by den (approximate at this point)
        let d = den.estimate();
        if d.abs() < 1e-300 {
            return self.clone();
        }
        let inv_d = 1.0 / d;
        ExactVec3::from_f64(
            numerator.x.estimate() * inv_d,
            numerator.y.estimate() * inv_d,
            numerator.z.estimate() * inv_d,
        )
    }
}

/// Compute the exact intersection point of a line segment (p0->p1) with a plane
/// defined by (a, b, c). Uses expansion arithmetic for the intersection parameter.
///
/// Returns the intersection point as an approximate Vec3d, computed from
/// exact arithmetic for the parameter t.
pub fn exact_segment_plane_intersection(
    p0: Vec3d,
    p1: Vec3d,
    plane_normal: Vec3d,
    plane_dist: f64,
) -> Vec3d {
    // t = (plane_dist - dot(normal, p0)) / dot(normal, p1 - p0)
    let ep0 = ExactVec3::from_vec3d(p0);
    let ep1 = ExactVec3::from_vec3d(p1);
    let en = ExactVec3::from_vec3d(plane_normal);
    let ed = Expansion::from_f64(plane_dist);

    let n_dot_p0 = en.dot(&ep0);
    let dir = ep1.sub(&ep0);
    let n_dot_dir = en.dot(&dir);

    let num = ed.sub(&n_dot_p0); // plane_dist - dot(n, p0)

    // t = num / n_dot_dir
    let t_approx = if n_dot_dir.estimate().abs() < 1e-300 {
        0.5 // degenerate, return midpoint
    } else {
        num.estimate() / n_dot_dir.estimate()
    };

    // Compute intersection using the exact-ish t
    Vec3d::new(
        p0.x + t_approx * (p1.x - p0.x),
        p0.y + t_approx * (p1.y - p0.y),
        p0.z + t_approx * (p1.z - p0.z),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_csg_math::dvec3;

    #[test]
    fn test_exact_vec3_add() {
        let a = ExactVec3::from_f64(1.0, 2.0, 3.0);
        let b = ExactVec3::from_f64(4.0, 5.0, 6.0);
        let c = a.add(&b);
        let v = c.to_vec3d();
        assert_eq!(v.x, 5.0);
        assert_eq!(v.y, 7.0);
        assert_eq!(v.z, 9.0);
    }

    #[test]
    fn test_exact_vec3_sub() {
        let a = ExactVec3::from_f64(5.0, 7.0, 9.0);
        let b = ExactVec3::from_f64(1.0, 2.0, 3.0);
        let c = a.sub(&b);
        let v = c.to_vec3d();
        assert_eq!(v.x, 4.0);
        assert_eq!(v.y, 5.0);
        assert_eq!(v.z, 6.0);
    }

    #[test]
    fn test_exact_vec3_dot() {
        let a = ExactVec3::from_f64(1.0, 2.0, 3.0);
        let b = ExactVec3::from_f64(4.0, 5.0, 6.0);
        let d = a.dot(&b);
        assert_eq!(d.estimate(), 32.0); // 4+10+18
    }

    #[test]
    fn test_exact_vec3_cross() {
        let a = ExactVec3::from_f64(1.0, 0.0, 0.0);
        let b = ExactVec3::from_f64(0.0, 1.0, 0.0);
        let c = a.cross(&b);
        let v = c.to_vec3d();
        assert_eq!(v.x, 0.0);
        assert_eq!(v.y, 0.0);
        assert_eq!(v.z, 1.0);
    }

    #[test]
    fn test_exact_cross_cancellation() {
        // Test a case where naive cross product loses precision
        let big = 1e15;
        let a = ExactVec3::from_f64(big, big + 1.0, big + 2.0);
        let b = ExactVec3::from_f64(big + 3.0, big + 4.0, big + 5.0);
        let c = a.cross(&b);
        let v = c.to_vec3d();
        // cross = ((big+1)(big+5) - (big+2)(big+4),
        //          (big+2)(big+3) - big*(big+5),
        //          big*(big+4) - (big+1)(big+3))
        // = (big^2+6big+5 - big^2-6big-8, ...) = (-3, 6, -3)
        assert!((v.x - (-3.0)).abs() < 1e-5, "cross x: {}", v.x);
        assert!((v.y - 6.0).abs() < 1e-5, "cross y: {}", v.y);
        assert!((v.z - (-3.0)).abs() < 1e-5, "cross z: {}", v.z);
    }

    #[test]
    fn test_segment_plane_intersection() {
        // Segment from (0,0,-1) to (0,0,1), plane z=0 (normal=(0,0,1), dist=0)
        let p0 = dvec3(0.0, 0.0, -1.0);
        let p1 = dvec3(0.0, 0.0, 1.0);
        let normal = dvec3(0.0, 0.0, 1.0);
        let result = exact_segment_plane_intersection(p0, p1, normal, 0.0);
        assert!(result.x.abs() < 1e-15);
        assert!(result.y.abs() < 1e-15);
        assert!(result.z.abs() < 1e-15);
    }

    #[test]
    fn test_segment_plane_intersection_offset() {
        // Segment from (0,0,0) to (0,0,2), plane z=1.5
        let p0 = dvec3(0.0, 0.0, 0.0);
        let p1 = dvec3(0.0, 0.0, 2.0);
        let normal = dvec3(0.0, 0.0, 1.0);
        let result = exact_segment_plane_intersection(p0, p1, normal, 1.5);
        assert!(result.x.abs() < 1e-15);
        assert!(result.y.abs() < 1e-15);
        assert!((result.z - 1.5).abs() < 1e-15);
    }
}
