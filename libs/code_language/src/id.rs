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
    /// Python source. A compiled frontend is registered.
    Python = 7,
    JavaScript = 8,
    TypeScript = 9,
    CSharp = 10,
    Html = 11,
    Css = 12,
    /// Java source; compiled frontend registered.
    Java = 13,
    /// Splash script source. Tokens and line summaries only; no semantic frontend.
    Splash = 14,
    /// XML document. Tokens and line summaries only; no semantic frontend.
    Xml = 15,
    /// SVG document shown as source text, never rendered. Tokens and line summaries only.
    Svg = 16,
    /// Markdown document. Tokens and line summaries only; no semantic frontend.
    Markdown = 17,
    /// SQL source. Structural frontend registered; dialect-permissive lexer and parser.
    Sql = 18,
    /// JSON document. Tokens and line summaries only; no semantic frontend.
    Json = 19,
    /// POSIX sh / Bash / Zsh script. Tokens and line summaries only; no semantic
    /// frontend. Never executed.
    Shell = 20,
    /// Go source; compiled frontend registered.
    Go = 21,
    /// PHP source; compiled frontend registered.
    Php = 22,
    /// Kotlin source; compiled frontend registered.
    Kotlin = 23,
    /// Dart source; compiled frontend registered.
    Dart = 24,
    /// Swift source; compiled frontend registered.
    Swift = 25,
    /// Ruby source; compiled frontend registered.
    Ruby = 26,
    /// F# source; compiled frontend registered.
    FSharp = 27,
    /// Zig source; compiled frontend registered.
    Zig = 28,
    /// Haskell source; compiled frontend registered.
    Haskell = 29,
}

impl LanguageId {
    pub const ALL: [LanguageId; 30] = [
        LanguageId::Unknown,
        LanguageId::Rust,
        LanguageId::Toml,
        LanguageId::Cpp,
        LanguageId::C,
        LanguageId::ObjectiveC,
        LanguageId::ObjectiveCpp,
        LanguageId::Python,
        LanguageId::JavaScript,
        LanguageId::TypeScript,
        LanguageId::CSharp,
        LanguageId::Html,
        LanguageId::Css,
        LanguageId::Java,
        LanguageId::Splash,
        LanguageId::Xml,
        LanguageId::Svg,
        LanguageId::Markdown,
        LanguageId::Sql,
        LanguageId::Json,
        LanguageId::Shell,
        LanguageId::Go,
        LanguageId::Php,
        LanguageId::Kotlin,
        LanguageId::Dart,
        LanguageId::Swift,
        LanguageId::Ruby,
        LanguageId::FSharp,
        LanguageId::Zig,
        LanguageId::Haskell,
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
            LanguageId::JavaScript => "javascript",
            LanguageId::TypeScript => "typescript",
            LanguageId::CSharp => "csharp",
            LanguageId::Html => "html",
            LanguageId::Css => "css",
            LanguageId::Java => "java",
            LanguageId::Splash => "splash",
            LanguageId::Xml => "xml",
            LanguageId::Svg => "svg",
            LanguageId::Markdown => "markdown",
            LanguageId::Sql => "sql",
            LanguageId::Json => "json",
            LanguageId::Shell => "shell",
            LanguageId::Go => "go",
            LanguageId::Php => "php",
            LanguageId::Kotlin => "kotlin",
            LanguageId::Dart => "dart",
            LanguageId::Swift => "swift",
            LanguageId::Ruby => "ruby",
            LanguageId::FSharp => "fsharp",
            LanguageId::Zig => "zig",
            LanguageId::Haskell => "haskell",
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
