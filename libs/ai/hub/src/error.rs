//! One error type for the whole service; variants keep enough shape for the
//! HTTP layer to pick a sensible status code.

use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum AssetAiError {
    /// HTTP client / transport failure (downloader, provider client).
    Http(String),
    /// Local filesystem failure.
    Io(String),
    /// Registry file failed to parse or validate.
    Registry(String),
    /// Model file download failed (includes sha256 mismatch).
    Download(String),
    /// A backend failed to load or generate.
    Backend(String),
    /// Malformed request parameters (bad base64 input, ...): a 400, not a 500.
    Params(String),
    /// Submit with queue_policy "reject" while a job is queued or running.
    Busy,
    /// Submit refused because the bounded FIFO already holds this many
    /// queued jobs (`MAKEPAD_ASSET_AI_MAX_QUEUE`). Same 409 class as `Busy`, but
    /// the message tells the caller the box is saturated, not merely busy.
    QueueFull(usize),
    /// The job's cancel flag was raised mid-run; the backend unwound at the
    /// next natural boundary (between steps/tiles/components). Maps to job
    /// state "cancelled", not "error".
    Cancelled,
    /// Model id not present in the registry.
    UnknownModel(String),
    /// A local pull or run was attempted before the current weight licence
    /// identity had been acknowledged.
    LicenseNotAcknowledged,
    /// A local run was attempted before all of the model's files were
    /// installed at their pinned sizes.
    NotInstalled(String),
    /// Model is in the registry but marked unavailable, or no backend is
    /// compiled in for it (e.g. `flux` without the cargo feature).
    Unavailable(String),
}

impl fmt::Display for AssetAiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AssetAiError::Http(m) => write!(f, "http error: {m}"),
            AssetAiError::Io(m) => write!(f, "io error: {m}"),
            AssetAiError::Registry(m) => write!(f, "registry error: {m}"),
            AssetAiError::Download(m) => write!(f, "download error: {m}"),
            AssetAiError::Backend(m) => write!(f, "backend error: {m}"),
            AssetAiError::Params(m) => write!(f, "bad request: {m}"),
            AssetAiError::Busy => write!(f, "busy: a job is already queued or running"),
            AssetAiError::QueueFull(limit) => {
                write!(f, "queue full: {limit} jobs already queued on this node")
            }
            AssetAiError::Cancelled => write!(f, "cancelled"),
            AssetAiError::UnknownModel(m) => write!(f, "unknown model: {m}"),
            AssetAiError::LicenseNotAcknowledged => {
                write!(f, "model licence has not been acknowledged")
            }
            AssetAiError::NotInstalled(m) => write!(f, "model not installed: {m}"),
            AssetAiError::Unavailable(m) => write!(f, "model unavailable: {m}"),
        }
    }
}

impl std::error::Error for AssetAiError {}

impl From<std::io::Error> for AssetAiError {
    fn from(err: std::io::Error) -> Self {
        AssetAiError::Io(err.to_string())
    }
}
