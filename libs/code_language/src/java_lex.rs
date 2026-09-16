//! Source-only Java lexer shared by the editor and the Java frontend.
//! Byte offsets address the original document. Continuation state is owned
//! here: block comments and text blocks survive line breaks; regular strings
//! and char literals are line-bounded.
//!
//! Unicode escapes (`\uXXXX`) outside string, char and text-block literals are
//! not translated: the backslash is an [`JavaKind::Unknown`] byte. The frontend
//! counts them. Inside literals they are consumed as escapes so an escaped
//! quote does not end the token.

use crate::token::{TokenRole, TokenSpan};
use crate::LexError;

/// Version of this lexer; part of parse and search cache identity.
pub const JAVA_LEXER_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
enum JavaMode {
    #[default]
    Normal,
    BlockComment,
    RegularString,
    Char,
    TextBlock,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct JavaState {
    mode: JavaMode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JavaKind {
    Whitespace,
    Comment,
    Identifier,
    Keyword,
    Annotation,
    Number,
    String,
    Char,
    Punctuator,
    Delimiter,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JavaToken {
    pub start: u32,
    pub end: u32,
    pub kind: JavaKind,
}

impl JavaKind {
    pub fn role(self) -> TokenRole {
        match self {
            JavaKind::Whitespace => TokenRole::Whitespace,
            JavaKind::Comment => TokenRole::Comment,
            JavaKind::Identifier => TokenRole::Identifier,
            JavaKind::Keyword => TokenRole::Keyword,
            JavaKind::Annotation => TokenRole::Preprocessor,
            JavaKind::Number => TokenRole::Number,
            JavaKind::String => TokenRole::String,
            JavaKind::Char => TokenRole::Char,
            JavaKind::Punctuator => TokenRole::Punctuator,
            JavaKind::Delimiter => TokenRole::Delimiter,
            JavaKind::Unknown => TokenRole::Unknown,
        }
    }
}

/// JLS reserved words plus the literals `true` `false` `null` and `_`.
/// Contextual keywords stay Identifier; the parser decides. Sorted for
/// binary search.
const KEYWORDS: &[&str] = &[
    "_",
    "abstract",
    "assert",
    "boolean",
    "break",
    "byte",
    "case",
    "catch",
    "char",
    "class",
    "const",
    "continue",
    "default",
    "do",
    "double",
    "else",
    "enum",
    "extends",
    "false",
    "final",
    "finally",
    "float",
    "for",
    "goto",
    "if",
    "implements",
    "import",
    "instanceof",
    "int",
    "interface",
    "long",
    "native",
    "new",
    "null",
    "package",
    "private",
    "protected",
    "public",
    "return",
    "short",
    "static",
    "strictfp",
    "super",
    "switch",
    "synchronized",
    "this",
    "throw",
    "throws",
    "transient",
    "true",
    "try",
    "void",
    "volatile",
    "while",
];

pub fn is_keyword(ident: &str) -> bool {
    KEYWORDS.binary_search(&ident).is_ok()
}

fn classify_identifier(ident: &str, next_is_paren: bool) -> TokenRole {
    match ident {
        "if" | "else" | "switch" | "case" | "default" | "try" | "catch" | "finally" | "return"
        | "throw" | "assert" => TokenRole::BranchKeyword,
        "for" | "while" | "do" | "break" | "continue" => TokenRole::LoopKeyword,
        "true" | "false" | "null" => TokenRole::Constant,
        "boolean" | "byte" | "char" | "short" | "int" | "long" | "float" | "double" | "void"
        | "var" => TokenRole::Typename,
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

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || b == b'$' || b >= 0x80
}

fn is_ident_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$' || b >= 0x80
}

fn is_newline(b: u8) -> bool {
    b == b'\n' || b == b'\r'
}

fn push_token(
    tokens: &mut Vec<JavaToken>,
    start: u32,
    end: usize,
    kind: JavaKind,
) -> Result<(), LexError> {
    if end as u32 <= start {
        return Err(LexError::Nonprogress { at: start });
    }
    tokens.push(JavaToken {
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

impl JavaState {
    fn reset_string(&mut self) {
        self.mode = JavaMode::Normal;
    }

    fn line_bounded(&self) -> bool {
        matches!(self.mode, JavaMode::RegularString | JavaMode::Char)
    }
}

fn count_quotes(bytes: &[u8], i: usize) -> u8 {
    let mut n = 0u8;
    while bytes.get(i + n as usize) == Some(&b'"') && n < 16 {
        n += 1;
    }
    n
}

/// JLS text-block opener: `"""` then optional whitespace then a line
/// terminator. In `lex_line` the newline is not part of `bytes`, so end of
/// the line after optional spaces also opens.
fn is_text_block_open(bytes: &[u8], i: usize) -> bool {
    if bytes.get(i) != Some(&b'"')
        || bytes.get(i + 1) != Some(&b'"')
        || bytes.get(i + 2) != Some(&b'"')
    {
        return false;
    }
    let mut j = i + 3;
    while j < bytes.len() && is_space(bytes[j]) {
        j += 1;
    }
    j >= bytes.len() || is_newline(bytes[j])
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
    if b == b'u' {
        let mut n = 0;
        while *i < bytes.len() && n < 4 && bytes[*i].is_ascii_hexdigit() {
            *i += 1;
            n += 1;
        }
        return;
    }
    if (b'0'..=b'7').contains(&b) {
        let mut n = 1;
        while *i < bytes.len() && n < 3 && (b'0'..=b'7').contains(&bytes[*i]) {
            *i += 1;
            n += 1;
        }
    }
}

/// Scan from `i` inside the current string/char/text-block mode. Returns
/// whether the literal closed (or recovered at a line boundary).
fn scan_string_body(bytes: &[u8], i: &mut usize, state: &mut JavaState) -> bool {
    while *i < bytes.len() {
        match state.mode {
            JavaMode::RegularString => {
                if scan_regular_or_char(bytes, i, state, b'"') {
                    return true;
                }
            }
            JavaMode::Char => {
                if scan_regular_or_char(bytes, i, state, b'\'') {
                    return true;
                }
            }
            JavaMode::TextBlock => {
                if scan_text_block(bytes, i, state) {
                    return true;
                }
            }
            JavaMode::Normal | JavaMode::BlockComment => return false,
        }
    }
    false
}

fn scan_regular_or_char(
    bytes: &[u8],
    i: &mut usize,
    state: &mut JavaState,
    quote: u8,
) -> bool {
    let b = bytes[*i];
    if b == b'\\' {
        *i += 1;
        consume_string_escape(bytes, i);
        return false;
    }
    if b == quote {
        *i += 1;
        state.reset_string();
        return true;
    }
    if is_newline(b) {
        state.reset_string();
        return true;
    }
    *i += 1;
    false
}

fn scan_text_block(bytes: &[u8], i: &mut usize, state: &mut JavaState) -> bool {
    let b = bytes[*i];
    if b == b'\\' {
        *i += 1;
        if *i < bytes.len() && !is_newline(bytes[*i]) {
            *i += 1;
        }
        return false;
    }
    if b == b'"'
        && bytes.get(*i + 1) == Some(&b'"')
        && bytes.get(*i + 2) == Some(&b'"')
    {
        *i += 3;
        state.reset_string();
        return true;
    }
    *i += 1;
    false
}

fn scan_number(bytes: &[u8], mut i: usize) -> usize {
    if bytes[i] == b'0'
        && matches!(
            bytes.get(i + 1).map(|b| b.to_ascii_lowercase()),
            Some(b'x' | b'b')
        )
    {
        let hex = bytes[i + 1].to_ascii_lowercase() == b'x';
        i += 2;
        if hex {
            while i < bytes.len() && (bytes[i].is_ascii_hexdigit() || bytes[i] == b'_') {
                i += 1;
            }
            if bytes.get(i) == Some(&b'.') {
                let next = bytes.get(i + 1).copied().unwrap_or(0);
                if next.is_ascii_hexdigit() || next.to_ascii_lowercase() == b'p' {
                    i += 1;
                    while i < bytes.len() && (bytes[i].is_ascii_hexdigit() || bytes[i] == b'_') {
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
            return consume_numeric_suffix(bytes, i);
        }
        while i < bytes.len() && (bytes[i] == b'0' || bytes[i] == b'1' || bytes[i] == b'_') {
            i += 1;
        }
        return consume_numeric_suffix(bytes, i);
    }
    if bytes[i] == b'.' {
        i += 1;
        while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'_') {
            i += 1;
        }
        i = consume_exponent(bytes, i);
        return consume_numeric_suffix(bytes, i);
    }
    while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'_') {
        i += 1;
    }
    if bytes.get(i) == Some(&b'.') {
        let next = bytes.get(i + 1).copied().unwrap_or(0);
        // Consume `.` only when a float continues (`1.5`, `1.e3`, `1.f`).
        // Stop before `.` so `arr[0].length` is Number `.` Identifier.
        if next.is_ascii_digit()
            || next == b'e'
            || next == b'E'
            || next == b'f'
            || next == b'F'
            || next == b'd'
            || next == b'D'
        {
            i += 1;
            while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'_') {
                i += 1;
            }
        }
    }
    i = consume_exponent(bytes, i);
    consume_numeric_suffix(bytes, i)
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

fn consume_numeric_suffix(bytes: &[u8], mut i: usize) -> usize {
    if matches!(
        bytes.get(i).map(|b| b.to_ascii_lowercase()),
        Some(b'l' | b'f' | b'd')
    ) {
        i += 1;
    }
    i
}

fn punct_len(bytes: &[u8], i: usize) -> usize {
    let rest = &bytes[i..];
    if rest.starts_with(b">>>=") {
        return 4;
    }
    if rest.starts_with(b">>>")
        || rest.starts_with(b">>=")
        || rest.starts_with(b"<<=")
        || rest.starts_with(b"...")
    {
        return 3;
    }
    if matches!(
        rest.get(..2),
        Some(
            b"->" | b"::"
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
                | b"%="
                | b"&="
                | b"|="
                | b"^="
                | b"<<"
                | b">>"
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
    state: &mut JavaState,
    line_start: &mut bool,
) -> Result<JavaToken, LexError> {
    let start_i = *i;
    let start = *i as u32;
    if *i >= bytes.len() {
        return Err(LexError::Nonprogress { at: start });
    }

    match state.mode {
        JavaMode::BlockComment => {
            while *i < bytes.len() {
                if bytes[*i] == b'*' && bytes.get(*i + 1) == Some(&b'/') {
                    *i += 2;
                    state.mode = JavaMode::Normal;
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
            return Ok(JavaToken {
                start,
                end: *i as u32,
                kind: JavaKind::Comment,
            });
        }
        JavaMode::RegularString | JavaMode::Char | JavaMode::TextBlock => {
            let kind = if state.mode == JavaMode::Char {
                JavaKind::Char
            } else {
                JavaKind::String
            };
            let closed = scan_string_body(bytes, i, state);
            if *i == start_i && *i < bytes.len() && !closed {
                if state.line_bounded() || matches!(state.mode, JavaMode::Normal) {
                    if is_newline(bytes[*i]) {
                        return Err(LexError::Nonprogress { at: start });
                    }
                    *i += 1;
                }
            }
            if *i as u32 <= start {
                return Err(LexError::Nonprogress { at: start });
            }
            *line_start = false;
            return Ok(JavaToken {
                start,
                end: *i as u32,
                kind,
            });
        }
        JavaMode::Normal => {}
    }

    let b = bytes[*i];
    if b == b'\r' {
        *i += 1;
        if bytes.get(*i) == Some(&b'\n') {
            *i += 1;
        }
        *line_start = true;
        return Ok(tok(start, *i, JavaKind::Whitespace));
    }
    if b == b'\n' {
        *i += 1;
        *line_start = true;
        return Ok(tok(start, *i, JavaKind::Whitespace));
    }
    if is_space(b) {
        *i += 1;
        while *i < bytes.len() && is_space(bytes[*i]) {
            *i += 1;
        }
        return Ok(tok(start, *i, JavaKind::Whitespace));
    }
    *line_start = false;
    if b == b'/' && bytes.get(*i + 1) == Some(&b'/') {
        *i += 2;
        while *i < bytes.len() && !is_newline(bytes[*i]) {
            *i += 1;
        }
        return Ok(tok(start, *i, JavaKind::Comment));
    }
    if b == b'/' && bytes.get(*i + 1) == Some(&b'*') {
        *i += 2;
        state.mode = JavaMode::BlockComment;
        while *i < bytes.len() {
            if bytes[*i] == b'*' && bytes.get(*i + 1) == Some(&b'/') {
                *i += 2;
                state.mode = JavaMode::Normal;
                break;
            }
            *i += 1;
        }
        return Ok(tok(start, *i, JavaKind::Comment));
    }
    if b == b'\'' {
        *i += 1;
        state.mode = JavaMode::Char;
        let closed = scan_string_body(bytes, i, state);
        if *i as u32 <= start {
            if !closed && *i == start_i + 1 {
                return Ok(tok(start, *i, JavaKind::Char));
            }
            return Err(LexError::Nonprogress { at: start });
        }
        return Ok(tok(start, *i, JavaKind::Char));
    }
    if b == b'"' {
        if is_text_block_open(bytes, *i) {
            *i += 3;
            while *i < bytes.len() && is_space(bytes[*i]) {
                *i += 1;
            }
            if *i < bytes.len() && is_newline(bytes[*i]) {
                if bytes[*i] == b'\r' {
                    *i += 1;
                    if bytes.get(*i) == Some(&b'\n') {
                        *i += 1;
                    }
                } else {
                    *i += 1;
                }
            }
            state.mode = JavaMode::TextBlock;
            let _closed = scan_string_body(bytes, i, state);
            if *i as u32 <= start {
                return Err(LexError::Nonprogress { at: start });
            }
            return Ok(tok(start, *i, JavaKind::String));
        }
        let n = count_quotes(bytes, *i);
        if n >= 2 {
            // JLS: `"""` not followed by a line terminator is an empty
            // string `""` plus a string opener. Consume two quotes only.
            *i += 2;
            return Ok(tok(start, *i, JavaKind::String));
        }
        *i += 1;
        state.mode = JavaMode::RegularString;
        let _closed = scan_string_body(bytes, i, state);
        if *i as u32 <= start {
            return Err(LexError::Nonprogress { at: start });
        }
        return Ok(tok(start, *i, JavaKind::String));
    }
    if b == b'@' {
        let next = bytes.get(*i + 1).copied().unwrap_or(0);
        if is_ident_start(next) {
            *i += 1;
            while *i < bytes.len() && is_ident_continue(bytes[*i]) {
                *i += 1;
            }
            return Ok(tok(start, *i, JavaKind::Annotation));
        }
        *i += 1;
        return Ok(tok(start, *i, JavaKind::Punctuator));
    }
    if is_ident_start(b) {
        *i += 1;
        while *i < bytes.len() && is_ident_continue(bytes[*i]) {
            *i += 1;
        }
        let ident = std::str::from_utf8(&bytes[start as usize..*i]).unwrap_or("");
        let kind = if is_keyword(ident) {
            JavaKind::Keyword
        } else {
            JavaKind::Identifier
        };
        return Ok(tok(start, *i, kind));
    }
    if b.is_ascii_digit() || (b == b'.' && bytes.get(*i + 1).copied().unwrap_or(0).is_ascii_digit())
    {
        *i = scan_number(bytes, *i);
        return Ok(tok(start, *i, JavaKind::Number));
    }
    let n = punct_len(bytes, *i);
    *i += n;
    let kind = if n == 1 && matches!(b, b'(' | b')' | b'{' | b'}' | b'[' | b']') {
        JavaKind::Delimiter
    } else if b == b'\\' {
        // `\uXXXX` outside literals is not translated; the backslash is
        // an Unknown byte so the frontend can count it.
        JavaKind::Unknown
    } else if b.is_ascii_graphic() {
        JavaKind::Punctuator
    } else {
        JavaKind::Unknown
    };
    if *i == start_i {
        return Err(LexError::Nonprogress { at: start });
    }
    Ok(tok(start, *i, kind))
}

fn tok(start: u32, end: usize, kind: JavaKind) -> JavaToken {
    JavaToken {
        start,
        end: end as u32,
        kind,
    }
}

/// Tokenize an entire document. Spans are byte offsets into `bytes`.
pub fn lex_document_cancellable(
    bytes: &[u8],
    cancel: &dyn Fn() -> bool,
) -> Result<Vec<JavaToken>, LexError> {
    let mut tokens = Vec::new();
    let mut state = JavaState::default();
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
                    push_token(&mut tokens, start, i, JavaKind::Whitespace)?;
                    continue;
                }
                if i == start_i && i < bytes.len() {
                    i += 1;
                    push_token(&mut tokens, at, i, JavaKind::Unknown)?;
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
pub fn lex_line(line: &str, incoming: JavaState) -> (JavaState, Vec<(usize, TokenRole)>) {
    let bytes = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    let mut state = incoming;
    let mut line_start = matches!(state.mode, JavaMode::Normal);
    while i < bytes.len() {
        let start = i;
        match lex_one(bytes, &mut i, &mut state, &mut line_start) {
            Ok(t) => {
                let end = (t.end as usize).min(bytes.len()).max(start);
                if end > start {
                    let ident = if t.kind == JavaKind::Keyword || t.kind == JavaKind::Identifier {
                        std::str::from_utf8(&bytes[t.start as usize..end]).unwrap_or("")
                    } else {
                        ""
                    };
                    let role = if t.kind == JavaKind::Keyword || t.kind == JavaKind::Identifier {
                        let mut k = end;
                        while k < bytes.len() && is_space(bytes[k]) {
                            k += 1;
                        }
                        let next_is_paren = bytes.get(k) == Some(&b'(');
                        classify_identifier(ident, next_is_paren)
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
    if matches!(state.mode, JavaMode::RegularString | JavaMode::Char) {
        state.reset_string();
    }
    if i < bytes.len() {
        let role = match state.mode {
            JavaMode::BlockComment => TokenRole::Comment,
            JavaMode::Normal => TokenRole::Unknown,
            JavaMode::Char => TokenRole::Char,
            JavaMode::RegularString | JavaMode::TextBlock => TokenRole::String,
        };
        push_role(bytes.len(), role, &mut out);
    }
    (state, out)
}

/// Convert document tokens into public spans, classifying keywords and
/// function names with one-token lookahead.
pub fn document_spans(bytes: &[u8], tokens: &[JavaToken]) -> Vec<TokenSpan> {
    let mut out = Vec::with_capacity(tokens.len());
    for (i, t) in tokens.iter().enumerate() {
        let mut role = t.kind.role();
        if t.kind == JavaKind::Keyword || t.kind == JavaKind::Identifier {
            let mut j = i + 1;
            while j < tokens.len() && tokens[j].kind.role().is_trivia() {
                j += 1;
            }
            let next_is_paren = j < tokens.len()
                && tokens[j].kind == JavaKind::Delimiter
                && bytes.get(tokens[j].start as usize) == Some(&b'(');
            let ident = std::str::from_utf8(&bytes[t.start as usize..t.end as usize]).unwrap_or("");
            role = classify_identifier(ident, next_is_paren);
        }
        out.push(TokenSpan::new(t.start, t.end, role));
    }
    out
}
