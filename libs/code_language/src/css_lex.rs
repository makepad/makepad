//! CSS Syntax Level 3 tokenisation shared by the editor and the CSS frontend.
//! Byte offsets address the original document. Continuation state is owned
//! here so highlighting and parsing agree on comments, strings and `url()`.

use crate::token::{TokenRole, TokenSpan};
use crate::LexError;

/// Version of this lexer; part of parse and search cache identity.
pub const CSS_LEXER_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
enum CssMode {
    #[default]
    Normal,
    BlockComment,
    DoubleString,
    SingleString,
    /// Unquoted `url(` body, continued across lines until `)`.
    UnquotedUrl,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct CssState {
    mode: CssMode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CssKind {
    Whitespace,
    Comment,
    Ident,
    Function,
    AtKeyword,
    Hash,
    String,
    Url,
    Number,
    Dimension,
    Percentage,
    Delim,
    Colon,
    Semicolon,
    Comma,
    OpenParen,
    CloseParen,
    OpenBracket,
    CloseBracket,
    OpenBrace,
    CloseBrace,
    Cdo,
    Cdc,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CssToken {
    pub start: u32,
    pub end: u32,
    pub kind: CssKind,
}

impl CssKind {
    pub fn role(self) -> TokenRole {
        match self {
            CssKind::Whitespace => TokenRole::Whitespace,
            CssKind::Comment | CssKind::Cdo | CssKind::Cdc => TokenRole::Comment,
            CssKind::Ident | CssKind::Hash => TokenRole::Identifier,
            CssKind::Function => TokenRole::Function,
            CssKind::AtKeyword => TokenRole::Preprocessor,
            CssKind::String | CssKind::Url => TokenRole::String,
            CssKind::Number | CssKind::Dimension | CssKind::Percentage => TokenRole::Number,
            CssKind::Delim | CssKind::Colon | CssKind::Semicolon | CssKind::Comma => {
                TokenRole::Punctuator
            }
            CssKind::OpenParen
            | CssKind::CloseParen
            | CssKind::OpenBracket
            | CssKind::CloseBracket
            | CssKind::OpenBrace
            | CssKind::CloseBrace => TokenRole::Delimiter,
            CssKind::Unknown => TokenRole::Unknown,
        }
    }
}

/// CSS ident unescape: hex escapes with optional single whitespace (`\31 0` →
/// `"10"`), other escaped code points literal (`\:` → `":"`).
pub fn decode_ident(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = String::with_capacity(raw.len());
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] != b'\\' {
            let Some(ch) = raw[i..].chars().next() else {
                break;
            };
            out.push(ch);
            i += ch.len_utf8();
            continue;
        }
        i += 1;
        if i >= bytes.len() {
            out.push('\u{FFFD}');
            break;
        }
        if bytes[i].is_ascii_hexdigit() {
            let hex_start = i;
            let mut n = 0;
            while i < bytes.len() && n < 6 && bytes[i].is_ascii_hexdigit() {
                i += 1;
                n += 1;
            }
            let code = u32::from_str_radix(&raw[hex_start..i], 16).unwrap_or(0);
            if i < bytes.len() && is_css_ws(bytes[i]) {
                if bytes[i] == b'\r' && bytes.get(i + 1) == Some(&b'\n') {
                    i += 2;
                } else {
                    i += 1;
                }
            }
            let ch = if code == 0 || code > 0x10FFFF {
                '\u{FFFD}'
            } else {
                char::from_u32(code).unwrap_or('\u{FFFD}')
            };
            out.push(ch);
            continue;
        }
        if is_css_newline(bytes, i) {
            // Not a valid escape inside an ident; keep the backslash.
            out.push('\\');
            continue;
        }
        let Some(ch) = raw[i..].chars().next() else {
            out.push('\u{FFFD}');
            break;
        };
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

pub fn lex_document_cancellable(
    bytes: &[u8],
    cancel: &dyn Fn() -> bool,
) -> Result<Vec<CssToken>, LexError> {
    let mut tokens = Vec::new();
    let mut state = CssState::default();
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
        let start = i as u32;
        match state.mode {
            CssMode::BlockComment => {
                if bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/') {
                    i += 2;
                    state.mode = CssMode::Normal;
                } else {
                    i += 1;
                    if i < bytes.len() && state.mode == CssMode::BlockComment {
                        continue;
                    }
                }
                push_token(&mut tokens, start, i, CssKind::Comment)?;
                continue;
            }
            CssMode::DoubleString => {
                resume_quoted(&mut tokens, bytes, &mut i, &mut state, start, b'"')?;
                continue;
            }
            CssMode::SingleString => {
                resume_quoted(&mut tokens, bytes, &mut i, &mut state, start, b'\'')?;
                continue;
            }
            CssMode::UnquotedUrl => {
                resume_unquoted_url(&mut tokens, bytes, &mut i, &mut state, start)?;
                continue;
            }
            CssMode::Normal => {}
        }

        if consume_normal(bytes, &mut i, &mut state, &mut tokens, start, cancel)? {
            if i == start_i {
                return Err(LexError::Nonprogress { at: start });
            }
            continue;
        }
        if i == start_i {
            return Err(LexError::Nonprogress { at: start });
        }
    }
    Ok(tokens)
}

pub fn lex_line(line: &str, incoming: CssState) -> (CssState, Vec<(usize, TokenRole)>) {
    let bytes = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    let mut state = incoming;
    while i < bytes.len() {
        let start = i;
        match state.mode {
            CssMode::BlockComment => {
                if bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/') {
                    i += 2;
                    push_role(i, TokenRole::Comment, &mut out);
                    state.mode = CssMode::Normal;
                } else {
                    i += 1;
                    if i == bytes.len() {
                        push_role(i, TokenRole::Comment, &mut out);
                    }
                }
                continue;
            }
            CssMode::DoubleString | CssMode::SingleString => {
                let quote = if state.mode == CssMode::DoubleString {
                    b'"'
                } else {
                    b'\''
                };
                let (next, end, bad) = finish_string_line(bytes, i, quote);
                i = end.max(i);
                let role = if bad {
                    TokenRole::Unknown
                } else {
                    TokenRole::String
                };
                if i > start {
                    push_role(i, role, &mut out);
                }
                state.mode = next;
                if matches!(state.mode, CssMode::DoubleString | CssMode::SingleString) {
                    break;
                }
                continue;
            }
            CssMode::UnquotedUrl => {
                let (next, end, kind) = finish_unquoted_url_line(bytes, i);
                i = end.max(i);
                if i > start {
                    push_role(i, kind.role(), &mut out);
                }
                state.mode = next;
                if state.mode == CssMode::UnquotedUrl {
                    break;
                }
                continue;
            }
            CssMode::Normal => {}
        }
        if !consume_normal_line(bytes, &mut i, &mut state, &mut out) {
            if i == start && i < bytes.len() {
                i += 1;
                push_role(i, TokenRole::Unknown, &mut out);
            }
        }
        if i == start && i < bytes.len() {
            i += 1;
            push_role(i, TokenRole::Unknown, &mut out);
        }
    }
    if i < bytes.len() {
        let role = match state.mode {
            CssMode::BlockComment => TokenRole::Comment,
            CssMode::DoubleString | CssMode::SingleString | CssMode::UnquotedUrl => {
                TokenRole::String
            }
            CssMode::Normal => TokenRole::Unknown,
        };
        push_role(bytes.len(), role, &mut out);
    }
    (state, out)
}

