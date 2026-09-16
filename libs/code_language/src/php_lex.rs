//! Source-only PHP lexer shared by the editor and the PHP frontend.
//! Byte offsets address the original document. Continuation state is owned
//! here: inline HTML, code, block comments, quoted strings, heredocs/nowdocs
//! and backticks survive line breaks.
//!
//! A PHP file starts in inline HTML. Inline HTML is one token per line
//! segment and is not tokenised as HTML (the HTML lexer cannot stop at `<?`).

use crate::token::{TokenRole, TokenSpan};
use crate::LexError;

/// Version of this lexer; part of parse and search cache identity.
pub const PHP_LEXER_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
enum PhpMode {
    #[default]
    InlineHtml,
    Code,
    BlockComment,
    SingleString,
    DoubleString,
    Heredoc,
    Backtick,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PhpState {
    mode: PhpMode,
    /// Heredoc/nowdoc label bytes; significant only in [`PhpMode::Heredoc`].
    label: [u8; 24],
    label_len: u8,
    nowdoc: bool,
}

impl Default for PhpState {
    fn default() -> Self {
        PhpState {
            mode: PhpMode::InlineHtml,
            label: [0; 24],
            label_len: 0,
            nowdoc: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhpKind {
    Whitespace,
    Comment,
    InlineHtml,
    OpenTag,
    CloseTag,
    Identifier,
    Variable,
    Keyword,
    Number,
    String,
    Punctuator,
    Delimiter,
    Attribute,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PhpToken {
    pub start: u32,
    pub end: u32,
    pub kind: PhpKind,
}

impl PhpKind {
    pub fn role(self) -> TokenRole {
        match self {
            PhpKind::Whitespace => TokenRole::Whitespace,
            PhpKind::Comment => TokenRole::Comment,
            PhpKind::InlineHtml => TokenRole::Unknown,
            PhpKind::OpenTag | PhpKind::CloseTag | PhpKind::Attribute => TokenRole::Preprocessor,
            PhpKind::Identifier | PhpKind::Variable => TokenRole::Identifier,
            PhpKind::Keyword => TokenRole::Keyword,
            PhpKind::Number => TokenRole::Number,
            PhpKind::String => TokenRole::String,
            PhpKind::Punctuator => TokenRole::Punctuator,
            PhpKind::Delimiter => TokenRole::Delimiter,
            PhpKind::Unknown => TokenRole::Unknown,
        }
    }
}

/// PHP keywords, literals and magic constants, stored lowercase, sorted for
/// binary search. Matching is ASCII case-insensitive.
const KEYWORDS: &[&str] = &[
    "__class__",
    "__dir__",
    "__file__",
    "__function__",
    "__line__",
    "__method__",
    "__namespace__",
    "__trait__",
    "abstract",
    "and",
    "array",
    "as",
    "break",
    "callable",
    "case",
    "catch",
    "class",
    "clone",
    "const",
    "continue",
    "declare",
    "default",
    "do",
    "echo",
    "else",
    "elseif",
    "empty",
    "enddeclare",
    "endfor",
    "endforeach",
    "endif",
    "endswitch",
    "endwhile",
    "enum",
    "eval",
    "exit",
    "extends",
    "false",
    "final",
    "finally",
    "fn",
    "for",
    "foreach",
    "function",
    "global",
    "goto",
    "if",
    "implements",
    "include",
    "include_once",
    "instanceof",
    "insteadof",
    "interface",
    "isset",
    "list",
    "match",
    "namespace",
    "new",
    "null",
    "or",
    "print",
    "private",
    "protected",
    "public",
    "readonly",
    "require",
    "require_once",
    "return",
    "static",
    "switch",
    "throw",
    "trait",
    "true",
    "try",
    "unset",
    "use",
    "var",
    "while",
    "xor",
    "yield",
];

const TYPE_NAMES: &[&str] = &[
    "array",
    "binary",
    "bool",
    "boolean",
    "callable",
    "double",
    "float",
    "int",
    "integer",
    "iterable",
    "mixed",
    "never",
    "object",
    "parent",
    "real",
    "self",
    "string",
    "unset",
    "void",
];

pub fn is_keyword(ident: &str) -> bool {
    keyword_lookup(ident).is_some()
}

fn keyword_lookup(ident: &str) -> Option<&'static str> {
    let mut buf = [0u8; 32];
    if ident.len() > buf.len() {
        return None;
    }
    for (i, b) in ident.bytes().enumerate() {
        buf[i] = b.to_ascii_lowercase();
    }
    let lower = std::str::from_utf8(&buf[..ident.len()]).ok()?;
    KEYWORDS.binary_search(&lower).ok().map(|i| KEYWORDS[i])
}

fn is_type_name(ident: &str) -> bool {
    let mut buf = [0u8; 16];
    if ident.len() > buf.len() {
        return false;
    }
    for (i, b) in ident.bytes().enumerate() {
        buf[i] = b.to_ascii_lowercase();
    }
    let lower = match std::str::from_utf8(&buf[..ident.len()]) {
        Ok(s) => s,
        Err(_) => return false,
    };
    TYPE_NAMES.binary_search(&lower).is_ok()
}

fn eq_ci(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.eq_ignore_ascii_case(y))
}

fn is_space(b: u8) -> bool {
    b == b' ' || b == b'\t' || b == 0x0b || b == 0x0c
}

fn is_newline(b: u8) -> bool {
    b == b'\n' || b == b'\r'
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || b >= 0x80
}

fn is_ident_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80
}

fn consume_newline(bytes: &[u8], i: &mut usize) -> bool {
    if *i >= bytes.len() {
        return false;
    }
    if bytes[*i] == b'\r' {
        *i += 1;
        if bytes.get(*i) == Some(&b'\n') {
            *i += 1;
        }
        return true;
    }
    if bytes[*i] == b'\n' {
        *i += 1;
        return true;
    }
    false
}

fn skip_spaces(bytes: &[u8], mut i: usize) -> usize {
    while i < bytes.len() && is_space(bytes[i]) {
        i += 1;
    }
    i
}

fn tok(start: u32, end: usize, kind: PhpKind) -> PhpToken {
    PhpToken {
        start,
        end: end as u32,
        kind,
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

/// `<?php` (case-insensitive, then whitespace/EOF/`?`), `<?=`, or `<?` plus
/// whitespace. Returns the tag length in bytes.
fn open_tag_len(bytes: &[u8], i: usize) -> Option<usize> {
    if bytes.get(i) != Some(&b'<') || bytes.get(i + 1) != Some(&b'?') {
        return None;
    }
    if bytes.get(i + 2) == Some(&b'=') {
        return Some(3);
    }
    if i + 5 <= bytes.len() && eq_ci(&bytes[i + 2..i + 5], b"php") {
        let next = bytes.get(i + 5).copied();
        if next.is_none()
            || next.is_some_and(|b| is_space(b) || is_newline(b) || b == b'?')
        {
            return Some(5);
        }
        return None;
    }
    match bytes.get(i + 2) {
        Some(&b) if is_space(b) || is_newline(b) => Some(2),
        None => None,
        _ => None,
    }
}

fn close_tag_end(bytes: &[u8], i: usize) -> Option<usize> {
    if bytes.get(i) == Some(&b'?') && bytes.get(i + 1) == Some(&b'>') {
        let mut j = i + 2;
        consume_newline(bytes, &mut j);
        Some(j)
    } else {
        None
    }
}

fn looks_like_enum(bytes: &[u8], after_ident: usize) -> bool {
    let mut j = after_ident;
    while j < bytes.len() && (is_space(bytes[j]) || is_newline(bytes[j])) {
        j += 1;
    }
    if j >= bytes.len() || !is_ident_start(bytes[j]) {
        return false;
    }
    j += 1;
    while j < bytes.len() && is_ident_continue(bytes[j]) {
        j += 1;
    }
    while j < bytes.len() && (is_space(bytes[j]) || is_newline(bytes[j])) {
        j += 1;
    }
    if bytes.get(j) == Some(&b'{') || bytes.get(j) == Some(&b':') {
        return true;
    }
    j + 10 <= bytes.len() && eq_ci(&bytes[j..j + 10], b"implements")
}

fn scan_number(bytes: &[u8], mut i: usize) -> usize {
    if bytes[i] == b'0'
        && matches!(
            bytes.get(i + 1).map(|b| b.to_ascii_lowercase()),
            Some(b'x' | b'b' | b'o')
        )
    {
        let kind = bytes[i + 1].to_ascii_lowercase();
        i += 2;
        match kind {
            b'x' => {
                while i < bytes.len() && (bytes[i].is_ascii_hexdigit() || bytes[i] == b'_') {
                    i += 1;
                }
            }
            b'b' => {
                while i < bytes.len() && (bytes[i] == b'0' || bytes[i] == b'1' || bytes[i] == b'_')
                {
                    i += 1;
                }
            }
            _ => {
                while i < bytes.len() && ((b'0'..=b'7').contains(&bytes[i]) || bytes[i] == b'_') {
                    i += 1;
                }
            }
        }
        return i;
    }
    if bytes[i] == b'.' {
        i += 1;
        while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'_') {
            i += 1;
        }
        return consume_exponent(bytes, i);
    }
    while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'_') {
        i += 1;
    }
    if bytes.get(i) == Some(&b'.') {
        i += 1;
        while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'_') {
            i += 1;
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
    if rest.starts_with(b"?->")
        || rest.starts_with(b"<=>")
        || rest.starts_with(b"**=")
        || rest.starts_with(b"??=")
        || rest.starts_with(b"<<=")
        || rest.starts_with(b">>=")
        || rest.starts_with(b"===")
        || rest.starts_with(b"!==")
        || rest.starts_with(b"...")
    {
        return 3;
    }
    if matches!(
        rest.get(..2),
        Some(
            b"->" | b"::"
                | b"=>"
                | b"**"
                | b"??"
                | b"<<"
                | b">>"
                | b"=="
                | b"!="
                | b"<>"
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
                | b".="
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

fn heredoc_close_end(bytes: &[u8], i: usize, state: &PhpState) -> Option<usize> {
    if state.label_len == 0 {
        return None;
    }
    let j = skip_spaces(bytes, i);
    let n = state.label_len as usize;
    if j + n > bytes.len() {
        return None;
    }
    if bytes[j..j + n] != state.label[..n] {
        return None;
    }
    let after = bytes.get(j + n).copied();
    if after.is_some_and(is_ident_continue) {
        return None;
    }
    Some(j + n)
}

fn interpolation_start(bytes: &[u8], i: usize) -> bool {
    if bytes.get(i) != Some(&b'$') && bytes.get(i) != Some(&b'{') {
        return false;
    }
    if bytes[i] == b'{' {
        return bytes.get(i + 1) == Some(&b'$');
    }
    let next = bytes.get(i + 1).copied().unwrap_or(0);
    is_ident_start(next) || next == b'{' || next == b'$'
}

/// Consume one interpolated variable form. `i` is at `$` or `{`.
fn consume_interpolation(bytes: &[u8], i: &mut usize) {
    if bytes.get(*i) == Some(&b'{') && bytes.get(*i + 1) == Some(&b'$') {
        *i += 1;
        let mut depth = 1i32;
        while *i < bytes.len() && depth > 0 {
            let b = bytes[*i];
            if b == b'\\' && *i + 1 < bytes.len() {
                *i += 2;
                continue;
            }
            if b == b'\'' || b == b'"' {
                let q = b;
                *i += 1;
                while *i < bytes.len() && bytes[*i] != q {
                    if bytes[*i] == b'\\' && *i + 1 < bytes.len() {
                        *i += 2;
                    } else {
                        *i += 1;
                    }
                }
                if *i < bytes.len() {
                    *i += 1;
                }
                continue;
            }
            if b == b'{' {
                depth += 1;
            } else if b == b'}' {
                depth -= 1;
            }
            *i += 1;
        }
        return;
    }
    if bytes.get(*i) != Some(&b'$') {
        return;
    }
    while *i < bytes.len() && bytes[*i] == b'$' {
        *i += 1;
    }
    if *i < bytes.len() && bytes[*i] == b'{' {
        *i += 1;
        let mut depth = 1i32;
        while *i < bytes.len() && depth > 0 {
            if bytes[*i] == b'{' {
                depth += 1;
            } else if bytes[*i] == b'}' {
                depth -= 1;
            }
            *i += 1;
        }
        return;
    }
    if *i >= bytes.len() || !is_ident_start(bytes[*i]) {
        return;
    }
    *i += 1;
    while *i < bytes.len() && is_ident_continue(bytes[*i]) {
        *i += 1;
    }
    if bytes.get(*i) == Some(&b'?') && bytes.get(*i + 1) == Some(&b'-') && bytes.get(*i + 2) == Some(&b'>')
    {
        let mut j = *i + 3;
        if j < bytes.len() && is_ident_start(bytes[j]) {
            j += 1;
            while j < bytes.len() && is_ident_continue(bytes[j]) {
                j += 1;
            }
            *i = j;
        }
    } else if bytes.get(*i) == Some(&b'-') && bytes.get(*i + 1) == Some(&b'>') {
        let mut j = *i + 2;
        if j < bytes.len() && is_ident_start(bytes[j]) {
            j += 1;
            while j < bytes.len() && is_ident_continue(bytes[j]) {
                j += 1;
            }
            *i = j;
        }
    }
    if bytes.get(*i) == Some(&b'[') {
        let mut depth = 1i32;
        *i += 1;
        while *i < bytes.len() && depth > 0 {
            if bytes[*i] == b'[' {
                depth += 1;
            } else if bytes[*i] == b']' {
                depth -= 1;
            }
            *i += 1;
        }
    }
}

fn try_open_heredoc(bytes: &[u8], i: usize) -> Option<(usize, [u8; 24], u8, bool)> {
    if !bytes[i..].starts_with(b"<<<") {
        return None;
    }
    let mut j = skip_spaces(bytes, i + 3);
    let mut nowdoc = false;
    let quote = bytes.get(j).copied();
    if quote == Some(b'\'') || quote == Some(b'"') {
        nowdoc = quote == Some(b'\'');
        j += 1;
    }
    if j >= bytes.len() || !is_ident_start(bytes[j]) {
        return None;
    }
    let label_start = j;
    j += 1;
    while j < bytes.len() && is_ident_continue(bytes[j]) {
        j += 1;
    }
    let label_len = j - label_start;
    if label_len == 0 || label_len > 24 {
        return None;
    }
    if quote == Some(b'\'') || quote == Some(b'"') {
        if bytes.get(j) != quote.as_ref() {
            return None;
        }
        j += 1;
    }
    let mut label = [0u8; 24];
    label[..label_len].copy_from_slice(&bytes[label_start..label_start + label_len]);
    Some((j, label, label_len as u8, nowdoc))
}

fn classify_identifier(ident: &str, next_is_paren: bool, type_position: bool, is_cast: bool) -> TokenRole {
    let kw = keyword_lookup(ident);
    match kw {
        Some("true" | "false" | "null" | "__class__" | "__dir__" | "__file__" | "__function__"
            | "__line__" | "__method__" | "__namespace__" | "__trait__") => {
            return TokenRole::Constant;
        }
        _ => {}
    }
    if (type_position || is_cast) && is_type_name(ident) {
        return TokenRole::Typename;
    }
    match kw {
        Some("if" | "else" | "elseif" | "endif" | "switch" | "case" | "default" | "match"
            | "try" | "catch" | "finally" | "return" | "throw" | "exit") => {
            TokenRole::BranchKeyword
        }
        Some("for" | "foreach" | "while" | "do" | "endfor" | "endforeach" | "endwhile"
            | "break" | "continue") => TokenRole::LoopKeyword,
        Some(_) => TokenRole::Keyword,
        None if next_is_paren => TokenRole::Function,
        None if ident
            .chars()
            .next()
            .map(|c| c.is_ascii_uppercase())
            .unwrap_or(false) =>
        {
            TokenRole::Typename
        }
        None => TokenRole::Identifier,
    }
}

fn next_non_space(bytes: &[u8], mut i: usize) -> Option<u8> {
    while i < bytes.len() && is_space(bytes[i]) {
        i += 1;
    }
    bytes.get(i).copied()
}

/// Shared automaton step. Emits one token covering `[start, *i)`.
fn lex_one(
    bytes: &[u8],
    i: &mut usize,
    state: &mut PhpState,
    line_start: &mut bool,
) -> Result<PhpToken, LexError> {
    let start_i = *i;
    let start = *i as u32;
    if *i >= bytes.len() {
        return Err(LexError::Nonprogress { at: start });
    }

    match state.mode {
        PhpMode::InlineHtml => {
            if let Some(n) = open_tag_len(bytes, *i) {
                *i += n;
                state.mode = PhpMode::Code;
                *line_start = false;
                return Ok(tok(start, *i, PhpKind::OpenTag));
            }
            while *i < bytes.len() {
                if open_tag_len(bytes, *i).is_some() {
                    break;
                }
                if is_newline(bytes[*i]) {
                    consume_newline(bytes, i);
                    *line_start = true;
                    break;
                }
                *line_start = false;
                *i += 1;
            }
            if (*i as u32) <= start {
                return Err(LexError::Nonprogress { at: start });
            }
            return Ok(tok(start, *i, PhpKind::InlineHtml));
        }
        PhpMode::BlockComment => {
            while *i < bytes.len() {
                if bytes[*i] == b'*' && bytes.get(*i + 1) == Some(&b'/') {
                    *i += 2;
                    state.mode = PhpMode::Code;
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
            return Ok(tok(start, *i, PhpKind::Comment));
        }
        PhpMode::SingleString => {
            while *i < bytes.len() {
                let b = bytes[*i];
                if b == b'\\' && bytes.get(*i + 1).is_some_and(|n| *n == b'\\' || *n == b'\'') {
                    *i += 2;
                    *line_start = false;
                    continue;
                }
                if b == b'\'' {
                    *i += 1;
                    state.mode = PhpMode::Code;
                    *line_start = false;
                    break;
                }
                if is_newline(b) {
                    consume_newline(bytes, i);
                    *line_start = true;
                    continue;
                }
                *line_start = false;
                *i += 1;
            }
            if (*i as u32) <= start {
                return Err(LexError::Nonprogress { at: start });
            }
            return Ok(tok(start, *i, PhpKind::String));
        }
        PhpMode::DoubleString | PhpMode::Backtick | PhpMode::Heredoc => {
            return lex_interpolated(bytes, i, state, line_start, start, start_i);
        }
        PhpMode::Code => {}
    }

    let b = bytes[*i];
    if b == b'\r' || b == b'\n' {
        consume_newline(bytes, i);
        *line_start = true;
        return Ok(tok(start, *i, PhpKind::Whitespace));
    }
    if is_space(b) {
        *i += 1;
        while *i < bytes.len() && is_space(bytes[*i]) {
            *i += 1;
        }
        return Ok(tok(start, *i, PhpKind::Whitespace));
    }
    *line_start = false;

    if let Some(end) = close_tag_end(bytes, *i) {
        *i = end;
        state.mode = PhpMode::InlineHtml;
        *line_start = bytes.get(end.saturating_sub(1)).is_some_and(|&c| is_newline(c));
        return Ok(tok(start, *i, PhpKind::CloseTag));
    }

    if b == b'/' && bytes.get(*i + 1) == Some(&b'/') {
        *i += 2;
        while *i < bytes.len() && !is_newline(bytes[*i]) {
            if close_tag_end(bytes, *i).is_some() {
                break;
            }
            *i += 1;
        }
        return Ok(tok(start, *i, PhpKind::Comment));
    }
    if b == b'#' && bytes.get(*i + 1) != Some(&b'[') {
        *i += 1;
        while *i < bytes.len() && !is_newline(bytes[*i]) {
            if close_tag_end(bytes, *i).is_some() {
                break;
            }
            *i += 1;
        }
        return Ok(tok(start, *i, PhpKind::Comment));
    }
    if b == b'/' && bytes.get(*i + 1) == Some(&b'*') {
        *i += 2;
        state.mode = PhpMode::BlockComment;
        while *i < bytes.len() {
            if bytes[*i] == b'*' && bytes.get(*i + 1) == Some(&b'/') {
                *i += 2;
                state.mode = PhpMode::Code;
                break;
            }
            *i += 1;
        }
        return Ok(tok(start, *i, PhpKind::Comment));
    }
    if b == b'#' && bytes.get(*i + 1) == Some(&b'[') {
        *i += 2;
        return Ok(tok(start, *i, PhpKind::Attribute));
    }
    if b == b'\'' {
        *i += 1;
        state.mode = PhpMode::SingleString;
        while *i < bytes.len() {
            let c = bytes[*i];
            if c == b'\\' && bytes.get(*i + 1).is_some_and(|n| *n == b'\\' || *n == b'\'') {
                *i += 2;
                continue;
            }
            if c == b'\'' {
                *i += 1;
                state.mode = PhpMode::Code;
                break;
            }
            if is_newline(c) {
                consume_newline(bytes, i);
                *line_start = true;
                continue;
            }
            *i += 1;
        }
        return Ok(tok(start, *i, PhpKind::String));
    }
    if b == b'"' {
        *i += 1;
        state.mode = PhpMode::DoubleString;
        return lex_interpolated(bytes, i, state, line_start, start, start_i);
    }
    if b == b'`' {
        *i += 1;
        state.mode = PhpMode::Backtick;
        return lex_interpolated(bytes, i, state, line_start, start, start_i);
    }
    if bytes[*i..].starts_with(b"<<<") {
        if let Some((after_label, label, len, nowdoc)) = try_open_heredoc(bytes, *i) {
            *i = after_label;
            while *i < bytes.len() && is_space(bytes[*i]) {
                *i += 1;
            }
            consume_newline(bytes, i);
            *line_start = true;
            state.mode = PhpMode::Heredoc;
            state.label = label;
            state.label_len = len;
            state.nowdoc = nowdoc;
            return lex_interpolated(bytes, i, state, line_start, start, start_i);
        }
        *i += 3;
        return Ok(tok(start, *i, PhpKind::Punctuator));
    }
    if b == b'$' {
        let mut j = *i;
        while j < bytes.len() && bytes[j] == b'$' {
            j += 1;
        }
        if j < bytes.len() && is_ident_start(bytes[j]) {
            j += 1;
            while j < bytes.len() && is_ident_continue(bytes[j]) {
                j += 1;
            }
            *i = j;
            return Ok(tok(start, *i, PhpKind::Variable));
        }
        *i += 1;
        return Ok(tok(start, *i, PhpKind::Punctuator));
    }
    if is_ident_start(b) {
        *i += 1;
        while *i < bytes.len() && is_ident_continue(bytes[*i]) {
            *i += 1;
        }
        let ident = std::str::from_utf8(&bytes[start as usize..*i]).unwrap_or("");
        let kind = if keyword_lookup(ident).is_some() {
            if ident.eq_ignore_ascii_case("enum") && !looks_like_enum(bytes, *i) {
                PhpKind::Identifier
            } else {
                PhpKind::Keyword
            }
        } else {
            PhpKind::Identifier
        };
        return Ok(tok(start, *i, kind));
    }
    if b.is_ascii_digit() || (b == b'.' && bytes.get(*i + 1).copied().unwrap_or(0).is_ascii_digit())
    {
        *i = scan_number(bytes, *i);
        return Ok(tok(start, *i, PhpKind::Number));
    }
    let n = punct_len(bytes, *i);
    *i += n;
    let kind = if n == 1 && matches!(b, b'(' | b')' | b'{' | b'}' | b'[' | b']') {
        PhpKind::Delimiter
    } else if b.is_ascii_graphic() {
        PhpKind::Punctuator
    } else {
        PhpKind::Unknown
    };
    if *i == start_i {
        return Err(LexError::Nonprogress { at: start });
    }
    Ok(tok(start, *i, kind))
}

fn lex_interpolated(
    bytes: &[u8],
    i: &mut usize,
    state: &mut PhpState,
    line_start: &mut bool,
    start: u32,
    start_i: usize,
) -> Result<PhpToken, LexError> {
    let quote = match state.mode {
        PhpMode::DoubleString => Some(b'"'),
        PhpMode::Backtick => Some(b'`'),
        _ => None,
    };
    let interpolate = !(state.mode == PhpMode::Heredoc && state.nowdoc);
    if state.mode == PhpMode::Heredoc && *line_start {
        if let Some(end) = heredoc_close_end(bytes, *i, state) {
            *i = end;
            state.mode = PhpMode::Code;
            state.label_len = 0;
            *line_start = false;
            if (*i as u32) <= start {
                return Err(LexError::Nonprogress { at: start });
            }
            return Ok(tok(start, *i, PhpKind::String));
        }
    }
    if interpolate && interpolation_start(bytes, *i) && *i == start_i {
        consume_interpolation(bytes, i);
        *line_start = false;
        if (*i as u32) <= start {
            return Err(LexError::Nonprogress { at: start });
        }
        return Ok(tok(start, *i, PhpKind::Variable));
    }
    while *i < bytes.len() {
        if state.mode == PhpMode::Heredoc && *line_start {
            if let Some(end) = heredoc_close_end(bytes, *i, state) {
                *i = end;
                state.mode = PhpMode::Code;
                state.label_len = 0;
                *line_start = false;
                break;
            }
        }
        if interpolate && interpolation_start(bytes, *i) {
            if *i > start_i {
                break;
            }
            consume_interpolation(bytes, i);
            *line_start = false;
            if (*i as u32) <= start {
                return Err(LexError::Nonprogress { at: start });
            }
            return Ok(tok(start, *i, PhpKind::Variable));
        }
        let b = bytes[*i];
        if b == b'\\' && *i + 1 < bytes.len() {
            *i += 1;
            if *i < bytes.len() && !is_newline(bytes[*i]) {
                *i += 1;
            }
            *line_start = false;
            continue;
        }
        if let Some(q) = quote {
            if b == q {
                *i += 1;
                state.mode = PhpMode::Code;
                *line_start = false;
                break;
            }
        }
        if is_newline(b) {
            consume_newline(bytes, i);
            *line_start = true;
            continue;
        }
        *line_start = false;
        *i += 1;
    }
    if (*i as u32) <= start {
        return Err(LexError::Nonprogress { at: start });
    }
    Ok(tok(start, *i, PhpKind::String))
}

/// Tokenize an entire document. Spans are byte offsets into `bytes`.
pub fn lex_document_cancellable(
    bytes: &[u8],
    cancel: &dyn Fn() -> bool,
) -> Result<Vec<PhpToken>, LexError> {
    let mut tokens = Vec::new();
    let mut state = PhpState::default();
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
                if i == start_i && i < bytes.len() {
                    i += 1;
                    tokens.push(PhpToken {
                        start: at,
                        end: i as u32,
                        kind: PhpKind::Unknown,
                    });
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
pub fn lex_line(line: &str, incoming: PhpState) -> (PhpState, Vec<(usize, TokenRole)>) {
    let bytes = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    let mut state = incoming;
    let mut line_start = true;
    while i < bytes.len() {
        let start = i;
        match lex_one(bytes, &mut i, &mut state, &mut line_start) {
            Ok(t) => {
                let end = (t.end as usize).min(bytes.len()).max(start);
                if end > start {
                    let role = role_for_line(bytes, start, end, t.kind);
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
        let role = match state.mode {
            PhpMode::BlockComment => TokenRole::Comment,
            PhpMode::InlineHtml => TokenRole::Unknown,
            PhpMode::Code => TokenRole::Unknown,
            PhpMode::SingleString
            | PhpMode::DoubleString
            | PhpMode::Heredoc
            | PhpMode::Backtick => TokenRole::String,
        };
        push_role(bytes.len(), role, &mut out);
    }
    (state, out)
}

fn role_for_line(bytes: &[u8], start: usize, end: usize, kind: PhpKind) -> TokenRole {
    if kind == PhpKind::Keyword || kind == PhpKind::Identifier {
        let ident = std::str::from_utf8(&bytes[start..end]).unwrap_or("");
        let next_is_paren = next_non_space(bytes, end) == Some(b'(');
        let prev = {
            let mut p = start;
            while p > 0 && is_space(bytes[p - 1]) {
                p -= 1;
            }
            if p == 0 {
                None
            } else {
                Some(bytes[p - 1])
            }
        };
        let next = next_non_space(bytes, end);
        let type_position = matches!(prev, Some(b':' | b'|' | b'&' | b'?'))
            || next == Some(b'$');
        let is_cast = prev == Some(b'(') && next == Some(b')');
        classify_identifier(ident, next_is_paren, type_position, is_cast)
    } else {
        kind.role()
    }
}

fn prev_non_trivia(tokens: &[PhpToken], i: usize) -> Option<usize> {
    let mut j = i;
    while j > 0 {
        j -= 1;
        if !matches!(tokens[j].kind, PhpKind::Whitespace | PhpKind::Comment) {
            return Some(j);
        }
    }
    None
}

fn next_non_trivia(tokens: &[PhpToken], i: usize) -> Option<usize> {
    let mut j = i + 1;
    while j < tokens.len() {
        if !matches!(tokens[j].kind, PhpKind::Whitespace | PhpKind::Comment) {
            return Some(j);
        }
        j += 1;
    }
    None
}

/// Convert document tokens into public spans, classifying keywords and
/// function names with one-token lookahead.
pub fn document_spans(bytes: &[u8], tokens: &[PhpToken]) -> Vec<TokenSpan> {
    let mut out = Vec::with_capacity(tokens.len());
    for (i, t) in tokens.iter().enumerate() {
        let mut role = t.kind.role();
        if t.kind == PhpKind::Keyword || t.kind == PhpKind::Identifier {
            let ident = std::str::from_utf8(&bytes[t.start as usize..t.end as usize]).unwrap_or("");
            let next = next_non_trivia(tokens, i);
            let next_is_paren = next.is_some_and(|j| {
                tokens[j].kind == PhpKind::Delimiter
                    && bytes.get(tokens[j].start as usize) == Some(&b'(')
            });
            let prev = prev_non_trivia(tokens, i);
            let prev_byte = prev.and_then(|j| bytes.get(tokens[j].start as usize).copied());
            let next_byte = next.and_then(|j| bytes.get(tokens[j].start as usize).copied());
            let type_position = matches!(prev_byte, Some(b':' | b'|' | b'&' | b'?'))
                || next.is_some_and(|j| tokens[j].kind == PhpKind::Variable);
            let is_cast = prev_byte == Some(b'(')
                && next_byte == Some(b')')
                && next.is_some_and(|j| tokens[j].kind == PhpKind::Delimiter)
                && prev.is_some_and(|j| tokens[j].kind == PhpKind::Delimiter);
            role = classify_identifier(ident, next_is_paren, type_position, is_cast);
        }
        out.push(TokenSpan::new(t.start, t.end, role));
    }
    out
}
