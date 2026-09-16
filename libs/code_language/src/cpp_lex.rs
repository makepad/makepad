//! Source-only C++ lexer shared by the editor and the private C++ frontend.
//! Byte offsets address the original document. Continuation state is owned
//! here: C++ raw-string delimiters are not packed into a universal integer.

use crate::token::{LineMix, LineSummary, TokenRole, TokenSpan};

/// Version of this lexer; part of parse and search cache identity.
pub const CPP_LEXER_VERSION: u32 = 3;

/// Whole-document lex failure. A cancelled or non-progressing run is not a
/// successful complete tokenisation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LexError {
    Cancelled,
    /// Cursor did not advance. `at` is the original-document byte offset.
    Nonprogress { at: u32 },
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum CppState {
    Normal,
    BlockComment,
    String,
    Char,
    /// Raw string: delimiter and whether the body (after `(`) has started.
    RawString { delim: String, in_body: bool },
}

impl Default for CppState {
    fn default() -> Self {
        CppState::Normal
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CppToken {
    pub start: u32,
    pub end: u32,
    pub kind: CppKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CppKind {
    Whitespace,
    Comment,
    Identifier,
    Keyword,
    Number,
    String,
    Char,
    Punctuator,
    Delimiter,
    Preprocessor,
    Unknown,
}

impl CppKind {
    pub fn role(self) -> TokenRole {
        match self {
            CppKind::Whitespace => TokenRole::Whitespace,
            CppKind::Comment => TokenRole::Comment,
            CppKind::Identifier => TokenRole::Identifier,
            CppKind::Keyword => TokenRole::Keyword,
            CppKind::Number => TokenRole::Number,
            CppKind::String => TokenRole::String,
            CppKind::Char => TokenRole::Char,
            CppKind::Punctuator => TokenRole::Punctuator,
            CppKind::Delimiter => TokenRole::Delimiter,
            CppKind::Preprocessor => TokenRole::Preprocessor,
            CppKind::Unknown => TokenRole::Unknown,
        }
    }
}

const KEYWORDS: &[&str] = &[
    "alignas",
    "alignof",
    "and",
    "and_eq",
    "asm",
    "auto",
    "bitand",
    "bitor",
    "bool",
    "break",
    "case",
    "catch",
    "char",
    "char8_t",
    "char16_t",
    "char32_t",
    "class",
    "compl",
    "concept",
    "const",
    "consteval",
    "constexpr",
    "constinit",
    "const_cast",
    "continue",
    "co_await",
    "co_return",
    "co_yield",
    "decltype",
    "default",
    "delete",
    "do",
    "double",
    "dynamic_cast",
    "else",
    "enum",
    "explicit",
    "export",
    "extern",
    "false",
    "float",
    "for",
    "friend",
    "goto",
    "if",
    "inline",
    "int",
    "long",
    "mutable",
    "namespace",
    "new",
    "noexcept",
    "not",
    "not_eq",
    "nullptr",
    "operator",
    "or",
    "or_eq",
    "private",
    "protected",
    "public",
    "register",
    "reinterpret_cast",
    "requires",
    "return",
    "short",
    "signed",
    "sizeof",
    "static",
    "static_assert",
    "static_cast",
    "struct",
    "switch",
    "template",
    "this",
    "thread_local",
    "throw",
    "true",
    "try",
    "typedef",
    "typeid",
    "typename",
    "union",
    "unsigned",
    "using",
    "virtual",
    "void",
    "volatile",
    "wchar_t",
    "while",
    "xor",
    "xor_eq",
    "override",
    "final",
    "audit",
    "axiom",
    "import",
    "module",
];

pub fn is_keyword(ident: &str) -> bool {
    KEYWORDS.binary_search(&ident).is_ok() || KEYWORDS.contains(&ident)
}

pub fn keyword_role(ident: &str) -> TokenRole {
    match ident {
        "if" | "else" | "switch" | "case" | "default" | "try" | "catch" | "return" | "co_return"
        | "throw" | "goto" => TokenRole::BranchKeyword,
        "for" | "while" | "do" | "break" | "continue" | "co_await" | "co_yield" => {
            TokenRole::LoopKeyword
        }
        "true" | "false" | "nullptr" => TokenRole::Constant,
        "bool" | "char" | "char8_t" | "char16_t" | "char32_t" | "wchar_t" | "int" | "long"
        | "short" | "signed" | "unsigned" | "float" | "double" | "void" | "auto" | "decltype"
        | "const_cast" | "static_cast" | "dynamic_cast" | "reinterpret_cast" => TokenRole::Typename,
        other if is_keyword(other) => TokenRole::Keyword,
        _ => TokenRole::Identifier,
    }
}

/// Colour an identifier or keyword. Keywords win first so `if(` / `while(` are
/// never Function.
///
/// Macro is a naming heuristic only: ASCII `[A-Z0-9_]` with at least one
/// uppercase letter and at least one underscore. It is not preprocessor
/// expansion and makes no semantic claim. `DCHECK(` has no underscore so it
/// stays Function; `GURL` has no underscore so it stays Typename.
fn classify_identifier(ident: &str, next_is_paren: bool) -> TokenRole {
    if is_keyword(ident) {
        return keyword_role(ident);
    }
    let b = ident.as_bytes();
    if !b.is_empty()
        && b.iter()
            .all(|&c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_')
        && b.iter().any(|&c| c.is_ascii_uppercase())
        && b.iter().any(|&c| c == b'_')
    {
        return TokenRole::Macro;
    }
    if next_is_paren {
        return TokenRole::Function;
    }
    if ident.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
        TokenRole::Typename
    } else {
        TokenRole::Identifier
    }
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || b >= 0x80
}

fn is_ident_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80
}

fn is_raw_prefix(bytes: &[u8], i: usize) -> Option<usize> {
    // R"  u8R"  uR"  UR"  LR"
    let rest = &bytes[i..];
    if rest.starts_with(b"R\"") {
        return Some(1);
    }
    if rest.starts_with(b"u8R\"") {
        return Some(3);
    }
    if rest.starts_with(b"uR\"") || rest.starts_with(b"UR\"") || rest.starts_with(b"LR\"") {
        return Some(2);
    }
    None
}

/// Tokenize an entire document. Spans are byte offsets into `bytes`.
pub fn lex_document(bytes: &[u8]) -> Vec<CppToken> {
    lex_document_cancellable(bytes, &|| false).unwrap_or_default()
}

/// Like [`lex_document`], but observes `cancel` and refuses to loop without
/// advancing the original-byte cursor.
pub fn lex_document_cancellable(
    bytes: &[u8],
    cancel: &dyn Fn() -> bool,
) -> Result<Vec<CppToken>, LexError> {
    let mut tokens = Vec::new();
    let mut state = CppState::Normal;
    let mut line_start = true;
    let mut i = 0usize;
    // One token is at least one byte once zero-length emits are forbidden.
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
        match state {
            CppState::BlockComment => {
                if bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/') {
                    i += 2;
                    state = CppState::Normal;
                } else {
                    if bytes[i] == b'\n' {
                        line_start = true;
                    } else if !is_space(bytes[i]) {
                        line_start = false;
                    }
                    i += 1;
                    if i < bytes.len() && state == CppState::BlockComment {
                        continue;
                    }
                }
                push_token(&mut tokens, start, i, CppKind::Comment)?;
                continue;
            }
            CppState::String => {
                resume_quoted(&mut tokens, bytes, &mut i, &mut state, start, b'"', CppKind::String)?;
                line_start = false;
                continue;
            }
            CppState::Char => {
                resume_quoted(&mut tokens, bytes, &mut i, &mut state, start, b'\'', CppKind::Char)?;
                line_start = false;
                continue;
            }
            CppState::RawString { delim, in_body } => {
                let (next, end) = finish_raw(bytes, i, &delim, in_body);
                if end < i {
                    return Err(LexError::Nonprogress { at: start });
                }
                i = end;
                let done = matches!(next, CppState::Normal);
                state = next;
                push_token(&mut tokens, start, i, CppKind::String)?;
                line_start = false;
                if !done && i >= bytes.len() {
                    break;
                }
                if i == start_i && i < bytes.len() {
                    return Err(LexError::Nonprogress { at: start });
                }
                continue;
            }
            CppState::Normal => {}
        }

        let b = bytes[i];
        if b == b'\r' {
            i += 1;
            if bytes.get(i) == Some(&b'\n') {
                i += 1;
            }
            line_start = true;
            push_token(&mut tokens, start, i, CppKind::Whitespace)?;
            continue;
        }
        if b == b'\n' {
            i += 1;
            line_start = true;
            push_token(&mut tokens, start, i, CppKind::Whitespace)?;
            continue;
        }
        if b == b'\\' && matches!(bytes.get(i + 1), Some(&b'\n') | Some(&b'\r')) {
            i += 1;
            if bytes.get(i) == Some(&b'\r') {
                i += 1;
            }
            if bytes.get(i) == Some(&b'\n') {
                i += 1;
            }
            push_token(&mut tokens, start, i, CppKind::Whitespace)?;
            continue;
        }
        if is_space(b) {
            i += 1;
            while i < bytes.len() && is_space(bytes[i]) && bytes[i] != b'\n' && bytes[i] != b'\r' {
                i += 1;
            }
            push_token(&mut tokens, start, i, CppKind::Whitespace)?;
            continue;
        }
        if line_start && b == b'#' {
            i += 1;
            push_token(&mut tokens, start, i, CppKind::Preprocessor)?;
            line_start = false;
            continue;
        }
        line_start = false;
        if b == b'/' && bytes.get(i + 1) == Some(&b'/') {
            i += 2;
            while i < bytes.len() && bytes[i] != b'\n' && bytes[i] != b'\r' {
                if i & 255 == 0 && cancel() {
                    return Err(LexError::Cancelled);
                }
                if bytes[i] == b'\\' && matches!(bytes.get(i + 1), Some(&b'\n') | Some(&b'\r')) {
                    i += 1;
                    if bytes.get(i) == Some(&b'\r') {
                        i += 1;
                    }
                    if bytes.get(i) == Some(&b'\n') {
                        i += 1;
                    }
                    continue;
                }
                i += 1;
            }
            push_token(&mut tokens, start, i, CppKind::Comment)?;
            continue;
        }
        if b == b'/' && bytes.get(i + 1) == Some(&b'*') {
            i += 2;
            state = CppState::BlockComment;
            while i < bytes.len() {
                if i & 255 == 0 && cancel() {
                    return Err(LexError::Cancelled);
                }
                if bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/') {
                    i += 2;
                    state = CppState::Normal;
                    break;
                }
                i += 1;
            }
            push_token(&mut tokens, start, i, CppKind::Comment)?;
            continue;
        }
        if let Some(prefix_len) = is_raw_prefix(bytes, i) {
            i += prefix_len + 1; // consume prefix and opening quote
            let delim_start = i;
            while i < bytes.len()
                && bytes[i] != b'('
                && bytes[i] != b')'
                && bytes[i] != b'\\'
                && bytes[i] != b' '
                && bytes[i] != b'\n'
                && bytes[i] != b'\r'
                && (i - delim_start) < 16
            {
                i += 1;
            }
            let delim = String::from_utf8_lossy(&bytes[delim_start..i]).into_owned();
            let in_body = if bytes.get(i) == Some(&b'(') {
                i += 1;
                true
            } else {
                false
            };
            let body_start = i;
            if in_body {
                if let Some(end) = find_raw_end(bytes, i, &delim) {
                    i = end;
                    push_token(&mut tokens, start, i, CppKind::String)?;
                    state = CppState::Normal;
                    continue;
                }
            }
            i = bytes.len();
            push_token(&mut tokens, start, i, CppKind::String)?;
            state = CppState::RawString { delim, in_body };
            let _ = body_start;
            continue;
        }
        if b == b'"' {
            i += 1;
            resume_quoted(
                &mut tokens,
                bytes,
                &mut i,
                &mut state,
                start,
                b'"',
                CppKind::String,
            )?;
            continue;
        }
        if b == b'\'' {
            i += 1;
            resume_quoted(
                &mut tokens,
                bytes,
                &mut i,
                &mut state,
                start,
                b'\'',
                CppKind::Char,
            )?;
            continue;
        }
        if is_ident_start(b) {
            i += 1;
            while i < bytes.len() && is_ident_continue(bytes[i]) {
                i += 1;
            }
            let ident = std::str::from_utf8(&bytes[start as usize..i]).unwrap_or("");
            let kind = if is_keyword(ident) {
                CppKind::Keyword
            } else {
                CppKind::Identifier
            };
            push_token(&mut tokens, start, i, kind)?;
            continue;
        }
        if b.is_ascii_digit()
            || (b == b'.' && bytes.get(i + 1).copied().unwrap_or(0).is_ascii_digit())
        {
            i = scan_number(bytes, i);
            push_token(&mut tokens, start, i, CppKind::Number)?;
            continue;
        }
        // multi-char punctuators
        let two = bytes.get(i..i + 2).unwrap_or(&[]);
        let three = bytes.get(i..i + 3).unwrap_or(&[]);
        if three == b"<<=" || three == b">>=" || three == b"<=>" || three == b"->*" {
            i += 3;
            push_token(&mut tokens, start, i, CppKind::Punctuator)?;
            continue;
        }
        if matches!(
            two,
            b"::" | b"->"
                | b"++"
                | b"--"
                | b"<<"
                | b">>"
                | b"&&"
                | b"||"
                | b"=="
                | b"!="
                | b"<="
                | b">="
                | b"+="
                | b"-="
                | b"*="
                | b"/="
                | b"%="
                | b"&="
                | b"|="
                | b"^="
                | b".*"
                | b"##"
        ) {
            i += 2;
            push_token(&mut tokens, start, i, CppKind::Punctuator)?;
            continue;
        }
        i += 1;
        let kind = if matches!(b, b'(' | b')' | b'{' | b'}' | b'[' | b']') {
            CppKind::Delimiter
        } else if b.is_ascii_graphic() {
            CppKind::Punctuator
        } else {
            CppKind::Unknown
        };
        push_token(&mut tokens, start, i, kind)?;
        if i == start_i {
            return Err(LexError::Nonprogress { at: start });
        }
    }
    Ok(tokens)
}

