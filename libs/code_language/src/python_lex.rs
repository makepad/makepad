//! Source-only Python lexer shared by the editor and the Python frontend.
//! Byte offsets address the original document. Continuation state is owned
//! here: triple-quoted string delimiters are not packed into a universal integer.

use crate::token::{TokenRole, TokenSpan};
use crate::LexError;

/// Version of this lexer; part of parse and search cache identity.
pub const PYTHON_LEXER_VERSION: u32 = 1;

/// Provider-owned line continuation. Default is Normal (not inside a
/// string). `lex_line` and the document lexer share one state machine so
/// highlighting and parsing agree. Triple-quoted strings span lines via
/// `triple_quote`. A single-quoted string stays open only when a line
/// ended immediately after a non-raw `\`; an unescaped end of line resets.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct PythonState {
    /// Opening quote of an open triple-quoted string (`'` or `"`), or 0.
    triple_quote: u8,
    /// Raw prefix (`r`/`R`/`rb`/`rf`/…) of the open triple-quoted string.
    triple_raw: bool,
    /// Opening quote of a single-quoted string continued after `\` at EOL, or 0.
    single_quote: u8,
    /// Raw prefix of the continued single-quoted string.
    single_raw: bool,
}

impl PythonState {
    fn in_triple(self) -> bool {
        self.triple_quote != 0
    }

    fn in_single(self) -> bool {
        self.single_quote != 0
    }

