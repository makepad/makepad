use crate::{Length, Point};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Range {
    start: Point,
    end: Point,
}

impl Range {
    pub fn new(start: Point, end: Point) -> Self {
        assert!(start <= end);
        Self { start, end }
    }

    pub fn is_empty(self) -> bool {
        self.start == self.end
    }

    pub fn length(self) -> Length {
        self.end - self.start
    }

    pub fn contains(&self, position: Point) -> bool {
        self.start <= position && position <= self.end
    }

    pub fn start(self) -> Point {
        self.start
    }

    pub fn end(self) -> Point {
        self.end
    }
}
