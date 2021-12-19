use crate::live_id::LiveId;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TokenWithLen {
    pub len: usize,
    pub kind: TokenKind,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TokenKind {
    Comment,
    Whitespace,
    Unknown,
    Punct(LiveId),
    Ident(LiveId),
    Keyword(LiveId),
    Branch(LiveId),
    Loop(LiveId),
    Lifetime,
    Open(Delim),
    Close(Delim),
    String,
    Number,
    Color,
}

impl TokenKind {

    pub fn is_ws_or_comment(&self)->bool{
        match self {
            TokenKind::Whitespace | TokenKind::Comment => true,
            _ => false
        }
    }

    pub fn is_open_delimiter(&self) -> bool {
        match self {
            TokenKind::Open(_) => true,
            _ => false
        }
    }

    pub fn is_close_delimiter(&self) -> bool {
        match self {
            TokenKind::Close(_) => true,
            _ => false
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Delim {
    Paren,
    Bracket,
    Brace,
}