    fn clear_string(&mut self) {
        self.triple_quote = 0;
        self.triple_raw = false;
        self.single_quote = 0;
        self.single_raw = false;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PythonKind {
    Whitespace,
    Newline,
    Comment,
    Identifier,
    Keyword,
    Number,
    String,
    Punctuator,
    Delimiter,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PythonToken {
    pub start: u32,
    pub end: u32,
    pub kind: PythonKind,
}

impl PythonKind {
    pub fn role(self) -> TokenRole {
        match self {
            PythonKind::Whitespace | PythonKind::Newline => TokenRole::Whitespace,
            PythonKind::Comment => TokenRole::Comment,
            PythonKind::Identifier => TokenRole::Identifier,
            PythonKind::Keyword => TokenRole::Keyword,
            PythonKind::Number => TokenRole::Number,
            PythonKind::String => TokenRole::String,
            PythonKind::Punctuator => TokenRole::Punctuator,
            PythonKind::Delimiter => TokenRole::Delimiter,
            PythonKind::Unknown => TokenRole::Unknown,
        }
    }
}

/// Python 3.12 hard keywords. Soft keywords `match`, `case`, `type`, `_`
/// are lexed as Identifier (the parser decides).
const KEYWORDS: &[&str] = &[
    "False",
    "None",
    "True",
    "and",
    "as",
    "assert",
    "async",
    "await",
    "break",
    "class",
    "continue",
    "def",
    "del",
    "elif",
    "else",
    "except",
    "finally",
    "for",
    "from",
    "global",
    "if",
    "import",
    "in",
    "is",
    "lambda",
    "nonlocal",
    "not",
    "or",
    "pass",
    "raise",
    "return",
    "try",
    "while",
    "with",
    "yield",
];

fn is_keyword(ident: &str) -> bool {
    KEYWORDS.binary_search(&ident).is_ok()
}

fn classify_identifier(ident: &str, next_is_paren: bool) -> TokenRole {
    match ident {
        "if" | "elif" | "else" | "try" | "except" | "finally" | "return" | "raise" | "with"
        | "match" | "case" => TokenRole::BranchKeyword,
        "for" | "while" | "break" | "continue" | "async" | "await" | "yield" => {
            TokenRole::LoopKeyword
        }
        "True" | "False" | "None" => TokenRole::Constant,
        "def" | "class" | "lambda" | "import" | "from" | "as" | "pass" | "global" | "nonlocal"
        | "assert" | "del" | "in" | "is" | "not" | "and" | "or" => TokenRole::Keyword,
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
    b.is_ascii_alphabetic() || b == b'_' || b >= 0x80
}

fn is_ident_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80
}

fn is_newline(b: u8) -> bool {
    b == b'\n' || b == b'\r'
}

/// Prefixes `r R b B u U f F rb br fr rf` (any case) immediately before a quote.
/// Returns `(prefix_len, raw, is_fstring)`.
fn string_prefix(bytes: &[u8], i: usize) -> Option<(usize, bool, bool)> {
    let rest = &bytes[i..];
    if rest.len() >= 3 {
        let a = rest[0].to_ascii_lowercase();
        let b = rest[1].to_ascii_lowercase();
        let q = rest[2];
        if q == b'\'' || q == b'"' {
            match (a, b) {
                (b'r', b'b') | (b'b', b'r') => return Some((2, true, false)),
                (b'r', b'f') | (b'f', b'r') => return Some((2, true, true)),
                _ => {}
            }
        }
    }
    if rest.len() >= 2 {
        let a = rest[0].to_ascii_lowercase();
        let q = rest[1];
        if q == b'\'' || q == b'"' {
            match a {
                b'r' => return Some((1, true, false)),
                b'b' | b'u' => return Some((1, false, false)),
                b'f' => return Some((1, false, true)),
                _ => {}
            }
        }
    }
    None
}

fn is_triple_quote(bytes: &[u8], i: usize, quote: u8) -> bool {
    bytes.get(i) == Some(&quote)
        && bytes.get(i + 1) == Some(&quote)
        && bytes.get(i + 2) == Some(&quote)
}

/// Scan the body of a string from `i` (after the opening quotes).
///
/// Single-quoted: an unescaped newline recovers without consuming it.
/// Triple-quoted: newlines are content. Non-raw `\` escapes the next byte
/// (including a newline). Raw: `\` only prevents a following quote from
/// closing; it is otherwise literal. f-string `{...}` fields are not
/// lexed; a same-type quote inside a replacement field terminates the
/// string (PEP 701 is not implemented).
///
/// Returns `(closed, end, continued)` where `continued` is true when a
/// non-raw buffer ended immediately after `\`, so `lex_line` must keep
/// the single-quoted string open for the next line.
fn scan_string_body(
    bytes: &[u8],
    mut i: usize,
    quote: u8,
    raw: bool,
    triple: bool,
) -> (bool, usize, bool) {
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'\\' {
            if raw {
                if bytes.get(i + 1) == Some(&quote) {
                    i += 2;
                    continue;
                }
                i += 1;
                continue;
            }
            i += 1;
            if i >= bytes.len() {
                return (false, i, true);
            }
            if bytes[i] == b'\r' {
                i += 1;
                if bytes.get(i) == Some(&b'\n') {
                    i += 1;
                }
                continue;
            }
            i += 1;
            continue;
        }
        if triple {
            if is_triple_quote(bytes, i, quote) {
                return (true, i + 3, false);
            }
            i += 1;
            continue;
        }
        if b == quote {
            return (true, i + 1, false);
        }
        if is_newline(b) {
            return (false, i, false);
        }
        i += 1;
    }
    (false, i, false)
}

fn scan_number(bytes: &[u8], mut i: usize) -> usize {
    let start = i;
    if bytes[i] == b'0' {
        let tag = bytes.get(i + 1).map(|b| b.to_ascii_lowercase());
        if matches!(tag, Some(b'x' | b'o' | b'b')) {
            i += 2;
            while i < bytes.len() {
                let b = bytes[i];
                let ok = match tag {
                    Some(b'x') => b.is_ascii_hexdigit() || b == b'_',
                    Some(b'o') => (b'0'..=b'7').contains(&b) || b == b'_',
                    Some(b'b') => b == b'0' || b == b'1' || b == b'_',
                    _ => false,
                };
                if !ok {
                    break;
                }
                i += 1;
            }
            return i.max(start + 1);
        }
    }
    if bytes[i] == b'.' {
        i += 1;
        while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'_') {
            i += 1;
        }
    } else {
        while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'_') {
            i += 1;
        }
        if i < bytes.len() && bytes[i] == b'.' && bytes.get(i + 1) != Some(&b'.') {
            i += 1;
            while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'_') {
                i += 1;
            }
        }
    }
    if i < bytes.len() && matches!(bytes[i], b'e' | b'E') {
        let mut j = i + 1;
        if j < bytes.len() && matches!(bytes[j], b'+' | b'-') {
            j += 1;
        }
        if j < bytes.len() && bytes[j].is_ascii_digit() {
            i = j;
            while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'_') {
                i += 1;
            }
        }
    }
    if i < bytes.len() && matches!(bytes[i], b'j' | b'J') {
        i += 1;
    }
    i.max(start + 1)
}

