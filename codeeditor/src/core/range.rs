use crate::core::Position;

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}
