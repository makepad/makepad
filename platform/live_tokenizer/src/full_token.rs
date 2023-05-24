use {
    std::{
        rc::Rc,
        ops::Deref,
        ops::DerefMut,
    },
    crate::live_id::LiveId
};

#[derive(Clone, Debug, PartialEq)]
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

#[derive(Clone, Debug, PartialEq)]
pub enum FullToken {
    Punct(LiveId),
    Ident(LiveId),
    
    Open(Delim),
    Close(Delim),

    String(Rc<String>),
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
        matches!(self, FullToken::Whitespace)
    }

    pub fn is_comment(&self) -> bool {
        matches!(self, FullToken::Comment)
    }
    
    pub fn is_ws_or_comment(&self)->bool{
        matches!(self, FullToken::Whitespace | FullToken::Comment)
    }
        
    pub fn is_open(&self) -> bool {
        matches!(self, FullToken::Open(_))
    }
    
    pub fn is_close(&self) -> bool {
        matches!(self, FullToken::Close(_))
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
        matches!(self, FullToken::Int(_))
    }
    
    pub fn is_float(&self) -> bool {
        matches!(self, FullToken::Float(_))
    }
    
        
    pub fn is_color(&self) -> bool {
        matches!(self, FullToken::Color(_))
    }

    pub fn is_parsed_number(&self) -> bool {
        matches!(self, FullToken::Int(_) | FullToken::Float(_))
    }


    pub fn is_bool(&self) -> bool {
        matches!(self, FullToken::Bool(_))
    }
    
    pub fn is_value_type(&self) -> bool {
        matches!(
            self,
            FullToken::Color(_)
                | FullToken::Bool(_)
                | FullToken::Int(_)
                | FullToken::Float(_)
        )
    }
    
    pub fn is_ident(&self) -> bool {
        matches!(self, FullToken::Ident(_))
    }
    
    pub fn is_punct(&self) -> bool {
        matches!(self, FullToken::Punct(_))
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
