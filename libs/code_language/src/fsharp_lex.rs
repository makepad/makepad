//! Source-only F# lexer shared by the editor and the F# frontend.
//! Byte offsets address the original document. Continuation state is owned
//! here: nested `(* *)` comments and multi-line strings are not packed into
//! a universal integer.
//!
//! Columns used by the frontend are byte offsets after each `Newline` token;
//! a tab counts as one column.

use crate::token::{TokenRole, TokenSpan};
use crate::LexError;

/// Version of this lexer; part of parse and search cache identity.
pub const FSHARP_LEXER_VERSION: u32 = 1;

/// Brace depth of interpolation holes. Cap 2 as specified (outer hole plus
/// one nested `{` in the hole expression). Nested interpolations inside a
/// hole are not opened.
const HOLE_CAP: u8 = 2;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
enum FSharpMode {
    #[default]
    Normal,
    BlockComment {
        depth: u8,
    },
    String {
        verbatim: bool,
        triple: bool,
        interpolated: bool,
    },
}

/// Provider-owned line continuation. Default is Normal (not inside a
/// comment or string). `lex_line` and the document lexer share one state
/// machine so highlighting and parsing agree.
///
/// `hole > 0` means the current interpolated string has an open `{ expr }`
/// hole; interior tokens are lexed in Normal until the matching `}`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct FSharpState {
    mode: FSharpMode,
    /// Brace depth inside an interpolation hole. 0 = not in a hole.
    hole: u8,
    /// Host string flags saved while `hole > 0`.
    hole_verbatim: bool,
    hole_triple: bool,
    /// Nested (non-host) string open inside a hole: 0 = none.
    nest: u8,
    nest_verbatim: bool,
    nest_triple: bool,
    /// The next `{` opens a hole (string scan stopped immediately before it).
    pending_hole: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FSharpKind {
    Whitespace,
    Newline,
    Comment,
    Identifier,
    Keyword,
    Directive,
    Number,
    String,
    Char,
    Punctuator,
    Delimiter,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FSharpToken {
    pub start: u32,
    pub end: u32,
    pub kind: FSharpKind,
}

impl FSharpKind {
    pub fn role(self) -> TokenRole {
        match self {
            FSharpKind::Whitespace | FSharpKind::Newline => TokenRole::Whitespace,
            FSharpKind::Comment => TokenRole::Comment,
            FSharpKind::Identifier => TokenRole::Identifier,
            FSharpKind::Keyword => TokenRole::Keyword,
            FSharpKind::Directive => TokenRole::Preprocessor,
            FSharpKind::Number => TokenRole::Number,
            FSharpKind::String => TokenRole::String,
            FSharpKind::Char => TokenRole::Char,
            FSharpKind::Punctuator => TokenRole::Punctuator,
            FSharpKind::Delimiter => TokenRole::Delimiter,
            FSharpKind::Unknown => TokenRole::Unknown,
        }
    }
}

/// F# keywords plus reserved identifiers, sorted for binary search.
const KEYWORDS: &[&str] = &[
    "abstract",
    "and",
    "as",
    "assert",
    "atomic",
    "base",
    "begin",
    "break",
    "checked",
    "class",
    "component",
    "const",
    "constraint",
    "constructor",
    "continue",
    "default",
    "delegate",
    "do",
    "done",
    "downcast",
    "downto",
    "eager",
    "elif",
    "else",
    "end",
    "event",
    "exception",
    "extern",
    "external",
    "false",
    "finally",
    "fixed",
    "for",
    "fun",
    "function",
    "functor",
    "global",
    "if",
    "in",
    "include",
    "inherit",
    "inline",
    "interface",
    "internal",
    "lazy",
    "let",
    "match",
    "member",
    "method",
    "mixin",
    "module",
    "mutable",
    "namespace",
    "new",
    "not",
    "null",
    "object",
    "of",
    "open",
    "or",
    "override",
    "parallel",
    "private",
    "process",
    "protected",
    "public",
    "pure",
    "rec",
    "return",
    "sealed",
    "select",
    "sig",
    "static",
    "struct",
    "tailcall",
    "then",
    "to",
    "trait",
    "true",
    "try",
    "type",
    "upcast",
    "use",
    "val",
    "virtual",
    "void",
    "volatile",
    "when",
    "while",
    "with",
    "yield",
];

const BANG_BASES: &[&str] = &["and", "do", "let", "match", "return", "use", "yield"];

const DIRECTIVES: &[&str] = &[
    "I", "else", "endif", "help", "if", "indent", "light", "line", "load", "nowarn", "quit", "r",
    "time",
];

const MAGIC: &[&str] = &["__LINE__", "__SOURCE_DIRECTORY__", "__SOURCE_FILE__"];

pub fn is_keyword(ident: &str) -> bool {
    if KEYWORDS.binary_search(&ident).is_ok() {
        return true;
    }
    if MAGIC.binary_search(&ident).is_ok() {
        return true;
    }
    if let Some(base) = ident.strip_suffix('!') {
        return BANG_BASES.binary_search(&base).is_ok();
    }
    false
}

fn classify_identifier(ident: &str, next_is_paren_nospace: bool) -> TokenRole {
    let name = strip_backticks(ident);
    match name {
        "if" | "then" | "elif" | "else" | "match" | "match!" | "with" | "when" | "try"
        | "finally" | "raise" | "failwith" | "return" | "return!" | "yield" | "yield!" => {
            TokenRole::BranchKeyword
        }
        "for" | "to" | "downto" | "while" | "do" | "do!" | "done" | "in" => TokenRole::LoopKeyword,
        "true" | "false" | "null" => TokenRole::Constant,
        "let" | "let!" | "use" | "use!" | "rec" | "and" | "and!" | "type" | "module"
        | "namespace" | "open" | "member" | "val" | "abstract" | "override" | "default"
        | "static" | "new" | "inherit" | "interface" | "exception" | "mutable" | "inline"
        | "private" | "internal" | "public" => TokenRole::Keyword,
        other if is_keyword(other) => TokenRole::Keyword,
        _ if next_is_paren_nospace => TokenRole::Function,
        other if other.starts_with('\'') => TokenRole::Identifier,
        other if other
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

fn strip_backticks(ident: &str) -> &str {
    if ident.len() >= 4 && ident.starts_with("``") && ident.ends_with("``") {
        &ident[2..ident.len() - 2]
    } else {
        ident
    }
}

fn is_space(b: u8) -> bool {
    b == b' ' || b == b'\t' || b == 0x0b || b == 0x0c
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || b >= 0x80
}

fn is_ident_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'\'' || b >= 0x80
}

fn is_newline(b: u8) -> bool {
    b == b'\n' || b == b'\r'
}

fn is_op_char(b: u8) -> bool {
    matches!(
        b,
        b'!' | b'%'
            | b'&'
            | b'*'
            | b'+'
            | b'-'
            | b'.'
            | b'/'
            | b'<'
            | b'='
            | b'>'
            | b'?'
            | b'@'
            | b'^'
            | b'|'
            | b'~'
            | b':'
            | b'$'
    )
}

fn push_token(
    tokens: &mut Vec<FSharpToken>,
    start: u32,
    end: usize,
    kind: FSharpKind,
) -> Result<(), LexError> {
    if end as u32 <= start {
        return Err(LexError::Nonprogress { at: start });
    }
    tokens.push(FSharpToken {
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

fn consume_byte_suffix(bytes: &[u8], i: &mut usize) {
    if bytes.get(*i) == Some(&b'B')
        && bytes
            .get(*i + 1)
            .map(|&c| !is_ident_continue(c))
            .unwrap_or(true)
    {
        *i += 1;
    }
}

fn is_directive_word(word: &str) -> bool {
    DIRECTIVES.binary_search(&word).is_ok()
}

/// Longest operator token starting at `i`. Delimiter-prefixed pairs (`[|`,
/// `{|`, `(|`, `[<`) are handled by the caller.
fn op_token_len(bytes: &[u8], i: usize) -> usize {
    let rest = &bytes[i..];
    if rest.is_empty() {
        return 0;
    }
    // Longest first among the operators that must stay separate tokens,
    // plus `|||>` / `||>` so pipeline forms are one Punctuator.
    if rest.starts_with(b"|||>") {
        return 4;
    }
    if rest.starts_with(b":?>")
        || rest.starts_with(b"||>")
        || rest.starts_with(b"<@@")
        || rest.starts_with(b"@@>")
    {
        return 3;
    }
    if matches!(
        rest.get(..2),
        Some(
            b"->" | b"<-"
                | b"|>"
                | b"<|"
                | b">>"
                | b"<<"
                | b"::"
                | b":>"
                | b":?"
                | b":="
                | b".."
                | b"&&"
                | b"||"
                | b"<@"
                | b"@>"
                | b"|]"
                | b"|}"
                | b"|)"
                | b">]"
        )
    ) {
        return 2;
    }
    if is_op_char(rest[0]) {
        1
    } else {
        0
    }
}

fn delim_pair_len(bytes: &[u8], i: usize) -> Option<usize> {
    let rest = &bytes[i..];
    if rest.starts_with(b"[|")
        || rest.starts_with(b"[<")
        || rest.starts_with(b"{|")
        || rest.starts_with(b"(|")
    {
        return Some(2);
    }
    None
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
            return consume_numeric_suffix(bytes, i.max(start + 1));
        }
    }
    while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'_') {
        i += 1;
    }
    if bytes.get(i) == Some(&b'.') {
        let next = bytes.get(i + 1).copied().unwrap_or(0);
        if next == b'.' {
            // `1..10`: leave `..` for the operator scanner.
        } else if is_ident_start(next) {
            // `1.foo`: method access; do not consume the `.`.
        } else if next.is_ascii_digit() {
            i += 1;
            while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'_') {
                i += 1;
            }
        } else {
            // `1.` followed by non-digit, non-ident: a float.
            i += 1;
        }
    }
    if i < bytes.len() && matches!(bytes[i], b'e' | b'E') {
        let mut j = i + 1;
        if j < bytes.len() && matches!(bytes[j], b'+' | b'-') {
            j += 1;
        }
        if j < bytes.len() && bytes[j].is_ascii_digit() {
            i = j + 1;
            while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'_') {
                i += 1;
            }
        }
    }
    consume_numeric_suffix(bytes, i.max(start + 1))
}

