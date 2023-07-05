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

    pub fn try_merge(mut self, mut other: Self) -> Option<Range> {
        use std::mem;

        if self.start() > other.start() {
            mem::swap(&mut self, &mut other);
        }
        if self.end() < other.start() {
            return None;
        }
        if self.end() == other.end() && !(self.is_empty() || other.is_empty()) {
            return None;
        }
        Some(Range::new(
            self.start.min(other.start),
            self.end.max(other.end),
        ))
    }
}
