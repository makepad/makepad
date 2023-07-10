use crate::{Length, Position};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Range {
    start: Position,
    end: Position,
}

impl Range {
    pub fn new(start: Position, end: Position) -> Self {
        assert!(start <= end);
        Self { start, end }
    }

    pub fn is_empty(self) -> bool {
        self.start == self.end
    }

    pub fn length(self) -> Length {
        self.end - self.start
    }

    pub fn contains(&self, position: Position) -> bool {
        self.start <= position && position <= self.end
    }

    pub fn start(self) -> Position {
        self.start
    }

    pub fn end(self) -> Position {
        self.end
    }
}