pub fn document_spans(_bytes: &[u8], tokens: &[CssToken]) -> Vec<TokenSpan> {
    tokens
        .iter()
        .map(|t| TokenSpan::new(t.start, t.end, t.kind.role()))
        .collect()
}

fn consume_normal(
    bytes: &[u8],
    i: &mut usize,
    state: &mut CssState,
    tokens: &mut Vec<CssToken>,
    start: u32,
    cancel: &dyn Fn() -> bool,
) -> Result<bool, LexError> {
    if *i >= bytes.len() {
        return Ok(false);
    }
    let b = bytes[*i];
    if b == b'\r' {
        *i += 1;
        if bytes.get(*i) == Some(&b'\n') {
            *i += 1;
        }
        push_token(tokens, start, *i, CssKind::Whitespace)?;
        return Ok(true);
    }
    if b == b'\n' {
        *i += 1;
        push_token(tokens, start, *i, CssKind::Whitespace)?;
        return Ok(true);
    }
    if is_css_space(b) {
        *i += 1;
        while *i < bytes.len() && is_css_space(bytes[*i]) {
            *i += 1;
        }
        push_token(tokens, start, *i, CssKind::Whitespace)?;
        return Ok(true);
    }
    if b == b'/' && bytes.get(*i + 1) == Some(&b'*') {
        *i += 2;
        state.mode = CssMode::BlockComment;
        while *i < bytes.len() {
            if *i & 255 == 0 && cancel() {
                return Err(LexError::Cancelled);
            }
            if bytes[*i] == b'*' && bytes.get(*i + 1) == Some(&b'/') {
                *i += 2;
                state.mode = CssMode::Normal;
                break;
            }
            *i += 1;
        }
        push_token(tokens, start, *i, CssKind::Comment)?;
        return Ok(true);
    }
    if bytes[*i..].starts_with(b"<!--") {
        *i += 4;
        push_token(tokens, start, *i, CssKind::Cdo)?;
        return Ok(true);
    }
    if bytes[*i..].starts_with(b"-->") {
        *i += 3;
        push_token(tokens, start, *i, CssKind::Cdc)?;
        return Ok(true);
    }
    if b == b'"' {
        *i += 1;
        resume_quoted(tokens, bytes, i, state, start, b'"')?;
        return Ok(true);
    }
    if b == b'\'' {
        *i += 1;
        resume_quoted(tokens, bytes, i, state, start, b'\'')?;
        return Ok(true);
    }
    if b == b'#' {
        if *i + 1 < bytes.len() && (is_ident_code_point(bytes[*i + 1]) || is_valid_escape(bytes, *i + 1))
        {
            *i += 1;
            *i = consume_ident_sequence(bytes, *i);
            push_token(tokens, start, *i, CssKind::Hash)?;
            return Ok(true);
        }
        *i += 1;
        push_token(tokens, start, *i, CssKind::Delim)?;
        return Ok(true);
    }
    if b == b'@' {
        if *i + 1 < bytes.len() && would_start_ident(bytes, *i + 1) {
            *i += 1;
            *i = consume_ident_sequence(bytes, *i);
            push_token(tokens, start, *i, CssKind::AtKeyword)?;
            return Ok(true);
        }
        *i += 1;
        push_token(tokens, start, *i, CssKind::Delim)?;
        return Ok(true);
    }
    if would_start_number(bytes, *i) {
        consume_numeric(bytes, i, tokens, start)?;
        return Ok(true);
    }
    if would_start_ident(bytes, *i) {
        consume_ident_like(bytes, i, state, tokens, start)?;
        return Ok(true);
    }
    *i += 1;
    let kind = match b {
        b':' => CssKind::Colon,
        b';' => CssKind::Semicolon,
        b',' => CssKind::Comma,
        b'(' => CssKind::OpenParen,
        b')' => CssKind::CloseParen,
        b'[' => CssKind::OpenBracket,
        b']' => CssKind::CloseBracket,
        b'{' => CssKind::OpenBrace,
        b'}' => CssKind::CloseBrace,
        _ if b.is_ascii_graphic() => CssKind::Delim,
        _ => CssKind::Unknown,
    };
    push_token(tokens, start, *i, kind)?;
    Ok(true)
}

