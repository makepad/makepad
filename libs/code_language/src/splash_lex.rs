//! Source-only Splash lexer shared by the editor and the text frontend.
//!
//! Token grammar is copied from `platform/script/src/tokenizer.rs`
//! (`ScriptTokenizer::tokenize`). That tokenizer is char-indexed, retains the
//! whole original text, interns identifiers as `LiveId` on a `ScriptHeap`, and
//! has no cancellation or `Eq`/`Hash` continuation, so it cannot serve as a
//! leaf byte lexer. This module keeps the same classification decisions as a
//! cancellable byte lexer with per-line continuation.
//!
//! Keywords are the parser reserved set listed in
//! `platform/script/src/parser.rs` (the highlighting subset named in the
//! text-formats design). `#(...)` bodies are not lexed as Rust.

use crate::cpp_lex::LexError;
use crate::token::{TokenRole, TokenSpan};

/// Version of this lexer; part of parse and search cache identity.
pub const SPLASH_LEXER_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
enum SplashMode {
    #[default]
    Normal,
    BlockComment,
    String,
}

/// Provider-owned line continuation. Default is Normal.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct SplashState {
    mode: SplashMode,
    /// Nesting depth of `/* */` comments. Capped at 255.
    comment_depth: u8,
    /// `true` = `"..."`, `false` = `'...'`. Meaningful in `String` mode.
    string_double: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplashKind {
    Whitespace,
    Comment,
    Identifier,
    Keyword,
    Number,
    String,
    Color,
    RustValue,
    Punctuator,
    Delimiter,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SplashToken {
    pub start: u32,
    pub end: u32,
    pub kind: SplashKind,
}

impl SplashKind {
    pub fn role(self) -> TokenRole {
        match self {
            SplashKind::Whitespace => TokenRole::Whitespace,
            SplashKind::Comment => TokenRole::Comment,
            SplashKind::Identifier => TokenRole::Identifier,
            SplashKind::Keyword => TokenRole::Keyword,
            SplashKind::Number => TokenRole::Number,
            SplashKind::String => TokenRole::String,
            SplashKind::Color => TokenRole::Constant,
            SplashKind::RustValue => TokenRole::Preprocessor,
            SplashKind::Punctuator => TokenRole::Punctuator,
            SplashKind::Delimiter => TokenRole::Delimiter,
            SplashKind::Unknown => TokenRole::Unknown,
        }
    }
}

/// Parser reserved words that this lexer highlights. Sorted for binary search.
const KEYWORDS: &[&str] = &[
    "and", "break", "continue", "do", "else", "false", "fn", "for", "if", "in", "is", "let", "loop",
    "match", "nil", "or", "return", "self", "true", "try", "use", "var", "while",
];

fn is_keyword(ident: &str) -> bool {
    KEYWORDS.binary_search(&ident).is_ok()
}

fn classify_identifier(ident: &str, next_is_paren: bool) -> TokenRole {
    match ident {
        "if" | "else" | "match" | "return" | "try" => TokenRole::BranchKeyword,
        "for" | "in" | "while" | "loop" | "break" | "continue" => TokenRole::LoopKeyword,
        "true" | "false" | "nil" => TokenRole::Constant,
        other if is_keyword(other) => TokenRole::Keyword,
        _ if next_is_paren => TokenRole::Function,
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

fn is_operator_char(b: u8) -> bool {
    matches!(
        b,
        b'!' | b'^' | b'&' | b'*' | b'+' | b'-' | b'|' | b'?' | b':' | b'=' | b'@' | b'>' | b'<'
            | b'.' | b'/' | b'~' | b'%'
    )
}

fn is_hex(b: u8) -> bool {
    b.is_ascii_hexdigit()
}

/// Advance one UTF-8 scalar if `bytes[i]` starts a valid sequence; otherwise
/// one byte. Never splits a valid scalar.
fn bump_char(bytes: &[u8], i: usize) -> usize {
    let Some(&b) = bytes.get(i) else {
        return i;
    };
    let want = if b < 0x80 {
        1
    } else if b & 0xe0 == 0xc0 {
        2
    } else if b & 0xf0 == 0xe0 {
        3
    } else if b & 0xf8 == 0xf0 {
        4
    } else {
        1
    };
    if i + want <= bytes.len() && std::str::from_utf8(&bytes[i..i + want]).is_ok() {
        i + want
    } else {
        i + 1
    }
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

/// Longest valid operator at `i`. Comments (`//` `/*`) are not operators.
fn operator_len(bytes: &[u8], i: usize) -> usize {
    let rest = &bytes[i..];
    if rest.starts_with(b"===")
        || rest.starts_with(b"!==")
        || rest.starts_with(b"<<=")
        || rest.starts_with(b">>=")
        || rest.starts_with(b"...")
    {
        return 3;
    }
    if rest.len() >= 2 {
        match &rest[..2] {
            b"==" | b"!=" | b"<=" | b">=" | b"&&" | b"||" | b"|?" | b"+=" | b"-=" | b"*="
            | b"/=" | b"%=" | b"&=" | b"|=" | b"^=" | b":=" | b"<<" | b">>" | b".." | b"->"
            | b".?" | b">:" | b"<:" | b"^:" | b"+:" | b"?=" | b"++" | b"-:" | b"=>" => {
                return 2;
            }
            b"/*" | b"//" => return 0,
            _ => {}
        }
    }
    if rest.first().copied().map(is_operator_char).unwrap_or(false) {
        1
    } else {
        0
    }
}

fn consume_string_escape(bytes: &[u8], i: &mut usize) {
    if bytes.get(*i) != Some(&b'\\') {
        return;
    }
    *i += 1;
    let Some(&c) = bytes.get(*i) else {
        return;
    };
    if is_newline(c) {
        return;
    }
    match c {
        b'x' => {
            *i += 1;
            let mut n = 0;
            while n < 2 && bytes.get(*i).copied().map(is_hex).unwrap_or(false) {
                *i += 1;
                n += 1;
            }
        }
        b'u' => {
            *i += 1;
            if bytes.get(*i) == Some(&b'{') {
                *i += 1;
                while *i < bytes.len() && bytes[*i] != b'}' && !is_newline(bytes[*i]) {
                    *i += 1;
                }
                if bytes.get(*i) == Some(&b'}') {
                    *i += 1;
                }
            } else {
                let mut n = 0;
                while n < 4 && bytes.get(*i).copied().map(is_hex).unwrap_or(false) {
                    *i += 1;
                    n += 1;
                }
            }
        }
        _ => {
            *i += 1;
        }
    }
}

fn scan_string_body(bytes: &[u8], i: &mut usize, double: bool) -> bool {
    let quote = if double { b'"' } else { b'\'' };
    while *i < bytes.len() {
        let c = bytes[*i];
        if c == b'\\' {
            consume_string_escape(bytes, i);
            continue;
        }
        if c == quote {
            *i += 1;
            return true;
        }
        *i += 1;
    }
    false
}

fn scan_block_comment(bytes: &[u8], i: &mut usize, state: &mut SplashState) -> bool {
    while *i < bytes.len() {
        if bytes[*i] == b'/' && bytes.get(*i + 1) == Some(&b'*') {
            *i += 2;
            if state.comment_depth < 255 {
                state.comment_depth = state.comment_depth.saturating_add(1);
            }
            continue;
        }
        if bytes[*i] == b'*' && bytes.get(*i + 1) == Some(&b'/') {
            *i += 2;
            state.comment_depth = state.comment_depth.saturating_sub(1);
            if state.comment_depth == 0 {
                state.mode = SplashMode::Normal;
                return true;
            }
            continue;
        }
        *i += 1;
    }
    false
}

/// `#(` ... matching `)`. Balanced parens; strings inside respected; stops at
/// the line terminator. Unclosed runs to line end (newline not consumed).
fn scan_hash_paren(bytes: &[u8], i: &mut usize) {
    let mut depth: u32 = 1;
    let mut in_string = false;
    let mut double = false;
    while *i < bytes.len() {
        let c = bytes[*i];
        if is_newline(c) {
            return;
        }
        if in_string {
            if c == b'\\' {
                consume_string_escape(bytes, i);
                continue;
            }
            let q = if double { b'"' } else { b'\'' };
            if c == q {
                in_string = false;
            }
            *i += 1;
            continue;
        }
        match c {
            b'"' => {
                in_string = true;
                double = true;
                *i += 1;
            }
            b'\'' => {
                in_string = true;
                double = false;
                *i += 1;
            }
            b'(' => {
                depth = depth.saturating_add(1);
                *i += 1;
            }
            b')' => {
                *i += 1;
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return;
                }
            }
            _ => {
                *i += 1;
            }
        }
    }
}

fn scan_number(bytes: &[u8], i: &mut usize) {
    let start = *i;
    let mut hex = false;
    let mut saw_dot = false;
    let mut saw_exp = false;
    if bytes.get(*i) == Some(&b'.') {
        saw_dot = true;
        *i += 1;
    }
    while *i < bytes.len() {
        let c = bytes[*i];
        if c == b'_' {
            *i += 1;
            continue;
        }
        if hex {
            if c.is_ascii_hexdigit() {
                *i += 1;
                continue;
            }
            break;
        }
        if c.is_ascii_digit() {
            *i += 1;
            continue;
        }
        if (c == b'x' || c == b'X')
            && !saw_dot
            && !saw_exp
            && *i == start + 1
            && bytes.get(start) == Some(&b'0')
        {
            hex = true;
            *i += 1;
            continue;
        }
        if c == b'.' {
            if bytes.get(*i + 1) == Some(&b'.') {
                break;
            }
            if saw_dot || saw_exp {
                break;
            }
            saw_dot = true;
            *i += 1;
            continue;
        }
        if (c == b'e' || c == b'E') && !saw_exp && !hex {
            let mut j = *i + 1;
            if matches!(bytes.get(j), Some(&b'+' | &b'-')) {
                j += 1;
            }
            saw_exp = true;
            *i = j;
            continue;
        }
        break;
    }
    if matches!(bytes.get(*i), Some(&b'f' | &b'u' | &b'i' | &b'h')) {
        *i += 1;
    }
}

fn consume_ident(bytes: &[u8], i: &mut usize) {
    *i = bump_char(bytes, *i);
    while *i < bytes.len() && is_ident_continue(bytes[*i]) {
        *i = bump_char(bytes, *i);
    }
}

fn next_is_paren(bytes: &[u8], mut i: usize) -> bool {
    while i < bytes.len() && is_space(bytes[i]) {
        i += 1;
    }
    bytes.get(i) == Some(&b'(')
}

fn ident_kind(bytes: &[u8], start: usize, end: usize) -> SplashKind {
    let ident = std::str::from_utf8(&bytes[start..end]).unwrap_or("");
    if is_keyword(ident) {
        SplashKind::Keyword
    } else {
        SplashKind::Identifier
    }
}

fn lex_one(
    bytes: &[u8],
    i: &mut usize,
    state: &mut SplashState,
) -> Result<SplashToken, LexError> {
    let start = *i as u32;
    if *i >= bytes.len() {
        return Err(LexError::Nonprogress { at: start });
    }

    match state.mode {
        SplashMode::BlockComment => {
            let _ = scan_block_comment(bytes, i, state);
            if (*i as u32) <= start {
                return Err(LexError::Nonprogress { at: start });
            }
            return Ok(SplashToken {
                start,
                end: *i as u32,
                kind: SplashKind::Comment,
            });
        }
        SplashMode::String => {
            let closed = scan_string_body(bytes, i, state.string_double);
            if closed {
                state.mode = SplashMode::Normal;
            }
            if (*i as u32) <= start {
                return Err(LexError::Nonprogress { at: start });
            }
            return Ok(SplashToken {
                start,
                end: *i as u32,
                kind: SplashKind::String,
            });
        }
        SplashMode::Normal => {}
    }

    let b = bytes[*i];
    if b == b'\r' {
        *i += 1;
        if bytes.get(*i) == Some(&b'\n') {
            *i += 1;
        }
        return Ok(SplashToken {
            start,
            end: *i as u32,
            kind: SplashKind::Whitespace,
        });
    }
    if b == b'\n' {
        *i += 1;
        return Ok(SplashToken {
            start,
            end: *i as u32,
            kind: SplashKind::Whitespace,
        });
    }
    if is_space(b) {
        *i += 1;
        while *i < bytes.len() && is_space(bytes[*i]) {
            *i += 1;
        }
        return Ok(SplashToken {
            start,
            end: *i as u32,
            kind: SplashKind::Whitespace,
        });
    }

    if b == b'/' && bytes.get(*i + 1) == Some(&b'/') {
        *i += 2;
        while *i < bytes.len() && !is_newline(bytes[*i]) {
            *i += 1;
        }
        return Ok(SplashToken {
            start,
            end: *i as u32,
            kind: SplashKind::Comment,
        });
    }
    if b == b'/' && bytes.get(*i + 1) == Some(&b'*') {
        *i += 2;
        state.mode = SplashMode::BlockComment;
        state.comment_depth = 1;
        let _ = scan_block_comment(bytes, i, state);
        if (*i as u32) <= start {
            return Err(LexError::Nonprogress { at: start });
        }
        return Ok(SplashToken {
            start,
            end: *i as u32,
            kind: SplashKind::Comment,
        });
    }

    if b == b'#' && bytes.get(*i + 1) == Some(&b'(') {
        *i += 2;
        scan_hash_paren(bytes, i);
        return Ok(SplashToken {
            start,
            end: *i as u32,
            kind: SplashKind::RustValue,
        });
    }
    if b == b'#' {
        *i += 1;
        if bytes.get(*i) == Some(&b'x') || bytes.get(*i) == Some(&b'X') {
            *i += 1;
        }
        let mut n = 0u8;
        while n < 8 && bytes.get(*i).copied().map(is_hex).unwrap_or(false) {
            *i += 1;
            n += 1;
        }
        if (*i as u32) <= start {
            return Err(LexError::Nonprogress { at: start });
        }
        return Ok(SplashToken {
            start,
            end: *i as u32,
            kind: SplashKind::Color,
        });
    }

    if b == b'@' && bytes.get(*i + 1) == Some(&b'(') {
        *i += 2;
        while *i < bytes.len() && bytes[*i].is_ascii_digit() {
            *i += 1;
        }
        if bytes.get(*i) == Some(&b')') {
            *i += 1;
        }
        return Ok(SplashToken {
            start,
            end: *i as u32,
            kind: SplashKind::RustValue,
        });
    }

    if b == b'"' || b == b'\'' {
        state.string_double = b == b'"';
        *i += 1;
        let closed = scan_string_body(bytes, i, state.string_double);
        if closed {
            state.mode = SplashMode::Normal;
        } else {
            state.mode = SplashMode::String;
        }
        return Ok(SplashToken {
            start,
            end: *i as u32,
            kind: SplashKind::String,
        });
    }

    if b.is_ascii_digit() || (b == b'.' && bytes.get(*i + 1).map(|c| c.is_ascii_digit()).unwrap_or(false))
    {
        scan_number(bytes, i);
        return Ok(SplashToken {
            start,
            end: *i as u32,
            kind: SplashKind::Number,
        });
    }

    if is_ident_start(b) {
        consume_ident(bytes, i);
        let kind = ident_kind(bytes, start as usize, *i);
        return Ok(SplashToken {
            start,
            end: *i as u32,
            kind,
        });
    }

    match b {
        b'(' | b')' | b'{' | b'}' | b'[' | b']' => {
            *i += 1;
            return Ok(SplashToken {
                start,
                end: *i as u32,
                kind: SplashKind::Delimiter,
            });
        }
        b',' | b';' => {
            *i += 1;
            return Ok(SplashToken {
                start,
                end: *i as u32,
                kind: SplashKind::Punctuator,
            });
        }
        _ => {}
    }

    let n = operator_len(bytes, *i);
    if n > 0 {
        *i += n;
        return Ok(SplashToken {
            start,
            end: *i as u32,
            kind: SplashKind::Punctuator,
        });
    }

    *i = bump_char(bytes, *i);
    if (*i as u32) <= start {
        *i = start as usize + 1;
    }
    Ok(SplashToken {
        start,
        end: *i as u32,
        kind: SplashKind::Unknown,
    })
}

fn token_role(bytes: &[u8], t: SplashToken, next_paren: bool) -> TokenRole {
    if t.kind == SplashKind::Keyword || t.kind == SplashKind::Identifier {
        let ident = std::str::from_utf8(&bytes[t.start as usize..t.end as usize]).unwrap_or("");
        classify_identifier(ident, next_paren)
    } else {
        t.kind.role()
    }
}

/// Tokenize an entire document. Spans are byte offsets into `bytes`.
pub fn lex_document_cancellable(
    bytes: &[u8],
    cancel: &dyn Fn() -> bool,
) -> Result<Vec<SplashToken>, LexError> {
    let mut tokens = Vec::new();
    let mut state = SplashState::default();
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
            Err(e) => return Err(e),
        }
        if i == start_i {
            return Err(LexError::Nonprogress { at: i as u32 });
        }
    }
    Ok(tokens)
}

/// Tokenize one display line (without the newline) given incoming continuation.
/// Each pair is `(end_index, role)` covering `[prev_end, end)` of `line`.
pub fn lex_line(line: &str, incoming: SplashState) -> (SplashState, Vec<(usize, TokenRole)>) {
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
                    let next_paren = next_is_paren(bytes, end);
                    let role = token_role(bytes, t, next_paren);
                    push_role(end, role, &mut out);
                }
                i = end.max(i);
            }
            Err(LexError::Nonprogress { .. }) => {
                if i == start && i < bytes.len() {
                    i += 1;
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
    if i < bytes.len() {
        let role = match state.mode {
            SplashMode::BlockComment => TokenRole::Comment,
            SplashMode::String => TokenRole::String,
            SplashMode::Normal => TokenRole::Unknown,
        };
        push_role(bytes.len(), role, &mut out);
    }
    (state, out)
}

/// Convert document tokens into public spans, classifying keywords and
/// function names with one-token lookahead.
pub fn document_spans(bytes: &[u8], tokens: &[SplashToken]) -> Vec<TokenSpan> {
    let mut out = Vec::with_capacity(tokens.len());
    for (i, t) in tokens.iter().enumerate() {
        let next_paren = if t.kind == SplashKind::Keyword || t.kind == SplashKind::Identifier {
            let mut j = i + 1;
            while j < tokens.len() && tokens[j].kind.role().is_trivia() {
                j += 1;
            }
            j < tokens.len()
                && tokens[j].kind == SplashKind::Delimiter
                && bytes.get(tokens[j].start as usize) == Some(&b'(')
        } else {
            false
        };
        out.push(TokenSpan::new(t.start, t.end, token_role(bytes, *t, next_paren)));
    }
    out
}
