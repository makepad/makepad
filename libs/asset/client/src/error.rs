//! Client refusal vocabulary. One typed error for every way a fetch can go
//! wrong, so callers can render honest states instead of guessing from
//! strings.
//!
//! Rules:
//! - `what`/`op` fields are static strings chosen by this crate; server bytes
//!   never become part of an error identity.
//! - The only server text an error may carry is [`ClientError::Server`]'s
//!   `detail`, which is bounded and control-stripped before it is stored.
//! - Digest/size mismatches carry both sides so a caller can log or display
//!   exactly what was refused.

use makepad_asset_data::AssetDataError;
use crate::location::ClientMode;

pub type ClientResult<T> = Result<T, ClientError>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClientError {
    /// The selected client mode deliberately does not provide this
    /// capability. This is a local refusal and performs no I/O.
    Unavailable { capability: &'static str, mode: ClientMode },
    /// Socket / filesystem failure.
    Io { op: &'static str, kind: std::io::ErrorKind },
    /// A wall-clock deadline expired before the operation finished.
    Timeout { op: &'static str },
    /// The peer sent bytes that violate the protocol this client speaks
    /// (malformed head, forbidden framing, refused redirect, bad JSON, DTO
    /// field out of shape). Always fail-closed, never "best effort".
    Protocol { what: &'static str },
    /// A declared or observed size exceeds a client budget.
    OverBudget { what: &'static str, limit: u64, found: u64 },
    /// The server refused the request. `status` is the HTTP status; `detail`
    /// is the bounded, sanitized `error` string from the response body when
    /// one was present.
    Server { status: u16, detail: Option<String> },
    /// Uniform credential refusal (HTTP 401), regardless of why.
    Unauthenticated,
    /// Authenticated but not allowed (HTTP 403).
    Denied,
    /// The named object does not exist on the server (HTTP 404).
    NotFound { what: &'static str },
    /// Received bytes do not hash to the identity that was requested.
    DigestMismatch { what: &'static str, expected: [u8; 32], found: [u8; 32] },
    /// Received byte count contradicts a declared length.
    SizeMismatch { what: &'static str, expected: u64, found: u64 },
    /// Canonical content decode refusal (from `makepad-asset-data`).
    Content(AssetDataError),
    /// The server at the endpoint is not the server the caller expected.
    ServerIdentityMismatch { expected: [u8; 16], found: [u8; 16] },
    /// The server advertises TLS; this client does not speak TLS and refuses
    /// to silently downgrade.
    TlsUnsupported,
    /// Caller-supplied input refused before any I/O (bad token shape, illegal
    /// query text, invalid budget).
    InvalidInput { what: &'static str },
    /// A pagination cursor minted against one server was used on another.
    WrongServerCursor,
    /// A publication would change the rights of an already published asset
    /// (license/credits/source differ from its published head). Rights are
    /// immutable per asset on this client: re-state them exactly, or
    /// publish under a new asset identity.
    RightsConflict { what: &'static str },
    /// The local cache refused admission (object over budget, or pinned bytes
    /// alone exceed the total budget).
    CacheAdmission { what: &'static str, limit: u64, found: u64 },
    /// The background runtime worker is gone; the request cannot be run.
    RuntimeDown,
    /// The caller cancelled this request before (or while) it ran. Never a
    /// server refusal; any partial transfer stays resumable on disk.
    Cancelled,
    /// Another live process already owns this cache root (the
    /// single-owner-per-root contract, enforced by an advisory lock).
    CacheBusy,
}

impl From<AssetDataError> for ClientError {
    fn from(e: AssetDataError) -> Self {
        ClientError::Content(e)
    }
}

impl ClientError {
    /// Is this "the document that is already there cannot be read under
    /// today's content contract"?
    ///
    /// Two shapes, because a stored document can be refused at either end:
    /// this client refuses it locally with [`AssetDataError::UnsupportedSchema`],
    /// and the SERVER refuses to serve it at all with a 422, because it
    /// validates what it holds against the same contract.
    ///
    /// Only ever ask this about reading an EXISTING document. A 422 while
    /// writing means the document being written is bad, which is a reason to
    /// stop, not a reason to write it again.
    pub fn is_unreadable_stored_document(&self) -> bool {
        matches!(
            self,
            ClientError::Content(AssetDataError::UnsupportedSchema { .. })
                | ClientError::Server { status: 422, .. }
        )
    }
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use ClientError::*;
        match self {
            Unavailable { capability, mode } => {
                write!(f, "capability unavailable in {mode:?} mode: {capability}")
            }
            Io { op, kind } => write!(f, "io failure during {op}: {kind:?}"),
            Timeout { op } => write!(f, "timeout during {op}"),
            Protocol { what } => write!(f, "protocol violation: {what}"),
            OverBudget { what, limit, found } => {
                write!(f, "over budget: {what} (limit {limit}, found {found})")
            }
            Server { status, detail } => match detail {
                Some(d) => write!(f, "server refused: {status} {d}"),
                None => write!(f, "server refused: {status}"),
            },
            Unauthenticated => write!(f, "unauthenticated"),
            Denied => write!(f, "denied"),
            NotFound { what } => write!(f, "not found: {what}"),
            DigestMismatch { what, .. } => write!(f, "digest mismatch: {what}"),
            SizeMismatch { what, expected, found } => {
                write!(f, "size mismatch: {what} (expected {expected}, found {found})")
            }
            Content(e) => write!(f, "content contract violation: {e}"),
            ServerIdentityMismatch { .. } => write!(f, "server identity mismatch"),
            TlsUnsupported => write!(f, "server requires tls; not supported"),
            InvalidInput { what } => write!(f, "invalid input: {what}"),
            WrongServerCursor => write!(f, "cursor belongs to a different server"),
            RightsConflict { what } => write!(f, "rights conflict: {what}"),
            CacheAdmission { what, limit, found } => {
                write!(f, "cache admission refused: {what} (limit {limit}, found {found})")
            }
            RuntimeDown => write!(f, "client runtime worker is gone"),
            Cancelled => write!(f, "request cancelled"),
            CacheBusy => write!(f, "cache root is owned by another process"),
        }
    }
}

impl std::error::Error for ClientError {}

/// Map an io::Error into a ClientError with a static operation name.
pub fn io_err(op: &'static str) -> impl Fn(std::io::Error) -> ClientError {
    move |e| ClientError::Io { op, kind: e.kind() }
}