fn scan_punct_len(bytes: &[u8], i: usize) -> usize {
    let b = bytes[i];
    let b1 = bytes.get(i + 1).copied();
    let b2 = bytes.get(i + 2).copied();
    match (b, b1, b2) {
        (b'.', Some(b'.'), Some(b'.')) => 3,
        (b'*', Some(b'*'), Some(b'=')) => 3,
        (b'/', Some(b'/'), Some(b'=')) => 3,
        (b'<', Some(b'<'), Some(b'=')) => 3,
        (b'>', Some(b'>'), Some(b'=')) => 3,
        (b'=', Some(b'='), _)
        | (b'!', Some(b'='), _)
        | (b'<', Some(b'='), _)
        | (b'>', Some(b'='), _)
        | (b'<', Some(b'<'), _)
        | (b'>', Some(b'>'), _)
        | (b'+', Some(b'='), _)
        | (b'-', Some(b'='), _)
        | (b'*', Some(b'='), _)
        | (b'/', Some(b'='), _)
        | (b'%', Some(b'='), _)
        | (b'&', Some(b'='), _)
        | (b'|', Some(b'='), _)
        | (b'^', Some(b'='), _)
        | (b'@', Some(b'='), _)
        | (b':', Some(b'='), _)
        | (b'-', Some(b'>'), _)
        | (b'*', Some(b'*'), _)
        | (b'/', Some(b'/'), _) => 2,
        _ => 1,
    }
}

