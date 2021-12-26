use {
    std::fmt,
    crate::span::TextSpan
};

#[derive(Clone, Default, PartialEq)]
pub struct LiveErrorOrigin {
    pub filename: String,
    pub line:usize
}

#[derive(Clone)]
pub struct LiveError {
    pub origin: LiveErrorOrigin,
    pub span: TextSpan,
    pub message: String,
}

#[derive(Clone, PartialEq)]
pub struct LiveFileError {
    pub origin: LiveErrorOrigin,
    pub file: String,
    pub span: TextSpan,
    pub message: String,
}

impl fmt::Display for LiveFileError {
    // This trait requires `fmt` with this exact signature.
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "{}: {}:{} - {}",
            self.file,
            self.span.start.line+1,
            self.span.start.column+1,
            self.message
        )
    }
}



impl LiveError{
    
    pub fn to_live_file_error(&self, file:&str)->LiveFileError{
        LiveFileError {
            origin: self.origin.clone(),
            file: file.to_string(),
            span: self.span,
            message: self.to_string(),
        }
    }
}

impl fmt::Display for LiveError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{} - origin: {} ", self.message, self.origin)
    }
}

impl fmt::Display for LiveErrorOrigin {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}:{} ", self.filename, self.line)
    }
}
