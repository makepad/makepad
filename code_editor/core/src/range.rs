use crate::{Len, Pos};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Range {
    pub start: Pos,
    pub end: Pos,
}

impl Range {
    pub fn is_empty(&self) -> bool {
        self.len() == Len::default()
    }

    pub fn len(&self) -> Len {
        self.end - self.start
    }
}