fn is_space(b: u8) -> bool {
    b == b' ' || b == b'\t' || b == 0x0b || b == 0x0c
}

fn push_token(
    tokens: &mut Vec<CppToken>,
    start: u32,
    end: usize,
    kind: CppKind,
) -> Result<(), LexError> {
    if end as u32 <= start {
        return Err(LexError::Nonprogress { at: start });
    }
    tokens.push(CppToken {
        start,
        end: end as u32,
        kind,
    });
    Ok(())
}

/// Resume a quoted literal whose opening quote is already consumed.
/// An unescaped newline recovers to `Normal` without consuming the newline, so
/// the document lexer can emit it as whitespace. Empty remainder at the
/// newline is not a token (the opening quote was already in `start..i`).
fn resume_quoted(
    tokens: &mut Vec<CppToken>,
    bytes: &[u8],
    i: &mut usize,
    state: &mut CppState,
    start: u32,
    quote: u8,
    kind: CppKind,
) -> Result<(), LexError> {
    let (next, end) = finish_string(bytes, *i, quote);
    if end < *i {
        return Err(LexError::Nonprogress { at: start });
    }
    *state = next;
    if end > *i || end as u32 > start {
        // Include the opening quote when this call started at it (`start < *i`).
        push_token(tokens, start, end.max(*i), kind)?;
    }
    *i = end;
    Ok(())
}

