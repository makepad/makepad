//! Public language identity and lexical providers.
//!
//! This crate is a leaf: no UI, graph, Cargo, watcher or model dependency.
//! The public editor and private Scope analysers share `LanguageId`, detection
//! and the C++ lexer so highlighting and analysis cannot disagree on raw
//! strings or preprocessor lines.

pub mod cpp_lex;
pub mod csharp_lex;
pub mod css_lex;
pub mod dart_lex;
pub mod detect;
pub mod fsharp_lex;
pub mod go_lex;
pub mod haskell_lex;
pub mod html_lex;
pub mod id;
pub mod java_lex;
pub mod json_lex;
pub mod kotlin_lex;
pub mod lex;
pub mod markdown_lex;
pub mod php_lex;
pub mod python_lex;
pub mod ruby_lex;
pub mod script_lex;
pub mod shell_lex;
pub mod splash_lex;
pub mod sql_lex;
pub mod swift_lex;
pub mod toml_lex;
pub mod token;
pub mod zig_lex;

pub use cpp_lex::{
    document_spans, lex_document, lex_document_cancellable, lex_line, line_summaries, CppKind,
    CppState, CppToken, LexError, CPP_LEXER_VERSION,
};
pub use csharp_lex::{CSharpState, CSHARP_LEXER_VERSION};
pub use css_lex::{CssState, CSS_LEXER_VERSION};
pub use dart_lex::{DartState, DART_LEXER_VERSION};
pub use detect::{
    cpp_family_extensions, detect_path, extension_of, has_compiled_frontend, inventory_extensions,
    inventory_filenames, is_cpp_family, is_script_family, is_text_format,
};
pub use fsharp_lex::{FSharpState, FSHARP_LEXER_VERSION};
pub use go_lex::{GoState, GO_LEXER_VERSION};
pub use haskell_lex::{HaskellState, HASKELL_LEXER_VERSION};
pub use html_lex::{HtmlState, MarkupDialect, HTML_LEXER_VERSION};
pub use id::{Detection, DetectionSource, Dialect, LanguageId, LanguageVersion};
pub use java_lex::{JavaState, JAVA_LEXER_VERSION};
pub use json_lex::{JsonState, JSON_LEXER_VERSION};
pub use kotlin_lex::{KotlinState, KOTLIN_LEXER_VERSION};
pub use lex::{
    lexical_provider, lexer_schema, provider_for_detection, provider_for_path,
    CSharpLexicalProvider, CppLexicalProvider, CssLexicalProvider, DartLexicalProvider,
    FSharpLexicalProvider, GoLexicalProvider, HaskellLexicalProvider,
    HtmlLexicalProvider, JavaLexicalProvider, JsonLexicalProvider, KotlinLexicalProvider,
    LexContinuation, LexicalProvider, MarkdownLexicalProvider, MarkupLexicalProvider,
    PhpLexicalProvider,
    PlainTextProvider,
    PythonLexicalProvider, RubyLexicalProvider, ScriptLexicalProvider, ShellLexicalProvider,
    SplashLexicalProvider, SqlLexicalProvider, SwiftLexicalProvider, TomlLexicalProvider,
    ZigLexicalProvider, PLAIN_LEXER_VERSION,
};
pub use markdown_lex::{MarkdownState, MARKDOWN_LEXER_VERSION};
pub use php_lex::{PhpState, PHP_LEXER_VERSION};
pub use python_lex::{PythonState, PYTHON_LEXER_VERSION};
pub use ruby_lex::{RubyState, RUBY_LEXER_VERSION};
pub use script_lex::{ScriptDialect, ScriptState, SCRIPT_LEXER_VERSION};
pub use shell_lex::{ShellDialect, ShellState, SHELL_LEXER_VERSION};
pub use splash_lex::{SplashState, SPLASH_LEXER_VERSION};
pub use sql_lex::{SqlDialect, SqlState, SQL_LEXER_VERSION};
pub use swift_lex::{SwiftState, SWIFT_LEXER_VERSION};
pub use toml_lex::{TomlState, TOML_LEXER_VERSION};
pub use token::{LineMix, LineSummary, TokenRole, TokenSpan};
pub use zig_lex::{ZigState, ZIG_LEXER_VERSION};

/// Parse-cache identity domain. Frontends append their own version.
pub const LANGUAGE_CACHE_DOMAIN: &str = "makepad-code-language/1";
