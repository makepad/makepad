// Planed - f64 plane for CSG operations
// Equation: normal.dot(point) - dist = 0
// Points on the front side have normal.dot(point) - dist > 0

use crate::vec3::Vec3d;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Side {
    Front,
    Back,
    On,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Planed {
    pub normal: Vec3d,
    pub dist: f64, // normal.dot(point_on_plane)
}

impl Planed {
    /// Create a plane from a normal vector and a point on the plane.
    pub fn from_normal_and_point(normal: Vec3d, point: Vec3d) -> Planed {
        let n = normal.normalize();
        Planed {
            normal: n,
            dist: n.dot(point),
        }
    }

    /// Create a plane from three points (CCW winding = front-facing normal).
    pub fn from_points(a: Vec3d, b: Vec3d, c: Vec3d) -> Option<Planed> {
        let normal = (b - a).cross(c - a);
        let len_sq = normal.length_sq();
        if len_sq < 1e-30 {
            return None; // degenerate triangle
        }
        let n = normal * (1.0 / len_sq.sqrt());
        Some(Planed {
            normal: n,
            dist: n.dot(a),
        })
    }

    /// Signed distance from point to plane.
    /// Positive = front side (same side as normal).
    pub fn signed_distance(self, point: Vec3d) -> f64 {
        self.normal.dot(point) - self.dist
    }

    /// Classify a point relative to the plane.
    /// Uses the given epsilon for the "on" tolerance.
    pub fn classify_point(self, point: Vec3d, epsilon: f64) -> Side {
        let d = self.signed_distance(point);
        if d > epsilon {
            Side::Front
        } else if d < -epsilon {
            Side::Back
        } else {
            Side::On
        }
    }

    /// Flip the plane (reverse the normal).
    pub fn flip(self) -> Planed {
        Planed {
            normal: -self.normal,
            dist: -self.dist,
        }
    }

    /// Compute the intersection of a line segment (a->b) with this plane.
    /// Returns the interpolation parameter t where intersection = a + t*(b-a).
    /// Does NOT check if t is in [0,1].
    pub fn intersect_line_t(self, a: Vec3d, b: Vec3d) -> f64 {
        let da = self.signed_distance(a);
        let db = self.signed_distance(b);
        let denom = da - db;
        if denom.abs() < 1e-30 {
            0.5 // parallel, return midpoint
        } else {
            da / denom
        }
    }

    /// Compute the intersection point of a line segment with this plane.
    pub fn intersect_line(self, a: Vec3d, b: Vec3d) -> Vec3d {
        let t = self.intersect_line_t(a, b);
        a.lerp(b, t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vec3::dvec3;

    #[test]
    fn test_from_points() {
        let p = Planed::from_points(
            dvec3(0.0, 0.0, 0.0),
            dvec3(1.0, 0.0, 0.0),
            dvec3(0.0, 1.0, 0.0),
        )
        .unwrap();
        // Normal should point in +Z direction
        assert!((p.normal.z - 1.0).abs() < 1e-12);
        assert!(p.dist.abs() < 1e-12);
    }

    #[test]
    fn test_from_points_degenerate() {
        let p = Planed::from_points(
            dvec3(0.0, 0.0, 0.0),
            dvec3(1.0, 0.0, 0.0),
            dvec3(2.0, 0.0, 0.0), // collinear
        );
        assert!(p.is_none());
    }

    #[test]
    fn test_signed_distance() {
        let p = Planed::from_normal_and_point(Vec3d::Z, dvec3(0.0, 0.0, 5.0));
        assert!((p.signed_distance(dvec3(0.0, 0.0, 10.0)) - 5.0).abs() < 1e-12);
        assert!((p.signed_distance(dvec3(0.0, 0.0, 0.0)) - (-5.0)).abs() < 1e-12);
        assert!(p.signed_distance(dvec3(0.0, 0.0, 5.0)).abs() < 1e-12);
    }

    #[test]
    fn test_classify() {
        let p = Planed::from_normal_and_point(Vec3d::Z, Vec3d::ZERO);
        let eps = 1e-6;
        assert_eq!(p.classify_point(dvec3(0.0, 0.0, 1.0), eps), Side::Front);
        assert_eq!(p.classify_point(dvec3(0.0, 0.0, -1.0), eps), Side::Back);
        assert_eq!(p.classify_point(dvec3(0.0, 0.0, 0.0), eps), Side::On);
        assert_eq!(p.classify_point(dvec3(0.0, 0.0, 1e-8), eps), Side::On);
    }

    #[test]
    fn test_flip() {
        let p = Planed::from_normal_and_point(Vec3d::Z, dvec3(0.0, 0.0, 5.0));
        let pf = p.flip();
        assert!((pf.normal.z - (-1.0)).abs() < 1e-12);
        assert!((pf.dist - (-5.0)).abs() < 1e-12);
    }

    #[test]
    fn test_intersect_line() {
        let p = Planed::from_normal_and_point(Vec3d::Z, Vec3d::ZERO);
        let a = dvec3(0.0, 0.0, -1.0);
        let b = dvec3(0.0, 0.0, 1.0);
        let hit = p.intersect_line(a, b);
        assert!(hit.x.abs() < 1e-12);
        assert!(hit.y.abs() < 1e-12);
        assert!(hit.z.abs() < 1e-12);
    }

    #[test]
    fn test_intersect_line_offset() {
        let p = Planed::from_normal_and_point(Vec3d::Z, dvec3(0.0, 0.0, 3.0));
        let a = dvec3(5.0, 7.0, 0.0);
        let b = dvec3(5.0, 7.0, 6.0);
        let hit = p.intersect_line(a, b);
        assert!((hit.x - 5.0).abs() < 1e-12);
        assert!((hit.y - 7.0).abs() < 1e-12);
        assert!((hit.z - 3.0).abs() < 1e-12);
    }
}
