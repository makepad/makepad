//! Source-only Go lexer shared by the editor and the Go frontend.
//! Byte offsets address the original document. Continuation state is owned
//! here: block comments and raw strings survive line breaks; interpreted
//! strings and rune literals are line-bounded.
//!
//! Directive comments (`//go:build`, `//go:generate`, `//go:embed`,
//! `//export`, `// +build`) are ordinary comments; the frontend counts them.

use crate::token::{TokenRole, TokenSpan};
use crate::LexError;

/// Version of this lexer; part of parse and search cache identity.
pub const GO_LEXER_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
enum GoMode {
    #[default]
    Normal,
    BlockComment,
    RawString,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct GoState {
    mode: GoMode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GoKind {
    Whitespace,
    Newline,
    Comment,
    Identifier,
    Keyword,
    Number,
    String,
    RawString,
    Rune,
    Punctuator,
    Delimiter,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GoToken {
    pub start: u32,
    pub end: u32,
    pub kind: GoKind,
}

impl GoKind {
    pub fn role(self) -> TokenRole {
        match self {
            GoKind::Whitespace | GoKind::Newline => TokenRole::Whitespace,
            GoKind::Comment => TokenRole::Comment,
            GoKind::Identifier => TokenRole::Identifier,
            GoKind::Keyword => TokenRole::Keyword,
            GoKind::Number => TokenRole::Number,
            GoKind::String | GoKind::RawString => TokenRole::String,
            GoKind::Rune => TokenRole::Char,
            GoKind::Punctuator => TokenRole::Punctuator,
            GoKind::Delimiter => TokenRole::Delimiter,
            GoKind::Unknown => TokenRole::Unknown,
        }
    }
}

/// The 25 Go keywords. Sorted for binary search.
const KEYWORDS: &[&str] = &[
    "break",
    "case",
    "chan",
    "const",
    "continue",
    "default",
    "defer",
    "else",
    "fallthrough",
    "for",
    "func",
    "go",
    "goto",
    "if",
    "import",
    "interface",
    "map",
    "package",
    "range",
    "return",
    "select",
    "struct",
    "switch",
    "type",
    "var",
];

/// Predeclared type names. Stay Identifier kind; role Typename.
const PREDECLARED_TYPES: &[&str] = &[
    "any",
    "bool",
    "byte",
    "comparable",
    "complex128",
    "complex64",
    "error",
    "float32",
    "float64",
    "int",
    "int16",
    "int32",
    "int64",
    "int8",
    "rune",
    "string",
    "uint",
    "uint16",
    "uint32",
    "uint64",
    "uint8",
    "uintptr",
];

/// Predeclared builtin function names. Stay Identifier kind.
const BUILTIN_FUNCTIONS: &[&str] = &[
    "append", "cap", "clear", "close", "complex", "copy", "delete", "imag", "len", "make", "max",
    "min", "new", "panic", "print", "println", "real", "recover",
];

pub fn is_keyword(ident: &str) -> bool {
    KEYWORDS.binary_search(&ident).is_ok()
}

pub fn is_predeclared_type(ident: &str) -> bool {
    PREDECLARED_TYPES.binary_search(&ident).is_ok()
}

pub fn is_builtin_function(ident: &str) -> bool {
    BUILTIN_FUNCTIONS.binary_search(&ident).is_ok()
}

fn classify_identifier(ident: &str, next_is_paren: bool, prev_is_dot: bool) -> TokenRole {
    match ident {
        "if" | "else" | "switch" | "case" | "default" | "select" | "return" | "fallthrough"
        | "goto" => TokenRole::BranchKeyword,
        "for" | "range" | "break" | "continue" => TokenRole::LoopKeyword,
        "true" | "false" | "nil" | "iota" => TokenRole::Constant,
        other if is_predeclared_type(other) => TokenRole::Typename,
        other if is_keyword(other) => TokenRole::Keyword,
        _ if next_is_paren => TokenRole::Function,
        _ if !prev_is_dot
            && ident
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

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || b >= 0x80
}

fn is_ident_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80
}

fn is_newline(b: u8) -> bool {
    b == b'\n' || b == b'\r'
}

fn is_end_of_number(b: u8) -> bool {
    b == 0 || (!is_ident_continue(b) && b != b'.')
}

fn push_token(
    tokens: &mut Vec<GoToken>,
    start: u32,
    end: usize,
    kind: GoKind,
) -> Result<(), LexError> {
    if end as u32 <= start {
        return Err(LexError::Nonprogress { at: start });
    }
    tokens.push(GoToken {
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
            let mut n = 0;
            while *i < bytes.len() && n < 4 && bytes[*i].is_ascii_hexdigit() {
                *i += 1;
                n += 1;
            }
        }
        b'U' => {
            let mut n = 0;
            while *i < bytes.len() && n < 8 && bytes[*i].is_ascii_hexdigit() {
                *i += 1;
                n += 1;
            }
        }
        b'0'..=b'7' => {
            let mut n = 1;
            while *i < bytes.len() && n < 3 && (b'0'..=b'7').contains(&bytes[*i]) {
                *i += 1;
                n += 1;
            }
        }
        _ => {}
    }
}

/// Scan from `i` inside an interpreted string or rune. Returns whether the
/// literal closed or recovered at a line boundary. The newline is not consumed.
fn scan_quoted(bytes: &[u8], i: &mut usize, quote: u8) -> bool {
    while *i < bytes.len() {
        let b = bytes[*i];
        if b == b'\\' {
            *i += 1;
            consume_string_escape(bytes, i);
            continue;
        }
        if b == quote {
            *i += 1;
            return true;
        }
        if is_newline(b) {
            return true;
        }
        *i += 1;
    }
    false
}

fn scan_raw_string(bytes: &[u8], i: &mut usize, state: &mut GoState) -> bool {
    while *i < bytes.len() {
        if bytes[*i] == b'`' {
            *i += 1;
            state.mode = GoMode::Normal;
            return true;
        }
        *i += 1;
    }
    false
}

fn scan_number(bytes: &[u8], mut i: usize) -> usize {
    if bytes[i] == b'0'
        && matches!(
            bytes.get(i + 1).map(|b| b.to_ascii_lowercase()),
            Some(b'x' | b'o' | b'b')
        )
    {
        let tag = bytes[i + 1].to_ascii_lowercase();
        i += 2;
        match tag {
            b'x' => {
                while i < bytes.len() && (bytes[i].is_ascii_hexdigit() || bytes[i] == b'_') {
                    i += 1;
                }
                if bytes.get(i) == Some(&b'.') {
                    let next = bytes.get(i + 1).copied().unwrap_or(0);
                    if next.is_ascii_hexdigit() || next.to_ascii_lowercase() == b'p' {
                        i += 1;
                        while i < bytes.len() && (bytes[i].is_ascii_hexdigit() || bytes[i] == b'_')
                        {
                            i += 1;
                        }
                    }
                }
                if bytes.get(i).map(|b| b.to_ascii_lowercase()) == Some(b'p') {
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
                return consume_imaginary(bytes, i);
            }
            b'o' => {
                while i < bytes.len() && ((b'0'..=b'7').contains(&bytes[i]) || bytes[i] == b'_') {
                    i += 1;
                }
                return consume_imaginary(bytes, i);
            }
            _ => {
                while i < bytes.len() && (bytes[i] == b'0' || bytes[i] == b'1' || bytes[i] == b'_')
                {
                    i += 1;
                }
                return consume_imaginary(bytes, i);
            }
        }
    }
    if bytes[i] == b'.' {
        i += 1;
        while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'_') {
            i += 1;
        }
        i = consume_exponent(bytes, i);
        return consume_imaginary(bytes, i);
    }
    while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'_') {
        i += 1;
    }
    if bytes.get(i) == Some(&b'.') {
        let next = bytes.get(i + 1).copied().unwrap_or(0);
        // `.` after digits is part of the number only when followed by a
        // digit, exponent, imaginary suffix, or end-of-number.
        if next.is_ascii_digit()
            || next == b'e'
            || next == b'E'
            || next == b'p'
            || next == b'P'
            || next == b'i'
            || is_end_of_number(next)
        {
            i += 1;
            while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'_') {
                i += 1;
            }
        }
    }
    i = consume_exponent(bytes, i);
    consume_imaginary(bytes, i)
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

fn consume_imaginary(bytes: &[u8], mut i: usize) -> usize {
    if bytes.get(i) == Some(&b'i') {
        i += 1;
    }
    i
}

fn punct_len(bytes: &[u8], i: usize) -> usize {
    let rest = &bytes[i..];
    if rest.starts_with(b"<<=") || rest.starts_with(b">>=") || rest.starts_with(b"&^=") || rest.starts_with(b"...")
    {
        return 3;
    }
    if matches!(
        rest.get(..2),
        Some(
            b"&&" | b"||"
                | b"<-"
                | b"++"
                | b"--"
                | b"=="
                | b"!="
                | b"<="
                | b">="
                | b":="
                | b"+="
                | b"-="
                | b"*="
                | b"/="
                | b"%="
                | b"&="
                | b"|="
                | b"^="
                | b"<<"
                | b">>"
                | b"&^"
        )
    ) {
        return 2;
    }
    1
}

/// Shared automaton step. Emits one token covering `[start, *i)`.
fn lex_one(
    bytes: &[u8],
    i: &mut usize,
    state: &mut GoState,
    line_start: &mut bool,
) -> Result<GoToken, LexError> {
    let start_i = *i;
    let start = *i as u32;
    if *i >= bytes.len() {
        return Err(LexError::Nonprogress { at: start });
    }

    match state.mode {
        GoMode::BlockComment => {
            while *i < bytes.len() {
                if bytes[*i] == b'*' && bytes.get(*i + 1) == Some(&b'/') {
                    *i += 2;
                    state.mode = GoMode::Normal;
                    break;
                }
                if is_newline(bytes[*i]) {
                    *line_start = true;
                } else if !is_space(bytes[*i]) {
                    *line_start = false;
                }
                *i += 1;
            }
            if (*i as u32) <= start {
                return Err(LexError::Nonprogress { at: start });
            }
            return Ok(GoToken {
                start,
                end: *i as u32,
                kind: GoKind::Comment,
            });
        }
        GoMode::RawString => {
            let closed = scan_raw_string(bytes, i, state);
            if *i == start_i && *i < bytes.len() && !closed {
                *i += 1;
            }
            if *i as u32 <= start {
                return Err(LexError::Nonprogress { at: start });
            }
            *line_start = false;
            return Ok(tok(start, *i, GoKind::RawString));
        }
        GoMode::Normal => {}
    }

    let b = bytes[*i];
    if b == b'\r' {
        *i += 1;
        if bytes.get(*i) == Some(&b'\n') {
            *i += 1;
        }
        *line_start = true;
        return Ok(tok(start, *i, GoKind::Newline));
    }
    if b == b'\n' {
        *i += 1;
        *line_start = true;
        return Ok(tok(start, *i, GoKind::Newline));
    }
    if is_space(b) {
        *i += 1;
        while *i < bytes.len() && is_space(bytes[*i]) {
            *i += 1;
        }
        return Ok(tok(start, *i, GoKind::Whitespace));
    }
    *line_start = false;
    if b == b'/' && bytes.get(*i + 1) == Some(&b'/') {
        *i += 2;
        while *i < bytes.len() && !is_newline(bytes[*i]) {
            *i += 1;
        }
        return Ok(tok(start, *i, GoKind::Comment));
    }
    if b == b'/' && bytes.get(*i + 1) == Some(&b'*') {
        *i += 2;
        state.mode = GoMode::BlockComment;
        while *i < bytes.len() {
            if bytes[*i] == b'*' && bytes.get(*i + 1) == Some(&b'/') {
                *i += 2;
                state.mode = GoMode::Normal;
                break;
            }
            *i += 1;
        }
        return Ok(tok(start, *i, GoKind::Comment));
    }
    if b == b'\'' {
        *i += 1;
        let _closed = scan_quoted(bytes, i, b'\'');
        if *i as u32 <= start {
            if *i == start_i + 1 {
                return Ok(tok(start, *i, GoKind::Rune));
            }
            return Err(LexError::Nonprogress { at: start });
        }
        return Ok(tok(start, *i, GoKind::Rune));
    }
    if b == b'"' {
        *i += 1;
        let _closed = scan_quoted(bytes, i, b'"');
        if *i as u32 <= start {
            return Err(LexError::Nonprogress { at: start });
        }
        return Ok(tok(start, *i, GoKind::String));
    }
    if b == b'`' {
        *i += 1;
        state.mode = GoMode::RawString;
        let _closed = scan_raw_string(bytes, i, state);
        if *i as u32 <= start {
            return Err(LexError::Nonprogress { at: start });
        }
        return Ok(tok(start, *i, GoKind::RawString));
    }
    if is_ident_start(b) {
        *i += 1;
        while *i < bytes.len() && is_ident_continue(bytes[*i]) {
            *i += 1;
        }
        let ident = std::str::from_utf8(&bytes[start as usize..*i]).unwrap_or("");
        let kind = if is_keyword(ident) {
            GoKind::Keyword
        } else {
            GoKind::Identifier
        };
        return Ok(tok(start, *i, kind));
    }
    if b.is_ascii_digit() || (b == b'.' && bytes.get(*i + 1).copied().unwrap_or(0).is_ascii_digit())
    {
        *i = scan_number(bytes, *i);
        return Ok(tok(start, *i, GoKind::Number));
    }
    let n = punct_len(bytes, *i);
    *i += n;
    let kind = if n == 1 && matches!(b, b'(' | b')' | b'{' | b'}' | b'[' | b']') {
        GoKind::Delimiter
    } else if b.is_ascii_graphic() {
        GoKind::Punctuator
    } else {
        GoKind::Unknown
    };
    if *i == start_i {
        return Err(LexError::Nonprogress { at: start });
    }
    Ok(tok(start, *i, kind))
}

fn tok(start: u32, end: usize, kind: GoKind) -> GoToken {
    GoToken {
        start,
        end: end as u32,
        kind,
    }
}

/// Tokenize an entire document. Spans are byte offsets into `bytes`.
pub fn lex_document_cancellable(
    bytes: &[u8],
    cancel: &dyn Fn() -> bool,
) -> Result<Vec<GoToken>, LexError> {
    let mut tokens = Vec::new();
    let mut state = GoState::default();
    let mut line_start = true;
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
        match lex_one(bytes, &mut i, &mut state, &mut line_start) {
            Ok(t) => {
                if t.end <= t.start {
                    return Err(LexError::Nonprogress { at: t.start });
                }
                tokens.push(t);
            }
            Err(LexError::Nonprogress { at }) => {
                if i < bytes.len() && is_newline(bytes[i]) && i == start_i {
                    let start = i as u32;
                    i += 1;
                    if bytes.get(i - 1) == Some(&b'\r') && bytes.get(i) == Some(&b'\n') {
                        i += 1;
                    }
                    line_start = true;
                    push_token(&mut tokens, start, i, GoKind::Newline)?;
                    continue;
                }
                if i == start_i && i < bytes.len() {
                    i += 1;
                    push_token(&mut tokens, at, i, GoKind::Unknown)?;
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

/// Tokenize one display line (without the newline) given incoming continuation.
/// Each pair is `(end_index, role)` covering `[prev_end, end)` of `line`.
pub fn lex_line(line: &str, incoming: GoState) -> (GoState, Vec<(usize, TokenRole)>) {
    let bytes = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    let mut state = incoming;
    let mut line_start = matches!(state.mode, GoMode::Normal);
    let mut prev_is_dot = false;
    while i < bytes.len() {
        let start = i;
        match lex_one(bytes, &mut i, &mut state, &mut line_start) {
            Ok(t) => {
                let end = (t.end as usize).min(bytes.len()).max(start);
                if end > start {
                    let ident = if t.kind == GoKind::Keyword || t.kind == GoKind::Identifier {
                        std::str::from_utf8(&bytes[t.start as usize..end]).unwrap_or("")
                    } else {
                        ""
                    };
                    let role = if t.kind == GoKind::Keyword || t.kind == GoKind::Identifier {
                        let mut k = end;
                        while k < bytes.len() && is_space(bytes[k]) {
                            k += 1;
                        }
                        let next_is_paren = bytes.get(k) == Some(&b'(');
                        classify_identifier(ident, next_is_paren, prev_is_dot)
                    } else {
                        t.kind.role()
                    };
                    push_role(end, role, &mut out);
                    if t.kind != GoKind::Whitespace && t.kind != GoKind::Comment {
                        let slice = &bytes[t.start as usize..end];
                        prev_is_dot = t.kind == GoKind::Punctuator && slice == b".";
                    }
                }
                i = end.max(i);
            }
            Err(LexError::Nonprogress { .. }) => {
                if i == start && i < bytes.len() {
                    i += 1;
                    push_role(i, TokenRole::Unknown, &mut out);
                    prev_is_dot = false;
                } else if i < bytes.len() {
                    i = bytes.len();
                    push_role(i, TokenRole::Unknown, &mut out);
                    prev_is_dot = false;
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
                prev_is_dot = false;
            } else {
                break;
            }
        }
    }
    if i < bytes.len() {
        let role = match state.mode {
            GoMode::BlockComment => TokenRole::Comment,
            GoMode::RawString => TokenRole::String,
            GoMode::Normal => TokenRole::Unknown,
        };
        push_role(bytes.len(), role, &mut out);
    }
    (state, out)
}

/// Convert document tokens into public spans, classifying keywords and
/// function names with one-token lookahead. Capitalised identifiers after
/// `.` stay Identifier so `pkg.Name` is not a type name.
pub fn document_spans(bytes: &[u8], tokens: &[GoToken]) -> Vec<TokenSpan> {
    let mut out = Vec::with_capacity(tokens.len());
    for (i, t) in tokens.iter().enumerate() {
        let mut role = t.kind.role();
        if t.kind == GoKind::Keyword || t.kind == GoKind::Identifier {
            let mut j = i + 1;
            while j < tokens.len() && tokens[j].kind.role().is_trivia() {
                j += 1;
            }
            let next_is_paren = j < tokens.len()
                && tokens[j].kind == GoKind::Delimiter
                && bytes.get(tokens[j].start as usize) == Some(&b'(');
            let mut k = i;
            let mut prev_is_dot = false;
            while k > 0 {
                k -= 1;
                if tokens[k].kind.role().is_trivia() {
                    continue;
                }
                prev_is_dot = tokens[k].kind == GoKind::Punctuator
                    && bytes.get(tokens[k].start as usize..tokens[k].end as usize) == Some(b".");
                break;
            }
            let ident = std::str::from_utf8(&bytes[t.start as usize..t.end as usize]).unwrap_or("");
            role = classify_identifier(ident, next_is_paren, prev_is_dot);
        }
        out.push(TokenSpan::new(t.start, t.end, role));
    }
    out
}
