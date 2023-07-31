use crate::{BiasedPos, Cursor, Len, Pos};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Sel {
    pub anchor: BiasedPos,
    pub cursor: Cursor,
}

impl Sel {
    pub fn is_empty(self) -> bool {
        self.anchor == self.cursor.biased_pos
    }

    pub fn len(&self) -> Len {
        self.end().pos - self.start().pos
    }

    pub fn start(self) -> BiasedPos {
        self.anchor.min(self.cursor.biased_pos)
    }

    pub fn end(self) -> BiasedPos {
        self.anchor.max(self.cursor.biased_pos)
    }

    pub fn reset_anchor(self) -> Self {
        Self {
            anchor: self.cursor.biased_pos,
            ..self
        }
    }

    pub fn update_cursor(self, f: impl FnOnce(Cursor) -> Cursor) -> Self {
        Self {
            cursor: f(self.cursor),
            ..self
        }
    }

    pub fn try_merge(mut self, mut other: Self) -> Option<Self> {
        use std::mem;

        if self.start() > other.start() {
            mem::swap(&mut self, &mut other);
        }
        let should_merge = if self.is_empty() || other.is_empty() {
            self.end() >= other.start()
        } else {
            self.end() > other.start()
        };
        if !should_merge {
            return None;
        }
        Some(if self.anchor <= self.cursor.biased_pos {
            Sel {
                anchor: self.anchor,
                cursor: other.cursor,
            }
        } else {
            Sel {
                anchor: other.anchor,
                cursor: self.cursor,
            }
        })
    }
}

impl From<Pos> for Sel {
    fn from(pos: Pos) -> Self {
        Sel::from(BiasedPos::from(pos))
    }
}

impl From<BiasedPos> for Sel {
    fn from(pos: BiasedPos) -> Self {
        Sel::from(Cursor::from(pos))
    }
}

impl From<Cursor> for Sel {
    fn from(cursor: Cursor) -> Self {
        Self {
            anchor: cursor.biased_pos,
            cursor,
        }
    }
}
