//! Source-only Dart lexer shared by the editor and the Dart frontend.
//! Byte offsets address the original document. Continuation state is owned
//! here: nested block comments and triple-quoted strings survive line breaks.
//! Single-quoted and double-quoted strings are line-bounded except that
//! `${}` interpolations inside a spanning (triple-quoted) host persist.
//!
//! Block comments nest (`/* /* */ */`). String interpolation `$name` and
//! `${...}` split the surrounding string into Identifier / Punctuator spans;
//! nested strings inside a template that contain `${` beyond depth 2 stay
//! String. Dart requires semicolons: there are no Newline tokens.

use crate::token::{TokenRole, TokenSpan};
use crate::LexError;

/// Version of this lexer; part of parse and search cache identity.
pub const DART_LEXER_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
enum DartMode {
    #[default]
    Normal,
    /// One token per line; used for the `pubspec` dialect (`pubspec.yaml`).
    /// YAML is not tokenised: Comment / Whitespace / Identifier only.
    Plain,
    BlockComment {
        depth: u8,
    },
    /// `'...'` / `"..."` / `'''...'''` / `"""..."""`, including raw.
    MultiString {
        quote: u8,
        raw: bool,
        triple: bool,
    },
    /// `${...}` of an outer string.
    Interp {
        quote: u8,
        raw: bool,
        triple: bool,
        braces: u8,
    },
    /// String nested in [`DartMode::Interp`].
    NestStr {
        quote: u8,
        raw: bool,
        triple: bool,
        host_quote: u8,
        host_raw: bool,
        host_triple: bool,
        outer_braces: u8,
    },
    /// `${...}` of a nested string. Strings inside this do not split on `${`.
    Interp2 {
        quote: u8,
        raw: bool,
        triple: bool,
        host_quote: u8,
        host_raw: bool,
        host_triple: bool,
        braces: u8,
        outer_braces: u8,
    },
    /// String nested in [`DartMode::Interp2`]; `${` stays in the string.
    DeepStr {
        quote: u8,
        raw: bool,
        triple: bool,
        nest_quote: u8,
        nest_raw: bool,
        nest_triple: bool,
        host_quote: u8,
        host_raw: bool,
        host_triple: bool,
        braces: u8,
        outer_braces: u8,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct DartState {
    mode: DartMode,
}

impl DartState {
    /// Line-oriented mode for `pubspec.yaml`. Token identity of `.dart`
    /// files is unchanged (`DART_LEXER_VERSION` stays 1).
    pub fn plain() -> Self {
        DartState {
            mode: DartMode::Plain,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DartKind {
    Whitespace,
    Comment,
    Identifier,
    Keyword,
    Annotation,
    Number,
    String,
    Punctuator,
    Delimiter,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DartToken {
    pub start: u32,
    pub end: u32,
    pub kind: DartKind,
}

impl DartKind {
    pub fn role(self) -> TokenRole {
        match self {
            DartKind::Whitespace => TokenRole::Whitespace,
            DartKind::Comment => TokenRole::Comment,
            DartKind::Identifier => TokenRole::Identifier,
            DartKind::Keyword => TokenRole::Keyword,
            DartKind::Annotation => TokenRole::Preprocessor,
            DartKind::Number => TokenRole::Number,
            DartKind::String => TokenRole::String,
            DartKind::Punctuator => TokenRole::Punctuator,
            DartKind::Delimiter => TokenRole::Delimiter,
            DartKind::Unknown => TokenRole::Unknown,
        }
    }
}

/// Reserved words. Sorted. Built-in identifiers and contextual keywords stay
/// Identifier; the parser decides.
const RESERVED: &[&str] = &[
    "assert", "break", "case", "catch", "class", "const", "continue", "default", "do", "else",
    "enum", "extends", "false", "final", "finally", "for", "if", "in", "is", "new", "null",
    "rethrow", "return", "super", "switch", "this", "throw", "true", "try", "var", "void",
    "while", "with",
];

/// Built-in identifiers and contextual keywords. Sorted. Stay Identifier.
const BUILTIN: &[&str] = &[
    "Function",
    "abstract",
    "as",
    "async",
    "await",
    "base",
    "covariant",
    "deferred",
    "dynamic",
    "export",
    "extension",
    "external",
    "factory",
    "get",
    "hide",
    "implements",
    "import",
    "interface",
    "late",
    "library",
    "mixin",
    "of",
    "on",
    "operator",
    "part",
    "required",
    "sealed",
    "set",
    "show",
    "static",
    "sync",
    "typedef",
    "when",
    "yield",
];

/// Built-in identifiers used as declaration keywords. Role Keyword when
/// followed by an identifier or keyword on the same line.
const DECL_KEYWORDS: &[&str] = &[
    "abstract",
    "covariant",
    "export",
    "extension",
    "external",
    "factory",
    "final",
    "get",
    "import",
    "late",
    "library",
    "mixin",
    "operator",
    "part",
    "required",
    "set",
    "static",
    "typedef",
];

const TYPE_WORDS: &[&str] = &[
    "Function", "Never", "Null", "Object", "String", "bool", "double", "dynamic", "int", "num",
    "var", "void",
];

pub fn is_reserved_word(ident: &str) -> bool {
    RESERVED.binary_search(&ident).is_ok()
}

pub fn is_builtin_identifier(ident: &str) -> bool {
    BUILTIN.binary_search(&ident).is_ok()
}

fn is_decl_keyword(ident: &str) -> bool {
    DECL_KEYWORDS.binary_search(&ident).is_ok()
}

fn is_type_word(ident: &str) -> bool {
    TYPE_WORDS.binary_search(&ident).is_ok()
}

fn classify_identifier(ident: &str, next_is_call: bool, next_is_name: bool) -> TokenRole {
    match ident {
        "if" | "else" | "switch" | "case" | "default" | "try" | "catch" | "finally" | "return"
        | "throw" | "rethrow" | "assert" => TokenRole::BranchKeyword,
        "for" | "while" | "do" | "break" | "continue" => TokenRole::LoopKeyword,
        "true" | "false" | "null" => TokenRole::Constant,
        other if is_type_word(other) && next_is_name && !next_is_call => TokenRole::Typename,
        other if is_reserved_word(other) => TokenRole::Keyword,
        other if is_decl_keyword(other) && next_is_name => TokenRole::Keyword,
        _ if next_is_call => TokenRole::Function,
        _ if ident
            .chars()
            .next()
            .map(|c| c.is_uppercase())
            .unwrap_or(false) =>
        {
            TokenRole::Typename
        }
        _ => TokenRole::Identifier,
    }
}

fn is_space(b: u8) -> bool {
    b == b' ' || b == b'\t' || b == 0x0b || b == 0x0c
}

fn is_newline(b: u8) -> bool {
    b == b'\n' || b == b'\r'
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || b == b'$' || b >= 0x80
}

fn is_ident_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$' || b >= 0x80
}

/// Interpolation names cannot contain `$`.
fn is_interp_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || b >= 0x80
}

fn is_interp_ident_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80
}

fn tok(start: u32, end: usize, kind: DartKind) -> DartToken {
    DartToken {
        start,
        end: end as u32,
        kind,
    }
}

fn push_token(
    tokens: &mut Vec<DartToken>,
    start: u32,
    end: usize,
    kind: DartKind,
) -> Result<(), LexError> {
    if end as u32 <= start {
        return Err(LexError::Nonprogress { at: start });
    }
    tokens.push(tok(start, end, kind));
    Ok(())
}

fn push_role(end: usize, role: TokenRole, out: &mut Vec<(usize, TokenRole)>) {
    if end == 0 {
        return;
    }
    if out.last().map(|(_, r)| *r) == Some(role) {
        out.last_mut().unwrap().0 = end;
    } else {
        out.push((end, role));
    }
}

fn consume_string_escape(bytes: &[u8], i: &mut usize) {
    if *i >= bytes.len() {
        return;
    }
    if is_newline(bytes[*i]) {
        return;
    }
    let b = bytes[*i];
    *i += 1;
    match b {
        b'x' => {
            let mut n = 0;
            while *i < bytes.len() && n < 2 && bytes[*i].is_ascii_hexdigit() {
                *i += 1;
                n += 1;
            }
        }
        b'u' => {
            if bytes.get(*i) == Some(&b'{') {
                *i += 1;
                while *i < bytes.len() && bytes[*i] != b'}' && !is_newline(bytes[*i]) {
                    if bytes[*i].is_ascii_hexdigit() {
                        *i += 1;
                    } else {
                        break;
                    }
                }
                if bytes.get(*i) == Some(&b'}') {
                    *i += 1;
                }
            } else {
                let mut n = 0;
                while *i < bytes.len() && n < 4 && bytes[*i].is_ascii_hexdigit() {
                    *i += 1;
                    n += 1;
                }
            }
        }
        _ => {}
    }
}

fn looks_like_triple(bytes: &[u8], i: usize, quote: u8) -> bool {
    bytes.get(i) == Some(&quote)
        && bytes.get(i + 1) == Some(&quote)
        && bytes.get(i + 2) == Some(&quote)
}

fn interpolation_ident_end(bytes: &[u8], dollar: usize) -> Option<usize> {
    if bytes.get(dollar) != Some(&b'$') {
        return None;
    }
    let next = bytes.get(dollar + 1).copied().unwrap_or(0);
    if next == b'{' {
        return None;
    }
    if !is_interp_ident_start(next) {
        return None;
    }
    let mut j = dollar + 1;
    j += 1;
    while j < bytes.len() && is_interp_ident_continue(bytes[j]) {
        j += 1;
    }
    if j > dollar + 1 {
        Some(j)
    } else {
        None
    }
}

fn scan_number(bytes: &[u8], mut i: usize) -> usize {
    if bytes[i] == b'0' && matches!(bytes.get(i + 1).map(|b| b.to_ascii_lowercase()), Some(b'x')) {
        i += 2;
        while i < bytes.len() && (bytes[i].is_ascii_hexdigit() || bytes[i] == b'_') {
            i += 1;
        }
        return i;
    }
    while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'_') {
        i += 1;
    }
    if bytes.get(i) == Some(&b'.') {
        let next = bytes.get(i + 1).copied().unwrap_or(0);
        // `.5` is not a number; `1.` is `1` then `.`. A fraction continues
        // only when a digit follows the dot.
        if next.is_ascii_digit() {
            i += 1;
            while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'_') {
                i += 1;
            }
        }
    }
    consume_exponent(bytes, i)
}

fn consume_exponent(bytes: &[u8], mut i: usize) -> usize {
    if bytes.get(i).map(|b| b.to_ascii_lowercase()) == Some(b'e') {
        let mut j = i + 1;
        if matches!(bytes.get(j), Some(&b'+' | &b'-')) {
            j += 1;
        }
        if j < bytes.len() && bytes[j].is_ascii_digit() {
            i = j + 1;
            while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'_') {
                i += 1;
            }
        }
    }
    i
}

