//! The one structured refusal type for the Asset Server core.
//!
//! Same philosophy as the content contract's `AssetDataError`: every path is
//! total and fail-closed, and variants carry only static strings, numbers,
//! and fixed-size digests — never attacker-controlled bytes.

use makepad_asset_data::AssetDataError;

pub type ServerResult<T> = Result<T, ServerError>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServerError {
    /// A content-contract decode/validate refusal, passed through verbatim.
    Content(AssetDataError),
    /// A filesystem operation failed. Carries the operation name and the
    /// stable `std::io::ErrorKind`, not the OS message.
    Io {
        op: &'static str,
        kind: std::io::ErrorKind,
    },
    /// A SQLite call failed. Carries the operation name and the extended
    /// result code.
    Db { op: &'static str, code: i32 },
    /// Stored or supplied bytes do not hash to the digest that names them.
    /// This is the corruption refusal: the bytes are never served.
    DigestMismatch {
        what: &'static str,
        expected: [u8; 32],
        found: [u8; 32],
    },
    /// A recorded size does not match the bytes on disk.
    SizeMismatch {
        what: &'static str,
        expected: u64,
        found: u64,
    },
    /// An explicit budget was exceeded.
    OverBudget {
        what: &'static str,
        limit: u64,
        found: u64,
    },
    /// A referenced record does not exist. Fail closed: nothing is inferred.
    NotFound { what: &'static str },
    /// A record already exists with conflicting content.
    Conflict { what: &'static str },
    /// A lifecycle transition that the state machine forbids
    /// (for example publishing a quarantined candidate).
    InvalidState { what: &'static str, state: &'static str },
    /// Structurally invalid input (bad charset, empty name, zero lease…).
    InvalidInput { what: &'static str },
    /// A worker's lease is expired, stolen, or gone; its result is refused.
    LeaseLost { what: &'static str },
    /// Token authentication failed. Deliberately carries no detail: unknown,
    /// expired, revoked and disabled all present identically.
    Unauthenticated,
    /// The principal lacks the named capability for the requested scope.
    Denied { capability: &'static str },
    /// The on-disk catalog schema version is not one this build speaks.
    UnsupportedSchema { found: i64 },
}

impl std::fmt::Display for ServerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Content(e) => write!(f, "content: {e}"),
            Self::Io { op, kind } => write!(f, "io error in {op}: {kind:?}"),
            Self::Db { op, code } => write!(f, "database error in {op}: code {code}"),
            Self::DigestMismatch { what, .. } => write!(f, "digest mismatch for {what}"),
            Self::SizeMismatch { what, expected, found } => {
                write!(f, "size mismatch for {what}: expected {expected}, found {found}")
            }
            Self::OverBudget { what, limit, found } => {
                write!(f, "{what} over budget: {found} > {limit}")
            }
            Self::NotFound { what } => write!(f, "not found: {what}"),
            Self::Conflict { what } => write!(f, "conflict: {what}"),
            Self::InvalidState { what, state } => {
                write!(f, "invalid state for {what}: {state}")
            }
            Self::InvalidInput { what } => write!(f, "invalid input: {what}"),
            Self::LeaseLost { what } => write!(f, "lease lost: {what}"),
            Self::Unauthenticated => write!(f, "unauthenticated"),
            Self::Denied { capability } => write!(f, "denied: {capability}"),
            Self::UnsupportedSchema { found } => {
                write!(f, "unsupported catalog schema version {found}")
            }
        }
    }
}

impl std::error::Error for ServerError {}

impl From<AssetDataError> for ServerError {
    fn from(e: AssetDataError) -> Self {
        Self::Content(e)
    }
}

/// Attach an operation name to an io::Error, keeping only its stable kind.
pub fn io_err(op: &'static str) -> impl Fn(std::io::Error) -> ServerError {
    move |e| ServerError::Io { op, kind: e.kind() }
}
