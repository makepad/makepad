#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Token {
    pub len: usize,
    pub kind: TokenKind,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TokenKind {
    Comment,
    OpenDelimiter(Delimiter),
    CloseDelimiter(Delimiter),
    Identifier,
    Keyword(Keyword),
    Number,
    Punctuator(Punctuator),
    String,
    Whitespace,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Delimiter {
    Paren,
    Bracket,
    Brace,
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
