use crate::{Extent, Point, Range};

#[derive(Clone, Copy, Debug, Default, PartialEq, Hash, Eq)]
pub struct Selection {
    pub anchor: Point,
    pub cursor: Point,
    pub affinity: Affinity,
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

    pub fn extent(self) -> Extent {
        self.end() - self.start()
    }

    pub fn range(self) -> Range {
        Range::new(self.start(), self.end()).unwrap()
    }

    pub fn merge(self, other: Self) -> Option<Self> {
        if self.should_merge(other) {
            Some(if self.anchor <= self.cursor {
                Selection {
                    anchor: self.anchor,
                    cursor: other.cursor,
                    affinity: other.affinity,
                }
            } else {
                Selection {
                    anchor: other.anchor,
                    cursor: self.cursor,
                    affinity: self.affinity,
                }
            })
        } else {
            None
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
