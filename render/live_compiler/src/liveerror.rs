use {
    std::fmt,
    crate::span::Span
};

#[derive(Clone, Default, PartialEq)]
pub struct LiveErrorOrigin {
    pub filename: String,
    pub line:usize
}

#[derive(Clone)]
pub struct LiveError {
    pub origin: LiveErrorOrigin,
    pub span: Span,
    pub message: String,
}

#[derive(Clone, Default, PartialEq)]
pub struct LiveFileError {
    pub origin: LiveErrorOrigin,
    pub file: String,
    pub line_offset: usize,
    pub line_col: Option<(usize, usize)>,
    pub len: usize,
    pub message: String,
}

impl fmt::Display for LiveFileError {
    // This trait requires `fmt` with this exact signature.
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if let Some(line_col) = self.line_col{
            write!(
                f,
                "{}: {} {} - {}",
                self.file,
                line_col.0 + 1 + self.line_offset,
                line_col.1,
                self.message
            )
        }
        else{
            write!(
                f,
                "{}: <not found> - {}",
                self.file,
                self.message
            )
        }
    }
}



impl LiveError{
    pub fn byte_to_row_col(byte: usize, source: &str) -> Option<(usize, usize)> {
        //println!(" SEARCHING {} {}", byte, source.len());
        let lines = source.split("\n");
        let mut o = 0;
        for (index, line) in lines.enumerate() {
            if byte >= o && byte < o + line.len() {
                return Some((index, byte - o));
            }
            o += line.len() + 1;
        }
        return None;
    }
    
    pub fn to_live_file_error(&self, file:&str, source:&str, line_offset:usize)->LiveFileError{

        // lets find the span info
        let line_col = Self::byte_to_row_col(self.span.start() as usize, &source);
        LiveFileError {
            origin: self.origin.clone(),
            file: file.to_string(),
            line_offset,
            line_col,
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