fn punct_len(bytes: &[u8], i: usize) -> usize {
    let rest = &bytes[i..];
    if rest.starts_with(b">>>=") {
        return 4;
    }
    if rest.starts_with(b"?..")
        || rest.starts_with(b"...?")
        || rest.starts_with(b">>>")
        || rest.starts_with(b">>=")
        || rest.starts_with(b"<<=")
        || rest.starts_with(b"??=")
        || rest.starts_with(b"~/=")
        || rest.starts_with(b"...")
    {
        return 3;
    }
    if matches!(
        rest.get(..2),
        Some(
            b"?." | b".."
                | b"??"
                | b"=>"
                | b">>"
                | b"<<"
                | b"=="
                | b"!="
                | b"<="
                | b">="
                | b"&&"
                | b"||"
                | b"++"
                | b"--"
                | b"+="
                | b"-="
                | b"*="
                | b"/="
                | b"~/"
                | b"%="
                | b"&="
                | b"|="
                | b"^="
        )
    ) {
        return 2;
    }
    1
}

#[derive(Clone, Copy)]
struct StringRestore {
    quote: u8,
    raw: bool,
    triple: bool,
    host_quote: u8,
    host_raw: bool,
    host_triple: bool,
    nest_quote: u8,
    nest_raw: bool,
    nest_triple: bool,
    braces: u8,
    outer_braces: u8,
}

