use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Value {
    Bool(bool),
    Int(i32),
    Float(f32),
}

impl Value {
    pub fn to_bool(&self) -> Option<bool> {
        if let Value::Bool(value) = *self {
            Some(value)
        } else {
            None
        }
    }

    pub fn to_int(&self) -> Option<i32> {
        if let Value::Int(value) = *self {
            Some(value)
        } else {
            None
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Value::Bool(value) => write!(f, "{}", value),
            Value::Int(value) => write!(f, "{}", value),
            Value::Float(value) => write!(f, "{}", value),
        }
    }
}
