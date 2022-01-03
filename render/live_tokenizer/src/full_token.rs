use {
    std::{
        ops::Deref,
        ops::DerefMut,
    },
    crate::live_id::LiveId
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TokenWithLen {
    pub len: usize,
    pub token: FullToken,
}

impl Deref for TokenWithLen {
    type Target = FullToken;
    fn deref(&self) -> &Self::Target {&self.token}
}

impl DerefMut for TokenWithLen {
    fn deref_mut(&mut self) -> &mut Self::Target {&mut self.token}
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FullToken {
    Punct(LiveId),
    Ident(LiveId),
    
    Open(Delim),
    Close(Delim),

    String,
    Bool(bool),
    Color(u32),
    Float(f64),
    Int(i64),
    
    OtherNumber,
    Lifetime,
    Comment,
    Whitespace,
    Unknown,
}

impl FullToken {
    pub fn is_whitespace(&self)-> bool {
        match self {
            FullToken::Whitespace => true,
            _ => false
        }
    }

    pub fn is_comment(&self) -> bool {
        match self {
            FullToken::Comment => true,
            _ => false
        }
    }
    
    pub fn is_ws_or_comment(&self)->bool{
        match self {
            FullToken::Whitespace | FullToken::Comment => true,
            _ => false
        }
    }
        
    pub fn is_open(&self) -> bool {
        match self {
            FullToken::Open(_) => true,
            _ => false
        }
    }
    
    pub fn is_close(&self) -> bool {
        match self {
            FullToken::Close(_) => true,
            _ => false
        }
    }
    
    pub fn is_open_delim(&self, delim: Delim) -> bool {
        match self {
            FullToken::Open(d) => *d == delim,
            _ => false
        }
    }
    
    pub fn is_close_delim(&self, delim: Delim) -> bool {
        match self {
            FullToken::Close(d) => *d == delim,
            _ => false
        }
    }
    
    pub fn is_int(&self) -> bool {
        match self {
            FullToken::Int(_) => true,
            _ => false
        }
    }
    
    pub fn is_float(&self) -> bool {
        match self {
            FullToken::Float(_) => true,
            _ => false
        }
    }
    
        
    pub fn is_color(&self) -> bool {
        match self {
            FullToken::Color(_) => true,
            _ => false
        }
    }

    pub fn is_parsed_number(&self) -> bool {
        match self {
            FullToken::Int(_) => true,
            FullToken::Float(_) => true,
            _ => false
        }
    }


    pub fn is_bool(&self) -> bool {
        match self {
            FullToken::Bool(_) => true,
            _ => false
        }
    }
    
    pub fn is_value_type(&self) -> bool {
        match self {
            FullToken::Color(_) => true,
            FullToken::Bool(_) => true,
            FullToken::Int(_) => true,
            FullToken::Float(_) => true,
            _ => false
        }
    }
    
    pub fn is_ident(&self) -> bool {
        match self {
            FullToken::Ident(_) => true,
            _ => false
        }
    }
    
    pub fn is_punct(&self) -> bool {
        match self {
            FullToken::Punct(_) => true,
            _ => false
        }
    }
    
    pub fn is_punct_id(&self, id: LiveId) -> bool {
        match self {
            FullToken::Punct(v) => *v == id,
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
