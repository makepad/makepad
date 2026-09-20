//! Source-only JSON / JSONC / JSON5 lexer shared by the editor and the text frontend.
//!
//! Tokens only; no schema or validity check. Comments are recognised in every
//! dialect. Malformed input is classified, never rejected.

use crate::cpp_lex::LexError;
use crate::token::{TokenRole, TokenSpan};

/// Version of this lexer; part of parse and search cache identity.
pub const JSON_LEXER_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
enum JsonMode {
    #[default]
    Normal,
    BlockComment,
}

/// Provider-owned line continuation. Block comments span lines; strings do not.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct JsonState {
    mode: JsonMode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JsonKind {
    Whitespace,
    Comment,
    Identifier,
    Number,
    String,
    Constant,
    Punctuator,
    Delimiter,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JsonToken {
    pub start: u32,
    pub end: u32,
    pub kind: JsonKind,
}

impl JsonKind {
    pub fn role(self) -> TokenRole {
        match self {
            JsonKind::Whitespace => TokenRole::Whitespace,
            JsonKind::Comment => TokenRole::Comment,
            JsonKind::Identifier => TokenRole::Identifier,
            JsonKind::Number => TokenRole::Number,
            JsonKind::String => TokenRole::String,
            JsonKind::Constant => TokenRole::Constant,
            JsonKind::Punctuator => TokenRole::Punctuator,
            JsonKind::Delimiter => TokenRole::Delimiter,
            JsonKind::Unknown => TokenRole::Unknown,
        }
    }
}

fn is_space(b: u8) -> bool {
    b == b' ' || b == b'\t' || b == 0x0b || b == 0x0c
}

fn is_newline(b: u8) -> bool {
    b == b'\n' || b == b'\r'
}

fn tok(start: u32, end: usize, kind: JsonKind) -> JsonToken {
    JsonToken {
        start,
        end: end as u32,
        kind,
    }
}

fn consume_newline(bytes: &[u8], i: &mut usize) {
    if bytes.get(*i) == Some(&b'\r') {
        *i += 1;
        if bytes.get(*i) == Some(&b'\n') {
            *i += 1;
        }
    } else if bytes.get(*i) == Some(&b'\n') {
        *i += 1;
    }
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || b == b'$'
}

fn is_ident_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$'
}

fn scan_block_comment(bytes: &[u8], i: &mut usize) -> bool {
    while *i < bytes.len() {
        if bytes[*i] == b'*' && bytes.get(*i + 1) == Some(&b'/') {
            *i += 2;
            return true;
        }
        *i += 1;
    }
    false
}

fn scan_line_comment(bytes: &[u8], i: &mut usize) {
    while *i < bytes.len() && !is_newline(bytes[*i]) {
        *i += 1;
    }
}

