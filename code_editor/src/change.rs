use crate::{Point, Range, Text};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Change {
    pub drift: Drift,
    pub kind: ChangeKind,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Drift {
    Before,
    After,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum ChangeKind {
    Insert(Point, Text),
    Delete(Range),
}
