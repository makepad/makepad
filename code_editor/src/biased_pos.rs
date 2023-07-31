use crate::{Bias, Pos};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BiasedPos {
    pub line: usize,
    pub byte: usize,
    pub bias: Bias,
}

impl BiasedPos {
    pub fn from_pos_and_bias(pos: Pos, bias: Bias) -> Self {
        Self {
            line: pos.line,
            byte: pos.byte,
            bias,
        }
    }

    pub fn to_pos(self) -> Pos {
        Pos {
            line: self.line,
            byte: self.byte,
        }
    }

    pub fn update_pos(self, f: impl FnOnce(Pos) -> Pos) -> Self {
        Self::from_pos_and_bias(f(self.to_pos()), self.bias)
    }
}

impl From<Pos> for BiasedPos {
    fn from(pos: Pos) -> Self {
        Self {
            line: pos.line,
            byte: pos.byte,
            bias: Bias::default(),
        }
    }
}