use crate::{Cursor, BiasedPos, Pos, Len};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Selection {
    pub anchor_pos: BiasedPos,
    pub cursor: Cursor,
}

impl Selection {
    pub fn is_empty(self) -> bool {
        self.anchor_pos == self.cursor.pos
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

    pub fn length(&self) -> Len {
        self.end().to_pos() - self.start().to_pos()
    }

    pub fn start(self) -> BiasedPos {
        self.anchor_pos.min(self.cursor.pos)
    }

    pub fn end(self) -> BiasedPos {
        self.anchor_pos.max(self.cursor.pos)
    }

    pub fn reset_anchor(self) -> Self {
        Self {
            anchor_pos: self.cursor.pos,
            ..self
        }
    }

    pub fn update_cursor(
        self,
        f: impl FnOnce(Cursor) -> Cursor,
    ) -> Self {
        Self {
            cursor: f(self.cursor),
            ..self
        }
    }
}

impl From<Pos> for Selection {
    fn from(pos: Pos) -> Self {
        Selection::from(BiasedPos::from(pos))
    }
}

impl From<BiasedPos> for Selection {
    fn from(pos: BiasedPos) -> Self {
        Selection::from(Cursor::from(pos))
    }
}

impl From<Cursor> for Selection {
    fn from(cursor: Cursor) -> Self {
        Self {
            anchor_pos: cursor.pos,
            cursor,
        }
    }
}