fn string_context(mode: DartMode) -> Option<(u8, bool, bool, u8, StringRestore, bool)> {
    match mode {
        DartMode::MultiString {
            quote,
            raw,
            triple,
        } => Some((
            quote,
            raw,
            triple,
            0,
            StringRestore {
                quote,
                raw,
                triple,
                host_quote: quote,
                host_raw: raw,
                host_triple: triple,
                nest_quote: quote,
                nest_raw: raw,
                nest_triple: triple,
                braces: 0,
                outer_braces: 0,
            },
            true,
        )),
        DartMode::NestStr {
            quote,
            raw,
            triple,
            host_quote,
            host_raw,
            host_triple,
            outer_braces,
        } => Some((
            quote,
            raw,
            triple,
            1,
            StringRestore {
                quote,
                raw,
                triple,
                host_quote,
                host_raw,
                host_triple,
                nest_quote: quote,
                nest_raw: raw,
                nest_triple: triple,
                braces: 0,
                outer_braces,
            },
            true,
        )),
        DartMode::DeepStr {
            quote,
            raw,
            triple,
            nest_quote,
            nest_raw,
            nest_triple,
            host_quote,
            host_raw,
            host_triple,
            braces,
            outer_braces,
        } => Some((
            quote,
            raw,
            triple,
            2,
            StringRestore {
                quote,
                raw,
                triple,
                host_quote,
                host_raw,
                host_triple,
                nest_quote,
                nest_raw,
                nest_triple,
                braces,
                outer_braces,
            },
            false,
        )),
        _ => None,
    }
}

