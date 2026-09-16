//! Source-only Zig lexer shared by the editor and the Zig frontend.
//! Byte offsets address the original document. Zig has no block comments and
//! no string that spans a line break, so continuation state is empty.

use crate::token::{TokenRole, TokenSpan};
use crate::LexError;

/// Version of this lexer; part of parse and search cache identity.
pub const ZIG_LEXER_VERSION: u32 = 1;

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct ZigState;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZigKind {
    Whitespace,
    Newline,
    Comment,
    Identifier,
    Builtin,
    Keyword,
    Number,
    String,
    Char,
    Punctuator,
    Delimiter,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ZigToken {
    pub start: u32,
    pub end: u32,
    pub kind: ZigKind,
}

impl ZigKind {
    pub fn role(self) -> TokenRole {
        match self {
            ZigKind::Whitespace | ZigKind::Newline => TokenRole::Whitespace,
            ZigKind::Comment => TokenRole::Comment,
            ZigKind::Identifier => TokenRole::Identifier,
            ZigKind::Builtin => TokenRole::Function,
            ZigKind::Keyword => TokenRole::Keyword,
            ZigKind::Number => TokenRole::Number,
            ZigKind::String | ZigKind::Char => TokenRole::String,
            ZigKind::Punctuator => TokenRole::Punctuator,
            ZigKind::Delimiter => TokenRole::Delimiter,
            ZigKind::Unknown => TokenRole::Unknown,
        }
    }
}

/// Zig keywords. Sorted for binary search.
const KEYWORDS: &[&str] = &[
    "addrspace",
    "align",
    "allowzero",
    "and",
    "anyframe",
    "anytype",
    "asm",
    "async",
    "await",
    "break",
    "callconv",
    "catch",
    "comptime",
    "const",
    "continue",
    "defer",
    "else",
    "enum",
    "errdefer",
    "error",
    "export",
    "extern",
    "fn",
    "for",
    "if",
    "inline",
    "linksection",
    "noalias",
    "noinline",
    "nosuspend",
    "opaque",
    "or",
    "orelse",
    "packed",
    "pub",
    "resume",
    "return",
    "struct",
    "suspend",
    "switch",
    "test",
    "threadlocal",
    "try",
    "union",
    "unreachable",
    "usingnamespace",
    "var",
    "volatile",
    "while",
];

const NAMED_PRIMITIVES: &[&str] = &[
    "anyerror",
    "anyopaque",
    "bool",
    "c_char",
    "c_int",
    "c_long",
    "c_longdouble",
    "c_longlong",
    "c_short",
    "c_uint",
    "c_ulong",
    "c_ulonglong",
    "c_ushort",
    "comptime_float",
    "comptime_int",
    "f128",
    "f16",
    "f32",
    "f64",
    "f80",
    "isize",
    "noreturn",
    "type",
    "usize",
    "void",
];

pub fn is_keyword(ident: &str) -> bool {
    KEYWORDS.binary_search(&ident).is_ok()
}

pub fn is_primitive_type(ident: &str) -> bool {
    if NAMED_PRIMITIVES.binary_search(&ident).is_ok() {
        return true;
    }
    let b = ident.as_bytes();
    if b.len() < 2 {
        return false;
    }
    if b[0] != b'i' && b[0] != b'u' {
        return false;
    }
    b[1..].iter().all(|c| c.is_ascii_digit())
}

fn classify_identifier(ident: &str, kind: ZigKind, next_is_paren: bool) -> TokenRole {
    if kind == ZigKind::Builtin {
        return TokenRole::Function;
    }
    match ident {
        "if" | "else" | "switch" | "return" | "try" | "catch" | "orelse" | "unreachable"
        | "defer" | "errdefer" | "suspend" | "resume" | "await" | "async" | "nosuspend" => {
            TokenRole::BranchKeyword
        }
        "for" | "while" | "break" | "continue" => TokenRole::LoopKeyword,
        "true" | "false" | "null" | "undefined" => TokenRole::Constant,
        other if is_keyword(other) => TokenRole::Keyword,
        other if is_primitive_type(other) => TokenRole::Typename,
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

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || b >= 0x80
}

fn is_ident_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80
}

fn is_newline(b: u8) -> bool {
    b == b'\n' || b == b'\r'
}

fn push_token(
    tokens: &mut Vec<ZigToken>,
    start: u32,
    end: usize,
    kind: ZigKind,
) -> Result<(), LexError> {
    if end as u32 <= start {
        return Err(LexError::Nonprogress { at: start });
    }
    tokens.push(ZigToken {
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

fn consume_escape(bytes: &[u8], i: &mut usize) {
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
                while *i < bytes.len() && !is_newline(bytes[*i]) && bytes[*i] != b'}' {
                    *i += 1;
                }
                if bytes.get(*i) == Some(&b'}') {
                    *i += 1;
                }
            }
        }
        _ => {}
    }
}

/// Scan from `i` inside a line-bounded `"..."` or `'...'`. Returns whether the
/// literal closed. The newline is not consumed.
fn scan_quoted(bytes: &[u8], i: &mut usize, quote: u8) -> bool {
    while *i < bytes.len() {
        let b = bytes[*i];
        if b == b'\\' {
            *i += 1;
            consume_escape(bytes, i);
            continue;
        }
        if b == quote {
            *i += 1;
            return true;
        }
        if is_newline(b) {
            return false;
        }
        *i += 1;
    }
    false
}

fn scan_number(bytes: &[u8], mut i: usize) -> usize {
    if bytes[i] == b'0' {
        match bytes.get(i + 1).map(|b| b.to_ascii_lowercase()) {
            Some(b'x') => {
                i += 2;
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
                return i;
            }
            Some(b'o') => {
                i += 2;
                while i < bytes.len() && ((b'0'..=b'7').contains(&bytes[i]) || bytes[i] == b'_') {
                    i += 1;
                }
                return i;
            }
            Some(b'b') => {
                i += 2;
                while i < bytes.len() && (bytes[i] == b'0' || bytes[i] == b'1' || bytes[i] == b'_')
                {
                    i += 1;
                }
                return i;
            }
            _ => {}
        }
    }
    while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'_') {
        i += 1;
    }
    if bytes.get(i) == Some(&b'.') {
        let next = bytes.get(i + 1).copied().unwrap_or(0);
        // `1.` followed by an identifier byte or a second `.` is `1` then `.`.
        if next != b'.' && !is_ident_start(next) {
            i += 1;
            while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'_') {
                i += 1;
            }
        }
    }
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
    if rest.len() >= 3 {
        match &rest[..3] {
            b"..." | b"<<=" | b">>=" | b"*%=" | b"+%=" | b"-%=" | b"<<|" | b"+|=" | b"-|="
            | b"*|=" => return 3,
            _ => {}
        }
    }
    if rest.len() >= 2 {
        match &rest[..2] {
            b".." | b".*" | b".?" | b"=>" | b"==" | b"!=" | b"<=" | b">=" | b"<<" | b">>"
            | b"+=" | b"-=" | b"*=" | b"/=" | b"%=" | b"&=" | b"|=" | b"^=" | b"++" | b"**"
            | b"||" | b"+%" | b"-%" | b"*%" | b"+|" | b"-|" | b"*|" | b"->" => return 2,
            _ => {}
        }
    }
    1
}