fn consume_normal_line(
    bytes: &[u8],
    i: &mut usize,
    state: &mut CssState,
    out: &mut Vec<(usize, TokenRole)>,
) -> bool {
    if *i >= bytes.len() {
        return false;
    }
    let b = bytes[*i];
    if is_css_space(b) || b == b'\r' || b == b'\n' {
        *i += 1;
        while *i < bytes.len() && (is_css_space(bytes[*i]) || bytes[*i] == b'\r' || bytes[*i] == b'\n')
        {
            *i += 1;
        }
        push_role(*i, TokenRole::Whitespace, out);
        return true;
    }
    if b == b'/' && bytes.get(*i + 1) == Some(&b'*') {
        *i += 2;
        state.mode = CssMode::BlockComment;
        while *i < bytes.len() {
            if bytes[*i] == b'*' && bytes.get(*i + 1) == Some(&b'/') {
                *i += 2;
                state.mode = CssMode::Normal;
                break;
            }
            *i += 1;
        }
        push_role(*i, TokenRole::Comment, out);
        return true;
    }
    if bytes[*i..].starts_with(b"<!--") {
        *i += 4;
        push_role(*i, TokenRole::Comment, out);
        return true;
    }
    if bytes[*i..].starts_with(b"-->") {
        *i += 3;
        push_role(*i, TokenRole::Comment, out);
        return true;
    }
    if b == b'"' || b == b'\'' {
        let quote = b;
        *i += 1;
        let (next, end, bad) = finish_string_line(bytes, *i, quote);
        *i = end.max(*i);
        let role = if bad {
            TokenRole::Unknown
        } else {
            TokenRole::String
        };
        push_role(*i, role, out);
        state.mode = next;
        return true;
    }
    if b == b'#' {
        if *i + 1 < bytes.len() && (is_ident_code_point(bytes[*i + 1]) || is_valid_escape(bytes, *i + 1))
        {
            *i += 1;
            *i = consume_ident_sequence(bytes, *i);
            push_role(*i, TokenRole::Identifier, out);
            return true;
        }
        *i += 1;
        push_role(*i, TokenRole::Punctuator, out);
        return true;
    }
    if b == b'@' {
        if *i + 1 < bytes.len() && would_start_ident(bytes, *i + 1) {
            *i += 1;
            *i = consume_ident_sequence(bytes, *i);
            push_role(*i, TokenRole::Preprocessor, out);
            return true;
        }
        *i += 1;
        push_role(*i, TokenRole::Punctuator, out);
        return true;
    }
    if would_start_number(bytes, *i) {
        *i = consume_number(bytes, *i);
        if bytes.get(*i) == Some(&b'%') {
            *i += 1;
        } else if *i < bytes.len() && would_start_ident(bytes, *i) {
            *i = consume_ident_sequence(bytes, *i);
        }
        push_role(*i, TokenRole::Number, out);
        return true;
    }
    if would_start_ident(bytes, *i) {
        let ident_end = consume_ident_sequence(bytes, *i);
        if bytes.get(ident_end) == Some(&b'(') {
            if is_url_ident(&bytes[*i..ident_end]) {
                let mut k = ident_end + 1;
                while k < bytes.len() && is_css_space(bytes[k]) {
                    k += 1;
                }
                if bytes.get(k) == Some(&b'"') || bytes.get(k) == Some(&b'\'') {
                    *i = ident_end + 1;
                    push_role(*i, TokenRole::Function, out);
                    return true;
                }
                *i = ident_end + 1;
                let (next, end, kind) = finish_unquoted_url_line(bytes, *i);
                *i = end.max(*i);
                push_role(*i, kind.role(), out);
                state.mode = next;
                return true;
            }
            *i = ident_end + 1;
            push_role(*i, TokenRole::Function, out);
            return true;
        }
        *i = ident_end;
        push_role(*i, TokenRole::Identifier, out);
        return true;
    }
    *i += 1;
    let role = match b {
        b'(' | b')' | b'[' | b']' | b'{' | b'}' => TokenRole::Delimiter,
        _ => TokenRole::Punctuator,
    };
    push_role(*i, role, out);
    true
}