/// Scan from `i` (after the opening quote). Unescaped CR/LF terminates a
/// malformed literal and leaves the cursor on that newline. `\` + newline
/// (including CRLF) is a line continuation and stays inside the literal.
fn finish_string(bytes: &[u8], mut i: usize, quote: u8) -> (CppState, usize) {
    let open = if quote == b'"' {
        CppState::String
    } else {
        CppState::Char
    };
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'\\' {
            i += 1;
            if i >= bytes.len() {
                return (open, i);
            }
            if bytes[i] == b'\r' {
                i += 1;
                if i < bytes.len() && bytes[i] == b'\n' {
                    i += 1;
                }
                continue;
            }
            if bytes[i] == b'\n' {
                i += 1;
                continue;
            }
            i += 1;
            continue;
        }
        if b == quote {
            return (CppState::Normal, i + 1);
        }
        if b == b'\n' || b == b'\r' {
            return (CppState::Normal, i);
        }
        i += 1;
    }
    (open, i)
}

/// Odd trailing backslashes mean the last `\` continues the line.
fn line_ends_with_continuation(bytes: &[u8]) -> bool {
    let mut n = 0usize;
    let mut i = bytes.len();
    while i > 0 && bytes[i - 1] == b'\\' {
        n += 1;
        i -= 1;
    }
    n % 2 == 1
}