fn consume_numeric_suffix(bytes: &[u8], mut i: usize) -> usize {
    // y uy s us l ul L UL n un m f F I
    let a = bytes.get(i).copied();
    let b = bytes.get(i + 1).copied();
    let two = match (a, b) {
        (Some(b'u'), Some(b'y' | b's' | b'l' | b'n')) => true,
        (Some(b'U'), Some(b'L')) => true,
        _ => false,
    };
    if two
        && bytes
            .get(i + 2)
            .map(|&c| !is_ident_continue(c))
            .unwrap_or(true)
    {
        return i + 2;
    }
    if matches!(a, Some(b'y' | b's' | b'l' | b'L' | b'n' | b'm' | b'f' | b'F' | b'I'))
        && bytes
            .get(i + 1)
            .map(|&c| !is_ident_continue(c))
            .unwrap_or(true)
    {
        i += 1;
    }
    i
}

fn string_open(bytes: &[u8], i: usize) -> Option<(usize, bool, bool, bool)> {
    // Returns (open_len, verbatim, triple, interpolated).
    let mut j = i;
    let mut interpolated = false;
    let mut verbatim = false;
    if bytes.get(j) == Some(&b'$') {
        interpolated = true;
        j += 1;
    }
    if bytes.get(j) == Some(&b'@') {
        verbatim = true;
        j += 1;
        if !interpolated && bytes.get(j) == Some(&b'$') {
            interpolated = true;
            j += 1;
        }
    }
    if bytes.get(j) != Some(&b'"') {
        return None;
    }
    let triple = bytes.get(j + 1) == Some(&b'"') && bytes.get(j + 2) == Some(&b'"');
    if triple && verbatim {
        // `@"..."` is never triple: further quotes are content (`""`).
        j += 1;
        return Some((j - i, true, false, interpolated));
    }
    if triple {
        j += 3;
    } else {
        j += 1;
    }
    if j == i {
        return None;
    }
    Some((j - i, verbatim, triple, interpolated))
}

