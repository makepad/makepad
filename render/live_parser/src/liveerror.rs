use crate::span::Span;
//use std::error;
use std::fmt;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LiveErrorOrigin {
    pub filename: String,
    pub line:usize
}

#[derive(Clone, Debug)]
pub struct LiveError {
    pub origin: LiveErrorOrigin,
    pub span: Span,
    pub message: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LiveFileError {
    pub origin: LiveErrorOrigin,
    pub file: String,
    pub line: usize,
    pub column: usize,
    pub len: usize,
    pub message: String,
}

impl fmt::Display for LiveFileError {
    // This trait requires `fmt` with this exact signature.
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "{}: {} {} - {}",
            self.file,
            self.line + 1,
            self.column,
            self.message
        )
    }
}



impl LiveError{
    pub fn byte_to_row_col(byte: usize, source: &str) -> (usize, usize) {
        let lines = source.split("\n");
        let mut o = 0;
        for (index, line) in lines.enumerate() {
            if byte >= o && byte < o + line.len() {
                return (index, byte - o);
            }
            o += line.len() + 1;
        }
        return (0, 0);
    }
    
    pub fn to_live_file_error(&self, file:&str, source:&str)->LiveFileError{

        // lets find the span info
        let start = Self::byte_to_row_col(self.span.start() as usize, &source);
        LiveFileError {
            origin: self.origin.clone(),
            file: file.to_string(),
            line: start.0,
            column: start.1,
            len: (self.span.len()) as usize,
            message: self.to_string(),
        }
    }
}

impl fmt::Display for LiveError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{} - origin: {}:{} ", self.message, self.origin.filename, self.origin.line)
    }
}
