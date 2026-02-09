// BBox3d - f64 axis-aligned bounding box for spatial queries

use crate::vec3::Vec3d;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BBox3d {
    pub min: Vec3d,
    pub max: Vec3d,
}

impl BBox3d {
    /// An empty bbox (inverted so any expand will work correctly).
    pub fn empty() -> BBox3d {
        BBox3d {
            min: Vec3d::new(f64::INFINITY, f64::INFINITY, f64::INFINITY),
            max: Vec3d::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY),
        }
    }

    pub fn from_min_max(min: Vec3d, max: Vec3d) -> BBox3d {
        BBox3d { min, max }
    }

    pub fn from_points(points: &[Vec3d]) -> BBox3d {
        let mut b = BBox3d::empty();
        for &p in points {
            b.expand(p);
        }
        b
    }

    pub fn from_triangle(a: Vec3d, b: Vec3d, c: Vec3d) -> BBox3d {
        BBox3d {
            min: a.min(b).min(c),
            max: a.max(b).max(c),
        }
    }

    pub fn expand(&mut self, point: Vec3d) {
        self.min = self.min.min(point);
        self.max = self.max.max(point);
    }

    pub fn union(self, other: BBox3d) -> BBox3d {
        BBox3d {
            min: self.min.min(other.min),
            max: self.max.max(other.max),
        }
    }

    pub fn intersects(self, other: BBox3d) -> bool {
        self.min.x <= other.max.x
            && self.max.x >= other.min.x
            && self.min.y <= other.max.y
            && self.max.y >= other.min.y
            && self.min.z <= other.max.z
            && self.max.z >= other.min.z
    }

    pub fn contains(self, point: Vec3d) -> bool {
        point.x >= self.min.x
            && point.x <= self.max.x
            && point.y >= self.min.y
            && point.y <= self.max.y
            && point.z >= self.min.z
            && point.z <= self.max.z
    }

    pub fn center(self) -> Vec3d {
        (self.min + self.max) * 0.5
    }

    pub fn size(self) -> Vec3d {
        self.max - self.min
    }

    pub fn surface_area(self) -> f64 {
        let s = self.size();
        2.0 * (s.x * s.y + s.y * s.z + s.z * s.x)
    }

    pub fn is_empty(self) -> bool {
        self.min.x > self.max.x || self.min.y > self.max.y || self.min.z > self.max.z
    }

    /// Grow the bbox by delta in all directions.
    pub fn grow(self, delta: f64) -> BBox3d {
        let d = Vec3d::all(delta);
        BBox3d {
            min: self.min - d,
            max: self.max + d,
        }
    }

    /// Longest axis index (0=x, 1=y, 2=z).
    pub fn longest_axis(self) -> usize {
        let s = self.size();
        if s.x >= s.y && s.x >= s.z {
            0
        } else if s.y >= s.z {
            1
        } else {
            2
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vec3::dvec3;

    #[test]
    fn test_empty() {
        let b = BBox3d::empty();
        assert!(b.is_empty());
    }

    #[test]
    fn test_from_points() {
        let b = BBox3d::from_points(&[
            dvec3(1.0, 2.0, 3.0),
            dvec3(-1.0, -2.0, -3.0),
            dvec3(0.0, 0.0, 0.0),
        ]);
        assert_eq!(b.min, dvec3(-1.0, -2.0, -3.0));
        assert_eq!(b.max, dvec3(1.0, 2.0, 3.0));
    }

    #[test]
    fn test_center_size() {
        let b = BBox3d::from_min_max(dvec3(0.0, 0.0, 0.0), dvec3(10.0, 20.0, 30.0));
        assert_eq!(b.center(), dvec3(5.0, 10.0, 15.0));
        assert_eq!(b.size(), dvec3(10.0, 20.0, 30.0));
    }

    #[test]
    fn test_intersects() {
        let a = BBox3d::from_min_max(dvec3(0.0, 0.0, 0.0), dvec3(2.0, 2.0, 2.0));
        let b = BBox3d::from_min_max(dvec3(1.0, 1.0, 1.0), dvec3(3.0, 3.0, 3.0));
        let c = BBox3d::from_min_max(dvec3(5.0, 5.0, 5.0), dvec3(6.0, 6.0, 6.0));
        assert!(a.intersects(b));
        assert!(!a.intersects(c));
    }

    #[test]
    fn test_contains() {
        let b = BBox3d::from_min_max(dvec3(0.0, 0.0, 0.0), dvec3(10.0, 10.0, 10.0));
        assert!(b.contains(dvec3(5.0, 5.0, 5.0)));
        assert!(b.contains(dvec3(0.0, 0.0, 0.0)));
        assert!(!b.contains(dvec3(-1.0, 5.0, 5.0)));
    }

    #[test]
    fn test_union() {
        let a = BBox3d::from_min_max(dvec3(0.0, 0.0, 0.0), dvec3(1.0, 1.0, 1.0));
        let b = BBox3d::from_min_max(dvec3(2.0, 2.0, 2.0), dvec3(3.0, 3.0, 3.0));
        let u = a.union(b);
        assert_eq!(u.min, dvec3(0.0, 0.0, 0.0));
        assert_eq!(u.max, dvec3(3.0, 3.0, 3.0));
    }

    #[test]
    fn test_surface_area() {
        // 2x3x4 box: SA = 2*(2*3 + 3*4 + 4*2) = 2*(6+12+8) = 52
        let b = BBox3d::from_min_max(dvec3(0.0, 0.0, 0.0), dvec3(2.0, 3.0, 4.0));
        assert!((b.surface_area() - 52.0).abs() < 1e-12);
    }

    #[test]
    fn test_longest_axis() {
        let b = BBox3d::from_min_max(dvec3(0.0, 0.0, 0.0), dvec3(1.0, 5.0, 3.0));
        assert_eq!(b.longest_axis(), 1);
    }

    #[test]
    fn test_grow() {
        let b = BBox3d::from_min_max(dvec3(1.0, 1.0, 1.0), dvec3(2.0, 2.0, 2.0));
        let g = b.grow(0.5);
        assert_eq!(g.min, dvec3(0.5, 0.5, 0.5));
        assert_eq!(g.max, dvec3(2.5, 2.5, 2.5));
    }
}
