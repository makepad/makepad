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
