//! Public lexical providers. Continuation state is provider-owned.
//!
//! The editor stores one slot per line of `LexContinuation`. Adding a language
//! registers a provider here; it does not add a parallel state array in the
//! editor. Unknown languages use the plain-text provider, never Rust.

use crate::cpp_lex::{lex_line as cpp_lex_line, CppState, CPP_LEXER_VERSION};
use crate::csharp_lex::{lex_line as csharp_lex_line, CSharpState, CSHARP_LEXER_VERSION};
use crate::css_lex::{lex_line as css_lex_line, CssState, CSS_LEXER_VERSION};
use crate::dart_lex::{lex_line as dart_lex_line, DartState, DART_LEXER_VERSION};
use crate::detect::{detect_path, is_cpp_family, is_script_family};
use crate::fsharp_lex::{lex_line as fsharp_lex_line, FSharpState, FSHARP_LEXER_VERSION};
use crate::go_lex::{lex_line as go_lex_line, GoState, GO_LEXER_VERSION};
use crate::haskell_lex::{lex_line as haskell_lex_line, HaskellState, HASKELL_LEXER_VERSION};
use crate::html_lex::{
    lex_line as html_lex_line, HtmlState, MarkupDialect, HTML_LEXER_VERSION,
};
use crate::id::{Detection, Dialect, LanguageId};
use crate::java_lex::{lex_line as java_lex_line, JavaState, JAVA_LEXER_VERSION};
use crate::json_lex::{lex_line as json_lex_line, JsonState, JSON_LEXER_VERSION};
use crate::kotlin_lex::{lex_line as kotlin_lex_line, KotlinState, KOTLIN_LEXER_VERSION};
use crate::markdown_lex::{lex_line as markdown_lex_line, MarkdownState, MARKDOWN_LEXER_VERSION};
use crate::php_lex::{lex_line as php_lex_line, PhpState, PHP_LEXER_VERSION};
use crate::python_lex::{lex_line as python_lex_line, PythonState, PYTHON_LEXER_VERSION};
use crate::script_lex::{
    lex_line as script_lex_line, ScriptDialect, ScriptState, SCRIPT_LEXER_VERSION,
};
use crate::shell_lex::{
    lex_line as shell_lex_line, ShellDialect, ShellState, SHELL_LEXER_VERSION,
};
use crate::splash_lex::{lex_line as splash_lex_line, SplashState, SPLASH_LEXER_VERSION};
use crate::sql_lex::{
    lex_line as sql_lex_line, SqlDialect, SqlState, SQL_LEXER_VERSION,
};
use crate::ruby_lex::{lex_line as ruby_lex_line, RubyState, RUBY_LEXER_VERSION};
use crate::swift_lex::{lex_line as swift_lex_line, SwiftState, SWIFT_LEXER_VERSION};
use crate::toml_lex::{lex_line as toml_lex_line, TomlState, TOML_LEXER_VERSION};
use crate::token::{TokenRole, TokenSpan};
use crate::zig_lex::{lex_line as zig_lex_line, ZigState, ZIG_LEXER_VERSION};

/// Version of the plain-text lexer (byte-run classification only).
pub const PLAIN_LEXER_VERSION: u32 = 1;

/// Provider-owned line continuation. The editor holds this enum only.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum LexContinuation {
    #[default]
    Plain,
    Cpp(CppState),
    Script(ScriptState),
    Python(PythonState),
    CSharp(CSharpState),
    Html(HtmlState),
    Css(CssState),
    Java(JavaState),
    Splash(SplashState),
    Markdown(MarkdownState),
    Sql(SqlState),
    Toml(TomlState),
    Json(JsonState),
    Shell(ShellState),
    Go(GoState),
    Php(PhpState),
    Kotlin(KotlinState),
    Dart(DartState),
    Swift(SwiftState),
    Ruby(RubyState),
    FSharp(FSharpState),
    Zig(ZigState),
    Haskell(HaskellState),
}