fn close_string_parent(state: &mut DartState, nest: u8, restore: StringRestore) {
    match nest {
        0 => state.mode = DartMode::Normal,
        1 => {
            state.mode = DartMode::Interp {
                quote: restore.host_quote,
                raw: restore.host_raw,
                triple: restore.host_triple,
                braces: restore.outer_braces,
            };
        }
        _ => {
            state.mode = DartMode::Interp2 {
                quote: restore.nest_quote,
                raw: restore.nest_raw,
                triple: restore.nest_triple,
                host_quote: restore.host_quote,
                host_raw: restore.host_raw,
                host_triple: restore.host_triple,
                braces: restore.braces,
                outer_braces: restore.outer_braces,
            };
        }
    }
}

fn open_template(state: &mut DartState, nest: u8, restore: StringRestore) {
    if nest == 0 {
        state.mode = DartMode::Interp {
            quote: restore.quote,
            raw: restore.raw,
            triple: restore.triple,
            braces: 0,
        };
    } else {
        state.mode = DartMode::Interp2 {
            quote: restore.quote,
            raw: restore.raw,
            triple: restore.triple,
            host_quote: restore.host_quote,
            host_raw: restore.host_raw,
            host_triple: restore.host_triple,
            braces: 0,
            outer_braces: restore.outer_braces,
        };
    }
}

fn enter_nested_string(state: &mut DartState, quote: u8, raw: bool, triple: bool) {
    match state.mode {
        DartMode::Interp {
            quote: host_quote,
            raw: host_raw,
            triple: host_triple,
            braces,
        } => {
            state.mode = DartMode::NestStr {
                quote,
                raw,
                triple,
                host_quote,
                host_raw,
                host_triple,
                outer_braces: braces,
            };
        }
        DartMode::Interp2 {
            quote: nest_quote,
            raw: nest_raw,
            triple: nest_triple,
            host_quote,
            host_raw,
            host_triple,
            braces,
            outer_braces,
        } => {
            state.mode = DartMode::DeepStr {
                quote,
                raw,
                triple,
                nest_quote,
                nest_raw,
                nest_triple,
                host_quote,
                host_raw,
                host_triple,
                braces,
                outer_braces,
            };
        }
        _ => {
            state.mode = DartMode::MultiString {
                quote,
                raw,
                triple,
            };
        }
    }
}

fn scan_string_token(
    bytes: &[u8],
    i: &mut usize,
    state: &mut DartState,
    token_start: u32,
) -> Result<DartToken, LexError> {
    let start_i = *i;
    let start = token_start;
    let Some((quote, raw, triple, nest, restore, allow_interp)) = string_context(state.mode) else {
        return Err(LexError::Nonprogress { at: start });
    };

    if allow_interp && (*i as u32) == start {
        if let Some(end) = interpolation_ident_end(bytes, *i) {
            *i = end;
            return Ok(tok(start, *i, DartKind::Identifier));
        }
        if bytes.get(*i) == Some(&b'$') && bytes.get(*i + 1) == Some(&b'{') {
            *i += 2;
            open_template(state, nest, restore);
            return Ok(tok(start, *i, DartKind::Punctuator));
        }
    }

    if triple {
        if looks_like_triple(bytes, *i, quote) {
            *i += 3;
            close_string_parent(state, nest, restore);
            return Ok(tok(start, *i, DartKind::String));
        }
    } else if bytes.get(*i) == Some(&quote) {
        *i += 1;
        close_string_parent(state, nest, restore);
        return Ok(tok(start, *i, DartKind::String));
    }

    while *i < bytes.len() {
        if !triple && is_newline(bytes[*i]) {
            if *i == start_i {
                return Err(LexError::Nonprogress { at: start });
            }
            close_string_parent(state, nest, restore);
            return Ok(tok(start, *i, DartKind::String));
        }
        if allow_interp && !raw {
            if interpolation_ident_end(bytes, *i).is_some()
                || (bytes.get(*i) == Some(&b'$') && bytes.get(*i + 1) == Some(&b'{'))
            {
                if *i == start_i {
                    break;
                }
                return Ok(tok(start, *i, DartKind::String));
            }
        }
        if triple {
            if looks_like_triple(bytes, *i, quote) {
                *i += 3;
                close_string_parent(state, nest, restore);
                return Ok(tok(start, *i, DartKind::String));
            }
            if !raw && bytes[*i] == b'\\' {
                *i += 1;
                consume_string_escape(bytes, i);
                continue;
            }
            *i += 1;
            continue;
        }
        let b = bytes[*i];
        if !raw && b == b'\\' {
            *i += 1;
            consume_string_escape(bytes, i);
            continue;
        }
        if b == quote {
            *i += 1;
            close_string_parent(state, nest, restore);
            return Ok(tok(start, *i, DartKind::String));
        }
        *i += 1;
    }
    if *i as u32 <= start {
        if *i < bytes.len() {
            *i += 1;
        } else {
            return Err(LexError::Nonprogress { at: start });
        }
    }
    Ok(tok(start, *i, DartKind::String))
}

