use crate::{BiasedPos, Pos};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Cursor {
    pub pos: BiasedPos,
    pub col: Option<usize>,
}

impl From<Pos> for Cursor {
    fn from(pos: Pos) -> Self {
        Cursor::from(BiasedPos::from(pos))
    }
}

impl From<BiasedPos> for Cursor {
    fn from(pos: BiasedPos) -> Self {
        Self { pos, col: None }
    }
}