fn push_token(
    tokens: &mut Vec<PythonToken>,
    start: u32,
    end: usize,
    kind: PythonKind,
) -> Result<(), LexError> {
    if end as u32 <= start {
        return Err(LexError::Nonprogress { at: start });
    }
    tokens.push(PythonToken {
        start,
        end: end as u32,
        kind,
    });
    Ok(())
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

fn start_string(
    bytes: &[u8],
    i: &mut usize,
    state: &mut PythonState,
    tokens: &mut Vec<PythonToken>,
    start: u32,
    raw: bool,
) -> Result<(), LexError> {
    if *i >= bytes.len() {
        return Err(LexError::Nonprogress { at: start });
    }
    let quote = bytes[*i];
    let triple = is_triple_quote(bytes, *i, quote);
    if triple {
        *i += 3;
    } else {
        *i += 1;
    }
    let (closed, end, continued) = scan_string_body(bytes, *i, quote, raw, triple);
    if end < *i && (end as u32) < start {
        return Err(LexError::Nonprogress { at: start });
    }
    *i = end;
    if (*i as u32) > start {
        push_token(tokens, start, *i, PythonKind::String)?;
    } else {
        return Err(LexError::Nonprogress { at: start });
    }
    state.clear_string();
    if triple && !closed {
        state.triple_quote = quote;
        state.triple_raw = raw;
    } else if !triple && !closed && continued {
        // Line (or buffer) ended right after `\`: keep the single-quoted
        // string open so the next lex_line resumes it.
        state.single_quote = quote;
        state.single_raw = raw;
    }
    Ok(())
}

fn resume_triple(
    bytes: &[u8],
    i: &mut usize,
    state: &mut PythonState,
    tokens: &mut Vec<PythonToken>,
    start: u32,
) -> Result<(), LexError> {
    let quote = state.triple_quote;
    let raw = state.triple_raw;
    let (closed, end, _continued) = scan_string_body(bytes, *i, quote, raw, true);
    if end < *i {
        return Err(LexError::Nonprogress { at: start });
    }
    *i = end;
    if (*i as u32) > start {
        push_token(tokens, start, *i, PythonKind::String)?;
    }
    if closed {
        state.triple_quote = 0;
        state.triple_raw = false;
    }
    Ok(())
}

fn resume_single(
    bytes: &[u8],
    i: &mut usize,
    state: &mut PythonState,
    tokens: &mut Vec<PythonToken>,
    start: u32,
) -> Result<(), LexError> {
    let quote = state.single_quote;
    let raw = state.single_raw;
    let (closed, end, continued) = scan_string_body(bytes, *i, quote, raw, false);
    if end < *i {
        return Err(LexError::Nonprogress { at: start });
    }
    *i = end;
    if (*i as u32) > start {
        push_token(tokens, start, *i, PythonKind::String)?;
    }
    if closed {
        state.single_quote = 0;
        state.single_raw = false;
    } else if !continued {
        // Unescaped end of line or EOF: do not keep the single-quoted string.
        state.single_quote = 0;
        state.single_raw = false;
    }
    Ok(())
}

fn lex_one(
    bytes: &[u8],
    i: &mut usize,
    state: &mut PythonState,
    tokens: &mut Vec<PythonToken>,
    cancel: &dyn Fn() -> bool,
) -> Result<(), LexError> {
    if *i >= bytes.len() {
        return Ok(());
    }
    let start = *i as u32;

    if state.in_triple() {
        resume_triple(bytes, i, state, tokens, start)?;
        return Ok(());
    }
    if state.in_single() {
        resume_single(bytes, i, state, tokens, start)?;
        return Ok(());
    }

    let b = bytes[*i];

    if bytes[*i..].starts_with(&[0xef, 0xbb, 0xbf]) {
        *i += 3;
        push_token(tokens, start, *i, PythonKind::Whitespace)?;
        return Ok(());
    }

    if b == b'\\' && matches!(bytes.get(*i + 1), Some(&b'\n') | Some(&b'\r')) {
        *i += 1;
        consume_newline(bytes, i);
        push_token(tokens, start, *i, PythonKind::Whitespace)?;
        return Ok(());
    }

    if b == b'\r' || b == b'\n' {
        consume_newline(bytes, i);
        push_token(tokens, start, *i, PythonKind::Newline)?;
        return Ok(());
    }

    if is_space(b) {
        *i += 1;
        while *i < bytes.len() && is_space(bytes[*i]) {
            *i += 1;
        }
        push_token(tokens, start, *i, PythonKind::Whitespace)?;
        return Ok(());
    }

    if b == b'#' {
        *i += 1;
        while *i < bytes.len() && !is_newline(bytes[*i]) {
            if *i & 255 == 0 && cancel() {
                return Err(LexError::Cancelled);
            }
            *i += 1;
        }
        push_token(tokens, start, *i, PythonKind::Comment)?;
        return Ok(());
    }

    if let Some((plen, raw, _is_f)) = string_prefix(bytes, *i) {
        *i += plen;
        start_string(bytes, i, state, tokens, start, raw)?;
        return Ok(());
    }

    if b == b'\'' || b == b'"' {
        start_string(bytes, i, state, tokens, start, false)?;
        return Ok(());
    }

    if is_ident_start(b) {
        *i += 1;
        while *i < bytes.len() && is_ident_continue(bytes[*i]) {
            *i += 1;
        }
        let ident = std::str::from_utf8(&bytes[start as usize..*i]).unwrap_or("");
        let kind = if is_keyword(ident) {
            PythonKind::Keyword
        } else {
            PythonKind::Identifier
        };
        push_token(tokens, start, *i, kind)?;
        return Ok(());
    }

    if b.is_ascii_digit() || (b == b'.' && bytes.get(*i + 1).copied().unwrap_or(0).is_ascii_digit())
    {
        *i = scan_number(bytes, *i);
        push_token(tokens, start, *i, PythonKind::Number)?;
        return Ok(());
    }

    if matches!(b, b'(' | b')' | b'{' | b'}' | b'[' | b']') {
        *i += 1;
        push_token(tokens, start, *i, PythonKind::Delimiter)?;
        return Ok(());
    }

    let n = scan_punct_len(bytes, *i);
    *i += n;
    if (*i as u32) <= start {
        *i = (start as usize) + 1;
    }
    let kind = if bytes[start as usize].is_ascii_graphic() {
        PythonKind::Punctuator
    } else {
        PythonKind::Unknown
    };
    push_token(tokens, start, *i, kind)?;
    Ok(())
}

/// Tokenize an entire document. Spans are byte offsets into `bytes`.
/// Every byte is covered by exactly one token. `cancel` is observed every
/// 256 tokens. A run that does not advance the cursor is `Nonprogress`.
pub fn lex_document_cancellable(
    bytes: &[u8],
    cancel: &dyn Fn() -> bool,
) -> Result<Vec<PythonToken>, LexError> {
    let mut tokens = Vec::new();
    let mut state = PythonState::default();
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
        lex_one(bytes, &mut i, &mut state, &mut tokens, cancel)?;
        if i == start_i && i < bytes.len() {
            return Err(LexError::Nonprogress { at: i as u32 });
        }
    }
    Ok(tokens)
}

