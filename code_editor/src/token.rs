#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Token {
    pub len: usize,
    pub kind: TokenKind,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TokenKind {
    Unknown,
    BranchKeyword,
    Comment,
    Constant,
    Delimiter,
    Identifier,
    LoopKeyword,
    OtherKeyword,
    Number,
    Punctuator,
    Typename,
    Function,
    String,
    Whitespace,
}