fn template_mode(mode: DartMode) -> bool {
    matches!(mode, DartMode::Interp { .. } | DartMode::Interp2 { .. })
}

fn close_template(state: &mut DartState) {
    match state.mode {
        DartMode::Interp {
            quote,
            raw,
            triple,
            ..
        } => {
            state.mode = DartMode::MultiString {
                quote,
                raw,
                triple,
            };
        }
        DartMode::Interp2 {
            quote,
            raw,
            triple,
            host_quote,
            host_raw,
            host_triple,
            outer_braces,
            ..
        } => {
            state.mode = DartMode::NestStr {
                quote,
                raw,
                triple,
                host_quote,
                host_raw,
                host_triple,
                outer_braces,
            };
        }
        other => state.mode = other,
    }
}

fn inc_template_brace(state: &mut DartState) {
    match &mut state.mode {
        DartMode::Interp { braces, .. } | DartMode::Interp2 { braces, .. } => {
            *braces = braces.saturating_add(1);
        }
        _ => {}
    }
}

fn dec_template_brace(state: &mut DartState) -> bool {
    match &mut state.mode {
        DartMode::Interp { braces, .. } | DartMode::Interp2 { braces, .. } => {
            if *braces == 0 {
                return true;
            }
            *braces -= 1;
            false
        }
        _ => false,
    }
}

fn consume_string_opener(bytes: &[u8], i: &mut usize) -> Option<(u8, bool, bool)> {
    let mut raw = false;
    let mut at = *i;
    if bytes.get(at) == Some(&b'r')
        && matches!(bytes.get(at + 1), Some(&b'\'' | &b'"'))
    {
        raw = true;
        at += 1;
    }
    let quote = match bytes.get(at).copied() {
        Some(q) if q == b'\'' || q == b'"' => q,
        _ => return None,
    };
    let triple = looks_like_triple(bytes, at, quote);
    if triple {
        *i = at + 3;
    } else {
        *i = at + 1;
    }
    Some((quote, raw, triple))
}