pub trait LexicalProvider: Send + Sync {
    fn language(&self) -> LanguageId;
    fn lexer_version(&self) -> u32;
    fn initial(&self) -> LexContinuation;
    /// Dialect-aware initial continuation. Defaults to [`Self::initial`].
    fn initial_for_dialect(&self, _dialect: Dialect) -> LexContinuation {
        self.initial()
    }
    /// Tokens with offsets relative to `line`.
    fn lex_line(&self, line: &str, state: &mut LexContinuation) -> Vec<TokenSpan>;
}

pub struct PlainTextProvider;
pub struct CppLexicalProvider;
pub struct ScriptLexicalProvider {
    pub language: LanguageId,
    pub jsx_default: bool,
}
pub struct PythonLexicalProvider;
pub struct CSharpLexicalProvider;
pub struct HtmlLexicalProvider;
pub struct CssLexicalProvider;
pub struct JavaLexicalProvider;
pub struct SplashLexicalProvider;
pub struct MarkdownLexicalProvider;
pub struct SqlLexicalProvider;
pub struct TomlLexicalProvider;
pub struct JsonLexicalProvider;
pub struct ShellLexicalProvider;
pub struct GoLexicalProvider;
pub struct PhpLexicalProvider;
pub struct KotlinLexicalProvider;
pub struct DartLexicalProvider;
pub struct SwiftLexicalProvider;
pub struct RubyLexicalProvider;
pub struct FSharpLexicalProvider;
pub struct ZigLexicalProvider;
pub struct HaskellLexicalProvider;
/// XML / SVG share the HTML tokeniser with a markup dialect in continuation state.
pub struct MarkupLexicalProvider {
    pub language: LanguageId,
    pub dialect: MarkupDialect,
}

impl LexicalProvider for PlainTextProvider {
    fn language(&self) -> LanguageId {
        LanguageId::Unknown
    }
    fn lexer_version(&self) -> u32 {
        PLAIN_LEXER_VERSION
    }
    fn initial(&self) -> LexContinuation {
        LexContinuation::Plain
    }
    fn lex_line(&self, line: &str, state: &mut LexContinuation) -> Vec<TokenSpan> {
        *state = LexContinuation::Plain;
        if line.is_empty() {
            return Vec::new();
        }
        vec![TokenSpan::new(0, line.len() as u32, TokenRole::Unknown)]
    }
}

impl LexicalProvider for CppLexicalProvider {
    fn language(&self) -> LanguageId {
        LanguageId::Cpp
    }
    fn lexer_version(&self) -> u32 {
        CPP_LEXER_VERSION
    }
    fn initial(&self) -> LexContinuation {
        LexContinuation::Cpp(CppState::default())
    }
    fn lex_line(&self, line: &str, state: &mut LexContinuation) -> Vec<TokenSpan> {
        let incoming = match state {
            LexContinuation::Cpp(s) => s.clone(),
            _ => CppState::default(),
        };
        let (next, roles) = cpp_lex_line(line, incoming);
        *state = LexContinuation::Cpp(next);
        roles_to_spans(roles)
    }
}

impl LexicalProvider for ScriptLexicalProvider {
    fn language(&self) -> LanguageId {
        self.language
    }
    fn lexer_version(&self) -> u32 {
        SCRIPT_LEXER_VERSION
    }
    fn initial(&self) -> LexContinuation {
        self.initial_for_dialect(Dialect::default_for(self.language))
    }
    fn initial_for_dialect(&self, dialect: Dialect) -> LexContinuation {
        let language = if is_script_family(dialect.language) {
            dialect.language
        } else {
            self.language
        };
        let mut script = ScriptDialect::from_dialect(Dialect::new(language, dialect.name));
        // jsx/tsx turn JSX on; dts turns it off; JS default may start in JSX.
        if dialect.name.is_empty() && self.language == LanguageId::JavaScript && self.jsx_default {
            script = ScriptDialect::Jsx;
        }
        LexContinuation::Script(ScriptState::for_dialect(script))
    }
    fn lex_line(&self, line: &str, state: &mut LexContinuation) -> Vec<TokenSpan> {
        let incoming = match state {
            LexContinuation::Script(s) => s.clone(),
            _ => match self.initial() {
                LexContinuation::Script(s) => s,
                _ => ScriptState::default(),
            },
        };
        let (next, roles) = script_lex_line(line, incoming);
        *state = LexContinuation::Script(next);
        roles_to_spans(roles)
    }
}

