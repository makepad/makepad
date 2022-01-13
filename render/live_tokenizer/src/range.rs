use {
    makepad_micro_serde::*,
    crate::{
        position::Position
    }
};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq, SerBin, DeBin)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}