/// Tokenize one display line (without the newline) given incoming continuation.
/// Each pair is `(end_index, role)` covering `[prev_end, end)` of `line`.
/// Triple-quoted string state crosses lines. A single-quoted string that
/// ended the previous line immediately after `\` is resumed; an unescaped
/// end of line (including an empty line) resets that continuation.
pub fn lex_line(line: &str, incoming: PythonState) -> (PythonState, Vec<(usize, TokenRole)>) {
    let bytes = line.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0usize;
    let mut state = incoming;
    if bytes.is_empty() {
        if !state.in_triple() {
            state.single_quote = 0;
            state.single_raw = false;
        }
        return (state, Vec::new());
    }
    let cancel = || false;
    while i < bytes.len() {
        let start_i = i;
        if lex_one(bytes, &mut i, &mut state, &mut tokens, &cancel).is_err() {
            if i == start_i {
                i += 1;
                let _ = push_token(&mut tokens, start_i as u32, i, PythonKind::Unknown);
            }
        }
        if i == start_i && i < bytes.len() {
            i += 1;
            let _ = push_token(&mut tokens, start_i as u32, i, PythonKind::Unknown);
        }
    }
    let roles = tokens_to_roles(bytes, &tokens);
    (state, roles)
}

/// Convert document tokens into public spans, classifying keywords and
/// function names with one-token lookahead.
pub fn document_spans(bytes: &[u8], tokens: &[PythonToken]) -> Vec<TokenSpan> {
    let mut out = Vec::with_capacity(tokens.len());
    for (i, t) in tokens.iter().enumerate() {
        let mut role = t.kind.role();
        if t.kind == PythonKind::Keyword || t.kind == PythonKind::Identifier {
            let mut j = i + 1;
            while j < tokens.len() && tokens[j].kind.role().is_trivia() {
                j += 1;
            }
            let next_is_paren = j < tokens.len()
                && tokens[j].kind == PythonKind::Delimiter
                && bytes.get(tokens[j].start as usize) == Some(&b'(');
            let ident = std::str::from_utf8(&bytes[t.start as usize..t.end as usize]).unwrap_or("");
            role = classify_identifier(ident, next_is_paren);
        }
        out.push(TokenSpan::new(t.start, t.end, role));
    }
    out
}

fn tokens_to_roles(bytes: &[u8], tokens: &[PythonToken]) -> Vec<(usize, TokenRole)> {
    let mut out = Vec::new();
    for (i, t) in tokens.iter().enumerate() {
        let mut role = t.kind.role();
        if t.kind == PythonKind::Keyword || t.kind == PythonKind::Identifier {
            let mut j = i + 1;
            while j < tokens.len() && tokens[j].kind.role().is_trivia() {
                j += 1;
            }
            let next_is_paren = j < tokens.len()
                && tokens[j].kind == PythonKind::Delimiter
                && bytes.get(tokens[j].start as usize) == Some(&b'(');
            let ident = std::str::from_utf8(&bytes[t.start as usize..t.end as usize]).unwrap_or("");
            role = classify_identifier(ident, next_is_paren);
        }
        push_role(t.end as usize, role, &mut out);
    }
    out
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
