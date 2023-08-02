use crate::{BiasedTextPos, TextPos};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Cursor {
    pub pos: BiasedTextPos,
    pub col: Option<usize>,
}

impl From<TextPos> for Cursor {
    fn from(pos: TextPos) -> Self {
        Cursor::from(BiasedTextPos::from(pos))
    }
}

impl From<BiasedTextPos> for Cursor {
    fn from(pos: BiasedTextPos) -> Self {
        Self { pos, col: None }
    }
}
