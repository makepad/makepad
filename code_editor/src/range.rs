use crate::{Extent, Point};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Range {
    start: Point,
    end: Point,
}

impl Range {
    pub fn new(start: Point, end: Point) -> Option<Self> {
        if start > end {
            return None;
        }
        Some(Self { start, end })
    }

    pub fn from_start_and_extent(start: Point, extent: Extent) -> Self {
        Self {
            start,
            end: start + extent,
        }
    }

    pub fn is_empty(self) -> bool {
        self.start == self.end
    }

    pub fn start(self) -> Point {
        self.start
    }

    pub fn end(self) -> Point {
        self.end
    }

    pub fn extent(self) -> Extent {
        self.end - self.start
    }
}
