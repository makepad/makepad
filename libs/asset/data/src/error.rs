//! The one structured refusal type for the whole content contract.
//!
//! Every decode/validate path in this crate is TOTAL: hostile bytes or an
//! over-budget value yield a `AssetDataError`, never a panic and never a guessed
//! substitute. Variants carry only static strings and numbers so an attacker
//! cannot reflect arbitrary bytes back through an error message.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssetDataError {
    /// Input ended before the named field was complete.
    Truncated { what: &'static str },
    /// A document decoded fully but bytes remain after it.
    TrailingBytes,
    /// The 4-byte canonical-encoding magic is wrong.
    BadMagic,
    /// The document is canonical encoding but not the expected document kind.
    BadDocKind { expected: u8, found: u8 },
    /// A schema version this build does not speak. Fail closed, never guess.
    UnsupportedSchema { found: u16 },
    /// An unknown enum tag.
    BadTag { what: &'static str, found: u8 },
    /// Structurally invalid: bad UTF-8, bad charset, non-finite float,
    /// non-canonical bit pattern, bad bool byte, and similar.
    Malformed { what: &'static str },
    /// A count, length, dimension, or metric above its admission budget.
    OverBudget {
        what: &'static str,
        limit: u64,
        found: u64,
    },
    /// Below a required minimum (for example a thumbnail under 256 px).
    TooSmall {
        what: &'static str,
        min: u64,
        found: u64,
    },
    /// A collection that must be strictly sorted for canonical bytes is not.
    NotSorted { what: &'static str },
    /// A key/alias/slot that must be unique appears twice.
    Duplicate { what: &'static str },
    /// A required part is absent (for example a mesh without a thumbnail).
    Missing { what: &'static str },
    /// Two fields that must agree do not (for example a migration plan whose
    /// mode downgrades what its own reasons require).
    Mismatch { what: &'static str },
}

impl std::fmt::Display for AssetDataError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Truncated { what } => write!(f, "truncated input at {what}"),
            Self::TrailingBytes => write!(f, "trailing bytes after document"),
            Self::BadMagic => write!(f, "not canonical content encoding"),
            Self::BadDocKind { expected, found } => {
                write!(f, "wrong document kind: expected {expected}, found {found}")
            }
            Self::UnsupportedSchema { found } => {
                write!(f, "unsupported schema version {found}")
            }
            Self::BadTag { what, found } => write!(f, "unknown {what} tag {found}"),
            Self::Malformed { what } => write!(f, "malformed {what}"),
            Self::OverBudget { what, limit, found } => {
                write!(f, "{what} over budget: {found} > {limit}")
            }
            Self::TooSmall { what, min, found } => {
                write!(f, "{what} too small: {found} < {min}")
            }
            Self::NotSorted { what } => write!(f, "{what} not in canonical order"),
            Self::Duplicate { what } => write!(f, "duplicate {what}"),
            Self::Missing { what } => write!(f, "missing {what}"),
            Self::Mismatch { what } => write!(f, "mismatch: {what}"),
        }
    }
}

impl std::error::Error for AssetDataError {}
