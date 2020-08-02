use crate::math::Vec4;
use std::fmt;
#[derive(Clone, Debug, PartialEq)]
pub enum Val {
    Bool(bool),
    Int(i32),
    Float(f32),
    Vec4(Vec4),
}

impl Val {
    pub fn to_bool(&self) -> Option<bool> {
        match *self {
            Val::Bool(val) => Some(val),
            _ => None,
        }
    }

    pub fn to_int(&self) -> Option<i32> {
        match *self {
            Val::Int(val) => Some(val),
            _ => None,
        }
    }
}

impl fmt::Display for Val {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Val::Bool(val) => write!(f, "{}", val),
            Val::Int(val) => write!(f, "{}", val),
            Val::Float(v) => {
                if v.abs().fract() < 0.00000001 {
                    write!(f, "{}.0", v)
                } else {
                    write!(f, "{}", v)
                }
            }
            Val::Vec4(val) => write!(f, "{}", val),
        }
    }
}
