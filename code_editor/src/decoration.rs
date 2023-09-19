use crate::text::{Edit, Length, Position};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Decoration {
    pub id: usize,
    start: Position,
    end: Position,
}

impl Decoration {
    pub fn new(id: usize, start: Position, end: Position) -> Self {
        assert!(start <= end);
        Self {
            id,
            start,
            end,
        }
    }

    pub fn is_empty(self) -> bool {
        self.start == self.end
    }

    pub fn length(self) -> Length {
        self.end - self.start
    }

    pub fn start(self) -> Position {
        self.start
    }

    pub fn end(self) -> Position {
        self.end
    }

    pub fn apply_edit(self, edit: &Edit) -> Self {
        Self {
            start: self.start.apply_edit(edit),
            end: self.end.apply_edit(edit),
            ..self
        }
    }
}
