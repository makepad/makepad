use{
    std::fmt,
    crate::{
        live_id::{LiveId, LiveFileId},
        span::Span
    }
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TokenWithSpan {
    pub span: Span,
    pub token: Token,
}


impl fmt::Display for TokenWithSpan {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{} " , self.token)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Token {
    Eof,
    Punct(LiveId),
    Ident(LiveId),
    OpenParen,
    OpenBrace,
    OpenBracket,
    CloseParen,
    CloseBrace,
    CloseBracket,
    String{index:u32, len:u32},
    Bool(bool),
    Int(i64),
    Float(f64),
    Color(u32),
}

impl fmt::Display for Token {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Token::Eof => write!(f, "<eof>"),
            Token::String{..} => write!(f, "\"STRINGDATANOTAVAILABLE\""),
            Token::Punct(id) => write!(f, "{}", id),
            Token::Ident(id) => write!(f, "{}", id),
            Token::OpenParen=>write!(f, "("),
            Token::OpenBrace=>write!(f, "{{"),
            Token::OpenBracket=>write!(f, "["),
            Token::CloseParen=>write!(f, ")"),
            Token::CloseBrace=>write!(f, "}}"),
            Token::CloseBracket=>write!(f, "]"),
            Token::Bool(lit) => write!(f, "{}", lit),
            Token::Int(lit) => write!(f, "{}", lit),
            Token::Float(lit) => write!(f, "{}", lit),
            Token::Color(lit) => write!(f, "#{:x}", lit),
        }
    }
}

impl TokenId {
    pub fn new(file_id: LiveFileId, token: usize)->Self{
        TokenId(
            (((file_id.to_index() as u32) & 0x0fff) << 20) |
            ((token as u32) & 0xfffff) 
        )
    }
    
    pub fn token_index(&self)->usize{
        (self.0&0xfffff) as usize
    }
    
    pub fn file_id(&self)->LiveFileId{
        LiveFileId(((self.0>>20)&0xfff) as u16)
    }
    
    pub fn to_bits(&self)->u32{self.0}
    pub fn from_bits(v:u32)->Self{Self(v)}

}

#[derive(Clone, Copy, Eq, Ord, PartialOrd, PartialEq)]
pub struct TokenId(u32);

impl fmt::Debug for TokenId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "TokenId(token_index:{}, file_id:{})", self.token_index(), self.file_id().to_index())
    }
}

