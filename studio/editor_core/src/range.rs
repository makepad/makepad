use {
    crate::makepad_micro_serde::*,
    crate::{
        position::Position
    }
};

/// A type for representing a range in a text.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq, SerBin, DeBin)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}
