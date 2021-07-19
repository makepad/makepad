#[derive(Clone, Copy, Debug)]
pub struct Token {
    pub len: usize,
    pub kind: Kind,
}

#[derive(Clone, Copy, Debug)]
pub enum Kind {
    Comment,
    Delimiter(Delimiter),
    Identifier,
    Keyword,
    Number,
    Punctuator,
    String,
    Whitespace,
    Unknown,
}

#[derive(Clone, Copy, Debug)]
pub enum Delimiter {
    LeftParen,
    RightParen,
}