use crate::{
    code_editor::{
        position::Position, range::Range
    }
};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Cursor {
    pub head: Position,
    pub tail: Position,
    pub max_column: usize,
}

impl Cursor {
    pub fn range(self) -> Range {
        Range {
            start: self.head.min(self.tail),
            end: self.head.max(self.tail),
        }
    }
}
