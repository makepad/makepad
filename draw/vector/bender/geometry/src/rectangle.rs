use crate::Point;

#[derive(Clone, Copy, Debug)]
pub struct Rectangle {
    start: Point,
    end: Point,
}

impl Rectangle {
    pub fn new(start: Point, end: Point) -> Self {
        Self { start, end }
    }

    pub fn start(self) -> Point {
        self.start
    }

    pub fn end(self) -> Point {
        self.end
    }

    pub fn clamp(self, point: Point) -> Point {
        Point::new(
            point.x().max(self.start.x()).min(self.end.x()),
            point.y().max(self.start.y()).min(self.end.y()),
        )
    }

    pub fn intersect(self, other: Self) -> Self {
        Self {
            start: Point::new(
                self.start.x().max(other.start.x()),
                self.start.y().max(other.start.y()),
            ),
            end: Point::new(
                self.end.x().min(other.end.x()),
                self.end.y().min(other.end.y()),
            ),
        }
    }
}
