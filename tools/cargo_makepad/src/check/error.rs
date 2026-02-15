/// Error types for the script check command.

use std::fmt;
use std::path::PathBuf;

/// Severity level for diagnostics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub enum Severity {
    /// Hard error - must be fixed.
    Error,
    /// Warning - should be addressed.
    Warning,
    /// Note - informational.
    Note,
    /// Help - suggestion for fixing.
    Help,
}

impl Severity {
    /// ANSI color code for this severity.
    pub fn color_code(&self) -> &'static str {
        match self {
            Self::Error => "\x1b[1;31m",   // Bold red
            Self::Warning => "\x1b[1;33m", // Bold yellow
            Self::Note => "\x1b[1;36m",    // Bold cyan
            Self::Help => "\x1b[1;32m",    // Bold green
        }
    }

    /// Label text for this severity.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Note => "note",
            Self::Help => "help",
        }
    }
}

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

impl IssueKind {
    /// Get the severity for this issue kind.
    pub fn severity(&self) -> Severity {
        match self {
            Self::RuntimeError | Self::ParseError => Severity::Error,
            Self::FallbackWarning => Severity::Warning,
        }
    }

    /// Get a short code for this issue kind.
    pub fn code(&self) -> Option<&'static str> {
        match self {
            Self::RuntimeError => Some("E0001"),
            Self::ParseError => Some("E0002"),
            Self::FallbackWarning => None,
        }
    }
}

/// A script check issue with location information.
#[derive(Debug, Clone)]
pub struct ScriptIssue {
    pub kind: IssueKind,
    pub file: String,
    pub line: u32,
    pub column: u32,
    pub message: String,
    /// Optional suggestion for fixing the issue.
    pub suggestion: Option<String>,
    /// Optional secondary label (for multi-span diagnostics).
    pub secondary_label: Option<String>,
}

impl ScriptIssue {
    pub fn new(kind: IssueKind, file: String, line: u32, column: u32, message: String) -> Self {
        Self {
            kind,
            file,
            line,
            column,
            message,
            suggestion: None,
            secondary_label: None,
        }
    }

    pub fn fallback_warning(message: String) -> Self {
        Self {
            kind: IssueKind::FallbackWarning,
            file: "<script-check>".to_string(),
            line: 1,
            column: 1,
            message,
            suggestion: None,
            secondary_label: None,
        }
    }

    /// Add a suggestion for how to fix this issue.
    #[allow(dead_code)]
    pub fn with_suggestion(mut self, suggestion: String) -> Self {
        self.suggestion = Some(suggestion);
        self
    }

    /// Add a secondary label.
    #[allow(dead_code)]
    pub fn with_secondary(mut self, label: String) -> Self {
        self.secondary_label = Some(label);
        self
    }

    pub fn is_error(&self) -> bool {
        matches!(self.kind, IssueKind::RuntimeError | IssueKind::ParseError)
    }

    pub fn is_warning(&self) -> bool {
        matches!(self.kind, IssueKind::FallbackWarning)
    }

    /// Get the severity of this issue.
    pub fn severity(&self) -> Severity {
        self.kind.severity()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_issue_severity() {
        let error = ScriptIssue::new(
            IssueKind::ParseError,
            "test.rs".into(),
            10,
            5,
            "syntax error".into(),
        );
        assert_eq!(error.severity(), Severity::Error);
        assert!(error.is_error());

        let warning = ScriptIssue::fallback_warning("fallback".into());
        assert_eq!(warning.severity(), Severity::Warning);
        assert!(warning.is_warning());
    }

    #[test]
    fn test_issue_with_suggestion() {
        let issue = ScriptIssue::new(
            IssueKind::ParseError,
            "test.rs".into(),
            10,
            5,
            "undefined variable `x`".into(),
        )
        .with_suggestion("did you mean `y`?".into());

        assert_eq!(issue.suggestion, Some("did you mean `y`?".into()));
    }
}