impl LexicalProvider for PythonLexicalProvider {
    fn language(&self) -> LanguageId {
        LanguageId::Python
    }
    fn lexer_version(&self) -> u32 {
        PYTHON_LEXER_VERSION
    }
    fn initial(&self) -> LexContinuation {
        LexContinuation::Python(PythonState::default())
    }
    fn lex_line(&self, line: &str, state: &mut LexContinuation) -> Vec<TokenSpan> {
        let incoming = match state {
            LexContinuation::Python(s) => s.clone(),
            _ => PythonState::default(),
        };
        let (next, roles) = python_lex_line(line, incoming);
        *state = LexContinuation::Python(next);
        roles_to_spans(roles)
    }
}

impl LexicalProvider for CSharpLexicalProvider {
    fn language(&self) -> LanguageId {
        LanguageId::CSharp
    }
    fn lexer_version(&self) -> u32 {
        CSHARP_LEXER_VERSION
    }
    fn initial(&self) -> LexContinuation {
        LexContinuation::CSharp(CSharpState::default())
    }
    fn lex_line(&self, line: &str, state: &mut LexContinuation) -> Vec<TokenSpan> {
        let incoming = match state {
            LexContinuation::CSharp(s) => s.clone(),
            _ => CSharpState::default(),
        };
        let (next, roles) = csharp_lex_line(line, incoming);
        *state = LexContinuation::CSharp(next);
        roles_to_spans(roles)
    }
}

impl LexicalProvider for HtmlLexicalProvider {
    fn language(&self) -> LanguageId {
        LanguageId::Html
    }
    fn lexer_version(&self) -> u32 {
        HTML_LEXER_VERSION
    }
    fn initial(&self) -> LexContinuation {
        LexContinuation::Html(HtmlState::default())
    }
    fn lex_line(&self, line: &str, state: &mut LexContinuation) -> Vec<TokenSpan> {
        let incoming = match state {
            LexContinuation::Html(s) => s.clone(),
            _ => HtmlState::default(),
        };
        let (next, roles) = html_lex_line(line, incoming);
        *state = LexContinuation::Html(next);
        roles_to_spans(roles)
    }
}

impl LexicalProvider for CssLexicalProvider {
    fn language(&self) -> LanguageId {
        LanguageId::Css
    }
    fn lexer_version(&self) -> u32 {
        CSS_LEXER_VERSION
    }
    fn initial(&self) -> LexContinuation {
        LexContinuation::Css(CssState::default())
    }
    fn lex_line(&self, line: &str, state: &mut LexContinuation) -> Vec<TokenSpan> {
        let incoming = match state {
            LexContinuation::Css(s) => s.clone(),
            _ => CssState::default(),
        };
        let (next, roles) = css_lex_line(line, incoming);
        *state = LexContinuation::Css(next);
        roles_to_spans(roles)
    }
}

impl LexicalProvider for JavaLexicalProvider {
    fn language(&self) -> LanguageId {
        LanguageId::Java
    }
    fn lexer_version(&self) -> u32 {
        JAVA_LEXER_VERSION
    }
    fn initial(&self) -> LexContinuation {
        LexContinuation::Java(JavaState::default())
    }
    fn lex_line(&self, line: &str, state: &mut LexContinuation) -> Vec<TokenSpan> {
        let incoming = match state {
            LexContinuation::Java(s) => s.clone(),
            _ => JavaState::default(),
        };
        let (next, roles) = java_lex_line(line, incoming);
        *state = LexContinuation::Java(next);
        roles_to_spans(roles)
    }
}