fn find_raw_end(bytes: &[u8], start: usize, delim: &str) -> Option<usize> {
    let needle = format!("){delim}\"");
    let n = needle.as_bytes();
    let mut i = start;
    while i + n.len() <= bytes.len() {
        if bytes[i..].starts_with(n) {
            return Some(i + n.len());
        }
        i += 1;
    }
    None
}

fn finish_raw(bytes: &[u8], i: usize, delim: &str, in_body: bool) -> (CppState, usize) {
    if !in_body {
        let mut j = i;
        while j < bytes.len()
            && bytes[j] != b'('
            && bytes[j] != b'\n'
            && (j - i) < 16
        {
            j += 1;
        }
        let extra = String::from_utf8_lossy(&bytes[i..j]).into_owned();
        let mut d = delim.to_string();
        d.push_str(&extra);
        if bytes.get(j) == Some(&b'(') {
            return finish_raw(bytes, j + 1, &d, true);
        }
        return (CppState::RawString { delim: d, in_body: false }, bytes.len());
    }
    if let Some(end) = find_raw_end(bytes, i, delim) {
        (CppState::Normal, end)
    } else {
        (
            CppState::RawString {
                delim: delim.to_string(),
                in_body: true,
            },
            bytes.len(),
        )
    }
}

fn scan_number(bytes: &[u8], mut i: usize) -> usize {
    if bytes[i] == b'0'
        && matches!(bytes.get(i + 1).map(|b| b.to_ascii_lowercase()), Some(b'x' | b'b' | b'o'))
    {
        i += 2;
    } else if bytes[i] == b'.' {
        i += 1;
    }
    while i < bytes.len() {
        let b = bytes[i];
        if b.is_ascii_alphanumeric() || b == b'_' || b == b'\'' || b == b'.' {
            i += 1;
            continue;
        }
        if matches!(b, b'+' | b'-')
            && i > 0
            && matches!(bytes[i - 1].to_ascii_lowercase(), b'e' | b'p')
        {
            i += 1;
            continue;
        }
        break;
    }
    i
}

