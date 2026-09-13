//! Stable language identity. Compact, serialisable, and independent of any
//! analyser, editor, or filesystem type.

use std::fmt;

/// A compiled-in language. New languages append; numeric values are part of
/// cache keys and must not be reused.
#[repr(u16)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum LanguageId {
    Unknown = 0,
    Rust = 1,
    Toml = 2,
    Cpp = 3,
    C = 4,
    ObjectiveC = 5,
    ObjectiveCpp = 6,
    /// Detected for inventory only. No frontend is compiled in.
    Python = 7,
}

impl LanguageId {
    pub const ALL: [LanguageId; 8] = [
        LanguageId::Unknown,
        LanguageId::Rust,
        LanguageId::Toml,
        LanguageId::Cpp,
        LanguageId::C,
        LanguageId::ObjectiveC,
        LanguageId::ObjectiveCpp,
        LanguageId::Python,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            LanguageId::Unknown => "unknown",
            LanguageId::Rust => "rust",
            LanguageId::Toml => "toml",
            LanguageId::Cpp => "cpp",
            LanguageId::C => "c",
            LanguageId::ObjectiveC => "objc",
            LanguageId::ObjectiveCpp => "objcpp",
            LanguageId::Python => "python",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        LanguageId::ALL.iter().copied().find(|k| k.as_str() == s)
    }

    pub fn from_u16(v: u16) -> Self {
        LanguageId::ALL
            .iter()
            .copied()
            .find(|k| *k as u16 == v)
            .unwrap_or(LanguageId::Unknown)
    }
}

impl fmt::Display for LanguageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A dialect of a language (`cxx`, `gnu`, `ambiguous-header`). Empty name is
/// the language's default dialect.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Dialect {
    pub language: LanguageId,
    pub name: &'static str,
}

impl Dialect {
    pub const fn new(language: LanguageId, name: &'static str) -> Self {
        Dialect { language, name }
    }

    pub const fn default_for(language: LanguageId) -> Self {
        Dialect {
            language,
            name: "",
        }
    }

    pub fn as_str(self) -> &'static str {
        if self.name.is_empty() {
            self.language.as_str()
        } else {
            self.name
        }
    }
}

/// Provider/language-version label stored in parse-cache identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LanguageVersion {
    pub language: LanguageId,
    pub label: &'static str,
}

impl LanguageVersion {
    pub const fn new(language: LanguageId, label: &'static str) -> Self {
        LanguageVersion { language, label }
    }
}

/// How a language was chosen for a path.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DetectionSource {
    Override,
    Context,
    Extension,
    Content,
    Fallback,
}

impl DetectionSource {
    pub fn as_str(self) -> &'static str {
        match self {
            DetectionSource::Override => "override",
            DetectionSource::Context => "context",
            DetectionSource::Extension => "extension",
            DetectionSource::Content => "content",
            DetectionSource::Fallback => "fallback",
        }
    }
}

/// Result of language detection for one document.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Detection {
    pub language: LanguageId,
    pub dialect: Dialect,
    pub source: DetectionSource,
    /// Human-readable reason retained as coverage, never as a claim of proof.
    pub note: &'static str,
}

impl Detection {
    pub fn unknown() -> Self {
        Detection {
            language: LanguageId::Unknown,
            dialect: Dialect::default_for(LanguageId::Unknown),
            source: DetectionSource::Fallback,
            note: "unrecognized",
        }
    }
}
