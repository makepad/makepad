use crate::ty::Ty;
use crate::value::Value;
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Lit {
    Bool(bool),
    Int(u32),
    Float(f32),
}

impl Lit {
    pub fn to_ty(self) -> Ty {
        match self {
            Lit::Bool(_) => Ty::Bool,
            Lit::Int(_) => Ty::Int,
            Lit::Float(_) => Ty::Float,
        }
    }

    pub fn to_value(self) -> Value {
        match self {
            Lit::Bool(lit) => Value::Bool(lit),
            Lit::Int(lit) => Value::Int(lit as i32),
            Lit::Float(lit) => Value::Float(lit),
        }
    }
}

impl fmt::Display for Lit {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Lit::Bool(lit) => write!(f, "{}", lit),
            Lit::Int(lit) => write!(f, "{}", lit),
            Lit::Float(lit) => write!(f, "{}", lit),
        }
    }
}
