#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Token {
    pub byte_count: usize,
    pub kind: TokenKind,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TokenKind {
    Unknown,
}
