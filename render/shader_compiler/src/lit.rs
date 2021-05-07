use crate::ty::Ty;
use crate::val::Val;
use makepad_live_parser::Vec4;
use makepad_live_parser::Token;
use makepad_live_parser::PrettyPrintedF32;
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Lit {
    Bool(bool),
    Int(i32),
    Float(f32),
    Color(u32),
}

impl Lit {
    pub fn to_ty(self) -> Ty {
        match self {
            Lit::Bool(_) => Ty::Bool,
            Lit::Int(_) => Ty::Int,
            Lit::Float(_) => Ty::Float,
            Lit::Color(_) => Ty::Vec4
        }
    }
    
    pub fn to_val(self) -> Val {
        match self {
            Lit::Bool(v) => Val::Bool(v),
            Lit::Int(v) => Val::Int(v),
            Lit::Float(v) => Val::Float(v),
            Lit::Color(v) => Val::Vec4(Vec4::from_u32(v))
        }
    }
    
    pub fn from_token(token: Token) -> Option<Lit> {
        match token {
            Token::Bool(v) => Some(Lit::Bool(v)),
            Token::Int(v) => Some(Lit::Int(v as i32)),
            Token::Float(v) => Some(Lit::Float(v as f32)),
            Token::Color(v) => Some(Lit::Color(v)),
            _ => None
        }
    }
}

impl fmt::Display for Lit {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Lit::Bool(lit) => write!(f, "{}", lit),
            Lit::Int(lit) => write!(f, "{}", lit),
            Lit::Float(lit) => write!(f, "{}", PrettyPrintedF32(*lit)),
            Lit::Color(lit) => {
                let v = Vec4::from_u32(*lit);
                write!(
                    f,
                    "vec4({},{},{},{})",
                    PrettyPrintedF32(v.x),
                    PrettyPrintedF32(v.y),
                    PrettyPrintedF32(v.z),
                    PrettyPrintedF32(v.w)
                )
            }
        }
    }
}

