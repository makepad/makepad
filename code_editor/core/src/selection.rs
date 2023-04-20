use crate::{DeltaLen, Position, Size};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Selection {
    pub cursor: Position,
    pub anchor: Position,
}

impl Selection {
    pub fn is_empty(self) -> bool {
        self.cursor == self.anchor
    }

    pub fn len(self) -> Size {
        self.end() - self.start()
    }

    pub fn start(self) -> Position {
        self.cursor.min(self.anchor)
    }

    pub fn end(self) -> Position {
        self.cursor.max(self.anchor)
    }

    pub fn apply_delta(self, delta_len: DeltaLen) -> Self {
        Self {
            cursor: self.cursor.apply_delta(delta_len),
            anchor: self.anchor.apply_delta(delta_len),
        }
    }

    pub fn merge(mut self, mut other: Self) -> Option<Self> {
        use std::{cmp::Ordering, mem};

        if self.start() > other.start() {
            mem::swap(&mut self, &mut other);
        }
        match (self.is_empty(), other.is_empty()) {
            (true, true) if self.cursor == other.cursor => Some(self),
            (false, true) if other.cursor <= self.end() => Some(self),
            (true, false) if self.cursor == other.start() => Some(other),
            (false, false) if self.end() > other.start() => {
                Some(match self.cursor.cmp(&self.anchor) {
                    Ordering::Less => Self {
                        cursor: self.cursor.min(other.cursor),
                        anchor: self.anchor.max(other.anchor),
                    },
                    Ordering::Greater => Self {
                        cursor: self.cursor.max(other.cursor),
                        anchor: self.anchor.min(other.anchor),
                    },
                    Ordering::Equal => unreachable!(),
                })
            }
            _ => None,
        }
    }
}
