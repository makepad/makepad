use crate::text::{Length, Position};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Decoration {
    pub id: usize,
    pub start: Position,
    pub length: Length,
}

impl Decoration {
    pub fn is_empty(self) -> bool {
        self.length == Length::zero()
    }

    pub fn end(self) -> Position {
        self.start + self.length
    }
}
