use crate::{F32Ext, Point, Transform, Transformation};
use std::cmp::Ordering;

/// A quadratic bezier curve segment in 2-dimensional Euclidian space.
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct LineSegment {
    pub p0: Point,
    pub p1: Point,
}

impl LineSegment {
    /// Creates a new line segment with the given endpoints.
    pub fn new(p0: Point, p1: Point) -> LineSegment {
        LineSegment { p0, p1 }
    }

    /// Compares `self` to the point `p`.
    ///
    /// Returns `Ordering::Less` if `self` lies below `p`, `Ordering::Greater` if `self` lies
    /// above `p`, and `Ordering::Equal` if `self` is incident to `p`.
    pub fn compare_to_point(self, p: Point) -> Option<Ordering> {
        // Compute the signed area of the triangle with vertices `p`, `p0`, and `p1`.
        (p - self.p0).cross(self.p1 - p).partial_cmp(&0.0)
    }

    /// Returns the intersection point of the supporting line of `self` with the vertical line
    /// through `x`, or None if these lines are coincident.
    pub fn intersect_with_vertical_line(self, x: f32) -> Option<Point> {
        let dx = self.p1.x - self.p0.x;
        if dx == 0.0 {
            return None;
        }
        let dx1 = x - self.p0.x;
        let dx2 = self.p1.x - x;
        Some(Point {
            x,
            y: if dx1 <= dx2 {
                self.p0.y.ext_lerp(self.p1.y, dx1 / dx)
            } else {
                self.p1.y.ext_lerp(self.p0.y, dx2 / dx)
            },
        })
    }
}

impl Transform for LineSegment {
    fn transform<T>(self, t: &T) -> LineSegment
    where
        T: Transformation,
    {
        LineSegment::new(self.p0.transform(t), self.p1.transform(t))
    }

    fn transform_mut<T>(&mut self, t: &T)
    where
        T: Transformation,
    {
        *self = self.transform(t);
    }
}
