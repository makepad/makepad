use crate::live_id::LiveId;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TokenWithLen {
    pub len: usize,
    pub token: FullToken,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FullToken {
    Punct(LiveId),
    Ident(LiveId),
    
    Open(Delim),
    Close(Delim),

    String,
    Bool(bool),
    Color(u32),

    Number,

    Lifetime,
    Comment,
    Whitespace,
    Unknown,
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
