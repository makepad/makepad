use crate::{Len, Pos};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Range {
    start: Pos,
    end: Pos,
}

impl Range {
    pub fn new(start: Pos, end: Pos) -> Self {
        assert!(start <= end);
        Self { start, end }
    }

    pub fn is_empty(self) -> bool {
        self.start == self.end
    }

    pub fn contains(&self, position: Pos) -> bool {
        self.start <= position && position <= self.end
    }

    pub fn length(self) -> Len {
        self.end - self.start
    }

    pub fn start(self) -> Pos {
        self.start
    }

    pub fn end(self) -> Pos {
        self.end
    }
}