fn consume_ident_like(
    bytes: &[u8],
    i: &mut usize,
    state: &mut CssState,
    tokens: &mut Vec<CssToken>,
    start: u32,
) -> Result<(), LexError> {
    let ident_end = consume_ident_sequence(bytes, *i);
    if bytes.get(ident_end) == Some(&b'(') {
        if is_url_ident(&bytes[*i..ident_end]) {
            let mut k = ident_end + 1;
            while k < bytes.len() && is_css_ws(bytes[k]) {
                k += 1;
            }
            if bytes.get(k) == Some(&b'"') || bytes.get(k) == Some(&b'\'') {
                *i = ident_end + 1;
                return push_token(tokens, start, *i, CssKind::Function);
            }
            *i = ident_end + 1;
            return consume_unquoted_url(bytes, i, state, tokens, start);
        }
        *i = ident_end + 1;
        return push_token(tokens, start, *i, CssKind::Function);
    }
    *i = ident_end;
    push_token(tokens, start, *i, CssKind::Ident)
}

fn consume_numeric(
    bytes: &[u8],
    i: &mut usize,
    tokens: &mut Vec<CssToken>,
    start: u32,
) -> Result<(), LexError> {
    *i = consume_number(bytes, *i);
    if bytes.get(*i) == Some(&b'%') {
        *i += 1;
        push_token(tokens, start, *i, CssKind::Percentage)
    } else if *i < bytes.len() && would_start_ident(bytes, *i) {
        *i = consume_ident_sequence(bytes, *i);
        push_token(tokens, start, *i, CssKind::Dimension)
    } else {
        push_token(tokens, start, *i, CssKind::Number)
    }
}

