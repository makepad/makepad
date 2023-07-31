use crate::{BiasedTextPos, Cursor, TextLen, TextPos};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Selection {
    pub anchor: BiasedTextPos,
    pub cursor: Cursor,
}

impl Selection {
    pub fn is_empty(self) -> bool {
        self.anchor == self.cursor.pos
    }

    pub fn len(&self) -> TextLen {
        self.end().pos - self.start().pos
    }

    pub fn start(self) -> BiasedTextPos {
        self.anchor.min(self.cursor.pos)
    }

    pub fn end(self) -> BiasedTextPos {
        self.anchor.max(self.cursor.pos)
    }

    pub fn reset_anchor(self) -> Self {
        Self {
            anchor: self.cursor.pos,
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
        Some(if self.anchor <= self.cursor.pos {
            Selection {
                anchor: self.anchor,
                cursor: other.cursor
            }
        } else {
            Selection {
                anchor: other.anchor,
                cursor: self.cursor,
            }
        })
    }

}

impl From<TextPos> for Selection {
    fn from(pos: TextPos) -> Self {
        Selection::from(BiasedTextPos::from(pos))
    }
}

impl From<BiasedTextPos> for Selection {
    fn from(pos: BiasedTextPos) -> Self {
        Selection::from(Cursor::from(pos))
    }
}

impl From<Cursor> for Selection {
    fn from(cursor: Cursor) -> Self {
        Self {
            anchor: cursor.pos,
            cursor,
        }
    }
}
