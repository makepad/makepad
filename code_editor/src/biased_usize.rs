use crate::Bias;

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BiasedUsize {
    pub value: usize,
    pub bias: Bias,
}

impl From<usize> for BiasedUsize {
    fn from(value: usize) -> Self {
        Self {
            value,
            ..Self::default()
        }
    }
}
