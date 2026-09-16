//! Source-only TOML lexer shared by the editor and the text frontend.
//!
//! Tokens and line continuation only. Cargo manifest semantics stay with the
//! Cargo project provider. Malformed input is classified, never rejected.

use crate::cpp_lex::LexError;
use crate::token::{TokenRole, TokenSpan};

/// Version of this lexer; part of parse and search cache identity.
pub const TOML_LEXER_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
enum TomlMode {
    #[default]
    Normal,
    MultiBasic,
    MultiLiteral,
}

/// Provider-owned line continuation. Multi-line strings and array depth persist.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct TomlState {
    mode: TomlMode,
    array_depth: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TomlKind {
    Whitespace,
    Comment,
    Identifier,
    Typename,
    Number,
    String,
    Constant,
    Punctuator,
    Delimiter,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TomlToken {
    pub start: u32,
    pub end: u32,
    pub kind: TomlKind,
}

impl TomlKind {
    pub fn role(self) -> TokenRole {
        match self {
            TomlKind::Whitespace => TokenRole::Whitespace,
            TomlKind::Comment => TokenRole::Comment,
            TomlKind::Identifier => TokenRole::Identifier,
            TomlKind::Typename => TokenRole::Typename,
            TomlKind::Number => TokenRole::Number,
            TomlKind::String => TokenRole::String,
            TomlKind::Constant => TokenRole::Constant,
            TomlKind::Punctuator => TokenRole::Punctuator,
            TomlKind::Delimiter => TokenRole::Delimiter,
            TomlKind::Unknown => TokenRole::Unknown,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct LineCtx {
    bol: bool,
    expect_key: bool,
    inline_depth: u8,
    in_header: bool,
}

impl LineCtx {
    fn from_state(state: &TomlState) -> Self {
        LineCtx {
            bol: true,
            expect_key: state.array_depth == 0 && matches!(state.mode, TomlMode::Normal),
            inline_depth: 0,
            in_header: false,
        }
    }

    fn on_newline(state: &TomlState) -> Self {
        Self::from_state(state)
    }
}

fn is_space(b: u8) -> bool {
    b == b' ' || b == b'\t' || b == 0x0b || b == 0x0c
}

fn is_newline(b: u8) -> bool {
    b == b'\n' || b == b'\r'
}

fn is_bare_key(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

fn tok(start: u32, end: usize, kind: TomlKind) -> TomlToken {
    TomlToken {
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

fn scan_line_comment(bytes: &[u8], i: &mut usize) {
    while *i < bytes.len() && !is_newline(bytes[*i]) {
        *i += 1;
    }
}

fn scan_basic_escape(bytes: &[u8], i: &mut usize) {
    if bytes.get(*i) != Some(&b'\\') {
        return;
    }
    *i += 1;
    let Some(&c) = bytes.get(*i) else {
        return;
    };
    if is_newline(c) {
        return;
    }
    match c {
        b'u' => {
            *i += 1;
            let mut n = 0;
            while n < 4 && bytes.get(*i).copied().map(|b| b.is_ascii_hexdigit()).unwrap_or(false) {
                *i += 1;
                n += 1;
            }
        }
        b'U' => {
            *i += 1;
            let mut n = 0;
            while n < 8 && bytes.get(*i).copied().map(|b| b.is_ascii_hexdigit()).unwrap_or(false) {
                *i += 1;
                n += 1;
            }
        }
        _ => {
            *i += 1;
        }
    }
}

/// Line-bounded basic string. Returns whether the closing quote was seen.
fn scan_basic_line(bytes: &[u8], i: &mut usize) -> bool {
    while *i < bytes.len() {
        let c = bytes[*i];
        if is_newline(c) {
            return false;
        }
        if c == b'\\' {
            scan_basic_escape(bytes, i);
            continue;
        }
        if c == b'"' {
            *i += 1;
            return true;
        }
        *i += 1;
    }
    false
}

fn scan_literal_line(bytes: &[u8], i: &mut usize) -> bool {
    while *i < bytes.len() {
        let c = bytes[*i];
        if is_newline(c) {
            return false;
        }
        if c == b'\'' {
            *i += 1;
            return true;
        }
        *i += 1;
    }
    false
}

fn scan_multi_basic(bytes: &[u8], i: &mut usize) -> bool {
    while *i < bytes.len() {
        if bytes[*i] == b'\\' {
            scan_basic_escape(bytes, i);
            if *i < bytes.len() && is_newline(bytes[*i]) {
                consume_newline(bytes, i);
            }
            continue;
        }
        if bytes[*i] == b'"' && bytes.get(*i + 1) == Some(&b'"') && bytes.get(*i + 2) == Some(&b'"') {
            *i += 3;
            return true;
        }
        *i += 1;
    }
    false
}

fn scan_multi_literal(bytes: &[u8], i: &mut usize) -> bool {
    while *i < bytes.len() {
        if bytes[*i] == b'\''
            && bytes.get(*i + 1) == Some(&b'\'')
            && bytes.get(*i + 2) == Some(&b'\'')
        {
            *i += 3;
            return true;
        }
        *i += 1;
    }
    false
}

fn starts_special_float(bytes: &[u8], i: usize) -> bool {
    let rest = &bytes[i..];
    rest.len() >= 3 && (rest[..3].eq_ignore_ascii_case(b"inf") || rest[..3].eq_ignore_ascii_case(b"nan"))
}

fn scan_digits_us(bytes: &[u8], i: &mut usize) -> bool {
    let start = *i;
    while *i < bytes.len() {
        let c = bytes[*i];
        if c.is_ascii_digit() {
            *i += 1;
        } else if c == b'_' {
            *i += 1;
        } else {
            break;
        }
    }
    *i > start
}

fn scan_hex_us(bytes: &[u8], i: &mut usize) {
    while *i < bytes.len() {
        let c = bytes[*i];
        if c.is_ascii_hexdigit() || c == b'_' {
            *i += 1;
        } else {
            break;
        }
    }
}

fn scan_time_body(bytes: &[u8], i: &mut usize) {
    // HH:MM:SS[.frac][Z|+HH:MM|-HH:MM]
    for _ in 0..2 {
        if bytes.get(*i).copied().map(|b| b.is_ascii_digit()).unwrap_or(false) {
            *i += 1;
        }
    }
    if bytes.get(*i) == Some(&b':') {
        *i += 1;
    }
    for _ in 0..2 {
        if bytes.get(*i).copied().map(|b| b.is_ascii_digit()).unwrap_or(false) {
            *i += 1;
        }
    }
    if bytes.get(*i) == Some(&b':') {
        *i += 1;
    }
    for _ in 0..2 {
        if bytes.get(*i).copied().map(|b| b.is_ascii_digit()).unwrap_or(false) {
            *i += 1;
        }
    }
    if bytes.get(*i) == Some(&b'.') {
        *i += 1;
        while bytes.get(*i).copied().map(|b| b.is_ascii_digit()).unwrap_or(false) {
            *i += 1;
        }
    }
    match bytes.get(*i).copied() {
        Some(b'Z' | b'z') => *i += 1,
        Some(b'+' | b'-') => {
            *i += 1;
            for _ in 0..2 {
                if bytes.get(*i).copied().map(|b| b.is_ascii_digit()).unwrap_or(false) {
                    *i += 1;
                }
            }
            if bytes.get(*i) == Some(&b':') {
                *i += 1;
                for _ in 0..2 {
                    if bytes.get(*i).copied().map(|b| b.is_ascii_digit()).unwrap_or(false) {
                        *i += 1;
                    }
                }
            }
        }
        _ => {}
    }
}

fn scan_number(bytes: &[u8], i: &mut usize) {
    let start = *i;
    if matches!(bytes.get(*i), Some(&b'+' | &b'-')) {
        *i += 1;
    }
    if starts_special_float(bytes, *i) {
        *i += 3;
        return;
    }
    if bytes.get(*i) == Some(&b'0') {
        match bytes.get(*i + 1).copied() {
            Some(b'x' | b'X') => {
                *i += 2;
                scan_hex_us(bytes, i);
                return;
            }
            Some(b'o' | b'O') => {
                *i += 2;
                while *i < bytes.len() && (bytes[*i].is_ascii_digit() || bytes[*i] == b'_') {
                    *i += 1;
                }
                return;
            }
            Some(b'b' | b'B') => {
                *i += 2;
                while *i < bytes.len() && (bytes[*i] == b'0' || bytes[*i] == b'1' || bytes[*i] == b'_')
                {
                    *i += 1;
                }
                return;
            }
            _ => {}
        }
    }
    let digits_before = *i;
    scan_digits_us(bytes, i);
    // Date YYYY-MM-DD[Tt ]time
    if *i - digits_before == 4 && bytes.get(*i) == Some(&b'-') {
        let mut j = *i + 1;
        let mut n = 0;
        while n < 2 && bytes.get(j).copied().map(|b| b.is_ascii_digit()).unwrap_or(false) {
            j += 1;
            n += 1;
        }
        if n == 2 && bytes.get(j) == Some(&b'-') {
            j += 1;
            n = 0;
            while n < 2 && bytes.get(j).copied().map(|b| b.is_ascii_digit()).unwrap_or(false) {
                j += 1;
                n += 1;
            }
            if n == 2 {
                *i = j;
                if matches!(bytes.get(*i), Some(&b'T' | &b't'))
                    || (bytes.get(*i) == Some(&b' ')
                        && bytes.get(*i + 1).copied().map(|b| b.is_ascii_digit()).unwrap_or(false))
                {
                    *i += 1;
                    scan_time_body(bytes, i);
                }
                return;
            }
        }
    }
    // Time HH:MM:SS
    if *i - digits_before == 2 && bytes.get(*i) == Some(&b':') {
        *i = digits_before;
        scan_time_body(bytes, i);
        if *i > start {
            return;
        }
        *i = digits_before;
        scan_digits_us(bytes, i);
    }
    if bytes.get(*i) == Some(&b'.')
        && bytes.get(*i + 1).copied().map(|b| b.is_ascii_digit()).unwrap_or(false)
    {
        *i += 1;
        scan_digits_us(bytes, i);
    }
    if matches!(bytes.get(*i), Some(&b'e' | &b'E')) {
        let mut j = *i + 1;
        if matches!(bytes.get(j), Some(&b'+' | &b'-')) {
            j += 1;
        }
        if bytes.get(j).copied().map(|b| b.is_ascii_digit()).unwrap_or(false) {
            *i = j;
            scan_digits_us(bytes, i);
        }
    }
    if *i == start {
        *i = start + 1;
    }
}

fn quoted_key_kind(in_header: bool) -> TomlKind {
    if in_header {
        TomlKind::Typename
    } else {
        TomlKind::Identifier
    }
}

fn lex_one(
    bytes: &[u8],
    i: &mut usize,
    state: &mut TomlState,
    line: &mut LineCtx,
) -> Result<TomlToken, LexError> {
    let start = *i as u32;
    if *i >= bytes.len() {
        return Err(LexError::Nonprogress { at: start });
    }

    match state.mode {
        TomlMode::MultiBasic => {
            let closed = scan_multi_basic(bytes, i);
            if closed {
                state.mode = TomlMode::Normal;
            }
            if (*i as u32) <= start {
                return Err(LexError::Nonprogress { at: start });
            }
            line.bol = false;
            line.expect_key = false;
            return Ok(tok(start, *i, TomlKind::String));
        }
        TomlMode::MultiLiteral => {
            let closed = scan_multi_literal(bytes, i);
            if closed {
                state.mode = TomlMode::Normal;
            }
            if (*i as u32) <= start {
                return Err(LexError::Nonprogress { at: start });
            }
            line.bol = false;
            line.expect_key = false;
            return Ok(tok(start, *i, TomlKind::String));
        }
        TomlMode::Normal => {}
    }

    let b = bytes[*i];
    if is_newline(b) {
        consume_newline(bytes, i);
        *line = LineCtx::on_newline(state);
        return Ok(tok(start, *i, TomlKind::Whitespace));
    }
    if is_space(b) {
        *i += 1;
        while *i < bytes.len() && is_space(bytes[*i]) {
            *i += 1;
        }
        return Ok(tok(start, *i, TomlKind::Whitespace));
    }

    if b == b'#' {
        *i += 1;
        scan_line_comment(bytes, i);
        line.bol = false;
        return Ok(tok(start, *i, TomlKind::Comment));
    }

    let key_pos = line.expect_key || line.in_header;
    let first_content = line.bol;
    line.bol = false;

    if b == b'[' {
        let header = first_content && state.array_depth == 0 && key_pos;
        if header {
            line.in_header = true;
            line.expect_key = false;
            if bytes.get(*i + 1) == Some(&b'[') {
                *i += 2;
            } else {
                *i += 1;
            }
            return Ok(tok(start, *i, TomlKind::Delimiter));
        }
        *i += 1;
        state.array_depth = state.array_depth.saturating_add(1);
        line.expect_key = false;
        line.in_header = false;
        return Ok(tok(start, *i, TomlKind::Delimiter));
    }
    if b == b']' {
        if line.in_header {
            if bytes.get(*i + 1) == Some(&b']') {
                *i += 2;
            } else {
                *i += 1;
            }
            line.in_header = false;
            line.expect_key = false;
            return Ok(tok(start, *i, TomlKind::Delimiter));
        }
        *i += 1;
        if state.array_depth > 0 {
            state.array_depth -= 1;
        }
        line.expect_key = false;
        return Ok(tok(start, *i, TomlKind::Delimiter));
    }
    if b == b'{' {
        *i += 1;
        line.inline_depth = line.inline_depth.saturating_add(1);
        line.expect_key = true;
        line.in_header = false;
        return Ok(tok(start, *i, TomlKind::Delimiter));
    }
    if b == b'}' {
        *i += 1;
        if line.inline_depth > 0 {
            line.inline_depth -= 1;
        }
        line.expect_key = false;
        return Ok(tok(start, *i, TomlKind::Delimiter));
    }
    if b == b',' {
        *i += 1;
        line.expect_key = line.inline_depth > 0 && state.array_depth == 0;
        return Ok(tok(start, *i, TomlKind::Delimiter));
    }
    if b == b'=' {
        *i += 1;
        line.expect_key = false;
        line.in_header = false;
        return Ok(tok(start, *i, TomlKind::Punctuator));
    }
    if b == b'.' && key_pos {
        *i += 1;
        return Ok(tok(start, *i, TomlKind::Punctuator));
    }

    if b == b'"' {
        if bytes.get(*i + 1) == Some(&b'"') && bytes.get(*i + 2) == Some(&b'"') {
            *i += 3;
            let closed = scan_multi_basic(bytes, i);
            if !closed {
                state.mode = TomlMode::MultiBasic;
            }
            line.expect_key = false;
            let kind = if key_pos {
                quoted_key_kind(line.in_header)
            } else {
                TomlKind::String
            };
            return Ok(tok(start, *i, kind));
        }
        *i += 1;
        let _ = scan_basic_line(bytes, i);
        line.expect_key = false;
        let kind = if key_pos {
            quoted_key_kind(line.in_header)
        } else {
            TomlKind::String
        };
        return Ok(tok(start, *i, kind));
    }
    if b == b'\'' {
        if bytes.get(*i + 1) == Some(&b'\'') && bytes.get(*i + 2) == Some(&b'\'') {
            *i += 3;
            let closed = scan_multi_literal(bytes, i);
            if !closed {
                state.mode = TomlMode::MultiLiteral;
            }
            line.expect_key = false;
            let kind = if key_pos {
                quoted_key_kind(line.in_header)
            } else {
                TomlKind::String
            };
            return Ok(tok(start, *i, kind));
        }
        *i += 1;
        let _ = scan_literal_line(bytes, i);
        line.expect_key = false;
        let kind = if key_pos {
            quoted_key_kind(line.in_header)
        } else {
            TomlKind::String
        };
        return Ok(tok(start, *i, kind));
    }

    if key_pos && is_bare_key(b) {
        *i += 1;
        while *i < bytes.len() && is_bare_key(bytes[*i]) {
            *i += 1;
        }
        let kind = if line.in_header {
            TomlKind::Typename
        } else {
            TomlKind::Identifier
        };
        return Ok(tok(start, *i, kind));
    }

    if !key_pos {
        if b.is_ascii_alphabetic() {
            let ident_start = *i;
            *i += 1;
            while *i < bytes.len() && (bytes[*i].is_ascii_alphanumeric() || bytes[*i] == b'_') {
                *i += 1;
            }
            let word = std::str::from_utf8(&bytes[ident_start..*i]).unwrap_or("");
            let kind = match word {
                "true" | "false" => TomlKind::Constant,
                "inf" | "nan" => TomlKind::Number,
                _ => TomlKind::Unknown,
            };
            return Ok(tok(start, *i, kind));
        }
        if b.is_ascii_digit() || matches!(b, b'+' | b'-') {
            scan_number(bytes, i);
            if (*i as u32) <= start {
                *i = start as usize + 1;
            }
            return Ok(tok(start, *i, TomlKind::Number));
        }
    }

    *i += 1;
    if (*i as u32) <= start {
        return Err(LexError::Nonprogress { at: start });
    }
    Ok(tok(start, *i, TomlKind::Unknown))
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
) -> Result<Vec<TomlToken>, LexError> {
    let mut tokens = Vec::new();
    let mut state = TomlState::default();
    let mut line = LineCtx::from_state(&state);
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
        match lex_one(bytes, &mut i, &mut state, &mut line) {
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
pub fn lex_line(line: &str, incoming: TomlState) -> (TomlState, Vec<(usize, TokenRole)>) {
    let bytes = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    let mut state = incoming;
    let mut ctx = LineCtx::from_state(&state);
    while i < bytes.len() {
        let start = i;
        match lex_one(bytes, &mut i, &mut state, &mut ctx) {
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
            TomlMode::MultiBasic | TomlMode::MultiLiteral => TokenRole::String,
            TomlMode::Normal => TokenRole::Unknown,
        };
        push_role(bytes.len(), role, &mut out);
    }
    (state, out)
}

/// Convert document tokens into public spans.
pub fn document_spans(_bytes: &[u8], tokens: &[TomlToken]) -> Vec<TokenSpan> {
    tokens
        .iter()
        .map(|t| TokenSpan::new(t.start, t.end, t.kind.role()))
        .collect()
}