/// Shared automaton step. Emits one token covering `[start, *i)`.
fn lex_one(
    bytes: &[u8],
    i: &mut usize,
    state: &mut DartState,
) -> Result<DartToken, LexError> {
    let start_i = *i;
    let start = *i as u32;
    if *i >= bytes.len() {
        return Err(LexError::Nonprogress { at: start });
    }

    if let DartMode::BlockComment { depth } = state.mode {
        let mut depth = depth;
        while *i < bytes.len() {
            if bytes[*i] == b'/' && bytes.get(*i + 1) == Some(&b'*') {
                depth = depth.saturating_add(1);
                *i += 2;
                continue;
            }
            if bytes[*i] == b'*' && bytes.get(*i + 1) == Some(&b'/') {
                *i += 2;
                if depth <= 1 {
                    state.mode = DartMode::Normal;
                    break;
                }
                depth -= 1;
                continue;
            }
            *i += 1;
        }
        if matches!(state.mode, DartMode::BlockComment { .. }) {
            state.mode = DartMode::BlockComment { depth };
        }
        if (*i as u32) <= start {
            return Err(LexError::Nonprogress { at: start });
        }
        return Ok(tok(start, *i, DartKind::Comment));
    }

    if string_context(state.mode).is_some() {
        return scan_string_token(bytes, i, state, start);
    }

    if template_mode(state.mode) {
        let b = bytes[*i];
        if b == b'}' {
            *i += 1;
            if dec_template_brace(state) {
                close_template(state);
            }
            return Ok(tok(start, *i, DartKind::Punctuator));
        }
        if b == b'{' {
            *i += 1;
            inc_template_brace(state);
            return Ok(tok(start, *i, DartKind::Delimiter));
        }
        if b == b'\'' || b == b'"' || (b == b'r' && matches!(bytes.get(*i + 1), Some(&b'\'' | &b'"')))
        {
            if let Some((quote, raw, triple)) = consume_string_opener(bytes, i) {
                enter_nested_string(state, quote, raw, triple);
                return scan_string_token(bytes, i, state, start);
            }
        }
        // Fall through to Normal-like lexing of the interpolation body.
    }

    let b = bytes[*i];
    if b == b'\r' {
        *i += 1;
        if bytes.get(*i) == Some(&b'\n') {
            *i += 1;
        }
        return Ok(tok(start, *i, DartKind::Whitespace));
    }
    if b == b'\n' {
        *i += 1;
        return Ok(tok(start, *i, DartKind::Whitespace));
    }
    if is_space(b) {
        *i += 1;
        while *i < bytes.len() && is_space(bytes[*i]) {
            *i += 1;
        }
        return Ok(tok(start, *i, DartKind::Whitespace));
    }
    if b == b'/' && bytes.get(*i + 1) == Some(&b'/') {
        *i += 2;
        while *i < bytes.len() && !is_newline(bytes[*i]) {
            *i += 1;
        }
        return Ok(tok(start, *i, DartKind::Comment));
    }
    if b == b'/' && bytes.get(*i + 1) == Some(&b'*') {
        *i += 2;
        let mut depth = 1u8;
        state.mode = DartMode::BlockComment { depth };
        while *i < bytes.len() {
            if bytes[*i] == b'/' && bytes.get(*i + 1) == Some(&b'*') {
                depth = depth.saturating_add(1);
                *i += 2;
                state.mode = DartMode::BlockComment { depth };
                continue;
            }
            if bytes[*i] == b'*' && bytes.get(*i + 1) == Some(&b'/') {
                *i += 2;
                if depth <= 1 {
                    state.mode = DartMode::Normal;
                    break;
                }
                depth -= 1;
                state.mode = DartMode::BlockComment { depth };
                continue;
            }
            *i += 1;
        }
        if matches!(state.mode, DartMode::BlockComment { .. }) {
            state.mode = DartMode::BlockComment { depth };
        }
        return Ok(tok(start, *i, DartKind::Comment));
    }
    if b == b'\''
        || b == b'"'
        || (b == b'r' && matches!(bytes.get(*i + 1), Some(&b'\'' | &b'"')))
    {
        if let Some((quote, raw, triple)) = consume_string_opener(bytes, i) {
            enter_nested_string(state, quote, raw, triple);
            return scan_string_token(bytes, i, state, start);
        }
    }
    if b == b'@' {
        let next = bytes.get(*i + 1).copied().unwrap_or(0);
        if is_ident_start(next) {
            *i += 1;
            while *i < bytes.len() && is_ident_continue(bytes[*i]) {
                *i += 1;
            }
            return Ok(tok(start, *i, DartKind::Annotation));
        }
        *i += 1;
        return Ok(tok(start, *i, DartKind::Punctuator));
    }
    if is_ident_start(b) {
        *i += 1;
        while *i < bytes.len() && is_ident_continue(bytes[*i]) {
            *i += 1;
        }
        let ident = std::str::from_utf8(&bytes[start as usize..*i]).unwrap_or("");
        let kind = if is_reserved_word(ident) {
            DartKind::Keyword
        } else {
            DartKind::Identifier
        };
        return Ok(tok(start, *i, kind));
    }
    if b.is_ascii_digit() {
        *i = scan_number(bytes, *i);
        return Ok(tok(start, *i, DartKind::Number));
    }
    let n = punct_len(bytes, *i);
    *i += n;
    let kind = if n == 1 && matches!(b, b'(' | b')' | b'{' | b'}' | b'[' | b']') {
        DartKind::Delimiter
    } else if b.is_ascii_graphic() {
        DartKind::Punctuator
    } else {
        DartKind::Unknown
    };
    if *i == start_i {
        return Err(LexError::Nonprogress { at: start });
    }
    Ok(tok(start, *i, kind))
}

fn reset_line_bounded(state: &mut DartState) {
    match state.mode {
        DartMode::MultiString { triple, .. } if !triple => {
            state.mode = DartMode::Normal;
        }
        DartMode::NestStr {
            triple,
            host_quote,
            host_raw,
            host_triple,
            outer_braces,
            ..
        } if !triple => {
            state.mode = DartMode::Interp {
                quote: host_quote,
                raw: host_raw,
                triple: host_triple,
                braces: outer_braces,
            };
        }
        DartMode::DeepStr {
            triple,
            nest_quote,
            nest_raw,
            nest_triple,
            host_quote,
            host_raw,
            host_triple,
            braces,
            outer_braces,
            ..
        } if !triple => {
            state.mode = DartMode::Interp2 {
                quote: nest_quote,
                raw: nest_raw,
                triple: nest_triple,
                host_quote,
                host_raw,
                host_triple,
                braces,
                outer_braces,
            };
        }
        DartMode::Interp { triple, .. } if !triple => {
            state.mode = DartMode::Normal;
        }
        DartMode::Interp2 {
            host_quote,
            host_raw,
            host_triple,
            outer_braces,
            ..
        } if !host_triple => {
            state.mode = DartMode::Interp {
                quote: host_quote,
                raw: host_raw,
                triple: host_triple,
                braces: outer_braces,
            };
        }
        _ => {}
    }
}

