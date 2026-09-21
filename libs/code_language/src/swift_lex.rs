//! Source-only Swift lexer shared by the editor and the Swift frontend.
//! Byte offsets address the original document. Continuation state is owned
//! here: nested block comments and multi-line / raw strings survive line
//! breaks; regular `"..."` strings are line-bounded. Interpolation `\( ... )`
//! splits the surrounding string into Punctuator spans; nested strings inside
//! interpolation that contain `\( ` beyond depth 2 stay String.

use crate::token::{TokenRole, TokenSpan};
use crate::LexError;

/// Version of this lexer; part of parse and search cache identity.
pub const SWIFT_LEXER_VERSION: u32 = 1;

const HASH_CAP: u8 = 8;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
enum SwiftMode {
    #[default]
    Normal,
    BlockComment {
        depth: u8,
    },
    /// `"..."` / `"""..."""` / `#"..."#` / `#"""..."""#`.
    /// `hashes` is this string's raw delimiter count (0 = ordinary quotes).
    /// `multi` is a triple-quoted opener. `nest` is interpolation nesting
    /// (0 outer, 1 inside `\(`, 2 inside a nested string's `\( `).
    /// `host_*` is the immediate parent string; `root_*` is the nest-0 host.
    MultiString {
        hashes: u8,
        multi: bool,
        nest: u8,
        host_hashes: u8,
        host_multi: bool,
        root_hashes: u8,
        root_multi: bool,
    },
    /// `\( ... )` of a string. `parens` is extra `(` depth inside the hole.
    Interp {
        hashes: u8,
        multi: bool,
        nest: u8,
        parens: u8,
        host_hashes: u8,
        host_multi: bool,
        root_hashes: u8,
        root_multi: bool,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct SwiftState {
    mode: SwiftMode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwiftKind {
    Whitespace,
    Newline,
    Comment,
    Identifier,
    Keyword,
    Attribute,
    Directive,
    Number,
    String,
    Punctuator,
    Delimiter,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SwiftToken {
    pub start: u32,
    pub end: u32,
    pub kind: SwiftKind,
}

impl SwiftKind {
    pub fn role(self) -> TokenRole {
        match self {
            SwiftKind::Whitespace | SwiftKind::Newline => TokenRole::Whitespace,
            SwiftKind::Comment => TokenRole::Comment,
            SwiftKind::Identifier => TokenRole::Identifier,
            SwiftKind::Keyword => TokenRole::Keyword,
            SwiftKind::Attribute | SwiftKind::Directive => TokenRole::Preprocessor,
            SwiftKind::Number => TokenRole::Number,
            SwiftKind::String => TokenRole::String,
            SwiftKind::Punctuator => TokenRole::Punctuator,
            SwiftKind::Delimiter => TokenRole::Delimiter,
            SwiftKind::Unknown => TokenRole::Unknown,
        }
    }
}

/// Hard keywords. Contextual `actor`/`async`/`macro`/`consuming`/`borrowing`
/// and `some`/`any` stay Identifier unless classified. Sorted.
const KEYWORDS: &[&str] = &[
    "Any",
    "Self",
    "as",
    "associatedtype",
    "await",
    "break",
    "case",
    "catch",
    "class",
    "continue",
    "default",
    "defer",
    "deinit",
    "do",
    "else",
    "enum",
    "extension",
    "fallthrough",
    "false",
    "fileprivate",
    "for",
    "func",
    "guard",
    "if",
    "import",
    "in",
    "init",
    "inout",
    "internal",
    "is",
    "let",
    "nil",
    "open",
    "operator",
    "precedencegroup",
    "private",
    "protocol",
    "public",
    "repeat",
    "rethrows",
    "return",
    "self",
    "static",
    "struct",
    "subscript",
    "super",
    "switch",
    "throw",
    "throws",
    "true",
    "try",
    "typealias",
    "var",
    "where",
    "while",
];

const CONTEXTUAL: &[&str] = &["actor", "async", "borrowing", "consuming", "macro"];

const DIRECTIVES: &[&str] = &[
    "available",
    "else",
    "elseif",
    "endif",
    "error",
    "file",
    "function",
    "if",
    "keyPath",
    "line",
    "selector",
    "sourceLocation",
    "warning",
];

pub fn is_keyword(ident: &str) -> bool {
    KEYWORDS.binary_search(&ident).is_ok()
}

fn is_contextual(ident: &str) -> bool {
    CONTEXTUAL.binary_search(&ident).is_ok()
}

fn is_directive_word(ident: &str) -> bool {
    DIRECTIVES.binary_search(&ident).is_ok()
}

fn classify_identifier(ident: &str, next_is_call: bool, next_ident: Option<&str>) -> TokenRole {
    match ident {
        "if" | "else" | "guard" | "switch" | "case" | "default" | "defer" | "do" | "catch"
        | "return" | "throw" | "try" | "fallthrough" => TokenRole::BranchKeyword,
        "for" | "while" | "repeat" | "break" | "continue" => TokenRole::LoopKeyword,
        "true" | "false" | "nil" | "self" | "Self" | "super" => TokenRole::Constant,
        other if is_keyword(other) => TokenRole::Keyword,
        other if is_contextual(other) && next_ident.is_some() => TokenRole::Keyword,
        "some" | "any"
            if next_ident
                .and_then(|n| n.chars().next())
                .is_some_and(|c| c.is_uppercase()) =>
        {
            TokenRole::Keyword
        }
        _ if next_is_call => TokenRole::Function,
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

fn is_operator_char(b: u8) -> bool {
    matches!(
        b,
        b'/' | b'=' | b'-' | b'+' | b'!' | b'*' | b'%' | b'<' | b'>' | b'&' | b'|' | b'^' | b'~' | b'?'
    )
}

fn push_token(
    tokens: &mut Vec<SwiftToken>,
    start: u32,
    end: usize,
    kind: SwiftKind,
) -> Result<(), LexError> {
    if end as u32 <= start {
        return Err(LexError::Nonprogress { at: start });
    }
    tokens.push(SwiftToken {
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

fn tok(start: u32, end: usize, kind: SwiftKind) -> SwiftToken {
    SwiftToken {
        start,
        end: end as u32,
        kind,
    }
}

fn consume_ident(bytes: &[u8], i: &mut usize) {
    if *i >= bytes.len() {
        return;
    }
    if bytes[*i] == b'`' {
        *i += 1;
        while *i < bytes.len() && bytes[*i] != b'`' && !is_newline(bytes[*i]) {
            *i += 1;
        }
        if *i < bytes.len() && bytes[*i] == b'`' {
            *i += 1;
        }
        return;
    }
    if !is_ident_start(bytes[*i]) {
        return;
    }
    *i += 1;
    while *i < bytes.len() && is_ident_continue(bytes[*i]) {
        *i += 1;
    }
}

fn count_hashes(bytes: &[u8], i: usize) -> usize {
    let mut n = 0usize;
    while i + n < bytes.len() && bytes[i + n] == b'#' && n < HASH_CAP as usize {
        n += 1;
    }
    n
}

fn looks_like_triple(bytes: &[u8], i: usize) -> bool {
    bytes.get(i) == Some(&b'"')
        && bytes.get(i + 1) == Some(&b'"')
        && bytes.get(i + 2) == Some(&b'"')
}

fn string_close_len(bytes: &[u8], i: usize, hashes: u8, multi: bool) -> Option<usize> {
    if multi {
        if looks_like_triple(bytes, i) {
            let h = hashes as usize;
            if h == 0 {
                return Some(3);
            }
            if i + 3 + h <= bytes.len() && bytes[i + 3..i + 3 + h].iter().all(|b| *b == b'#') {
                return Some(3 + h);
            }
        }
        None
    } else if bytes.get(i) == Some(&b'"') {
        let h = hashes as usize;
        if h == 0 {
            Some(1)
        } else if i + 1 + h <= bytes.len() && bytes[i + 1..i + 1 + h].iter().all(|b| *b == b'#') {
            Some(1 + h)
        } else {
            None
        }
    } else {
        None
    }
}

/// Interpolation opener length at `i`: `\( ` or `\` + hashes `#` + `(`.
fn interp_open_len(bytes: &[u8], i: usize, hashes: u8) -> Option<usize> {
    if bytes.get(i) != Some(&b'\\') {
        return None;
    }
    let h = hashes as usize;
    if h == 0 {
        if bytes.get(i + 1) == Some(&b'(') {
            return Some(2);
        }
        return None;
    }
    if i + 1 + h < bytes.len()
        && bytes[i + 1..i + 1 + h].iter().all(|b| *b == b'#')
        && bytes.get(i + 1 + h) == Some(&b'(')
    {
        return Some(2 + h);
    }
    None
}

fn consume_string_escape(bytes: &[u8], i: &mut usize, hashes: u8) {
    if *i >= bytes.len() {
        return;
    }
    if is_newline(bytes[*i]) {
        return;
    }
    if hashes == 0 {
        let b = bytes[*i];
        *i += 1;
        if b == b'u' && bytes.get(*i) == Some(&b'{') {
            *i += 1;
            while *i < bytes.len() && bytes[*i] != b'}' && !is_newline(bytes[*i]) {
                *i += 1;
            }
            if *i < bytes.len() && bytes[*i] == b'}' {
                *i += 1;
            }
        }
        return;
    }
    // Raw: `\` + hashes `#` + next is an escape; consume the hashes and the
    // escaped byte when present. Interpolation is handled by the caller.
    let h = hashes as usize;
    if i.saturating_add(h) <= bytes.len() && bytes[*i..*i + h].iter().all(|b| *b == b'#') {
        *i += h;
        if *i < bytes.len() && !is_newline(bytes[*i]) {
            *i += 1;
        }
    }
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
                    if next.is_ascii_hexdigit() {
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
                while i < bytes.len()
                    && ((bytes[i] >= b'0' && bytes[i] <= b'7') || bytes[i] == b'_')
                {
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
        if next.is_ascii_digit() {
            i += 1;
            while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'_') {
                i += 1;
            }
        } else if next == 0 {
            i += 1;
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

fn operator_run_len(bytes: &[u8], i: usize) -> usize {
    if bytes.get(i) != Some(&b'?') && !is_operator_char(bytes.get(i).copied().unwrap_or(0)) {
        return 0;
    }
    // `?.` optional chaining is `?` then `.` (`.` is not an operator-run char).
    if bytes[i] == b'?' && bytes.get(i + 1) == Some(&b'.') {
        return 1;
    }
    let mut n = 1usize;
    while i + n < bytes.len() && is_operator_char(bytes[i + n]) {
        if bytes[i + n] == b'/'
            && matches!(bytes.get(i + n + 1), Some(&b'/' | &b'*'))
        {
            break;
        }
        if bytes[i + n] == b'?' && bytes.get(i + n + 1) == Some(&b'.') {
            break;
        }
        n += 1;
    }
    n
}

fn dot_run_len(bytes: &[u8], i: usize) -> usize {
    if bytes.get(i) != Some(&b'.') {
        return 0;
    }
    if bytes.get(i + 1) == Some(&b'.') && bytes.get(i + 2) == Some(&b'.') {
        return 3;
    }
    if bytes.get(i + 1) == Some(&b'.') && bytes.get(i + 2) == Some(&b'<') {
        return 3;
    }
    1
}

#[derive(Clone, Copy)]
struct StringFrame {
    hashes: u8,
    multi: bool,
    nest: u8,
    host_hashes: u8,
    host_multi: bool,
    root_hashes: u8,
    root_multi: bool,
}

fn string_frame(mode: SwiftMode) -> Option<StringFrame> {
    match mode {
        SwiftMode::MultiString {
            hashes,
            multi,
            nest,
            host_hashes,
            host_multi,
            root_hashes,
            root_multi,
        } => Some(StringFrame {
            hashes,
            multi,
            nest,
            host_hashes,
            host_multi,
            root_hashes,
            root_multi,
        }),
        _ => None,
    }
}

#[derive(Clone, Copy)]
struct InterpFrame {
    hashes: u8,
    multi: bool,
    nest: u8,
    parens: u8,
    host_hashes: u8,
    host_multi: bool,
    root_hashes: u8,
    root_multi: bool,
}

fn interp_frame(mode: SwiftMode) -> Option<InterpFrame> {
    match mode {
        SwiftMode::Interp {
            hashes,
            multi,
            nest,
            parens,
            host_hashes,
            host_multi,
            root_hashes,
            root_multi,
        } => Some(InterpFrame {
            hashes,
            multi,
            nest,
            parens,
            host_hashes,
            host_multi,
            root_hashes,
            root_multi,
        }),
        _ => None,
    }
}

fn close_string(state: &mut SwiftState, frame: StringFrame) {
    if frame.nest == 0 {
        state.mode = SwiftMode::Normal;
    } else {
        state.mode = SwiftMode::Interp {
            hashes: frame.host_hashes,
            multi: frame.host_multi,
            nest: frame.nest - 1,
            parens: 0,
            host_hashes: frame.root_hashes,
            host_multi: frame.root_multi,
            root_hashes: frame.root_hashes,
            root_multi: frame.root_multi,
        };
    }
}

fn open_interp(state: &mut SwiftState, frame: StringFrame) {
    state.mode = SwiftMode::Interp {
        hashes: frame.hashes,
        multi: frame.multi,
        nest: frame.nest,
        parens: 0,
        host_hashes: frame.host_hashes,
        host_multi: frame.host_multi,
        root_hashes: frame.root_hashes,
        root_multi: frame.root_multi,
    };
}

fn scan_string_token(
    bytes: &[u8],
    i: &mut usize,
    state: &mut SwiftState,
    token_start: u32,
) -> Result<SwiftToken, LexError> {
    let start_i = *i;
    let start = token_start;
    let Some(frame) = string_frame(state.mode) else {
        return Err(LexError::Nonprogress { at: start });
    };
    let hashes = frame.hashes;
    let multi = frame.multi;
    let nest = frame.nest;
    let allow_interp = nest < 2;

    if allow_interp {
        if let Some(n) = interp_open_len(bytes, *i, hashes) {
            if (*i as u32) == start {
                *i += n;
                open_interp(state, frame);
                return Ok(tok(start, *i, SwiftKind::Punctuator));
            }
        }
    }

    if let Some(n) = string_close_len(bytes, *i, hashes, multi) {
        *i += n;
        close_string(state, frame);
        return Ok(tok(start, *i, SwiftKind::String));
    }

    while *i < bytes.len() {
        if !multi && is_newline(bytes[*i]) {
            if *i == start_i {
                return Err(LexError::Nonprogress { at: start });
            }
            close_string(state, frame);
            return Ok(tok(start, *i, SwiftKind::String));
        }
        if allow_interp {
            if interp_open_len(bytes, *i, hashes).is_some() {
                if *i == start_i {
                    break;
                }
                return Ok(tok(start, *i, SwiftKind::String));
            }
        }
        if let Some(n) = string_close_len(bytes, *i, hashes, multi) {
            *i += n;
            close_string(state, frame);
            return Ok(tok(start, *i, SwiftKind::String));
        }
        if bytes[*i] == b'\\' {
            *i += 1;
            if hashes == 0 {
                if *i < bytes.len() && bytes[*i] == b'(' {
                    // Interpolation opener starts at the `\`; the loop will
                    // re-check `interp_open_len` after we rewind.
                    *i -= 1;
                    if *i == start_i {
                        break;
                    }
                    return Ok(tok(start, *i, SwiftKind::String));
                }
                consume_string_escape(bytes, i, 0);
            } else if interp_open_len(bytes, *i - 1, hashes).is_some() {
                *i -= 1;
                if *i == start_i {
                    break;
                }
                return Ok(tok(start, *i, SwiftKind::String));
            } else {
                consume_string_escape(bytes, i, hashes);
            }
            continue;
        }
        *i += 1;
    }
    if *i as u32 <= start {
        if *i < bytes.len() {
            *i += 1;
        } else {
            return Err(LexError::Nonprogress { at: start });
        }
    }
    Ok(tok(start, *i, SwiftKind::String))
}

fn open_string_from_quote(bytes: &[u8], i: &mut usize, state: &mut SwiftState, hashes: u8) {
    let multi = looks_like_triple(bytes, *i);
    if multi {
        *i += 3;
    } else {
        *i += 1;
    }
    match state.mode {
        SwiftMode::Interp {
            hashes: host_hashes,
            multi: host_multi,
            nest,
            root_hashes,
            root_multi,
            ..
        } => {
            state.mode = SwiftMode::MultiString {
                hashes,
                multi,
                nest: nest.saturating_add(1).min(2),
                host_hashes,
                host_multi,
                root_hashes,
                root_multi,
            };
        }
        _ => {
            state.mode = SwiftMode::MultiString {
                hashes,
                multi,
                nest: 0,
                host_hashes: 0,
                host_multi: false,
                root_hashes: hashes,
                root_multi: multi,
            };
        }
    }
}

/// Shared automaton step. Emits one token covering `[start, *i)`.
fn lex_one(
    bytes: &[u8],
    i: &mut usize,
    state: &mut SwiftState,
    line_start: &mut bool,
) -> Result<SwiftToken, LexError> {
    let start_i = *i;
    let start = *i as u32;
    if *i >= bytes.len() {
        return Err(LexError::Nonprogress { at: start });
    }

    if let SwiftMode::BlockComment { depth } = state.mode {
        let mut depth = depth;
        while *i < bytes.len() {
            if bytes[*i] == b'/' && bytes.get(*i + 1) == Some(&b'*') {
                depth = depth.saturating_add(1);
                *i += 2;
                continue;
            }
            if bytes[*i] == b'*' && bytes.get(*i + 1) == Some(&b'/') {
                *i += 2;
                if depth <= 1 {
                    state.mode = SwiftMode::Normal;
                    break;
                }
                depth -= 1;
                continue;
            }
            if is_newline(bytes[*i]) {
                *line_start = true;
            } else if !is_space(bytes[*i]) {
                *line_start = false;
            }
            *i += 1;
        }
        if matches!(state.mode, SwiftMode::BlockComment { .. }) {
            state.mode = SwiftMode::BlockComment { depth };
        }
        if (*i as u32) <= start {
            return Err(LexError::Nonprogress { at: start });
        }
        return Ok(tok(start, *i, SwiftKind::Comment));
    }

    if string_frame(state.mode).is_some() {
        let t = scan_string_token(bytes, i, state, start)?;
        *line_start = false;
        return Ok(t);
    }

    if let Some(ip) = interp_frame(state.mode) {
        let b = bytes[*i];
        if b == b')' {
            *i += 1;
            *line_start = false;
            if ip.parens == 0 {
                state.mode = SwiftMode::MultiString {
                    hashes: ip.hashes,
                    multi: ip.multi,
                    nest: ip.nest,
                    host_hashes: ip.host_hashes,
                    host_multi: ip.host_multi,
                    root_hashes: ip.root_hashes,
                    root_multi: ip.root_multi,
                };
                return Ok(tok(start, *i, SwiftKind::Punctuator));
            }
            state.mode = SwiftMode::Interp {
                hashes: ip.hashes,
                multi: ip.multi,
                nest: ip.nest,
                parens: ip.parens - 1,
                host_hashes: ip.host_hashes,
                host_multi: ip.host_multi,
                root_hashes: ip.root_hashes,
                root_multi: ip.root_multi,
            };
            return Ok(tok(start, *i, SwiftKind::Delimiter));
        }
        if b == b'(' {
            *i += 1;
            *line_start = false;
            state.mode = SwiftMode::Interp {
                hashes: ip.hashes,
                multi: ip.multi,
                nest: ip.nest,
                parens: ip.parens.saturating_add(1),
                host_hashes: ip.host_hashes,
                host_multi: ip.host_multi,
                root_hashes: ip.root_hashes,
                root_multi: ip.root_multi,
            };
            return Ok(tok(start, *i, SwiftKind::Delimiter));
        }
        if b == b'"' || (b == b'#' && count_hashes(bytes, *i) > 0) {
            let (nh, rest) = if b == b'#' {
                let h = count_hashes(bytes, *i);
                if h >= 1 && h <= HASH_CAP as usize && bytes.get(*i + h) == Some(&b'"') {
                    *i += h;
                    (h as u8, true)
                } else {
                    (0, false)
                }
            } else {
                (0, true)
            };
            if rest {
                open_string_from_quote(bytes, i, state, nh);
                *line_start = false;
                return scan_string_token(bytes, i, state, start);
            }
        }
        // Fall through to Normal-like lexing of the interpolation body.
    }

    let b = bytes[*i];
    if b == b'\r' {
        *i += 1;
        if bytes.get(*i) == Some(&b'\n') {
            *i += 1;
        }
        *line_start = true;
        return Ok(tok(start, *i, SwiftKind::Newline));
    }
    if b == b'\n' {
        *i += 1;
        *line_start = true;
        return Ok(tok(start, *i, SwiftKind::Newline));
    }
    if is_space(b) {
        *i += 1;
        while *i < bytes.len() && is_space(bytes[*i]) {
            *i += 1;
        }
        return Ok(tok(start, *i, SwiftKind::Whitespace));
    }
    *line_start = false;
    if b == b'/' && bytes.get(*i + 1) == Some(&b'/') {
        *i += 2;
        while *i < bytes.len() && !is_newline(bytes[*i]) {
            *i += 1;
        }
        return Ok(tok(start, *i, SwiftKind::Comment));
    }
    if b == b'/' && bytes.get(*i + 1) == Some(&b'*') {
        *i += 2;
        let mut depth = 1u8;
        state.mode = SwiftMode::BlockComment { depth };
        while *i < bytes.len() {
            if bytes[*i] == b'/' && bytes.get(*i + 1) == Some(&b'*') {
                depth = depth.saturating_add(1);
                *i += 2;
                state.mode = SwiftMode::BlockComment { depth };
                continue;
            }
            if bytes[*i] == b'*' && bytes.get(*i + 1) == Some(&b'/') {
                *i += 2;
                if depth <= 1 {
                    state.mode = SwiftMode::Normal;
                    break;
                }
                depth -= 1;
                state.mode = SwiftMode::BlockComment { depth };
                continue;
            }
            *i += 1;
        }
        if matches!(state.mode, SwiftMode::BlockComment { .. }) {
            state.mode = SwiftMode::BlockComment { depth };
        }
        return Ok(tok(start, *i, SwiftKind::Comment));
    }
    if b == b'"' {
        open_string_from_quote(bytes, i, state, 0);
        return scan_string_token(bytes, i, state, start);
    }
    if b == b'#' {
        let h = count_hashes(bytes, *i);
        if h >= 1 && h <= HASH_CAP as usize && bytes.get(*i + h) == Some(&b'"') {
            *i += h;
            open_string_from_quote(bytes, i, state, h as u8);
            return scan_string_token(bytes, i, state, start);
        }
        let mut j = *i + 1;
        if j < bytes.len() && is_ident_start(bytes[j]) {
            let word_start = j;
            j += 1;
            while j < bytes.len() && is_ident_continue(bytes[j]) {
                j += 1;
            }
            let word = std::str::from_utf8(&bytes[word_start..j]).unwrap_or("");
            if is_directive_word(word) {
                *i = j;
                return Ok(tok(start, *i, SwiftKind::Directive));
            }
        }
        *i += 1;
        return Ok(tok(start, *i, SwiftKind::Punctuator));
    }
    if b == b'@' {
        let next = bytes.get(*i + 1).copied().unwrap_or(0);
        if is_ident_start(next) {
            *i += 1;
            consume_ident(bytes, i);
            return Ok(tok(start, *i, SwiftKind::Attribute));
        }
        *i += 1;
        return Ok(tok(start, *i, SwiftKind::Punctuator));
    }
    if b == b'`' {
        consume_ident(bytes, i);
        return Ok(tok(start, *i, SwiftKind::Identifier));
    }
    if b == b'$' {
        let next = bytes.get(*i + 1).copied().unwrap_or(0);
        if is_ident_continue(next) {
            *i += 1;
            while *i < bytes.len() && is_ident_continue(bytes[*i]) {
                *i += 1;
            }
            return Ok(tok(start, *i, SwiftKind::Identifier));
        }
        *i += 1;
        return Ok(tok(start, *i, SwiftKind::Unknown));
    }
    if is_ident_start(b) {
        *i += 1;
        while *i < bytes.len() && is_ident_continue(bytes[*i]) {
            *i += 1;
        }
        let ident = std::str::from_utf8(&bytes[start as usize..*i]).unwrap_or("");
        let kind = if is_keyword(ident) {
            SwiftKind::Keyword
        } else if is_contextual(ident) {
            let mut k = *i;
            while k < bytes.len() && is_space(bytes[k]) {
                k += 1;
            }
            if k < bytes.len() && !is_newline(bytes[k]) && (is_ident_start(bytes[k]) || bytes[k] == b'`')
            {
                SwiftKind::Keyword
            } else {
                SwiftKind::Identifier
            }
        } else {
            SwiftKind::Identifier
        };
        return Ok(tok(start, *i, kind));
    }
    if b.is_ascii_digit() {
        *i = scan_number(bytes, *i);
        return Ok(tok(start, *i, SwiftKind::Number));
    }
    if b == b'.' {
        let n = dot_run_len(bytes, *i);
        *i += n;
        return Ok(tok(start, *i, SwiftKind::Punctuator));
    }
    if matches!(b, b'(' | b')' | b'{' | b'}' | b'[' | b']') {
        *i += 1;
        return Ok(tok(start, *i, SwiftKind::Delimiter));
    }
    if matches!(b, b':' | b';' | b',' | b'\\') {
        *i += 1;
        return Ok(tok(start, *i, SwiftKind::Punctuator));
    }
    if is_operator_char(b) {
        let n = operator_run_len(bytes, *i);
        *i += n.max(1);
        return Ok(tok(start, *i, SwiftKind::Punctuator));
    }
    *i += 1;
    if *i == start_i {
        return Err(LexError::Nonprogress { at: start });
    }
    Ok(tok(start, *i, SwiftKind::Unknown))
}

fn reset_line_bounded(state: &mut SwiftState) {
    match state.mode {
        SwiftMode::MultiString { multi: false, .. } => {
            if let Some(frame) = string_frame(state.mode) {
                close_string(state, frame);
            }
            if let SwiftMode::Interp { multi: false, .. } = state.mode {
                state.mode = SwiftMode::Normal;
            }
        }
        SwiftMode::Interp { multi: false, .. } => {
            state.mode = SwiftMode::Normal;
        }
        _ => {}
    }
}

/// Tokenize an entire document. Spans are byte offsets into `bytes`.
pub fn lex_document_cancellable(
    bytes: &[u8],
    cancel: &dyn Fn() -> bool,
) -> Result<Vec<SwiftToken>, LexError> {
    let mut tokens = Vec::new();
    let mut state = SwiftState::default();
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
                    reset_line_bounded(&mut state);
                    push_token(&mut tokens, start, i, SwiftKind::Newline)?;
                    continue;
                }
                if i == start_i && i < bytes.len() {
                    i += 1;
                    push_token(&mut tokens, at, i, SwiftKind::Unknown)?;
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

fn next_is_call_bytes(bytes: &[u8], end: usize) -> bool {
    let mut k = end;
    while k < bytes.len() && is_space(bytes[k]) {
        k += 1;
    }
    matches!(bytes.get(k), Some(&b'(' | &b'{'))
}

fn next_ident_bytes(bytes: &[u8], end: usize) -> Option<&str> {
    let mut k = end;
    while k < bytes.len() && is_space(bytes[k]) {
        k += 1;
    }
    if k >= bytes.len() || is_newline(bytes[k]) {
        return None;
    }
    if bytes[k] == b'`' {
        let mut j = k + 1;
        while j < bytes.len() && bytes[j] != b'`' && !is_newline(bytes[j]) {
            j += 1;
        }
        if j < bytes.len() && bytes[j] == b'`' {
            j += 1;
        }
        return Some(std::str::from_utf8(&bytes[k..j]).unwrap_or(""));
    }
    if !is_ident_start(bytes[k]) {
        return None;
    }
    let mut j = k + 1;
    while j < bytes.len() && is_ident_continue(bytes[j]) {
        j += 1;
    }
    Some(std::str::from_utf8(&bytes[k..j]).unwrap_or(""))
}

/// Tokenize one display line (without the newline) given incoming continuation.
/// Each pair is `(end_index, role)` covering `[prev_end, end)` of `line`.
pub fn lex_line(line: &str, incoming: SwiftState) -> (SwiftState, Vec<(usize, TokenRole)>) {
    let bytes = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    let mut state = incoming;
    let mut line_start = matches!(state.mode, SwiftMode::Normal);
    while i < bytes.len() {
        let start = i;
        match lex_one(bytes, &mut i, &mut state, &mut line_start) {
            Ok(t) => {
                let end = (t.end as usize).min(bytes.len()).max(start);
                if end > start {
                    let role = if matches!(t.kind, SwiftKind::Keyword | SwiftKind::Identifier) {
                        let ident =
                            std::str::from_utf8(&bytes[t.start as usize..end]).unwrap_or("");
                        classify_identifier(
                            ident,
                            next_is_call_bytes(bytes, end),
                            next_ident_bytes(bytes, end),
                        )
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
    reset_line_bounded(&mut state);
    if i < bytes.len() {
        let role = match state.mode {
            SwiftMode::BlockComment { .. } => TokenRole::Comment,
            SwiftMode::MultiString { .. } => TokenRole::String,
            SwiftMode::Normal | SwiftMode::Interp { .. } => TokenRole::Unknown,
        };
        push_role(bytes.len(), role, &mut out);
    }
    (state, out)
}

fn same_line(tokens: &[SwiftToken], a: usize, b: usize) -> bool {
    tokens[a + 1..b]
        .iter()
        .all(|t| t.kind != SwiftKind::Newline)
}

/// Convert document tokens into public spans, classifying keywords and
/// function names with one-token lookahead.
pub fn document_spans(bytes: &[u8], tokens: &[SwiftToken]) -> Vec<TokenSpan> {
    let mut out = Vec::with_capacity(tokens.len());
    for (i, t) in tokens.iter().enumerate() {
        let mut role = t.kind.role();
        if t.kind == SwiftKind::Keyword || t.kind == SwiftKind::Identifier {
            let mut j = i + 1;
            while j < tokens.len()
                && matches!(tokens[j].kind, SwiftKind::Whitespace | SwiftKind::Comment)
            {
                j += 1;
            }
            let next_is_call = j < tokens.len()
                && tokens[j].kind == SwiftKind::Delimiter
                && matches!(bytes.get(tokens[j].start as usize), Some(&b'(' | &b'{'))
                && same_line(tokens, i, j);
            let next_ident = if j < tokens.len()
                && tokens[j].kind != SwiftKind::Newline
                && matches!(
                    tokens[j].kind,
                    SwiftKind::Identifier | SwiftKind::Keyword
                )
                && same_line(tokens, i, j)
            {
                Some(
                    std::str::from_utf8(&bytes[tokens[j].start as usize..tokens[j].end as usize])
                        .unwrap_or(""),
                )
            } else {
                None
            };
            let ident = std::str::from_utf8(&bytes[t.start as usize..t.end as usize]).unwrap_or("");
            role = classify_identifier(ident, next_is_call, next_ident);
        }
        out.push(TokenSpan::new(t.start, t.end, role));
    }
    out
}
