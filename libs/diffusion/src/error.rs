use makepad_mlx::MlxRtError;
use std::fmt;
use std::path::PathBuf;

pub type Result<T> = std::result::Result<T, DiffusionError>;

#[derive(Debug)]
pub enum DiffusionError {
    Io { path: PathBuf, message: String },
    Json { path: PathBuf, message: String },
    Workflow(String),
    Model(String),
    Mlx(MlxRtError),
}

impl DiffusionError {
    pub fn io(path: impl Into<PathBuf>, message: impl Into<String>) -> Self {
        Self::Io {
            path: path.into(),
            message: message.into(),
        }
    }

    pub fn json(path: impl Into<PathBuf>, message: impl Into<String>) -> Self {
        Self::Json {
            path: path.into(),
            message: message.into(),
        }
    }

    pub fn workflow(message: impl Into<String>) -> Self {
        Self::Workflow(message.into())
    }

    pub fn model(message: impl Into<String>) -> Self {
        Self::Model(message.into())
    }
}

impl fmt::Display for DiffusionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, message } => write!(f, "I/O error at {}: {}", path.display(), message),
            Self::Json { path, message } => {
                write!(f, "JSON decode error at {}: {}", path.display(), message)
            }
            Self::Workflow(message) => write!(f, "workflow error: {}", message),
            Self::Model(message) => write!(f, "model error: {}", message),
            Self::Mlx(err) => err.fmt(f),
        }
    }
}

impl std::error::Error for DiffusionError {}

impl From<MlxRtError> for DiffusionError {
    fn from(value: MlxRtError) -> Self {
        Self::Mlx(value)
    }
}
