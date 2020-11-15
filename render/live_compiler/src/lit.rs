use crate::ty::Ty;
use crate::val::Val;
use crate::colors::Color;
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Lit {
    Bool(bool),
    Int(i32),
    Float(f32),
    Color(Color)
    
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
            Lit::Bool(lit) => Val::Bool(lit),
            Lit::Int(lit) => Val::Int(lit as i32),
            Lit::Float(lit) => Val::Float(lit),
            Lit::Color(lit) => Val::Vec4(lit.to_vec4())
        }
    }
}

impl fmt::Display for Lit {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Lit::Bool(lit) => write!(f, "{}", lit),
            Lit::Int(lit) => write!(f, "{}", lit),
            Lit::Float(lit) => {
                if lit.abs().fract() < 0.00000001 {
                    write!(f, "{}.0", lit)
                } else {
                    write!(f, "{}", lit)
                }
            },
            Lit::Color(lit) =>{
                write!(f, "color({},{},{},{})", lit.r,lit.g,lit.b,lit.a)
            }
        }
    }
}