impl LexicalProvider for SplashLexicalProvider {
    fn language(&self) -> LanguageId {
        LanguageId::Splash
    }
    fn lexer_version(&self) -> u32 {
        SPLASH_LEXER_VERSION
    }
    fn initial(&self) -> LexContinuation {
        LexContinuation::Splash(SplashState::default())
    }
    fn lex_line(&self, line: &str, state: &mut LexContinuation) -> Vec<TokenSpan> {
        let incoming = match state {
            LexContinuation::Splash(s) => s.clone(),
            _ => SplashState::default(),
        };
        let (next, roles) = splash_lex_line(line, incoming);
        *state = LexContinuation::Splash(next);
        roles_to_spans(roles)
    }
}

impl LexicalProvider for MarkdownLexicalProvider {
    fn language(&self) -> LanguageId {
        LanguageId::Markdown
    }
    fn lexer_version(&self) -> u32 {
        MARKDOWN_LEXER_VERSION
    }
    fn initial(&self) -> LexContinuation {
        LexContinuation::Markdown(MarkdownState::default())
    }
    fn lex_line(&self, line: &str, state: &mut LexContinuation) -> Vec<TokenSpan> {
        let incoming = match state {
            LexContinuation::Markdown(s) => s.clone(),
            _ => MarkdownState::default(),
        };
        let (next, roles) = markdown_lex_line(line, incoming);
        *state = LexContinuation::Markdown(next);
        roles_to_spans(roles)
    }
}

impl LexicalProvider for SqlLexicalProvider {
    fn language(&self) -> LanguageId {
        LanguageId::Sql
    }
    fn lexer_version(&self) -> u32 {
        SQL_LEXER_VERSION
    }
    fn initial(&self) -> LexContinuation {
        LexContinuation::Sql(SqlState::default())
    }
    fn initial_for_dialect(&self, dialect: Dialect) -> LexContinuation {
        LexContinuation::Sql(SqlState::for_dialect(SqlDialect::from_dialect(dialect)))
    }
    fn lex_line(&self, line: &str, state: &mut LexContinuation) -> Vec<TokenSpan> {
        let incoming = match state {
            LexContinuation::Sql(s) => s.clone(),
            _ => match self.initial() {
                LexContinuation::Sql(s) => s,
                _ => SqlState::default(),
            },
        };
        let (next, roles) = sql_lex_line(line, incoming);
        *state = LexContinuation::Sql(next);
        roles_to_spans(roles)
    }
}

impl LexicalProvider for TomlLexicalProvider {
    fn language(&self) -> LanguageId {
        LanguageId::Toml
    }
    fn lexer_version(&self) -> u32 {
        TOML_LEXER_VERSION
    }
    fn initial(&self) -> LexContinuation {
        LexContinuation::Toml(TomlState::default())
    }
    fn lex_line(&self, line: &str, state: &mut LexContinuation) -> Vec<TokenSpan> {
        let incoming = match state {
            LexContinuation::Toml(s) => s.clone(),
            _ => TomlState::default(),
        };
        let (next, roles) = toml_lex_line(line, incoming);
        *state = LexContinuation::Toml(next);
        roles_to_spans(roles)
    }
}

impl LexicalProvider for JsonLexicalProvider {
    fn language(&self) -> LanguageId {
        LanguageId::Json
    }
    fn lexer_version(&self) -> u32 {
        JSON_LEXER_VERSION
    }
    fn initial(&self) -> LexContinuation {
        LexContinuation::Json(JsonState::default())
    }
    fn lex_line(&self, line: &str, state: &mut LexContinuation) -> Vec<TokenSpan> {
        let incoming = match state {
            LexContinuation::Json(s) => s.clone(),
            _ => JsonState::default(),
        };
        let (next, roles) = json_lex_line(line, incoming);
        *state = LexContinuation::Json(next);
        roles_to_spans(roles)
    }
}

