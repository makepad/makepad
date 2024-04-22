use std::{error::Error, fmt};

#[derive(Clone, Copy, Debug)]
pub enum Trap {
    Unreachable,
    TypeMismatch,
    ElemUninited,
    IntDivByZero,
    IntOverflow,
    InvalidConversionToInt,
    TableAccessOutOfBounds,
    MemAccessOutOfBounds,
    StackOverflow,
}

impl Trap {
    pub(crate) fn from_usize(val: usize) -> Option<Self> {
        match val {
            0 => Some(Self::Unreachable),
            1 => Some(Self::TypeMismatch),
            2 => Some(Self::ElemUninited),
            3 => Some(Self::IntDivByZero),
            4 => Some(Self::IntOverflow),
            5 => Some(Self::InvalidConversionToInt),
            6 => Some(Self::TableAccessOutOfBounds),
            7 => Some(Self::MemAccessOutOfBounds),
            8 => Some(Self::StackOverflow),
            _ => None,
        }
    }

    pub(crate) fn to_usize(self) -> usize {
        self as usize
    }
}

impl fmt::Display for Trap {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Unreachable => write!(f, "unreachable"),
            Self::TypeMismatch => write!(f, "type mismatch"),
            Self::ElemUninited => write!(f, "element uninitialized"),
            Self::IntDivByZero => write!(f, "integer divide by zero"),
            Self::IntOverflow => write!(f, "integer overflow"),
            Self::InvalidConversionToInt => write!(f, "invalid conversion to integer"),
            Self::TableAccessOutOfBounds => write!(f, "table access out of bounds"),
            Self::MemAccessOutOfBounds => write!(f, "memory access out of bounds"),
            Self::StackOverflow => write!(f, "stack overflow"),
        }
    }
}

impl Error for Trap {}
