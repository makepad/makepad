#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Bias {
    Before,
    After,
}

impl Default for Bias {
    fn default() -> Self {
        Bias::Before
    }
}