impl LexicalProvider for ShellLexicalProvider {
    fn language(&self) -> LanguageId {
        LanguageId::Shell
    }
    fn lexer_version(&self) -> u32 {
        SHELL_LEXER_VERSION
    }
    fn initial(&self) -> LexContinuation {
        self.initial_for_dialect(Dialect::default_for(LanguageId::Shell))
    }
    fn initial_for_dialect(&self, dialect: Dialect) -> LexContinuation {
        LexContinuation::Shell(ShellState::for_dialect(ShellDialect::from_dialect(dialect)))
    }
    fn lex_line(&self, line: &str, state: &mut LexContinuation) -> Vec<TokenSpan> {
        let incoming = match state {
            LexContinuation::Shell(s) => s.clone(),
            _ => match self.initial() {
                LexContinuation::Shell(s) => s,
                _ => ShellState::default(),
            },
        };
        let (next, roles) = shell_lex_line(line, incoming);
        *state = LexContinuation::Shell(next);
        roles_to_spans(roles)
    }
}

impl LexicalProvider for GoLexicalProvider {
    fn language(&self) -> LanguageId {
        LanguageId::Go
    }
    fn lexer_version(&self) -> u32 {
        GO_LEXER_VERSION
    }
    fn initial(&self) -> LexContinuation {
        LexContinuation::Go(GoState::default())
    }
    fn lex_line(&self, line: &str, state: &mut LexContinuation) -> Vec<TokenSpan> {
        let incoming = match state {
            LexContinuation::Go(s) => s.clone(),
            _ => GoState::default(),
        };
        let (next, roles) = go_lex_line(line, incoming);
        *state = LexContinuation::Go(next);
        roles_to_spans(roles)
    }
}

impl LexicalProvider for PhpLexicalProvider {
    fn language(&self) -> LanguageId {
        LanguageId::Php
    }
    fn lexer_version(&self) -> u32 {
        PHP_LEXER_VERSION
    }
    fn initial(&self) -> LexContinuation {
        LexContinuation::Php(PhpState::default())
    }
    fn lex_line(&self, line: &str, state: &mut LexContinuation) -> Vec<TokenSpan> {
        let incoming = match state {
            LexContinuation::Php(s) => s.clone(),
            _ => PhpState::default(),
        };
        let (next, roles) = php_lex_line(line, incoming);
        *state = LexContinuation::Php(next);
        roles_to_spans(roles)
    }
}

impl LexicalProvider for KotlinLexicalProvider {
    fn language(&self) -> LanguageId {
        LanguageId::Kotlin
    }
    fn lexer_version(&self) -> u32 {
        KOTLIN_LEXER_VERSION
    }
    fn initial(&self) -> LexContinuation {
        LexContinuation::Kotlin(KotlinState::default())
    }
    fn lex_line(&self, line: &str, state: &mut LexContinuation) -> Vec<TokenSpan> {
        let incoming = match state {
            LexContinuation::Kotlin(s) => s.clone(),
            _ => KotlinState::default(),
        };
        let (next, roles) = kotlin_lex_line(line, incoming);
        *state = LexContinuation::Kotlin(next);
        roles_to_spans(roles)
    }
}

impl LexicalProvider for DartLexicalProvider {
    fn language(&self) -> LanguageId {
        LanguageId::Dart
    }
    fn lexer_version(&self) -> u32 {
        DART_LEXER_VERSION
    }
    fn initial(&self) -> LexContinuation {
        LexContinuation::Dart(DartState::default())
    }
    fn initial_for_dialect(&self, dialect: Dialect) -> LexContinuation {
        if dialect.name == "pubspec" {
            LexContinuation::Dart(DartState::plain())
        } else {
            LexContinuation::Dart(DartState::default())
        }
    }
    fn lex_line(&self, line: &str, state: &mut LexContinuation) -> Vec<TokenSpan> {
        let incoming = match state {
            LexContinuation::Dart(s) => s.clone(),
            _ => DartState::default(),
        };
        let (next, roles) = dart_lex_line(line, incoming);
        *state = LexContinuation::Dart(next);
        roles_to_spans(roles)
    }
}

