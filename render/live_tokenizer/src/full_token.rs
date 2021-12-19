use crate::live_id::LiveId;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TokenWithLen {
    pub len: usize,
    pub token: FullToken,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FullToken {
    Comment,
    Whitespace,
    Unknown,
    Punct(LiveId),
    Ident(LiveId),
    Lifetime,
    Open(Delim),
    Close(Delim),
    Bool(bool),
    String,
    Number,
    Color,
}

impl FullToken {

    pub fn is_ws_or_comment(&self)->bool{
        match self {
            FullToken::Whitespace | FullToken::Comment => true,
            _ => false
        }
    }

    pub fn is_open_delimiter(&self) -> bool {
        match self {
            FullToken::Open(_) => true,
            _ => false
        }
    }

    pub fn is_close_delimiter(&self) -> bool {
        match self {
            FullToken::Close(_) => true,
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
