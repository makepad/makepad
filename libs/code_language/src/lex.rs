//! Public lexical providers. Continuation state is provider-owned.
//!
//! The editor stores one slot per line of `LexContinuation`. Adding a language
//! registers a provider here; it does not add a parallel state array in the
//! editor. Unknown languages use the plain-text provider, never Rust.

use crate::cpp_lex::{lex_line as cpp_lex_line, CppState, CPP_LEXER_VERSION};
use crate::id::LanguageId;
use crate::token::{TokenRole, TokenSpan};
use crate::{detect_path, is_cpp_family};

/// Version of the plain-text lexer (byte-run classification only).
pub const PLAIN_LEXER_VERSION: u32 = 1;

/// Provider-owned line continuation. The editor holds this enum only.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum LexContinuation {
    #[default]
    Plain,
    Cpp(CppState),
}

pub trait LexicalProvider: Send + Sync {
    fn language(&self) -> LanguageId;
    fn lexer_version(&self) -> u32;
    fn initial(&self) -> LexContinuation;
    /// Tokens with offsets relative to `line`.
    fn lex_line(&self, line: &str, state: &mut LexContinuation) -> Vec<TokenSpan>;
}

pub struct PlainTextProvider;
pub struct CppLexicalProvider;

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
}

static PLAIN: PlainTextProvider = PlainTextProvider;
static CPP: CppLexicalProvider = CppLexicalProvider;

/// Lexical provider for a language. Rust keeps its richer editor lexer;
/// every other language, including unknown and Python inventory, uses this
/// table. Path-aware callers must not default to Rust.
pub fn lexical_provider(language: LanguageId) -> &'static dyn LexicalProvider {
    if is_cpp_family(language) {
        &CPP
    } else {
        &PLAIN
    }
}

/// Lexer schema stored in durable search/cache keys. Rust's schema lives with
/// the private analyser so this crate does not depend on it.
pub fn lexer_schema(language: LanguageId) -> u32 {
    if is_cpp_family(language) {
        CPP_LEXER_VERSION
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
