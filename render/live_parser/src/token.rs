use crate::id::Id;
use crate::id::FileId;
use crate::span::Span;
use std::fmt;

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
    Punct(Id),
    Ident(Id),
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

#[derive(Clone, Copy, Eq, Ord, PartialOrd, PartialEq)]
pub struct TokenId {
    pub file_id:FileId,
    pub token_id:u32
}


impl fmt::Display for TokenId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "TokenId(token_id:{}, live_file_id:{})", self.token_id, self.file_id.to_index())
    }
}

impl fmt::Debug for TokenId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "TokenId(token_id:{}, live_file_id:{})", self.token_id, self.file_id.to_index())
    }
}

