//! Loaders for the standard SMuFL registry and per-font metadata.

mod json;
mod metadata;
mod registry;

pub use metadata::{
    EngravingDefaults, FontMetadata, GlyphAlternate, GlyphAlternates, GlyphAnchors, GlyphBBox,
    GlyphLigature, GlyphSet, OptionalGlyph, SetGlyph,
};
pub use registry::{GlyphClasses, GlyphInfo, GlyphRange, GlyphRanges, GlyphRegistry};

use std::fmt;

/// A structured failure while decoding SMuFL JSON data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SmuflError {
    Utf8,
    Json {
        message: String,
        line: usize,
        column: usize,
    },
    MissingField {
        path: String,
    },
    WrongType {
        path: String,
        expected: &'static str,
        found: &'static str,
    },
    InvalidCodepoint {
        path: String,
        value: String,
    },
    DuplicateCodepoint {
        codepoint: char,
        first_name: String,
        second_name: String,
    },
}

impl fmt::Display for SmuflError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Utf8 => f.write_str("SMuFL JSON is not valid UTF-8"),
            Self::Json {
                message,
                line,
                column,
            } => write!(f, "malformed SMuFL JSON at {line}:{column}: {message}"),
            Self::MissingField { path } => write!(f, "missing required SMuFL field `{path}`"),
            Self::WrongType {
                path,
                expected,
                found,
            } => write!(
                f,
                "SMuFL field `{path}` must be {expected}, but is {found}"
            ),
            Self::InvalidCodepoint { path, value } => {
                write!(f, "SMuFL field `{path}` has invalid codepoint `{value}`")
            }
            Self::DuplicateCodepoint {
                codepoint,
                first_name,
                second_name,
            } => write!(
                f,
                "SMuFL codepoint U+{:04X} is assigned to both `{first_name}` and `{second_name}`",
                u32::from(*codepoint)
            ),
        }
    }
}

impl std::error::Error for SmuflError {}

pub type SmuflResult<T> = Result<T, SmuflError>;
