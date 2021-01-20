use crate::position::Position;

#[derive(Clone, Copy, Debug)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}
