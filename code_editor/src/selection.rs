use crate::{Affinity, Position};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Selection {
    pub anchor: (Position, Affinity),
    pub cursor: (Position, Affinity),
}

impl Selection {
    pub fn new(anchor: (Position, Affinity), cursor: (Position, Affinity)) -> Self {
        Self { anchor, cursor }
    }

    pub fn from_cursor(cursor: (Position, Affinity)) -> Self {
        Self {
            anchor: cursor,
            cursor,
        }
    }

    pub fn is_empty(self) -> bool {
        self.anchor == self.cursor
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
        f: impl FnOnce((Position, Affinity)) -> (Position, Affinity),
    ) -> Self {
        let cursor = f(self.cursor);
        Self { cursor, ..self }
    }
}
