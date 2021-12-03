use crate::{
    code_editor::{
        position::Position
    }
};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}
