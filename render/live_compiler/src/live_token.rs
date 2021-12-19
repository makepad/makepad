use{
    std::fmt,
    makepad_live_tokenizer::{
        LiveId,
        Delim,
        FullToken
    },
    crate::{
        live_ptr::{LiveFileId},
        span::Span
    }
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TokenWithSpan {
    pub span: Span,
    pub token: LiveToken,
}

impl fmt::Display for TokenWithSpan {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{} " , self.token)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LiveToken {
    Punct(LiveId),
    Ident(LiveId),
    
    Open(Delim),
    Close(Delim),
    
    String{index:u32, len:u32},
    Bool(bool),
    Int(i64),
    Float(f64),
    Color(u32),

    Eof,
}

impl LiveToken{
    pub fn from_full_token(full_token:FullToken)->Option<Self>{
        match full_token{
            FullToken::Punct(p)=>Some(LiveToken::Punct(p)),
            FullToken::Ident(p)=>Some(LiveToken::Ident(p)),
            FullToken::Open(p)=>Some(LiveToken::Open(p)),
            FullToken::Close(p)=>Some(LiveToken::Close(p)),
            FullToken::Bool(p)=>Some(LiveToken::Bool(p)),
            FullToken::Int(p)=>Some(LiveToken::Int(p)),
            FullToken::Float(p)=>Some(LiveToken::Float(p)),
            FullToken::Color(p)=>Some(LiveToken::Color(p)),
            _=>None
        }
    }
}

impl fmt::Display for LiveToken {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Eof => write!(f, "<eof>"),
            Self::String{..} => write!(f, "\"STRINGDATANOTAVAILABLE\""),
            Self::Punct(id) => write!(f, "{}", id),
            Self::Ident(id) => write!(f, "{}", id),
            Self::Open(Delim::Paren)=>write!(f, "("),
            Self::Open(Delim::Brace)=>write!(f, "{{"),
            Self::Open(Delim::Bracket)=>write!(f, "["),
            Self::Close(Delim::Paren)=>write!(f, ")"),
            Self::Close(Delim::Brace)=>write!(f, "}}"),
            Self::Close(Delim::Bracket)=>write!(f, "]"),
            Self::Bool(lit) => write!(f, "{}", lit),
            Self::Int(lit) => write!(f, "{}", lit),
            Self::Float(lit) => write!(f, "{}", lit),
            Self::Color(lit) => write!(f, "#{:x}", lit),
        }
    }
}

impl TokenId {
    pub fn new(file_id: LiveFileId, token: usize)->Self{
        let file_id = file_id.to_index();
        if file_id == 0 || file_id > 0x3ff ||  token > 0x3ffff{
            panic!();
        }
        TokenId(
            (((file_id as u32) & 0x3ff) << 18) |
            ((token as u32) & 0x3ffff) 
        )
    }
    
    pub fn is_empty(&self)->bool{
        ((self.0>>18)&0x3ff) == 0
    }
    
    pub fn token_index(&self)->usize{
        (self.0&0x3ffff) as usize
    }
    
    pub fn file_id(&self)->LiveFileId{
        LiveFileId(((self.0>>18)&0x3ff) as u16)
    }
    
    pub fn to_bits(&self)->u32{self.0}
    pub fn from_bits(v:u32)->Option<Self>{
        if (v&0xf000_0000)!=0{
            panic!();
        }
        if ((v>>18)&0x3ff) == 0{
            return None
        }
        return Some(Self(v))
    }
}

#[derive(Clone, Copy, Eq, Ord, PartialOrd, PartialEq)]
pub struct TokenId(u32);

impl fmt::Debug for TokenId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "TokenId(token_index:{}, file_id:{})", self.token_index(), self.file_id().to_index())
    }
}