impl LexicalProvider for SwiftLexicalProvider {
    fn language(&self) -> LanguageId {
        LanguageId::Swift
    }
    fn lexer_version(&self) -> u32 {
        SWIFT_LEXER_VERSION
    }
    fn initial(&self) -> LexContinuation {
        LexContinuation::Swift(SwiftState::default())
    }
    fn lex_line(&self, line: &str, state: &mut LexContinuation) -> Vec<TokenSpan> {
        let incoming = match state {
            LexContinuation::Swift(s) => s.clone(),
            _ => SwiftState::default(),
        };
        let (next, roles) = swift_lex_line(line, incoming);
        *state = LexContinuation::Swift(next);
        roles_to_spans(roles)
    }
}

impl LexicalProvider for RubyLexicalProvider {
    fn language(&self) -> LanguageId {
        LanguageId::Ruby
    }
    fn lexer_version(&self) -> u32 {
        RUBY_LEXER_VERSION
    }
    fn initial(&self) -> LexContinuation {
        LexContinuation::Ruby(RubyState::default())
    }
    fn lex_line(&self, line: &str, state: &mut LexContinuation) -> Vec<TokenSpan> {
        let incoming = match state {
            LexContinuation::Ruby(s) => s.clone(),
            _ => RubyState::default(),
        };
        let (next, roles) = ruby_lex_line(line, incoming);
        *state = LexContinuation::Ruby(next);
        roles_to_spans(roles)
    }
}

impl LexicalProvider for FSharpLexicalProvider {
    fn language(&self) -> LanguageId {
        LanguageId::FSharp
    }
    fn lexer_version(&self) -> u32 {
        FSHARP_LEXER_VERSION
    }
    fn initial(&self) -> LexContinuation {
        LexContinuation::FSharp(FSharpState::default())
    }
    fn lex_line(&self, line: &str, state: &mut LexContinuation) -> Vec<TokenSpan> {
        let incoming = match state {
            LexContinuation::FSharp(s) => s.clone(),
            _ => FSharpState::default(),
        };
        let (next, roles) = fsharp_lex_line(line, incoming);
        *state = LexContinuation::FSharp(next);
        roles_to_spans(roles)
    }
}

impl LexicalProvider for ZigLexicalProvider {
    fn language(&self) -> LanguageId {
        LanguageId::Zig
    }
    fn lexer_version(&self) -> u32 {
        ZIG_LEXER_VERSION
    }
    fn initial(&self) -> LexContinuation {
        LexContinuation::Zig(ZigState::default())
    }
    fn lex_line(&self, line: &str, state: &mut LexContinuation) -> Vec<TokenSpan> {
        let incoming = match state {
            LexContinuation::Zig(s) => s.clone(),
            _ => ZigState::default(),
        };
        let (next, roles) = zig_lex_line(line, incoming);
        *state = LexContinuation::Zig(next);
        roles_to_spans(roles)
    }
}

impl LexicalProvider for HaskellLexicalProvider {
    fn language(&self) -> LanguageId {
        LanguageId::Haskell
    }
    fn lexer_version(&self) -> u32 {
        HASKELL_LEXER_VERSION
    }
    fn initial(&self) -> LexContinuation {
        self.initial_for_dialect(Dialect::default_for(LanguageId::Haskell))
    }
    fn initial_for_dialect(&self, dialect: Dialect) -> LexContinuation {
        if dialect.name == "literate" {
            LexContinuation::Haskell(HaskellState::literate())
        } else {
            LexContinuation::Haskell(HaskellState::default())
        }
    }
    fn lex_line(&self, line: &str, state: &mut LexContinuation) -> Vec<TokenSpan> {
        let incoming = match state {
            LexContinuation::Haskell(s) => s.clone(),
            _ => match self.initial() {
                LexContinuation::Haskell(s) => s,
                _ => HaskellState::default(),
            },
        };
        let (next, roles) = haskell_lex_line(line, incoming);
        *state = LexContinuation::Haskell(next);
        roles_to_spans(roles)
    }
}

