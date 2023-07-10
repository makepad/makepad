use crate::{Affinity, Length, Position};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Selection {
    pub anchor: (Position, Affinity),
    pub cursor: (Position, Affinity),
    pub preferred_column: Option<usize>,
}

impl Selection {
    pub fn new(
        anchor: (Position, Affinity),
        cursor: (Position, Affinity),
        preferred_column: Option<usize>,
    ) -> Self {
        Self {
            anchor,
            cursor,
            preferred_column,
        }
    }

    pub fn from_cursor(cursor: (Position, Affinity)) -> Self {
        Self {
            anchor: cursor,
            cursor,
            preferred_column: None,
        }
    }

    pub fn is_empty(self) -> bool {
        self.anchor == self.cursor
    }

    pub fn length(&self) -> Length {
        self.end().0 - self.start().0
    }

    pub fn start(self) -> (Position, Affinity) {
        self.anchor.min(self.cursor)
    }

    pub fn end(self) -> (Position, Affinity) {
        self.anchor.max(self.cursor)
    }

    pub fn reset_anchor(self) -> Self {
        Self {
            anchor: self.cursor,
            ..self
        }
    }

    pub fn update_cursor(
        self,
        f: impl FnOnce((Position, Affinity), Option<usize>) -> ((Position, Affinity), Option<usize>),
    ) -> Self {
        let (cursor, column) = f(self.cursor, self.preferred_column);
        Self {
            cursor,
            preferred_column: column,
            ..self
        }
    }
}
