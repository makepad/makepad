//! Option and Result.

pub enum Option<T> {
    None,
    Some(T),
}

pub use Option::None;
pub use Option::Some;

impl<T> Option<T> {
    pub fn is_some(&self) -> bool {
        match self {
            Some(_) => true,
            None => false,
        }
    }
    pub fn is_none(&self) -> bool {
        match self {
            Some(_) => false,
            None => true,
        }
    }
    pub fn unwrap(self) -> T {
        match self {
            Some(v) => v,
            None => panic!("called `Option::unwrap()` on a `None` value"),
        }
    }
    pub fn expect(self, msg: &str) -> T {
        match self {
            Some(v) => v,
            None => panic!("{}", msg),
        }
    }
    pub fn unwrap_or(self, d: T) -> T {
        match self {
            Some(v) => v,
            None => d,
        }
    }
    pub fn as_ref(&self) -> Option<&T> {
        match self {
            Some(v) => Some(v),
            None => None,
        }
    }
    pub fn as_mut(&mut self) -> Option<&mut T> {
        match self {
            Some(v) => Some(v),
            None => None,
        }
    }
    pub fn take(&mut self) -> Option<T> {
        crate::mem::replace(self, None)
    }
}

pub enum Result<T, E> {
    Ok(T),
    Err(E),
}

pub use Result::Err;
pub use Result::Ok;

impl<T, E> Result<T, E> {
    pub fn is_ok(&self) -> bool {
        match self {
            Ok(_) => true,
            Err(_) => false,
        }
    }
    pub fn is_err(&self) -> bool {
        match self {
            Ok(_) => false,
            Err(_) => true,
        }
    }
    pub fn unwrap(self) -> T {
        match self {
            Ok(v) => v,
            Err(_) => panic!("called `Result::unwrap()` on an `Err` value"),
        }
    }
    pub fn ok(self) -> Option<T> {
        match self {
            Ok(v) => Some(v),
            Err(_) => None,
        }
    }
}
