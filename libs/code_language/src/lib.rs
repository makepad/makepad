//! Public language identity and lexical providers.
//!
//! This crate is a leaf: no UI, graph, Cargo, watcher or model dependency.
//! The public editor and private Scope analysers share `LanguageId`, detection
//! and the C++ lexer so highlighting and analysis cannot disagree on raw
//! strings or preprocessor lines.

pub mod cpp_lex;
pub mod detect;
pub mod id;
pub mod lex;
pub mod token;

pub use cpp_lex::{
    document_spans, lex_document, lex_document_cancellable, lex_line, line_summaries, CppKind,
    CppState, CppToken, LexError, CPP_LEXER_VERSION,
};
pub use detect::{
    cpp_family_extensions, detect_path, extension_of, has_compiled_frontend, inventory_extensions,
    is_cpp_family,
};
pub use id::{Detection, DetectionSource, Dialect, LanguageId, LanguageVersion};
pub use lex::{
    lexical_provider, lexer_schema, provider_for_path, CppLexicalProvider, LexContinuation,
    LexicalProvider, PlainTextProvider, PLAIN_LEXER_VERSION,
};
pub use token::{LineMix, LineSummary, TokenRole, TokenSpan};

/// Parse-cache identity domain. Frontends append their own version.
pub const LANGUAGE_CACHE_DOMAIN: &str = "makepad-code-language/1";
