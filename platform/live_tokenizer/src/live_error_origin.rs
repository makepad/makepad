use std::fmt;

#[macro_export]
macro_rules!live_error_origin{
    () => {
        LiveErrorOrigin { filename : file ! ( ) . to_string ( ) , line : line ! ( ) as usize }
    }
}

#[derive(Clone, Default, PartialEq)]
pub struct LiveErrorOrigin {
    pub filename: String,
    pub line:usize
}

impl fmt::Display for LiveErrorOrigin {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}:{} ", self.filename, self.line)
    }
}

impl fmt::Debug for LiveErrorOrigin {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}:{} ", self.filename, self.line)
    }
}
