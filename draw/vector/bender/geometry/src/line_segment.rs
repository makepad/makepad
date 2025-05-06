use crate::{Point, Rectangle, Vector};
use std::cmp::Ordering;

#[derive(Clone, Copy, Debug)]
pub struct LineSegment {
    start: Point,
    end: Point,
}

impl LineSegment {
    pub fn new(start: Point, end: Point) -> Self {
        Self { start, end }
    }

    pub fn start(self) -> Point {
        self.start
    }

    pub fn end(self) -> Point {
        self.end
    }

    pub fn compare_to_point(self, point: Point) -> Option<Ordering> {
        (point - self.start)
            .cross(self.end - point)
            .partial_cmp(&0.0)
    }

    pub fn intersect(self, other: Self) -> Option<Point> {
        let a = self.end - self.start;
        let b = other.start - other.end;
        let c = self.start - other.start;
        let denom = a.cross(b);
        if denom == 0.0 {
            return None;
        }
        let numer_0 = b.cross(c);
        let numer_1 = c.cross(a);
        if denom < 0.0 {
            if (numer_0 < denom || 0.0 < numer_0) || (numer_1 < denom || 0.0 < numer_1) {
                return None;
            }
        } else {
            if (numer_0 < 0.0 || denom < numer_0) || (numer_1 < 0.0 || denom < numer_1) {
                return None;
            }
        }
        Some(self.start.lerp(self.end, numer_0 / denom))
    }

    pub fn normal(self) -> Vector {
        self.tangent().perpendicular()
    }

    pub fn tangent(self) -> Vector {
        self.end - self.start
    }

    pub fn bounds(self) -> Rectangle {
        Rectangle::new(
            Point::new(
                self.start.x().min(self.end.x()),
                self.start.y().min(self.end.y()),
            ),
            Point::new(
                self.start.x().max(self.end.x()),
                self.start.y().max(self.end.y()),
            ),
        )
    }

    pub fn reverse(self) -> Self {
        Self {
            end: self.start,
            start: self.end,
        }
    }
}
