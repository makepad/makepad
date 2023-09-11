use {
    crate::text::{Change, Length, Position, Range},
    std::ops,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Hash, Eq)]
pub struct Selection {
    pub anchor: Position,
    pub cursor: Position,
    pub affinity: Affinity,
    pub preferred_column: Option<usize>,
}

impl Selection {
    pub fn is_empty(self) -> bool {
        self.anchor == self.cursor
    }

    pub fn should_merge(self, other: Self) -> bool {
        if self.is_empty() || other.is_empty() {
            self.end() >= other.start()
        } else {
            self.end() > other.start()
        }
    }

    pub fn start(self) -> Position {
        self.anchor.min(self.cursor)
    }

    pub fn start_affinity(self) -> Affinity {
        if self.anchor < self.cursor {
            Affinity::After
        } else {
            self.affinity
        }
    }

    pub fn end(self) -> Position {
        self.anchor.max(self.cursor)
    }

    pub fn end_affinity(self) -> Affinity {
        if self.cursor < self.anchor {
            Affinity::Before
        } else {
            self.affinity
        }
    }

    pub fn extent(self) -> Length {
        self.end() - self.start()
    }

    pub fn range(self) -> Range {
        Range::new(self.start(), self.end()).unwrap()
    }

    pub fn line_range(self) -> ops::Range<usize> {
        if self.anchor <= self.cursor {
            self.anchor.line_index..self.cursor.line_index + 1
        } else {
            self.cursor.line_index..if self.anchor.byte_index == 0 {
                self.anchor.line_index
            } else {
                self.anchor.line_index + 1
            }
        }
    }

    pub fn reset_anchor(self) -> Self {
        Self {
            anchor: self.cursor,
            ..self
        }
    }

    pub fn update_cursor(
        self,
        f: impl FnOnce(Position, Affinity, Option<usize>) -> (Position, Affinity, Option<usize>),
    ) -> Self {
        let (cursor, affinity, preferred_column) =
            f(self.cursor, self.affinity, self.preferred_column);
        Self {
            cursor,
            affinity,
            preferred_column,
            ..self
        }
    }

    pub fn merge(self, other: Self) -> Option<Self> {
        if self.should_merge(other) {
            Some(if self.anchor <= self.cursor {
                Selection {
                    anchor: self.anchor,
                    cursor: other.cursor,
                    affinity: other.affinity,
                    preferred_column: other.preferred_column,
                }
            } else {
                Selection {
                    anchor: other.anchor,
                    cursor: self.cursor,
                    affinity: self.affinity,
                    preferred_column: self.preferred_column,
                }
            })
        } else {
            None
        }
    }

    pub fn apply_change(self, change: &Change) -> Selection {
        Self {
            anchor: self.anchor.apply_change(change),
            cursor: self.cursor.apply_change(change),
            ..self
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Affinity {
    Before,
    After,
}

impl Default for Affinity {
    fn default() -> Self {
        Self::Before
    }
}