fn consume_unquoted_url(
    bytes: &[u8],
    i: &mut usize,
    state: &mut CssState,
    tokens: &mut Vec<CssToken>,
    start: u32,
) -> Result<(), LexError> {
    let mut bad = false;
    while *i < bytes.len() {
        let b = bytes[*i];
        if b == b')' {
            *i += 1;
            let kind = if bad { CssKind::Unknown } else { CssKind::Url };
            return push_token(tokens, start, *i, kind);
        }
        if b == b'"' || b == b'\'' || b == b'(' || b < 0x20 && b != b'\t' && !is_css_newline(bytes, *i)
        {
            bad = true;
            *i += 1;
            continue;
        }
        if b == b'\\' {
            if is_valid_escape(bytes, *i) {
                *i = consume_escape(bytes, *i);
                continue;
            }
            bad = true;
            *i += 1;
            continue;
        }
        *i += 1;
    }
    state.mode = CssMode::UnquotedUrl;
    let kind = if bad { CssKind::Unknown } else { CssKind::Url };
    push_token(tokens, start, *i, kind)
}

fn resume_unquoted_url(
    tokens: &mut Vec<CssToken>,
    bytes: &[u8],
    i: &mut usize,
    state: &mut CssState,
    start: u32,
) -> Result<(), LexError> {
    let mut bad = false;
    while *i < bytes.len() {
        let b = bytes[*i];
        if b == b')' {
            *i += 1;
            state.mode = CssMode::Normal;
            let kind = if bad { CssKind::Unknown } else { CssKind::Url };
            return push_token(tokens, start, *i, kind);
        }
        if b == b'"' || b == b'\'' || b == b'(' {
            bad = true;
        }
        if b == b'\\' && is_valid_escape(bytes, *i) {
            *i = consume_escape(bytes, *i);
            continue;
        }
        *i += 1;
    }
    let kind = if bad { CssKind::Unknown } else { CssKind::Url };
    push_token(tokens, start, *i, kind)
}

