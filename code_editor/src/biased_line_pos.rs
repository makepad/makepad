use crate::Bias;

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BiasedLinePos {
    pub pos: usize,
    pub bias: Bias,
}

impl From<usize> for BiasedLinePos {
    fn from(pos: usize) -> Self {
        Self {
            pos,
            ..Self::default()
        }
    }
}
