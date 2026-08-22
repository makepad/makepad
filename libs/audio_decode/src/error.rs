//! One error type for both codecs. Every variant is reachable from a
//! malformed file: nothing here is an assertion in disguise.

use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AudioError {
    /// The bytes match no format this crate reads.
    UnknownFormat,
    /// The stream ends inside a structure that must be complete.
    Truncated,
    /// A header field is present but nonsense (reserved value, zero rate).
    BadHeader(&'static str),
    /// Structurally valid but uses a feature this decoder refuses, with the
    /// feature named so a caller can tell the user *what* is unsupported.
    Unsupported(&'static str),
    /// The bitstream disagrees with itself (bad Huffman code, bad codebook,
    /// a residue partition that runs off the end of the vector).
    Corrupt(&'static str),
    /// The stream would exceed [`crate::Limits`].
    TooLarge(&'static str),
    /// Nothing decodable was found (no frames, no audio packets).
    Empty,
}

impl fmt::Display for AudioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AudioError::UnknownFormat => write!(f, "unrecognised audio format"),
            AudioError::Truncated => write!(f, "truncated stream"),
            AudioError::BadHeader(what) => write!(f, "invalid header: {what}"),
            AudioError::Unsupported(what) => write!(f, "unsupported: {what}"),
            AudioError::Corrupt(what) => write!(f, "corrupt bitstream: {what}"),
            AudioError::TooLarge(what) => write!(f, "exceeds decode limits: {what}"),
            AudioError::Empty => write!(f, "no audio data"),
        }
    }
}

impl std::error::Error for AudioError {}