fn resume_quoted(
    tokens: &mut Vec<CssToken>,
    bytes: &[u8],
    i: &mut usize,
    state: &mut CssState,
    start: u32,
    quote: u8,
) -> Result<(), LexError> {
    let (next, end, bad) = finish_string(bytes, *i, quote);
    if end < *i {
        return Err(LexError::Nonprogress { at: start });
    }
    state.mode = next;
    let kind = if bad { CssKind::Unknown } else { CssKind::String };
    if end > *i || end as u32 > start {
        push_token(tokens, start, end.max(*i), kind)?;
    }
    *i = end;
    Ok(())
}

/// Returns `(next_mode, end, bad_string)`.
fn finish_string(bytes: &[u8], mut i: usize, quote: u8) -> (CssMode, usize, bool) {
    let open = if quote == b'"' {
        CssMode::DoubleString
    } else {
        CssMode::SingleString
    };
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'\\' {
            i += 1;
            if i >= bytes.len() {
                return (open, i, false);
            }
            if bytes[i] == b'\r' {
                i += 1;
                if bytes.get(i) == Some(&b'\n') {
                    i += 1;
                }
                continue;
            }
            if bytes[i] == b'\n' || bytes[i] == 0x0c {
                i += 1;
                continue;
            }
            i += utf8_len(bytes, i);
            continue;
        }
        if b == quote {
            return (CssMode::Normal, i + 1, false);
        }
        if b == b'\n' || b == b'\r' || b == 0x0c {
            // Bad-string: reconsume the newline.
            return (CssMode::Normal, i, true);
        }
        i += 1;
    }
    (open, i, false)
}

/// Line lexer: newline is not in `bytes`. A trailing `\` continues the string;
/// any other unclosed string is a bad-string and does not continue.
fn finish_string_line(bytes: &[u8], mut i: usize, quote: u8) -> (CssMode, usize, bool) {
    let open = if quote == b'"' {
        CssMode::DoubleString
    } else {
        CssMode::SingleString
    };
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'\\' {
            i += 1;
            if i >= bytes.len() {
                // `\` at end of line: continuation.
                return (open, i, false);
            }
            i += utf8_len(bytes, i);
            continue;
        }
        if b == quote {
            return (CssMode::Normal, i + 1, false);
        }
        if b == b'\n' || b == b'\r' || b == 0x0c {
            return (CssMode::Normal, i, true);
        }
        i += 1;
    }
    // Unescaped end of line: bad-string, next line starts Normal.
    (CssMode::Normal, i, true)
}

fn finish_unquoted_url_line(bytes: &[u8], mut i: usize) -> (CssMode, usize, CssKind) {
    let mut bad = false;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b')' {
            let kind = if bad { CssKind::Unknown } else { CssKind::Url };
            return (CssMode::Normal, i + 1, kind);
        }
        if b == b'"' || b == b'\'' || b == b'(' {
            bad = true;
        }
        if b == b'\\' && is_valid_escape(bytes, i) {
            i = consume_escape(bytes, i);
            continue;
        }
        i += 1;
    }
    let kind = if bad { CssKind::Unknown } else { CssKind::Url };
    (CssMode::UnquotedUrl, i, kind)
}

fn consume_ident_sequence(bytes: &[u8], mut i: usize) -> usize {
    while i < bytes.len() {
        if is_valid_escape(bytes, i) {
            i = consume_escape(bytes, i);
            continue;
        }
        if is_ident_code_point(bytes[i]) {
            i += 1;
            continue;
        }
        break;
    }
    i
}

fn consume_escape(bytes: &[u8], i: usize) -> usize {
    // `i` points at `\`.
    let mut j = i + 1;
    if j >= bytes.len() {
        return j;
    }
    if bytes[j].is_ascii_hexdigit() {
        let mut n = 0;
        while j < bytes.len() && n < 6 && bytes[j].is_ascii_hexdigit() {
            j += 1;
            n += 1;
        }
        if j < bytes.len() && is_css_ws(bytes[j]) {
            if bytes[j] == b'\r' && bytes.get(j + 1) == Some(&b'\n') {
                j += 2;
            } else {
                j += 1;
            }
        }
        return j;
    }
    j + utf8_len(bytes, j)
}

