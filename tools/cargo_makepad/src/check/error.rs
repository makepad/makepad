/// Error types for the script check command.

use std::fmt;
use std::path::PathBuf;

/// Represents the type of script check issue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IssueKind {
    /// A runtime error from script execution.
    RuntimeError,
    /// A parse error from script parsing.
    ParseError,
    /// A warning when falling back to parser-only validation.
    FallbackWarning,
}

/// A script check issue with location information.
#[derive(Debug, Clone)]
pub struct ScriptIssue {
    pub kind: IssueKind,
    pub file: String,
    pub line: u32,
    pub column: u32,
    pub message: String,
}

impl ScriptIssue {
    pub fn new(kind: IssueKind, file: String, line: u32, column: u32, message: String) -> Self {
        Self { kind, file, line, column, message }
    }

    pub fn fallback_warning(message: String) -> Self {
        Self {
            kind: IssueKind::FallbackWarning,
            file: "<script-check>".to_string(),
            line: 1,
            column: 1,
            message,
        }
    }

    pub fn is_error(&self) -> bool {
        matches!(self.kind, IssueKind::RuntimeError | IssueKind::ParseError)
    }

    pub fn is_warning(&self) -> bool {
        matches!(self.kind, IssueKind::FallbackWarning)
    }
}

impl fmt::Display for ScriptIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            IssueKind::FallbackWarning => write!(f, "warning: {}", self.message),
            _ => write!(f, "{}:{}:{}: {}", self.file, self.line, self.column, self.message),
        }
    }
}

/// Errors that can occur during script checking.
#[derive(Debug)]
#[allow(dead_code)]
pub enum CheckError {
    /// No Cargo.toml found in the directory tree.
    NoManifestFound,
    /// Failed to read or parse cargo metadata.
    CargoMetadataError(String),
    /// Could not determine which package to check.
    PackageNotFound,
    /// Package has no lib target for runtime checking.
    NoLibTarget,
    /// Invalid package manifest path.
    InvalidManifestPath(PathBuf),
    /// No script sources found to check.
    NoScriptSources,
    /// Failed to create runtime harness directory.
    HarnessDirectoryError(std::io::Error),
    /// Failed to write runtime harness files.
    HarnessWriteError(std::io::Error),
    /// Runtime harness execution failed.
    HarnessExecutionError(String),
    /// General I/O error.
    IoError(std::io::Error),
    /// Invalid command usage.
    UsageError(String),
}

impl fmt::Display for CheckError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoManifestFound => write!(f, "No Cargo.toml found in current directory tree"),
            Self::CargoMetadataError(msg) => write!(f, "Cargo metadata error: {}", msg),
            Self::PackageNotFound => write!(f, "Could not determine package for script check"),
            Self::NoLibTarget => write!(f, "Package has no lib target required for runtime script checking"),
            Self::InvalidManifestPath(path) => write!(f, "Invalid package manifest path: {}", path.display()),
            Self::NoScriptSources => write!(
                f,
                "No script found. Looked for script_mod!/script! in src/**/*.rs. Usage: cargo makepad check script"
            ),
            Self::HarnessDirectoryError(e) => write!(f, "Failed to create runtime harness directory: {}", e),
            Self::HarnessWriteError(e) => write!(f, "Failed to write runtime harness files: {}", e),
            Self::HarnessExecutionError(msg) => write!(f, "Runtime harness execution failed: {}", msg),
            Self::IoError(e) => write!(f, "I/O error: {}", e),
            Self::UsageError(msg) => write!(f, "{}", msg),
        }
    }
}

impl std::error::Error for CheckError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::HarnessDirectoryError(e) | Self::HarnessWriteError(e) | Self::IoError(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for CheckError {
    fn from(e: std::io::Error) -> Self {
        Self::IoError(e)
    }
}

/// Result type for script checking operations.
pub type CheckResult<T> = Result<T, CheckError>;
