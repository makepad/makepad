#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Token {
    pub len: usize,
    pub kind: TokenKind,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TokenKind {
    Comment,
    Identifier,
    Keyword(Keyword),
    Number,
    Punctuator(Punctuator),
    String,
    Whitespace,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Keyword {
    Branch,
    Loop,
    Other,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Punctuator {
    LeftParen,
    RightParen,
    LeftBrace,
    RightBrace,
    Other,
}

impl Punctuator {
    pub fn is_open_delimiter(self) -> bool {
        match self {
            Punctuator::LeftParen => true,
            Punctuator::LeftBrace => true,
            _ => false,
        }
    }

    pub fn is_close_delimiter(self) -> bool {
        match self {
            Punctuator::RightParen => true,
            Punctuator::RightBrace => true,
            _ => false,
        }
    }
}
