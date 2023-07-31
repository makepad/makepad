use crate::{Affinity, Length, Point};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Selection {
    pub anchor: (Point, Affinity),
    pub cursor: (Point, Affinity),
    pub preferred_column: Option<usize>,
}

impl Selection {
    pub fn new(
        anchor: (Point, Affinity),
        cursor: (Point, Affinity),
        preferred_column: Option<usize>,
    ) -> Self {
        Self {
            anchor,
            cursor,
            preferred_column,
        }
    }

    pub fn from_cursor(cursor: (Point, Affinity)) -> Self {
        Self {
            anchor: cursor,
            cursor,
            preferred_column: None,
        }
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

    pub fn length(&self) -> Length {
        self.end().0 - self.start().0
    }

    pub fn start(self) -> (Point, Affinity) {
        self.anchor.min(self.cursor)
    }

    pub fn end(self) -> (Point, Affinity) {
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
        f: impl FnOnce((Point, Affinity), Option<usize>) -> ((Point, Affinity), Option<usize>),
    ) -> Self {
        let (cursor, column) = f(self.cursor, self.preferred_column);
        Self {
            cursor,
            preferred_column: column,
            ..self
        }
    }
}
