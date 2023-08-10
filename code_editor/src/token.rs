#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Token {
    pub len: usize,
    pub kind: TokenKind,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TokenKind {
    Unknown,
    BranchKeyword,
    Identifier,
    LoopKeyword,
    OtherKeyword,
    Number,
    Punctuator,
    Typename,
    String,
    Whitespace,
}
