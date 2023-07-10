#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Token<'a> {
    pub text: &'a str,
    pub kind: TokenKind,
}

impl<'a> Token<'a> {
    pub fn new(text: &'a str, kind: TokenKind) -> Self {
        Self { text, kind }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TokenInfo {
    pub byte_count: usize,
    pub kind: TokenKind,
}

impl TokenInfo {
    pub fn new(len: usize, kind: TokenKind) -> Self {
        Self {
            byte_count: len,
            kind,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TokenKind {
    Unknown,
    Identifier,
    Keyword,
    Number,
    Punctuator,
    Whitespace,
}
