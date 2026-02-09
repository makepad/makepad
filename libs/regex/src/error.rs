use std::{error, fmt, result};

#[derive(Clone, Debug)]
pub struct Error {
    pub message: String,
    pub pos: usize,
}

impl error::Error for Error {}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at position {}", self.message, self.pos)
    }
}

pub type Result<T> = result::Result<T, Error>;