fn is_punct_start(b: u8) -> bool {
    matches!(
        b,
        b'+' | b'-'
            | b'*'
            | b'/'
            | b'%'
            | b'='
            | b'<'
            | b'>'
            | b'!'
            | b'&'
            | b'|'
            | b'^'
            | b'~'
            | b'?'
            | b'.'
            | b','
            | b';'
            | b':'
    )
}

/// Shared automaton step. Emits one token covering `[start, *i)`.
fn lex_one(bytes: &[u8], i: &mut usize) -> Result<ZigToken, LexError> {
    let start_i = *i;
    let start = *i as u32;
    if *i >= bytes.len() {
        return Err(LexError::Nonprogress { at: start });
    }

    let b = bytes[*i];
    if b == b'\r' {
        *i += 1;
        if bytes.get(*i) == Some(&b'\n') {
            *i += 1;
        }
        return Ok(tok(start, *i, ZigKind::Newline));
    }
    if b == b'\n' {
        *i += 1;
        return Ok(tok(start, *i, ZigKind::Newline));
    }
    if is_space(b) {
        *i += 1;
        while *i < bytes.len() && is_space(bytes[*i]) {
            *i += 1;
        }
        return Ok(tok(start, *i, ZigKind::Whitespace));
    }
    if b == b'/' && bytes.get(*i + 1) == Some(&b'/') {
        *i += 2;
        while *i < bytes.len() && !is_newline(bytes[*i]) {
            *i += 1;
        }
        return Ok(tok(start, *i, ZigKind::Comment));
    }
    if b == b'\\' && bytes.get(*i + 1) == Some(&b'\\') {
        *i += 2;
        while *i < bytes.len() && !is_newline(bytes[*i]) {
            *i += 1;
        }
        return Ok(tok(start, *i, ZigKind::String));
    }
    if b == b'"' {
        *i += 1;
        let _closed = scan_quoted(bytes, i, b'"');
        if *i as u32 <= start {
            return Err(LexError::Nonprogress { at: start });
        }
        return Ok(tok(start, *i, ZigKind::String));
    }
    if b == b'\'' {
        let saved = *i;
        *i += 1;
        let closed = scan_quoted(bytes, i, b'\'');
        if !closed {
            *i = saved + 1;
            return Ok(tok(start, *i, ZigKind::Unknown));
        }
        if *i as u32 <= start {
            return Err(LexError::Nonprogress { at: start });
        }
        return Ok(tok(start, *i, ZigKind::Char));
    }
    if b == b'@' && bytes.get(*i + 1) == Some(&b'"') {
        *i += 2;
        let _closed = scan_quoted(bytes, i, b'"');
        if *i as u32 <= start {
            return Err(LexError::Nonprogress { at: start });
        }
        return Ok(tok(start, *i, ZigKind::Identifier));
    }
    if b == b'@' && bytes.get(*i + 1).copied().is_some_and(is_ident_start) {
        *i += 1;
        while *i < bytes.len() && is_ident_continue(bytes[*i]) {
            *i += 1;
        }
        return Ok(tok(start, *i, ZigKind::Builtin));
    }
    if is_ident_start(b) {
        *i += 1;
        while *i < bytes.len() && is_ident_continue(bytes[*i]) {
            *i += 1;
        }
        let ident = std::str::from_utf8(&bytes[start as usize..*i]).unwrap_or("");
        let kind = if is_keyword(ident) {
            ZigKind::Keyword
        } else {
            ZigKind::Identifier
        };
        return Ok(tok(start, *i, kind));
    }
    if b.is_ascii_digit() {
        *i = scan_number(bytes, *i);
        return Ok(tok(start, *i, ZigKind::Number));
    }
    if matches!(b, b'(' | b')' | b'{' | b'}' | b'[' | b']') {
        *i += 1;
        return Ok(tok(start, *i, ZigKind::Delimiter));
    }
    if is_punct_start(b) {
        let n = punct_len(bytes, *i);
        *i += n;
        return Ok(tok(start, *i, ZigKind::Punctuator));
    }
    *i += 1;
    if *i == start_i {
        return Err(LexError::Nonprogress { at: start });
    }
    Ok(tok(start, *i, ZigKind::Unknown))
}