/// Tokenize an entire document. Spans are byte offsets into `bytes`.
pub fn lex_document_cancellable(
    bytes: &[u8],
    cancel: &dyn Fn() -> bool,
) -> Result<Vec<DartToken>, LexError> {
    lex_document_from(bytes, DartState::default(), cancel)
}

/// Tokenize an entire document from `state`. [`DartMode::Plain`] emits one
/// token per line (Comment / Whitespace / Identifier).
pub fn lex_document_from(
    bytes: &[u8],
    mut state: DartState,
    cancel: &dyn Fn() -> bool,
) -> Result<Vec<DartToken>, LexError> {
    if matches!(state.mode, DartMode::Plain) {
        return lex_document_plain(bytes, cancel);
    }
    let mut tokens = Vec::new();
    let mut i = 0usize;
    let token_cap = bytes.len().saturating_add(8);
    while i < bytes.len() {
        if tokens.len() & 255 == 0 && cancel() {
            return Err(LexError::Cancelled);
        }
        if tokens.len() > token_cap {
            return Err(LexError::Nonprogress { at: i as u32 });
        }
        let start_i = i;
        match lex_one(bytes, &mut i, &mut state) {
            Ok(t) => {
                if t.end <= t.start {
                    return Err(LexError::Nonprogress { at: t.start });
                }
                tokens.push(t);
            }
            Err(LexError::Nonprogress { at }) => {
                if i == start_i && i < bytes.len() {
                    i += 1;
                    push_token(&mut tokens, at, i, DartKind::Unknown)?;
                    continue;
                }
                return Err(LexError::Nonprogress { at });
            }
            Err(e) => return Err(e),
        }
        if i == start_i {
            return Err(LexError::Nonprogress { at: i as u32 });
        }
    }
    Ok(tokens)
}

fn plain_line_kind(line: &[u8]) -> DartKind {
    let text = std::str::from_utf8(line).unwrap_or("");
    let trimmed = text.trim();
    if trimmed.is_empty() {
        DartKind::Whitespace
    } else if trimmed.starts_with('#') {
        DartKind::Comment
    } else {
        DartKind::Identifier
    }
}

fn lex_document_plain(
    bytes: &[u8],
    cancel: &dyn Fn() -> bool,
) -> Result<Vec<DartToken>, LexError> {
    let mut tokens = Vec::new();
    let mut i = 0usize;
    let token_cap = bytes.len().saturating_add(8);
    while i < bytes.len() {
        if tokens.len() & 255 == 0 && cancel() {
            return Err(LexError::Cancelled);
        }
        if tokens.len() > token_cap {
            return Err(LexError::Nonprogress { at: i as u32 });
        }
        let start = i;
        while i < bytes.len() && bytes[i] != b'\n' {
            i += 1;
        }
        let content_end = i;
        if i < bytes.len() && bytes[i] == b'\n' {
            i += 1;
        }
        if i <= start {
            return Err(LexError::Nonprogress { at: start as u32 });
        }
        let kind = plain_line_kind(&bytes[start..content_end]);
        tokens.push(DartToken {
            start: start as u32,
            end: i as u32,
            kind,
        });
    }
    Ok(tokens)
}

fn next_is_call_bytes(bytes: &[u8], end: usize) -> bool {
    let mut k = end;
    while k < bytes.len() && is_space(bytes[k]) {
        k += 1;
    }
    bytes.get(k) == Some(&b'(')
}

fn next_is_name_bytes(bytes: &[u8], end: usize) -> bool {
    let mut k = end;
    while k < bytes.len() && is_space(bytes[k]) {
        k += 1;
    }
    if k >= bytes.len() || is_newline(bytes[k]) {
        return false;
    }
    if !is_ident_start(bytes[k]) {
        return false;
    }
    true
}

fn ws_is_newline(bytes: &[u8], t: DartToken) -> bool {
    bytes
        .get(t.start as usize..t.end as usize)
        .is_some_and(|s| s.iter().any(|&b| is_newline(b)))
}