/// Tokenize one display line (without the newline) given incoming continuation.
/// Each pair is `(end_index, role)` covering `[prev_end, end)` of `line`.
pub fn lex_line(line: &str, incoming: CppState) -> (CppState, Vec<(usize, TokenRole)>) {
    let bytes = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    let mut state = incoming;
    let push = |end: usize, role: TokenRole, out: &mut Vec<(usize, TokenRole)>| {
        if end == 0 {
            return;
        }
        if out.last().map(|(_, r)| *r) == Some(role) {
            out.last_mut().unwrap().0 = end;
        } else {
            out.push((end, role));
        }
    };
    while i < bytes.len() {
        let start = i;
        match state {
            CppState::BlockComment => {
                if bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/') {
                    i += 2;
                    push(i, TokenRole::Comment, &mut out);
                    state = CppState::Normal;
                } else {
                    i += 1;
                    if i == bytes.len() {
                        push(i, TokenRole::Comment, &mut out);
                    }
                }
                continue;
            }
            CppState::String => {
                let (next, end) = finish_string(bytes, i, b'"');
                i = end.max(i);
                if i > start {
                    push(i, TokenRole::String, &mut out);
                }
                state = next;
                if matches!(state, CppState::String) {
                    if i >= bytes.len() && !line_ends_with_continuation(bytes) {
                        state = CppState::Normal;
                    }
                    break;
                }
                continue;
            }
            CppState::Char => {
                let (next, end) = finish_string(bytes, i, b'\'');
                i = end.max(i);
                if i > start {
                    push(i, TokenRole::Char, &mut out);
                }
                state = next;
                if matches!(state, CppState::Char) {
                    if i >= bytes.len() && !line_ends_with_continuation(bytes) {
                        state = CppState::Normal;
                    }
                    break;
                }
                continue;
            }
            CppState::RawString { delim, in_body } => {
                let (next, end) = finish_raw(bytes, i, &delim, in_body);
                i = end.min(bytes.len()).max(i);
                push(i, TokenRole::String, &mut out);
                state = next;
                if !matches!(state, CppState::Normal) {
                    break;
                }
                continue;
            }
            CppState::Normal => {}
        }
        let b = bytes[i];
        if is_space(b) {
            i += 1;
            while i < bytes.len() && is_space(bytes[i]) {
                i += 1;
            }
            push(i, TokenRole::Whitespace, &mut out);
            continue;
        }
        // Leading ASCII spaces/tabs only; not `#` after code, comments, or strings.
        if b == b'#' && bytes[..i].iter().all(|&c| c == b' ' || c == b'\t') {
            i += 1;
            push(i, TokenRole::Preprocessor, &mut out);
            continue;
        }
        if b == b'/' && bytes.get(i + 1) == Some(&b'/') {
            push(bytes.len(), TokenRole::Comment, &mut out);
            break;
        }
        if b == b'/' && bytes.get(i + 1) == Some(&b'*') {
            i += 2;
            state = CppState::BlockComment;
            continue;
        }
        if let Some(prefix) = is_raw_prefix(bytes, i) {
            i += prefix + 1;
            let ds = i;
            while i < bytes.len() && bytes[i] != b'(' && (i - ds) < 16 && bytes[i] != b'\n' {
                i += 1;
            }
            let delim = String::from_utf8_lossy(&bytes[ds..i]).into_owned();
            let in_body = if bytes.get(i) == Some(&b'(') {
                i += 1;
                true
            } else {
                false
            };
            state = CppState::RawString { delim, in_body };
            continue;
        }
        if b == b'"' {
            let (next, end) = finish_string(bytes, i + 1, b'"');
            i = end.max(i + 1);
            push(i, TokenRole::String, &mut out);
            state = next;
            if matches!(state, CppState::String) {
                if i >= bytes.len() && !line_ends_with_continuation(bytes) {
                    state = CppState::Normal;
                }
                break;
            }
            continue;
        }
        if b == b'\'' {
            let (next, end) = finish_string(bytes, i + 1, b'\'');
            i = end.max(i + 1);
            push(i, TokenRole::Char, &mut out);
            state = next;
            if matches!(state, CppState::Char) {
                if i >= bytes.len() && !line_ends_with_continuation(bytes) {
                    state = CppState::Normal;
                }
                break;
            }
            continue;
        }
        if is_ident_start(b) {
            i += 1;
            while i < bytes.len() && is_ident_continue(bytes[i]) {
                i += 1;
            }
            let ident = &line[start..i];
            let mut k = i;
            while k < bytes.len() && is_space(bytes[k]) {
                k += 1;
            }
            let next_is_paren = bytes.get(k) == Some(&b'(');
            push(i, classify_identifier(ident, next_is_paren), &mut out);
            continue;
        }
        if b.is_ascii_digit()
            || (b == b'.' && bytes.get(i + 1).copied().unwrap_or(0).is_ascii_digit())
        {
            i = scan_number(bytes, i);
            push(i, TokenRole::Number, &mut out);
            continue;
        }
        i += 1;
        let role = if matches!(b, b'(' | b')' | b'{' | b'}' | b'[' | b']') {
            TokenRole::Delimiter
        } else {
            TokenRole::Punctuator
        };
        push(i, role, &mut out);
    }
    if i < bytes.len() && matches!(state, CppState::BlockComment) {
        push(bytes.len(), TokenRole::Comment, &mut out);
    }
    if matches!(state, CppState::String | CppState::Char) && !line_ends_with_continuation(bytes)
    {
        state = CppState::Normal;
    }
    (state, out)
}

