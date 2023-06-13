use {
    std::{
        rc::Rc,
        fmt,
        ops::Deref,
        ops::DerefMut,
    },
    crate::{
        makepad_math::Vec4,
        makepad_live_tokenizer::{
            LiveId,
            Delim,
            FullToken
        },
        live_ptr::{LiveFileId},
        span::TextSpan
    }
};

#[derive(Clone, Debug, PartialEq)]
pub struct TokenWithSpan {
    pub span: TextSpan,
    pub token: LiveToken,
}

impl Deref for TokenWithSpan {
    type Target = LiveToken;
    fn deref(&self) -> &Self::Target {&self.token}
}

impl DerefMut for TokenWithSpan {
    fn deref_mut(&mut self) -> &mut Self::Target {&mut self.token}
}

impl fmt::Display for TokenWithSpan {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{} ", self.token)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum LiveToken {
    Punct(LiveId),
    Ident(LiveId),
    
    Open(Delim),
    Close(Delim),
    
    String (Rc<String>),
    Bool(bool),
    Int(i64),
    Float(f64),
    Color(u32),
    
    Eof,
}

impl LiveToken {

    pub fn as_float(&self) -> Option<f64> {
        match self {
            Self::Float(v) => Some(*v),
            Self::Int(v) => Some(*v as f64),
            _ => None
        }
    }
    pub fn as_vec4(&self) -> Option<Vec4> {
        match self {
            Self::Color(c) => Some(Vec4::from_u32(*c)),
            _ => None
        }
    }    
    
    pub fn is_open(&self) -> bool {
        matches!(self, LiveToken::Open(_))
    }
    
    pub fn is_close(&self) -> bool {
        matches!(self, LiveToken::Close(_))
    }
    
    pub fn is_open_delim(&self, delim: Delim) -> bool {
        match self {
            LiveToken::Open(d) => *d == delim,
            _ => false
        }
    }
    
    pub fn is_close_delim(&self, delim: Delim) -> bool {
        match self {
            LiveToken::Close(d) => *d == delim,
            _ => false
        }
    }
    
    pub fn is_int(&self) -> bool {
        matches!(self, LiveToken::Int(_))
    }
    
    
    pub fn is_float(&self) -> bool {
        matches!(self, LiveToken::Float(_))
    }
    
        
    pub fn is_color(&self) -> bool {
        matches!(self, LiveToken::Color(_))
    }

    pub fn is_bool(&self) -> bool {
        matches!(self, LiveToken::Bool(_))
    }

    pub fn is_parsed_number(&self) -> bool {
        matches!(self, LiveToken::Int(_) | LiveToken::Float(_))
    }
    
    pub fn is_value_type(&self) -> bool {
        matches!(self, LiveToken::Color(_) | LiveToken::Bool(_) | LiveToken::Int(_) | LiveToken::Float(_))
    }
        
    pub fn is_ident(&self) -> bool {
        matches!(self, LiveToken::Ident(_))
    }
    
    pub fn is_punct(&self) -> bool {
        matches!(self, LiveToken::Punct(_))
    }
    
    pub fn is_punct_id(&self, id: LiveId) -> bool {
        match self {
            LiveToken::Punct(v) => *v == id,
            _ => false
        }
    }
    
    pub fn is_parse_equal(&self, other: &LiveToken) -> bool {
        match self {
            LiveToken::String(p) => if let LiveToken::String(o) = other {*p == *o}else {false},
            LiveToken::Punct(p) => if let LiveToken::Punct(o) = other {*p == *o}else {false},
            LiveToken::Ident(p) => if let LiveToken::Ident(o) = other {*p == *o}else {false},
            LiveToken::Open(p) => if let LiveToken::Open(o) = other {*p == *o}else {false},
            LiveToken::Close(p) => if let LiveToken::Close(o) = other {*p == *o}else {false},
            LiveToken::Bool(_) => matches!(other, LiveToken::Bool(_)),
            LiveToken::Int(_) => if let LiveToken::Int(_) = other {true} else { matches!(other, LiveToken::Float(_)) },
            LiveToken::Float(_) => if let LiveToken::Float(_) = other {true} else { matches!(other, LiveToken::Int(_)) },
            LiveToken::Color(_) => matches!(other, LiveToken::Color(_)),
            LiveToken::Eof => matches!(other, LiveToken::Eof),
        }
    }
    
    pub fn from_full_token(full_token: &FullToken) -> Option<Self> {
        match full_token {
            FullToken::String(s) => Some(LiveToken::String(s.clone())),
            FullToken::Punct(p) => Some(LiveToken::Punct(*p)),
            FullToken::Ident(p) => Some(LiveToken::Ident(*p)),
            FullToken::Open(p) => Some(LiveToken::Open(*p)),
            FullToken::Close(p) => Some(LiveToken::Close(*p)),
            FullToken::Bool(p) => Some(LiveToken::Bool(*p)),
            FullToken::Int(p) => Some(LiveToken::Int(*p)),
            FullToken::Float(p) => Some(LiveToken::Float(*p)),
            FullToken::Color(p) => Some(LiveToken::Color(*p)),
            _ => None
        }
    }
}

impl fmt::Display for LiveToken {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Eof => write!(f, "<eof>"),
            Self::String(v) => write!(f, "{}", v),
            Self::Punct(id) => write!(f, "{}", id),
            Self::Ident(id) => write!(f, "{}", id),
            Self::Open(Delim::Paren) => write!(f, "("),
            Self::Open(Delim::Brace) => write!(f, "{{"),
            Self::Open(Delim::Bracket) => write!(f, "["),
            Self::Close(Delim::Paren) => write!(f, ")"),
            Self::Close(Delim::Brace) => write!(f, "}}"),
            Self::Close(Delim::Bracket) => write!(f, "]"),
            Self::Bool(lit) => write!(f, "{}", lit),
            Self::Int(lit) => write!(f, "{}", lit),
            Self::Float(lit) => write!(f, "{}", lit),
            Self::Color(lit) => write!(f, "#{:x}", lit),
        }
    }
}

impl LiveTokenId {
    pub fn new(file_id: LiveFileId, token: usize) -> Self {
        let file_id = file_id.to_index();
        if file_id > 0x3fe || token > 0x3ffff {
            panic!();
        }
        LiveTokenId(
            (((file_id as u32 + 1) & 0x3ff) << 18) | ((token as u32) & 0x3ffff)
        )
    }
    
    pub fn is_empty(&self) -> bool {
        ((self.0 >> 18) & 0x3ff) == 0
    }
    
    pub fn token_index(&self) -> usize {
        (self.0 & 0x3ffff) as usize
    }
    
    pub fn file_id(&self) -> Option<LiveFileId> {
        let id = (self.0 >> 18) & 0x3ff;
        if id == 0{
            None
        }
        else{
            Some(LiveFileId((id - 1) as u16))
        }
    }
    
    pub fn to_bits(&self) -> u32 {self.0}
    pub fn from_bits(v: u32) -> Option<Self> {
        if (v & 0xf000_0000) != 0 {
            panic!();
        }
        if ((v >> 18) & 0x3ff) == 0 {
            None
        } else {
            Some(Self(v))
        }
    }
}

#[derive(Clone, Default, Copy, Eq, Ord, Hash, PartialOrd, PartialEq)]
pub struct LiveTokenId(u32);

impl fmt::Debug for LiveTokenId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "TokenId(token_index:{}, file_id:{})", self.token_index(), self.file_id().unwrap_or(LiveFileId::new(0)).to_index())
    }
}