fn consume_number(bytes: &[u8], mut i: usize) -> usize {
    if i < bytes.len() && (bytes[i] == b'+' || bytes[i] == b'-') {
        i += 1;
    }
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if bytes.get(i) == Some(&b'.') && bytes.get(i + 1).map(|c| c.is_ascii_digit()).unwrap_or(false) {
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
    }
    if i < bytes.len() && (bytes[i] == b'e' || bytes[i] == b'E') {
        let mut j = i + 1;
        if j < bytes.len() && (bytes[j] == b'+' || bytes[j] == b'-') {
            j += 1;
        }
        if j < bytes.len() && bytes[j].is_ascii_digit() {
            i = j;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
        }
    }
    i
}

fn would_start_ident(bytes: &[u8], i: usize) -> bool {
    if i >= bytes.len() {
        return false;
    }
    if is_valid_escape(bytes, i) {
        return true;
    }
    let b = bytes[i];
    if b == b'-' {
        if bytes.get(i + 1) == Some(&b'-') {
            return true;
        }
        if i + 1 < bytes.len() && is_ident_start_byte(bytes[i + 1]) {
            return true;
        }
        return i + 1 < bytes.len() && is_valid_escape(bytes, i + 1);
    }
    is_ident_start_byte(b)
}

fn would_start_number(bytes: &[u8], i: usize) -> bool {
    if i >= bytes.len() {
        return false;
    }
    let b = bytes[i];
    if b.is_ascii_digit() {
        return true;
    }
    if b == b'.' {
        return bytes.get(i + 1).map(|c| c.is_ascii_digit()).unwrap_or(false);
    }
    if b == b'+' || b == b'-' {
        if bytes.get(i + 1).map(|c| c.is_ascii_digit()).unwrap_or(false) {
            return true;
        }
        return bytes.get(i + 1) == Some(&b'.')
            && bytes.get(i + 2).map(|c| c.is_ascii_digit()).unwrap_or(false);
    }
    false
}

fn is_valid_escape(bytes: &[u8], i: usize) -> bool {
    if bytes.get(i) != Some(&b'\\') {
        return false;
    }
    match bytes.get(i + 1) {
        None => false,
        Some(&b'\n') | Some(&0x0c) => false,
        Some(&b'\r') => false,
        Some(_) => true,
    }
}

fn is_ident_code_point(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b >= 0x80
}

fn is_ident_start_byte(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || b >= 0x80
}

fn is_url_ident(raw: &[u8]) -> bool {
    raw.len() == 3 && raw.eq_ignore_ascii_case(b"url")
}

fn is_css_ws(b: u8) -> bool {
    b == b' ' || b == b'\t' || b == b'\n' || b == b'\r' || b == 0x0c
}

fn is_css_space(b: u8) -> bool {
    b == b' ' || b == b'\t' || b == 0x0b || b == 0x0c
}

fn is_css_newline(bytes: &[u8], i: usize) -> bool {
    matches!(bytes.get(i), Some(&b'\n') | Some(&b'\r') | Some(&0x0c))
}

fn utf8_len(bytes: &[u8], i: usize) -> usize {
    match bytes.get(i) {
        None => 0,
        Some(&b) if b < 0x80 => 1,
        Some(&b) if b & 0xe0 == 0xc0 => 2.min(bytes.len() - i),
        Some(&b) if b & 0xf0 == 0xe0 => 3.min(bytes.len() - i),
        Some(&b) if b & 0xf8 == 0xf0 => 4.min(bytes.len() - i),
        Some(_) => 1,
    }
}

fn push_token(
    tokens: &mut Vec<CssToken>,
    start: u32,
    end: usize,
    kind: CssKind,
) -> Result<(), LexError> {
    if end as u32 <= start {
        return Err(LexError::Nonprogress { at: start });
    }
    tokens.push(CssToken {
        start,
        end: end as u32,
        kind,
    });
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
