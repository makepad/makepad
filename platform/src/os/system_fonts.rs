//! System font provider trait for accessing operating system fonts.

use std::fmt;

/// Data returned from a system font query.
#[derive(Clone)]
pub struct SystemFontData {
    pub data: Vec<u8>,
    pub index: u32,
    pub family_name: String,
}

impl fmt::Debug for SystemFontData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SystemFontData")
            .field("data_len", &self.data.len())
            .field("index", &self.index)
            .field("family_name", &self.family_name)
            .finish()
    }
}

/// Errors that can occur when querying system fonts.
#[derive(Debug, Clone)]
pub enum SystemFontError {
    FontNotFound(String),
    AccessDenied,
    PlatformNotSupported,
    ReadError(String),
}

impl fmt::Display for SystemFontError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SystemFontError::FontNotFound(name) => write!(f, "Font not found: {}", name),
            SystemFontError::AccessDenied => write!(f, "Access denied to font file"),
            SystemFontError::PlatformNotSupported => write!(f, "System fonts not supported"),
            SystemFontError::ReadError(msg) => write!(f, "Failed to read font: {}", msg),
        }
    }
}

impl std::error::Error for SystemFontError {}

/// Trait for platform-specific system font providers.
pub trait SystemFontProvider: Send + Sync {
    /// Query a font by family name.
    fn query_font(&self, family: &str) -> Result<SystemFontData, SystemFontError>;

    /// List all available font family names.
    fn list_families(&self) -> Vec<String>;
}
