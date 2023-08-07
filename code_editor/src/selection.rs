use crate::Point;

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Selection {
    pub anchor: Point,
    pub cursor: Point,
    pub affinity: Affinity,
}

impl Selection {
    pub fn is_empty(self) -> bool {
        self.anchor == self.cursor
    }

    pub fn start(self) -> Point {
        self.anchor.min(self.cursor)
    }

    pub fn start_affinity(self) -> Affinity {
        if self.anchor < self.cursor {
            Affinity::After
        } else {
            self.affinity
        }
    }

    pub fn end(self) -> Point {
        self.anchor.max(self.cursor)
    }

    pub fn end_affinity(self) -> Affinity {
        if self.cursor < self.anchor {
            Affinity::Before
        } else {
            self.affinity
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