/// Scan string body from `i`. Stops before a hole `{` (not `{{`) when
/// interpolated. Returns `(end, closed, hole)`.
fn scan_string_body(
    bytes: &[u8],
    mut i: usize,
    verbatim: bool,
    triple: bool,
    interpolated: bool,
) -> (usize, bool, bool) {
    while i < bytes.len() {
        let b = bytes[i];
        if !verbatim && !triple && b == b'\\' {
            i += 1;
            if i < bytes.len() {
                if bytes[i] == b'\r' {
                    i += 1;
                    if bytes.get(i) == Some(&b'\n') {
                        i += 1;
                    }
                } else {
                    i += 1;
                }
            }
            continue;
        }
        if interpolated && b == b'{' {
            if bytes.get(i + 1) == Some(&b'{') {
                i += 2;
                continue;
            }
            return (i, false, true);
        }
        if interpolated && b == b'}' {
            if bytes.get(i + 1) == Some(&b'}') {
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }
        if triple {
            if b == b'"' && bytes.get(i + 1) == Some(&b'"') && bytes.get(i + 2) == Some(&b'"') {
                i += 3;
                consume_byte_suffix(bytes, &mut i);
                return (i, true, false);
            }
            i += 1;
            continue;
        }
        if b == b'"' {
            if verbatim && bytes.get(i + 1) == Some(&b'"') {
                i += 2;
                continue;
            }
            i += 1;
            consume_byte_suffix(bytes, &mut i);
            return (i, true, false);
        }
        i += 1;
    }
    (i, false, false)
}

fn enter_string(state: &mut FSharpState, verbatim: bool, triple: bool, interpolated: bool) {
    state.mode = FSharpMode::String {
        verbatim,
        triple,
        interpolated,
    };
}

fn reset_string(state: &mut FSharpState) {
    state.mode = FSharpMode::Normal;
    state.hole = 0;
    state.nest = 0;
    state.pending_hole = false;
}

fn resume_host_string(state: &mut FSharpState) {
    state.mode = FSharpMode::String {
        verbatim: state.hole_verbatim,
        triple: state.hole_triple,
        interpolated: true,
    };
    state.hole = 0;
    state.nest = 0;
}

fn lex_one(
    bytes: &[u8],
    i: &mut usize,
    state: &mut FSharpState,
    tokens: &mut Vec<FSharpToken>,
    line_start: &mut bool,
    cancel: &dyn Fn() -> bool,
) -> Result<(), LexError> {
    if *i >= bytes.len() {
        return Ok(());
    }
    let start = *i as u32;

    match state.mode {
        FSharpMode::BlockComment { depth } => {
            return resume_block_comment(bytes, i, state, tokens, start, depth, line_start);
        }
        FSharpMode::String {
            verbatim,
            triple,
            interpolated,
        } if state.hole == 0 && state.nest == 0 => {
            return resume_string(
                bytes,
                i,
                state,
                tokens,
                start,
                verbatim,
                triple,
                interpolated,
                line_start,
            );
        }
        _ => {}
    }

    if state.nest > 0 {
        return resume_nest_string(bytes, i, state, tokens, start, line_start);
    }

    if bytes[*i..].starts_with(&[0xef, 0xbb, 0xbf]) {
        *i += 3;
        push_token(tokens, start, *i, FSharpKind::Whitespace)?;
        return Ok(());
    }

    let b = bytes[*i];

    if b == b'\r' || b == b'\n' {
        consume_newline(bytes, i);
        *line_start = true;
        push_token(tokens, start, *i, FSharpKind::Newline)?;
        return Ok(());
    }

    if is_space(b) {
        *i += 1;
        while *i < bytes.len() && is_space(bytes[*i]) {
            *i += 1;
        }
        push_token(tokens, start, *i, FSharpKind::Whitespace)?;
        return Ok(());
    }

    if *line_start && b == b'#' {
        *i += 1;
        let word_start = *i;
        while *i < bytes.len() && is_ident_continue(bytes[*i]) {
            *i += 1;
        }
        let word = std::str::from_utf8(&bytes[word_start..*i]).unwrap_or("");
        if is_directive_word(word) {
            *line_start = false;
            push_token(tokens, start, *i, FSharpKind::Directive)?;
            return Ok(());
        }
        *i = (start as usize) + 1;
        *line_start = false;
        push_token(tokens, start, *i, FSharpKind::Punctuator)?;
        return Ok(());
    }

    *line_start = false;

    if state.pending_hole && b == b'{' {
        *i += 1;
        state.pending_hole = false;
        state.hole = 1;
        push_token(tokens, start, *i, FSharpKind::Punctuator)?;
        return Ok(());
    }
    if state.hole > 0 && b == b'}' {
        *i += 1;
        state.hole = state.hole.saturating_sub(1);
        push_token(tokens, start, *i, FSharpKind::Punctuator)?;
        if state.hole == 0 {
            resume_host_string(state);
        }
        return Ok(());
    }
    if state.hole > 0 && b == b'{' {
        *i += 1;
        if state.hole < HOLE_CAP {
            state.hole = state.hole.saturating_add(1);
        }
        push_token(tokens, start, *i, FSharpKind::Punctuator)?;
        return Ok(());
    }

    if b == b'/' && bytes.get(*i + 1) == Some(&b'/') {
        *i += 2;
        while *i < bytes.len() && !is_newline(bytes[*i]) {
            if *i & 255 == 0 && cancel() {
                return Err(LexError::Cancelled);
            }
            *i += 1;
        }
        push_token(tokens, start, *i, FSharpKind::Comment)?;
        return Ok(());
    }

    // `(*` starts a nested comment unless the next byte is `)` (`(*)` is
    // multiplication in parentheses). Stop at a newline so the first token
    // after a multi-line comment on the closing line measures its column
    // from that Newline, matching `resume_block_comment`.
    if b == b'(' && bytes.get(*i + 1) == Some(&b'*') && bytes.get(*i + 2) != Some(&b')') {
        *i += 2;
        state.mode = FSharpMode::BlockComment { depth: 1 };
        return resume_block_comment(bytes, i, state, tokens, start, 1, line_start);
    }

    if let Some((open_len, verbatim, triple, interpolated)) = string_open(bytes, *i) {
        *i += open_len;
        if state.hole > 0 {
            // One interpolation level: a string inside a hole is lexed as a
            // plain string; `{` in it does not open a nested hole.
            state.nest = 1;
            state.nest_verbatim = verbatim;
            state.nest_triple = triple;
            let (end, closed, _) = scan_string_body(bytes, *i, verbatim, triple, false);
            if end < *i {
                return Err(LexError::Nonprogress { at: start });
            }
            *i = end;
            if closed {
                state.nest = 0;
            }
            if (*i as u32) <= start {
                return Err(LexError::Nonprogress { at: start });
            }
            push_token(tokens, start, *i, FSharpKind::String)?;
            return Ok(());
        }
        enter_string(state, verbatim, triple, interpolated);
        let (end, closed, hole) = scan_string_body(bytes, *i, verbatim, triple, interpolated);
        if end < *i {
            return Err(LexError::Nonprogress { at: start });
        }
        *i = end;
        if hole {
            state.hole_verbatim = verbatim;
            state.hole_triple = triple;
            state.pending_hole = true;
            state.mode = FSharpMode::Normal;
            if (*i as u32) > start {
                push_token(tokens, start, *i, FSharpKind::String)?;
            } else {
                return Err(LexError::Nonprogress { at: start });
            }
            return Ok(());
        }
        if closed {
            reset_string(state);
        }
        if (*i as u32) <= start {
            return Err(LexError::Nonprogress { at: start });
        }
        push_token(tokens, start, *i, FSharpKind::String)?;
        return Ok(());
    }

    if b == b'\'' {
        return lex_char_or_tyvar(bytes, i, tokens, start);
    }

    if b == b'`' && bytes.get(*i + 1) == Some(&b'`') {
        *i += 2;
        while *i + 1 < bytes.len() {
            if bytes[*i] == b'`' && bytes[*i + 1] == b'`' {
                *i += 2;
                break;
            }
            *i += 1;
        }
        if (*i as u32) <= start {
            *i = (start as usize) + 1;
        }
        push_token(tokens, start, *i, FSharpKind::Identifier)?;
        return Ok(());
    }

    if is_ident_start(b) {
        *i += 1;
        while *i < bytes.len() && is_ident_continue(bytes[*i]) {
            *i += 1;
        }
        let ident = std::str::from_utf8(&bytes[start as usize..*i]).unwrap_or("");
        if BANG_BASES.binary_search(&ident).is_ok() && bytes.get(*i) == Some(&b'!') {
            *i += 1;
        }
        let ident = std::str::from_utf8(&bytes[start as usize..*i]).unwrap_or("");
        let kind = if is_keyword(ident) {
            FSharpKind::Keyword
        } else {
            FSharpKind::Identifier
        };
        push_token(tokens, start, *i, kind)?;
        return Ok(());
    }

    if b.is_ascii_digit() {
        *i = scan_number(bytes, *i);
        push_token(tokens, start, *i, FSharpKind::Number)?;
        return Ok(());
    }

    if b == b';' {
        *i += 1;
        if bytes.get(*i) == Some(&b';') {
            *i += 1;
        }
        push_token(tokens, start, *i, FSharpKind::Punctuator)?;
        return Ok(());
    }
    if b == b',' {
        *i += 1;
        push_token(tokens, start, *i, FSharpKind::Punctuator)?;
        return Ok(());
    }

    if let Some(n) = delim_pair_len(bytes, *i) {
        *i += n;
        push_token(tokens, start, *i, FSharpKind::Punctuator)?;
        return Ok(());
    }

    if matches!(b, b'(' | b')' | b'{' | b'}' | b'[' | b']') {
        *i += 1;
        push_token(tokens, start, *i, FSharpKind::Delimiter)?;
        return Ok(());
    }

    let n = op_token_len(bytes, *i);
    if n > 0 {
        *i += n;
        push_token(tokens, start, *i, FSharpKind::Punctuator)?;
        return Ok(());
    }

    *i += 1;
    if (*i as u32) <= start {
        *i = (start as usize) + 1;
    }
    let kind = if bytes[start as usize].is_ascii_graphic() {
        FSharpKind::Punctuator
    } else {
        FSharpKind::Unknown
    };
    push_token(tokens, start, *i, kind)?;
    Ok(())
}

fn resume_block_comment(
    bytes: &[u8],
    i: &mut usize,
    state: &mut FSharpState,
    tokens: &mut Vec<FSharpToken>,
    start: u32,
    mut depth: u8,
    line_start: &mut bool,
) -> Result<(), LexError> {
    while *i < bytes.len() && depth > 0 {
        if bytes[*i] == b'(' && bytes.get(*i + 1) == Some(&b'*') && bytes.get(*i + 2) != Some(&b')')
        {
            *i += 2;
            depth = depth.saturating_add(1);
            continue;
        }
        if bytes[*i] == b'*' && bytes.get(*i + 1) == Some(&b')') {
            *i += 2;
            depth = depth.saturating_sub(1);
            continue;
        }
        if is_newline(bytes[*i]) {
            if *i as u32 > start {
                state.mode = FSharpMode::BlockComment { depth };
                push_token(tokens, start, *i, FSharpKind::Comment)?;
                return Ok(());
            }
            consume_newline(bytes, i);
            *line_start = true;
            push_token(tokens, start, *i, FSharpKind::Newline)?;
            state.mode = FSharpMode::BlockComment { depth };
            return Ok(());
        }
        *i += 1;
    }
    if depth == 0 {
        state.mode = FSharpMode::Normal;
    } else {
        state.mode = FSharpMode::BlockComment { depth };
    }
    if (*i as u32) <= start {
        return Err(LexError::Nonprogress { at: start });
    }
    push_token(tokens, start, *i, FSharpKind::Comment)?;
    Ok(())
}

fn resume_string(
    bytes: &[u8],
    i: &mut usize,
    state: &mut FSharpState,
    tokens: &mut Vec<FSharpToken>,
    start: u32,
    verbatim: bool,
    triple: bool,
    interpolated: bool,
    _line_start: &mut bool,
) -> Result<(), LexError> {
    let (end, closed, hole) = scan_string_body(bytes, *i, verbatim, triple, interpolated);
    if end < *i {
        return Err(LexError::Nonprogress { at: start });
    }
    *i = end;
    if hole {
        state.hole_verbatim = verbatim;
        state.hole_triple = triple;
        state.pending_hole = true;
        state.mode = FSharpMode::Normal;
        if (*i as u32) > start {
            push_token(tokens, start, *i, FSharpKind::String)?;
            return Ok(());
        }
        return Err(LexError::Nonprogress { at: start });
    }
    if closed {
        reset_string(state);
    }
    if (*i as u32) <= start {
        return Err(LexError::Nonprogress { at: start });
    }
    push_token(tokens, start, *i, FSharpKind::String)?;
    Ok(())
}

fn resume_nest_string(
    bytes: &[u8],
    i: &mut usize,
    state: &mut FSharpState,
    tokens: &mut Vec<FSharpToken>,
    start: u32,
    _line_start: &mut bool,
) -> Result<(), LexError> {
    let verbatim = state.nest_verbatim;
    let triple = state.nest_triple;
    let (end, closed, _hole) = scan_string_body(bytes, *i, verbatim, triple, false);
    if end < *i {
        return Err(LexError::Nonprogress { at: start });
    }
    *i = end;
    if closed {
        state.nest = 0;
        state.mode = FSharpMode::Normal;
    }
    if (*i as u32) <= start {
        return Err(LexError::Nonprogress { at: start });
    }
    push_token(tokens, start, *i, FSharpKind::String)?;
    Ok(())
}

fn lex_char_or_tyvar(
    bytes: &[u8],
    i: &mut usize,
    tokens: &mut Vec<FSharpToken>,
    start: u32,
) -> Result<(), LexError> {
    // `'` at `*i`.
    let next = bytes.get(*i + 1).copied();
    if next == Some(b'\\') {
        *i += 2;
        if *i < bytes.len() && !is_newline(bytes[*i]) {
            *i += 1;
        }
        if bytes.get(*i) == Some(&b'\'') {
            *i += 1;
        }
        consume_byte_suffix(bytes, i);
        push_token(tokens, start, *i, FSharpKind::Char)?;
        return Ok(());
    }
    if next.is_some_and(is_ident_start) {
        let after = bytes.get(*i + 2).copied();
        if after == Some(b'\'') {
            *i += 3;
            consume_byte_suffix(bytes, i);
            push_token(tokens, start, *i, FSharpKind::Char)?;
            return Ok(());
        }
        *i += 1;
        while *i < bytes.len() && is_ident_continue(bytes[*i]) {
            *i += 1;
        }
        push_token(tokens, start, *i, FSharpKind::Identifier)?;
        return Ok(());
    }
    *i += 1;
    if *i < bytes.len() && !is_newline(bytes[*i]) && bytes[*i] != b'\'' {
        *i += 1;
    }
    if bytes.get(*i) == Some(&b'\'') {
        *i += 1;
    }
    consume_byte_suffix(bytes, i);
    if (*i as u32) <= start {
        *i = (start as usize) + 1;
    }
    push_token(tokens, start, *i, FSharpKind::Char)?;
    Ok(())
}

/// Tokenize an entire document. Spans are byte offsets into `bytes`.
/// Every byte is covered by exactly one token. `cancel` is observed every
/// 256 tokens. A run that does not advance the cursor is `Nonprogress`.
pub fn lex_document_cancellable(
    bytes: &[u8],
    cancel: &dyn Fn() -> bool,
) -> Result<Vec<FSharpToken>, LexError> {
    let mut tokens = Vec::new();
    let mut state = FSharpState::default();
    let mut i = 0usize;
    let mut line_start = true;
    let token_cap = bytes.len().saturating_add(8);
    while i < bytes.len() {
        if tokens.len() & 255 == 0 && cancel() {
            return Err(LexError::Cancelled);
        }
        if tokens.len() > token_cap {
            return Err(LexError::Nonprogress { at: i as u32 });
        }
        let start_i = i;
        lex_one(
            bytes,
            &mut i,
            &mut state,
            &mut tokens,
            &mut line_start,
            cancel,
        )?;
        if i == start_i && i < bytes.len() {
            return Err(LexError::Nonprogress { at: i as u32 });
        }
    }
    Ok(tokens)
}

/// Tokenize one display line (without the newline) given incoming continuation.
/// Each pair is `(end_index, role)` covering `[prev_end, end)` of `line`.
/// Nested `(* *)` comments, multi-line strings, triple-quoted strings and
/// interpolated holes persist across lines via `FSharpState`.
pub fn lex_line(line: &str, incoming: FSharpState) -> (FSharpState, Vec<(usize, TokenRole)>) {
    let bytes = line.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0usize;
    let mut state = incoming;
    let mut line_start = matches!(state.mode, FSharpMode::Normal) && state.hole == 0 && state.nest == 0;
    if bytes.is_empty() {
        return (state, Vec::new());
    }
    let cancel = || false;
    while i < bytes.len() {
        let start_i = i;
        if lex_one(
            bytes,
            &mut i,
            &mut state,
            &mut tokens,
            &mut line_start,
            &cancel,
        )
        .is_err()
        {
            if i == start_i {
                i += 1;
                let _ = push_token(&mut tokens, start_i as u32, i, FSharpKind::Unknown);
            }
        }
        if i == start_i && i < bytes.len() {
            i += 1;
            let _ = push_token(&mut tokens, start_i as u32, i, FSharpKind::Unknown);
        }
    }
    let roles = tokens_to_roles(bytes, &tokens);
    (state, roles)
}

/// Convert document tokens into public spans, classifying keywords and
/// function names. An identifier is `Function` only when the next token is
/// `(` with no intervening bytes.
pub fn document_spans(bytes: &[u8], tokens: &[FSharpToken]) -> Vec<TokenSpan> {
    let mut out = Vec::with_capacity(tokens.len());
    for (i, t) in tokens.iter().enumerate() {
        let mut role = t.kind.role();
        if t.kind == FSharpKind::Keyword || t.kind == FSharpKind::Identifier {
            let next_is_paren_nospace = tokens.get(i + 1).is_some_and(|n| {
                n.kind == FSharpKind::Delimiter
                    && n.start == t.end
                    && bytes.get(n.start as usize) == Some(&b'(')
            });
            let ident = std::str::from_utf8(&bytes[t.start as usize..t.end as usize]).unwrap_or("");
            role = classify_identifier(ident, next_is_paren_nospace);
        }
        out.push(TokenSpan::new(t.start, t.end, role));
    }
    out
}

fn tokens_to_roles(bytes: &[u8], tokens: &[FSharpToken]) -> Vec<(usize, TokenRole)> {
    let mut out = Vec::new();
    for (i, t) in tokens.iter().enumerate() {
        let mut role = t.kind.role();
        if t.kind == FSharpKind::Keyword || t.kind == FSharpKind::Identifier {
            let next_is_paren_nospace = tokens.get(i + 1).is_some_and(|n| {
                n.kind == FSharpKind::Delimiter
                    && n.start == t.end
                    && bytes.get(n.start as usize) == Some(&b'(')
            });
            let ident = std::str::from_utf8(&bytes[t.start as usize..t.end as usize]).unwrap_or("");
            role = classify_identifier(ident, next_is_paren_nospace);
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
