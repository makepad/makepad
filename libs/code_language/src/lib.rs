//! Public language identity and lexical providers.
//!
//! This crate is a leaf: no UI, graph, Cargo, watcher or model dependency.
//! The public editor and private Scope analysers share `LanguageId`, detection
//! and the C++ lexer so highlighting and analysis cannot disagree on raw
//! strings or preprocessor lines.

pub mod cpp_lex;
pub mod csharp_lex;
pub mod css_lex;
pub mod detect;
pub mod html_lex;
pub mod id;
pub mod java_lex;
pub mod lex;
pub mod markdown_lex;
pub mod python_lex;
pub mod script_lex;
pub mod splash_lex;
pub mod token;

pub use cpp_lex::{
    document_spans, lex_document, lex_document_cancellable, lex_line, line_summaries, CppKind,
    CppState, CppToken, LexError, CPP_LEXER_VERSION,
};
pub use csharp_lex::{CSharpState, CSHARP_LEXER_VERSION};
pub use css_lex::{CssState, CSS_LEXER_VERSION};
pub use detect::{
    cpp_family_extensions, detect_path, extension_of, has_compiled_frontend, inventory_extensions,
    is_cpp_family, is_script_family, is_text_format,
};
pub use html_lex::{HtmlState, MarkupDialect, HTML_LEXER_VERSION};
pub use id::{Detection, DetectionSource, Dialect, LanguageId, LanguageVersion};
pub use java_lex::{JavaState, JAVA_LEXER_VERSION};
pub use lex::{
    lexical_provider, lexer_schema, provider_for_detection, provider_for_path,
    CSharpLexicalProvider, CppLexicalProvider, CssLexicalProvider, HtmlLexicalProvider,
    JavaLexicalProvider, LexContinuation, LexicalProvider, MarkdownLexicalProvider,
    MarkupLexicalProvider, PlainTextProvider, PythonLexicalProvider, ScriptLexicalProvider,
    SplashLexicalProvider, PLAIN_LEXER_VERSION,
};
pub use markdown_lex::{MarkdownState, MARKDOWN_LEXER_VERSION};
pub use python_lex::{PythonState, PYTHON_LEXER_VERSION};
pub use script_lex::{ScriptDialect, ScriptState, SCRIPT_LEXER_VERSION};
pub use splash_lex::{SplashState, SPLASH_LEXER_VERSION};
pub use token::{LineMix, LineSummary, TokenRole, TokenSpan};

/// Parse-cache identity domain. Frontends append their own version.
pub const LANGUAGE_CACHE_DOMAIN: &str = "makepad-code-language/1";