fn scan_string_line(bytes: &[u8], i: &mut usize, quote: u8) -> bool {
    while *i < bytes.len() {
        let c = bytes[*i];
        if is_newline(c) {
            return false;
        }
        if c == b'\\' {
            *i += 1;
            let Some(&n) = bytes.get(*i) else {
                return false;
            };
            if is_newline(n) {
                return false;
            }
            if n == b'u' {
                *i += 1;
                let mut k = 0;
                while k < 4 && bytes.get(*i).copied().map(|b| b.is_ascii_hexdigit()).unwrap_or(false)
                {
                    *i += 1;
                    k += 1;
                }
                continue;
            }
            *i += 1;
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

fn next_non_ws_is_colon(bytes: &[u8], mut i: usize) -> bool {
    while i < bytes.len() {
        let c = bytes[i];
        if is_newline(c) {
            return false;
        }
        if is_space(c) {
            i += 1;
            continue;
        }
        return c == b':';
    }
    false
}

fn scan_number(bytes: &[u8], i: &mut usize) {
    if matches!(bytes.get(*i), Some(&b'+' | &b'-')) {
        *i += 1;
    }
    if bytes.len().saturating_sub(*i) >= 8 && bytes[*i..*i + 8].eq_ignore_ascii_case(b"infinity") {
        *i += 8;
        return;
    }
    if bytes.len() - *i >= 3 && bytes[*i..*i + 3].eq_ignore_ascii_case(b"nan") {
        *i += 3;
        return;
    }
    if bytes.get(*i) == Some(&b'0') && matches!(bytes.get(*i + 1), Some(&b'x' | &b'X')) {
        *i += 2;
        while bytes.get(*i).copied().map(|b| b.is_ascii_hexdigit()).unwrap_or(false) {
            *i += 1;
        }
        return;
    }
    while bytes.get(*i).copied().map(|b| b.is_ascii_digit()).unwrap_or(false) {
        *i += 1;
    }
    if bytes.get(*i) == Some(&b'.') {
        *i += 1;
        while bytes.get(*i).copied().map(|b| b.is_ascii_digit()).unwrap_or(false) {
            *i += 1;
        }
    }
    if matches!(bytes.get(*i), Some(&b'e' | &b'E')) {
        let mut j = *i + 1;
        if matches!(bytes.get(j), Some(&b'+' | &b'-')) {
            j += 1;
        }
        if bytes.get(j).copied().map(|b| b.is_ascii_digit()).unwrap_or(false) {
            *i = j;
            while bytes.get(*i).copied().map(|b| b.is_ascii_digit()).unwrap_or(false) {
                *i += 1;
            }
        }
    }
}

fn lex_one(bytes: &[u8], i: &mut usize, state: &mut JsonState) -> Result<JsonToken, LexError> {
    let start = *i as u32;
    if *i >= bytes.len() {
        return Err(LexError::Nonprogress { at: start });
    }

    if state.mode == JsonMode::BlockComment {
        let closed = scan_block_comment(bytes, i);
        if closed {
            state.mode = JsonMode::Normal;
        }
        if (*i as u32) <= start {
            return Err(LexError::Nonprogress { at: start });
        }
        return Ok(tok(start, *i, JsonKind::Comment));
    }

    let b = bytes[*i];
    if is_newline(b) {
        consume_newline(bytes, i);
        return Ok(tok(start, *i, JsonKind::Whitespace));
    }
    if is_space(b) {
        *i += 1;
        while *i < bytes.len() && is_space(bytes[*i]) {
            *i += 1;
        }
        return Ok(tok(start, *i, JsonKind::Whitespace));
    }

    if b == b'/' && bytes.get(*i + 1) == Some(&b'/') {
        *i += 2;
        scan_line_comment(bytes, i);
        return Ok(tok(start, *i, JsonKind::Comment));
    }
    if b == b'/' && bytes.get(*i + 1) == Some(&b'*') {
        *i += 2;
        let closed = scan_block_comment(bytes, i);
        if !closed {
            state.mode = JsonMode::BlockComment;
        }
        if (*i as u32) <= start {
            return Err(LexError::Nonprogress { at: start });
        }
        return Ok(tok(start, *i, JsonKind::Comment));
    }

    if b == b'"' {
        *i += 1;
        let _ = scan_string_line(bytes, i, b'"');
        let kind = if next_non_ws_is_colon(bytes, *i) {
            JsonKind::Identifier
        } else {
            JsonKind::String
        };
        return Ok(tok(start, *i, kind));
    }
    if b == b'\'' {
        *i += 1;
        let _ = scan_string_line(bytes, i, b'\'');
        return Ok(tok(start, *i, JsonKind::String));
    }

    match b {
        b'{' | b'}' | b'[' | b']' | b',' => {
            *i += 1;
            return Ok(tok(start, *i, JsonKind::Delimiter));
        }
        b':' => {
            *i += 1;
            return Ok(tok(start, *i, JsonKind::Punctuator));
        }
        _ => {}
    }

    if b.is_ascii_digit() || matches!(b, b'+' | b'-') {
        scan_number(bytes, i);
        if (*i as u32) <= start {
            *i = start as usize + 1;
            return Ok(tok(start, *i, JsonKind::Unknown));
        }
        return Ok(tok(start, *i, JsonKind::Number));
    }

    if is_ident_start(b) {
        *i += 1;
        while *i < bytes.len() && is_ident_continue(bytes[*i]) {
            *i += 1;
        }
        let word = std::str::from_utf8(&bytes[start as usize..*i]).unwrap_or("");
        let kind = match word {
            "true" | "false" | "null" => JsonKind::Constant,
            _ if word.eq_ignore_ascii_case("infinity") || word.eq_ignore_ascii_case("nan") => {
                JsonKind::Number
            }
            _ => JsonKind::Identifier,
        };
        return Ok(tok(start, *i, kind));
    }

    *i += 1;
    Ok(tok(start, *i, JsonKind::Unknown))
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

/// Tokenize an entire document. Spans are byte offsets into `bytes`.
pub fn lex_document_cancellable(
    bytes: &[u8],
    cancel: &dyn Fn() -> bool,
) -> Result<Vec<JsonToken>, LexError> {
    let mut tokens = Vec::new();
    let mut state = JsonState::default();
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
pub fn lex_line(line: &str, incoming: JsonState) -> (JsonState, Vec<(usize, TokenRole)>) {
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
                    push_role(end, t.kind.role(), &mut out);
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
            JsonMode::BlockComment => TokenRole::Comment,
            JsonMode::Normal => TokenRole::Unknown,
        };
        push_role(bytes.len(), role, &mut out);
    }
    (state, out)
}

/// Convert document tokens into public spans.
pub fn document_spans(_bytes: &[u8], tokens: &[JsonToken]) -> Vec<TokenSpan> {
    tokens
        .iter()
        .map(|t| TokenSpan::new(t.start, t.end, t.kind.role()))
        .collect()
}