fn tok(start: u32, end: usize, kind: ZigKind) -> ZigToken {
    ZigToken {
        start,
        end: end as u32,
        kind,
    }
}

/// Tokenize an entire document. Spans are byte offsets into `bytes`.
pub fn lex_document_cancellable(
    bytes: &[u8],
    cancel: &dyn Fn() -> bool,
) -> Result<Vec<ZigToken>, LexError> {
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
        match lex_one(bytes, &mut i) {
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
                    push_token(&mut tokens, start, i, ZigKind::Newline)?;
                    continue;
                }
                if i == start_i && i < bytes.len() {
                    i += 1;
                    push_token(&mut tokens, at, i, ZigKind::Unknown)?;
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
pub fn lex_line(line: &str, incoming: ZigState) -> (ZigState, Vec<(usize, TokenRole)>) {
    let bytes = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        let start = i;
        match lex_one(bytes, &mut i) {
            Ok(t) => {
                let end = (t.end as usize).min(bytes.len()).max(start);
                if end > start {
                    let role = if matches!(
                        t.kind,
                        ZigKind::Keyword | ZigKind::Identifier | ZigKind::Builtin
                    ) {
                        let ident = std::str::from_utf8(&bytes[t.start as usize..end]).unwrap_or("");
                        let mut k = end;
                        while k < bytes.len() && is_space(bytes[k]) {
                            k += 1;
                        }
                        let next_is_paren = bytes.get(k) == Some(&b'(');
                        classify_identifier(ident, t.kind, next_is_paren)
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
    if i < bytes.len() {
        push_role(bytes.len(), TokenRole::Unknown, &mut out);
    }
    (incoming, out)
}

/// Convert document tokens into public spans, classifying keywords and
/// function names with one-token lookahead. `(` after a newline is not a
/// Function role on the preceding identifier.
pub fn document_spans(bytes: &[u8], tokens: &[ZigToken]) -> Vec<TokenSpan> {
    let mut out = Vec::with_capacity(tokens.len());
    for (i, t) in tokens.iter().enumerate() {
        let mut role = t.kind.role();
        if matches!(
            t.kind,
            ZigKind::Keyword | ZigKind::Identifier | ZigKind::Builtin
        ) {
            let mut j = i + 1;
            let mut next_is_paren = false;
            while j < tokens.len() {
                match tokens[j].kind {
                    ZigKind::Newline => break,
                    ZigKind::Whitespace | ZigKind::Comment => j += 1,
                    ZigKind::Delimiter
                        if bytes.get(tokens[j].start as usize) == Some(&b'(') =>
                    {
                        next_is_paren = true;
                        break;
                    }
                    _ => break,
                }
            }
            let ident = std::str::from_utf8(&bytes[t.start as usize..t.end as usize]).unwrap_or("");
            role = classify_identifier(ident, t.kind, next_is_paren);
        }
        out.push(TokenSpan::new(t.start, t.end, role));
    }
    out
}
