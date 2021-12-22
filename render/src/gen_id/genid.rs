#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct GenId {
    pub index: usize,
    pub generation: usize,
}
