use crate::span::Span;
//use std::error;
use std::fmt;

#[derive(Clone, Debug)]
pub struct LiveError {
    pub span: Span,
    pub message: String,
}

//impl error::Error for Error {}

impl fmt::Display for LiveError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}
