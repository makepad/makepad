#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Token {
    pub kind: Kind,
    pub len: usize,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Kind {
    Unknown,
}
