use makepad_micro_serde::DeJsonErr;
use makepad_webp::DecodingError as WebpDecodeError;
use makepad_zip_file::ZipError;
use std::{error::Error, fmt, io, str::Utf8Error};

#[derive(Debug)]
pub enum SplatError {
    Io(io::Error),
    Utf8(Utf8Error),
    Json(DeJsonErr),
    Zip(ZipError),
    Webp(WebpDecodeError),
    Unsupported(String),
    InvalidData(String),
    MissingField(String),
}

impl fmt::Display for SplatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SplatError::Io(err) => write!(f, "io error: {err}"),
            SplatError::Utf8(err) => write!(f, "utf8 decode error: {err}"),
            SplatError::Json(err) => write!(f, "json parse error: {err:?}"),
            SplatError::Zip(err) => write!(f, "zip decode error: {err:?}"),
            SplatError::Webp(err) => write!(f, "webp decode error: {err}"),
            SplatError::Unsupported(msg) => write!(f, "unsupported: {msg}"),
            SplatError::InvalidData(msg) => write!(f, "invalid data: {msg}"),
            SplatError::MissingField(name) => write!(f, "missing required field: {name}"),
        }
    }
}

impl Error for SplatError {}

impl From<io::Error> for SplatError {
    fn from(value: io::Error) -> Self {
        SplatError::Io(value)
    }
}

impl From<Utf8Error> for SplatError {
    fn from(value: Utf8Error) -> Self {
        SplatError::Utf8(value)
    }
}

impl From<DeJsonErr> for SplatError {
    fn from(value: DeJsonErr) -> Self {
        SplatError::Json(value)
    }
}

impl From<ZipError> for SplatError {
    fn from(value: ZipError) -> Self {
        SplatError::Zip(value)
    }
}

impl From<WebpDecodeError> for SplatError {
    fn from(value: WebpDecodeError) -> Self {
        SplatError::Webp(value)
    }
}
