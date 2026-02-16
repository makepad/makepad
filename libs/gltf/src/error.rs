use makepad_micro_serde::DeJsonErr;
use std::{error::Error, fmt, io, str::Utf8Error};

#[derive(Debug)]
pub enum GltfError {
    Io(io::Error),
    Json(DeJsonErr),
    Utf8(Utf8Error),
    InvalidGlb(String),
    Validation(String),
    Unsupported(String),
    MissingBuffer { index: usize },
    UnsupportedUri(String),
    DataUri(String),
}

impl fmt::Display for GltfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GltfError::Io(err) => write!(f, "io error: {err}"),
            GltfError::Json(err) => write!(f, "json parse error: {err:?}"),
            GltfError::Utf8(err) => write!(f, "utf8 decode error: {err}"),
            GltfError::InvalidGlb(msg) => write!(f, "invalid glb: {msg}"),
            GltfError::Validation(msg) => write!(f, "validation error: {msg}"),
            GltfError::Unsupported(msg) => write!(f, "unsupported feature: {msg}"),
            GltfError::MissingBuffer { index } => write!(f, "missing buffer data at index {index}"),
            GltfError::UnsupportedUri(uri) => write!(f, "unsupported uri: {uri}"),
            GltfError::DataUri(msg) => write!(f, "invalid data uri: {msg}"),
        }
    }
}

impl Error for GltfError {}

impl From<io::Error> for GltfError {
    fn from(value: io::Error) -> Self {
        GltfError::Io(value)
    }
}

impl From<Utf8Error> for GltfError {
    fn from(value: Utf8Error) -> Self {
        GltfError::Utf8(value)
    }
}