/// Convert document tokens into public spans, classifying keywords and
/// function names with one-token lookahead.
pub fn document_spans(bytes: &[u8], tokens: &[CppToken]) -> Vec<TokenSpan> {
    let mut out = Vec::with_capacity(tokens.len());
    for (i, t) in tokens.iter().enumerate() {
        let mut role = t.kind.role();
        if t.kind == CppKind::Keyword || t.kind == CppKind::Identifier {
            let mut j = i + 1;
            while j < tokens.len() && tokens[j].kind.role().is_trivia() {
                j += 1;
            }
            let next_is_paren = j < tokens.len()
                && tokens[j].kind == CppKind::Delimiter
                && bytes.get(tokens[j].start as usize) == Some(&b'(');
            let ident = std::str::from_utf8(&bytes[t.start as usize..t.end as usize]).unwrap_or("");
            role = classify_identifier(ident, next_is_paren);
        }
        out.push(TokenSpan::new(t.start, t.end, role));
    }
    out
}

pub fn line_summaries(bytes: &[u8], spans: &[TokenSpan]) -> Vec<LineSummary> {
    let mut lines = 1usize;
    for &b in bytes {
        if b == b'\n' {
            lines += 1;
        }
    }
    let mut out = vec![LineSummary::default(); lines];
    let mut line = 0usize;
    let mut line_start = 0u32;
    let mut span_i = 0usize;
    for (pos, &b) in bytes.iter().enumerate() {
        if b == b'\n' {
            let end = pos as u32;
            while span_i < spans.len() && spans[span_i].start < end {
                let s = &spans[span_i];
                let a = s.start.max(line_start);
                let z = s.end.min(end);
                if z > a {
                    out[line].mix.add_role(s.role, z - a);
                    out[line].token_count = out[line].token_count.saturating_add(1);
                }
                if s.end <= end {
                    span_i += 1;
                } else {
                    break;
                }
            }
            line += 1;
            line_start = end + 1;
        }
    }
    let end = bytes.len() as u32;
    while span_i < spans.len() {
        let s = &spans[span_i];
        let a = s.start.max(line_start);
        let z = s.end.min(end);
        if z > a && line < out.len() {
            out[line].mix.add_role(s.role, z - a);
            out[line].token_count = out[line].token_count.saturating_add(1);
        }
        span_i += 1;
    }
    let _ = LineMix::from_counts;
    out
}