impl LexicalProvider for MarkupLexicalProvider {
    fn language(&self) -> LanguageId {
        self.language
    }
    fn lexer_version(&self) -> u32 {
        HTML_LEXER_VERSION
    }
    fn initial(&self) -> LexContinuation {
        LexContinuation::Html(HtmlState::for_dialect(self.dialect))
    }
    fn lex_line(&self, line: &str, state: &mut LexContinuation) -> Vec<TokenSpan> {
        let incoming = match state {
            LexContinuation::Html(s) => s.clone(),
            _ => HtmlState::for_dialect(self.dialect),
        };
        let (next, roles) = html_lex_line(line, incoming);
        *state = LexContinuation::Html(next);
        roles_to_spans(roles)
    }
}

fn roles_to_spans(roles: Vec<(usize, TokenRole)>) -> Vec<TokenSpan> {
    let mut out = Vec::with_capacity(roles.len());
    let mut at = 0u32;
    for (end, role) in roles {
        let end = end as u32;
        if end > at {
            out.push(TokenSpan::new(at, end, role));
            at = end;
        }
    }
    out
}

static PLAIN: PlainTextProvider = PlainTextProvider;
static CPP: CppLexicalProvider = CppLexicalProvider;
static SCRIPT_JS: ScriptLexicalProvider = ScriptLexicalProvider {
    language: LanguageId::JavaScript,
    jsx_default: true,
};
static SCRIPT_TS: ScriptLexicalProvider = ScriptLexicalProvider {
    language: LanguageId::TypeScript,
    jsx_default: false,
};
static PYTHON: PythonLexicalProvider = PythonLexicalProvider;
static CSHARP: CSharpLexicalProvider = CSharpLexicalProvider;
static HTML: HtmlLexicalProvider = HtmlLexicalProvider;
static CSS: CssLexicalProvider = CssLexicalProvider;
static JAVA: JavaLexicalProvider = JavaLexicalProvider;
static SPLASH: SplashLexicalProvider = SplashLexicalProvider;
static MARKDOWN: MarkdownLexicalProvider = MarkdownLexicalProvider;
static SQL: SqlLexicalProvider = SqlLexicalProvider;
static TOML: TomlLexicalProvider = TomlLexicalProvider;
static JSON: JsonLexicalProvider = JsonLexicalProvider;
static SHELL: ShellLexicalProvider = ShellLexicalProvider;
static GO: GoLexicalProvider = GoLexicalProvider;
static PHP: PhpLexicalProvider = PhpLexicalProvider;
static KOTLIN: KotlinLexicalProvider = KotlinLexicalProvider;
static DART: DartLexicalProvider = DartLexicalProvider;
static SWIFT: SwiftLexicalProvider = SwiftLexicalProvider;
static RUBY: RubyLexicalProvider = RubyLexicalProvider;
static FSHARP: FSharpLexicalProvider = FSharpLexicalProvider;
static ZIG: ZigLexicalProvider = ZigLexicalProvider;
static HASKELL: HaskellLexicalProvider = HaskellLexicalProvider;
static XML: MarkupLexicalProvider = MarkupLexicalProvider {
    language: LanguageId::Xml,
    dialect: MarkupDialect::Xml,
};
static SVG: MarkupLexicalProvider = MarkupLexicalProvider {
    language: LanguageId::Svg,
    dialect: MarkupDialect::Svg,
};

