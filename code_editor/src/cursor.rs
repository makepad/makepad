use crate::{BiasedPos, Pos};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Cursor {
    pub biased_pos: BiasedPos,
    pub column: Option<usize>,
}

impl From<Pos> for Cursor {
    fn from(pos: Pos) -> Self {
        Cursor::from(BiasedPos::from(pos))
    }
}

impl From<BiasedPos> for Cursor {
    fn from(biased_pos: BiasedPos) -> Self {
        Self {
            biased_pos,
            column: None,
        }
    }
}
