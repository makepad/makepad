use crate::geometry::{Point, Transform, Transformation};
use crate::internal_iter::InternalIterator;

/// A cubic bezier curve segment in 2-dimensional Euclidian space.
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct CubicSegment {
    pub p0: Point,
    pub p1: Point,
    pub p2: Point,
    pub p3: Point
}

impl CubicSegment {
    /// Creates a new cubic bezier curve segment with the given control points.
    pub fn new(p0: Point, p1: Point, p2: Point, p3: Point) -> CubicSegment {
        CubicSegment { p0, p1, p2, p3 }
    }

    /// Returns true if `self` is approximately linear with tolerance `epsilon`.
    pub fn is_approximately_linear(self, epsilon: f64) -> bool {
        let v1 = self.p1 - self.p0;
        let v2 = self.p2 - self.p0;
        if let Some(vx) = (self.p3 - self.p0).normalize() {
            // If the baseline is a line segment, the segment is approximately linear if the
            // rejection of both control points from the baseline is less than `epsilon`.
            v1.cross(vx).abs() < epsilon && v2.cross(vx).abs() < epsilon
        } else {
            // If the baseline is a single point, the segment is approximately linear if the
            // distance of both control points from the baseline is less than `epsilon`.
            v1.length() < epsilon && v2.length() < epsilon
        }
    }

    /// Splits `self` into two quadratic Bezier curve segments, at parameter `t`.
    pub fn split(self, t: f64) -> (CubicSegment, CubicSegment) {
        let p01 = self.p0.lerp(self.p1, t);
        let p12 = self.p1.lerp(self.p2, t);
        let p23 = self.p2.lerp(self.p3, t);
        let p012 = p01.lerp(p12, t);
        let p123 = p12.lerp(p23, t);
        let p0123 = p012.lerp(p123, t);
        (
            CubicSegment::new(self.p0, p01, p012, p0123),
            CubicSegment::new(p0123, p123, p23, self.p2),
        )
    }

    /// Returns an iterator over the points of a polyline that approximates `self` with tolerance
    /// `epsilon`, *excluding* the first point.
    pub fn linearize(self, epsilon: f64) -> Linearize {
        Linearize {
            segment: self,
            epsilon,
        }
    }
}

impl Transform for CubicSegment {
    fn transform<T>(self, t: &T) -> CubicSegment
    where
        T: Transformation,
    {
        CubicSegment::new(
            self.p0.transform(t),
            self.p1.transform(t),
            self.p2.transform(t),
            self.p3.transform(t),
        )
    }

    fn transform_mut<T>(&mut self, t: &T)
    where
        T: Transformation,
    {
        *self = self.transform(t);
    }
}

/// An iterator over the points of a polyline that approximates `self` with tolerance `epsilon`,
/// *excluding* the first point.
#[derive(Clone, Copy)]
pub struct Linearize {
    segment: CubicSegment,
    epsilon: f64,
}

impl InternalIterator for Linearize {
    type Item = Point;

    fn for_each<F>(self, f: &mut F) -> bool
    where
        F: FnMut(Point) -> bool,
    {
        if self.segment.is_approximately_linear(self.epsilon) {
            return f(self.segment.p2);
        }
        let (segment_0, segment_1) = self.segment.split(0.5);
        if !segment_0.linearize(self.epsilon).for_each(f) {
            return false;
        }
        segment_1.linearize(self.epsilon).for_each(f)
    }
}
