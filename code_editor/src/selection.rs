use crate::{Length, Position, Range};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Selection {
    pub anchor: Position,
    pub cursor: Position,
}

impl Selection {
    pub fn new(cursor: Position) -> Self {
        Self {
            anchor: cursor,
            cursor,
        }
    }

    pub fn is_empty(self) -> bool {
        self.anchor == self.cursor
    }

    pub fn should_merge_with(mut self, mut other: Self) -> bool {
        use std::mem;

        if self.start() > other.start() {
            mem::swap(&mut self, &mut other);
        }
        if self.is_empty() || other.is_empty() {
            self.end() >= other.start()
        } else {
            self.end() > other.start()
        }
    }

    pub fn length(self) -> Length {
        self.end() - self.start()
    }

    pub fn start(self) -> Position {
        self.anchor.min(self.cursor)
    }

    pub fn end(self) -> Position {
        self.anchor.max(self.cursor)
    }

    pub fn range(self) -> Range {
        Range::new(self.start(), self.end())
    }
}
