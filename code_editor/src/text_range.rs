use crate::{TextLen, TextPos};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct TextRange {
    start: TextPos,
    end: TextPos,
}

impl TextRange {
    pub fn new(start: TextPos, end: TextPos) -> Self {
        assert!(start <= end);
        Self { start, end }
    }

    pub fn is_empty(self) -> bool {
        self.start == self.end
    }

    pub fn contains(&self, position: TextPos) -> bool {
        self.start <= position && position <= self.end
    }

    pub fn length(self) -> TextLen {
        self.end - self.start
    }

    pub fn start(self) -> TextPos {
        self.start
    }

    pub fn end(self) -> TextPos {
        self.end
    }
}