/// Tokenize one display line (without the newline) given incoming continuation.
/// Each pair is `(end_index, role)` covering `[prev_end, end)` of `line`.
pub fn lex_line(line: &str, incoming: DartState) -> (DartState, Vec<(usize, TokenRole)>) {
    if matches!(incoming.mode, DartMode::Plain) {
        if line.is_empty() {
            return (incoming, Vec::new());
        }
        let trimmed = line.trim();
        let role = if trimmed.is_empty() {
            TokenRole::Whitespace
        } else if trimmed.starts_with('#') {
            TokenRole::Comment
        } else {
            TokenRole::Identifier
        };
        return (incoming, vec![(line.len(), role)]);
    }
    let bytes = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    let mut state = incoming;
    while i < bytes.len() {
        let start = i;
        match lex_one(bytes, &mut i, &mut state) {
            Ok(t) => {
                let end = (t.end as usize).min(bytes.len()).max(start);
                if end > start {
                    let role = if matches!(t.kind, DartKind::Keyword | DartKind::Identifier) {
                        let ident = std::str::from_utf8(&bytes[t.start as usize..end]).unwrap_or("");
                        classify_identifier(
                            ident,
                            next_is_call_bytes(bytes, end),
                            next_is_name_bytes(bytes, end),
                        )
                    } else {
                        t.kind.role()
                    };
                    push_role(end, role, &mut out);
                }
                i = end.max(i);
            }
            Err(LexError::Nonprogress { .. }) => {
                if i == start && i < bytes.len() {
                    i += 1;
                    push_role(i, TokenRole::Unknown, &mut out);
                } else if i < bytes.len() {
                    i = bytes.len();
                    push_role(i, TokenRole::Unknown, &mut out);
                } else {
                    break;
                }
            }
            Err(_) => break,
        }
        if i == start {
            if i < bytes.len() {
                i += 1;
                push_role(i, TokenRole::Unknown, &mut out);
            } else {
                break;
            }
        }
    }
    reset_line_bounded(&mut state);
    if i < bytes.len() {
        let role = match state.mode {
            DartMode::BlockComment { .. } => TokenRole::Comment,
            DartMode::MultiString { .. }
            | DartMode::NestStr { .. }
            | DartMode::DeepStr { .. } => TokenRole::String,
            DartMode::Normal
            | DartMode::Plain
            | DartMode::Interp { .. }
            | DartMode::Interp2 { .. } => TokenRole::Unknown,
        };
        push_role(bytes.len(), role, &mut out);
    }
    (state, out)
}

fn same_line(bytes: &[u8], tokens: &[DartToken], a: usize, b: usize) -> bool {
    tokens[a + 1..b]
        .iter()
        .all(|t| t.kind != DartKind::Whitespace || !ws_is_newline(bytes, *t))
}

/// Convert document tokens into public spans, classifying keywords and
/// function names with one-token lookahead.
pub fn document_spans(bytes: &[u8], tokens: &[DartToken]) -> Vec<TokenSpan> {
    let mut out = Vec::with_capacity(tokens.len());
    for (i, t) in tokens.iter().enumerate() {
        let mut role = t.kind.role();
        if t.kind == DartKind::Keyword || t.kind == DartKind::Identifier {
            let mut j = i + 1;
            while j < tokens.len()
                && (tokens[j].kind == DartKind::Comment
                    || (tokens[j].kind == DartKind::Whitespace && !ws_is_newline(bytes, tokens[j])))
            {
                j += 1;
            }
            let next_is_call = j < tokens.len()
                && tokens[j].kind == DartKind::Delimiter
                && bytes.get(tokens[j].start as usize) == Some(&b'(')
                && same_line(bytes, tokens, i, j);
            let next_is_name = if j < tokens.len()
                && !(tokens[j].kind == DartKind::Whitespace && ws_is_newline(bytes, tokens[j]))
            {
                let ident =
                    std::str::from_utf8(&bytes[tokens[j].start as usize..tokens[j].end as usize])
                        .unwrap_or("");
                (tokens[j].kind == DartKind::Identifier || tokens[j].kind == DartKind::Keyword)
                    && same_line(bytes, tokens, i, j)
                    && (is_reserved_word(ident)
                        || is_builtin_identifier(ident)
                        || tokens[j].kind == DartKind::Identifier
                        || tokens[j].kind == DartKind::Keyword)
            } else {
                false
            };
            let ident = std::str::from_utf8(&bytes[t.start as usize..t.end as usize]).unwrap_or("");
            role = classify_identifier(ident, next_is_call, next_is_name);
        }
        out.push(TokenSpan::new(t.start, t.end, role));
    }
    out
}
