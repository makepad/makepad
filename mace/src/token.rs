#[derive(Clone, Copy, Debug)]
pub struct Token {
    pub len: usize,
    pub kind: Kind,
}

#[derive(Clone, Copy, Debug)]
pub enum Kind {
    Comment,
    Identifier,
    Keyword(Keyword),
    Number,
    Punctuator(Punctuator),
    String,
    Whitespace,
    Unknown,
}

#[derive(Clone, Copy, Debug)]
pub enum Keyword {
    Other,
}

#[derive(Clone, Copy, Debug)]
pub enum Punctuator {
    LeftParen,
    RightParen,
    Other
}