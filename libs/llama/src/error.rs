use std::fmt::{Display, Formatter};

#[derive(Debug)]
pub enum LlamaError {
    Io(std::io::Error),
    Format(String),
    Unsupported(String),
}

impl LlamaError {
    pub fn format(msg: impl Into<String>) -> Self {
        Self::Format(msg.into())
    }

    pub fn unsupported(msg: impl Into<String>) -> Self {
        Self::Unsupported(msg.into())
    }
}

impl Display for LlamaError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(err) => write!(f, "io error: {}", err),
            Self::Format(msg) => write!(f, "gguf format error: {}", msg),
            Self::Unsupported(msg) => write!(f, "unsupported: {}", msg),
        }
    }
}

impl std::error::Error for LlamaError {}

impl From<std::io::Error> for LlamaError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

pub type Result<T> = std::result::Result<T, LlamaError>;
