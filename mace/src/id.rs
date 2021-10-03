#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Id {
    pub index: usize,
    pub generation: usize,
}
