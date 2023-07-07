use crate::{Length, Position, Range};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Selection {
    pub anchor: Position,
    pub cursor: Position,
    pub column_index: Option<usize>,
}

impl Selection {
    pub fn new(anchor: Position, cursor: Position, column_index: Option<usize>) -> Self {
        Self {
            anchor,
            cursor,
            column_index,
        }
    }

    pub fn from_cursor(cursor: Position) -> Self {
        Self::new(cursor, cursor, None)
    }

    pub fn is_empty(self) -> bool {
        self.anchor == self.cursor
    }

    pub fn should_merge(mut self, mut other: Self) -> bool {
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

    pub fn reset_anchor(self) -> Self {
        Self {
            anchor: self.cursor,
            ..self
        }
    }

    pub fn update_cursor(
        self,
        f: impl FnOnce(Position, Option<usize>) -> (Position, Option<usize>),
    ) -> Self {
        let (cursor, column_index) = f(self.cursor, self.column_index);
        Self {
            cursor,
            column_index,
            ..self
        }
    }
}
