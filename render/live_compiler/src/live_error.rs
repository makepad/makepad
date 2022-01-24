use {
    std::fmt,
    std::convert::Into,
    crate::live_token::LiveTokenId,
    crate::span::{TextSpan,TokenSpan}
};

#[derive(Clone, Default, PartialEq)]
pub struct LiveErrorOrigin {
    pub filename: String,
    pub line:usize
}


#[derive(Clone)]
pub enum LiveErrorSpan {
    Text(TextSpan),
    Token(TokenSpan)
}

impl LiveErrorSpan{
    fn into_text_span(self)->Option<TextSpan>{
        match self{
            Self::Text(span)=>Some(span),
            _=>None
        }
    }
}

impl Into<LiveErrorSpan> for TextSpan{
    fn into(self)->LiveErrorSpan{
        LiveErrorSpan::Text(self)
    }
}

impl Into<LiveErrorSpan> for TokenSpan{
    fn into(self)->LiveErrorSpan{
        LiveErrorSpan::Token(self)
    }
}

impl Into<LiveErrorSpan> for LiveTokenId{
    fn into(self)->LiveErrorSpan{
        LiveErrorSpan::Token(TokenSpan{token_id:self, len:1})
    }
}

#[derive(Clone)]
pub struct LiveError {
    pub origin: LiveErrorOrigin,
    pub span: LiveErrorSpan,
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
            "{}: {}:{} - {} - origin: {}",
            self.file,
            self.span.start.line+1,
            self.span.start.column+1,
            self.message,
            self.origin
        )
    }
}



impl LiveError{
    
    pub fn into_live_file_error(self, file:&str)->LiveFileError{
        LiveFileError {
            origin: self.origin.clone(),
            file: file.to_string(),
            span: self.span.into_text_span().unwrap(),
            message: self.message,
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


impl fmt::Debug for LiveError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{} - origin: {} ", self.message, self.origin)
    }
}

impl fmt::Debug for LiveErrorOrigin {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}:{} ", self.filename, self.line)
    }
}