/// Lexical provider for a language. Rust keeps its richer editor lexer;
/// every other language, including unknown, uses this table. Path-aware
/// callers must not default to Rust.
pub fn lexical_provider(language: LanguageId) -> &'static dyn LexicalProvider {
    if is_cpp_family(language) {
        &CPP
    } else if language == LanguageId::JavaScript {
        &SCRIPT_JS
    } else if language == LanguageId::TypeScript {
        &SCRIPT_TS
    } else if language == LanguageId::Python {
        &PYTHON
    } else if language == LanguageId::CSharp {
        &CSHARP
    } else if language == LanguageId::Html {
        &HTML
    } else if language == LanguageId::Css {
        &CSS
    } else if language == LanguageId::Java {
        &JAVA
    } else if language == LanguageId::Splash {
        &SPLASH
    } else if language == LanguageId::Xml {
        &XML
    } else if language == LanguageId::Svg {
        &SVG
    } else if language == LanguageId::Markdown {
        &MARKDOWN
    } else if language == LanguageId::Sql {
        &SQL
    } else if language == LanguageId::Toml {
        &TOML
    } else if language == LanguageId::Json {
        &JSON
    } else if language == LanguageId::Shell {
        &SHELL
    } else if language == LanguageId::Go {
        &GO
    } else if language == LanguageId::Php {
        &PHP
    } else if language == LanguageId::Kotlin {
        &KOTLIN
    } else if language == LanguageId::Dart {
        &DART
    } else if language == LanguageId::Swift {
        &SWIFT
    } else if language == LanguageId::Ruby {
        &RUBY
    } else if language == LanguageId::FSharp {
        &FSHARP
    } else if language == LanguageId::Zig {
        &ZIG
    } else if language == LanguageId::Haskell {
        &HASKELL
    } else {
        &PLAIN
    }
}

/// Lexer schema stored in durable search/cache keys. Rust's schema lives with
/// the private analyser so this crate does not depend on it.
pub fn lexer_schema(language: LanguageId) -> u32 {
    if is_cpp_family(language) {
        CPP_LEXER_VERSION
    } else if is_script_family(language) {
        SCRIPT_LEXER_VERSION
    } else if language == LanguageId::Python {
        PYTHON_LEXER_VERSION
    } else if language == LanguageId::CSharp {
        CSHARP_LEXER_VERSION
    } else if language == LanguageId::Html {
        HTML_LEXER_VERSION
    } else if language == LanguageId::Css {
        CSS_LEXER_VERSION
    } else if language == LanguageId::Java {
        JAVA_LEXER_VERSION
    } else if language == LanguageId::Splash {
        SPLASH_LEXER_VERSION
    } else if language == LanguageId::Xml || language == LanguageId::Svg {
        HTML_LEXER_VERSION
    } else if language == LanguageId::Markdown {
        MARKDOWN_LEXER_VERSION
    } else if language == LanguageId::Sql {
        SQL_LEXER_VERSION
    } else if language == LanguageId::Toml {
        TOML_LEXER_VERSION
    } else if language == LanguageId::Json {
        JSON_LEXER_VERSION
    } else if language == LanguageId::Shell {
        SHELL_LEXER_VERSION
    } else if language == LanguageId::Go {
        GO_LEXER_VERSION
    } else if language == LanguageId::Php {
        PHP_LEXER_VERSION
    } else if language == LanguageId::Kotlin {
        KOTLIN_LEXER_VERSION
    } else if language == LanguageId::Dart {
        DART_LEXER_VERSION
    } else if language == LanguageId::Swift {
        SWIFT_LEXER_VERSION
    } else if language == LanguageId::Ruby {
        RUBY_LEXER_VERSION
    } else if language == LanguageId::FSharp {
        FSHARP_LEXER_VERSION
    } else if language == LanguageId::Zig {
        ZIG_LEXER_VERSION
    } else if language == LanguageId::Haskell {
        HASKELL_LEXER_VERSION
    } else if language == LanguageId::Rust {
        0
    } else {
        PLAIN_LEXER_VERSION
    }
}

/// Detect then pick a provider. Unknown paths get plain text.
pub fn provider_for_path(path: &str, bytes: Option<&[u8]>) -> &'static dyn LexicalProvider {
    lexical_provider(detect_path(path, bytes).language)
}

/// Provider plus dialect-aware initial continuation for a finished detection.
pub fn provider_for_detection(
    detection: Detection,
) -> (&'static dyn LexicalProvider, LexContinuation) {
    let provider = lexical_provider(detection.language);
    let state = provider.initial_for_dialect(detection.dialect);
    (provider, state)
}
