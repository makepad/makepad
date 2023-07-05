use crate::{Point, Size};

#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub struct Rect {
    pub origin: Point,
    pub size: Size,
}

impl Rect {
    pub fn new(origin: Point, size: Size) -> Self {
        Self { origin, size }
    }

    pub fn contains(self, point: Point) -> bool {
        if point.x < self.origin.x || point.x > self.origin.x + self.size.width {
            return false;
        }
        if point.y < self.origin.y || point.y > self.origin.y + self.size.height {
            return false;
        }
        true
    }
}
