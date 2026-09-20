//! Source-only Kotlin lexer shared by the editor and the Kotlin frontend.
//! Byte offsets address the original document. Continuation state is owned
//! here: nested block comments and raw strings survive line breaks; regular
//! strings and char literals are line-bounded except for `${}` interpolations,
//! which may span lines.
//!
//! Block comments nest (`/* /* */ */`). String templates `$name` and `${...}`
//! split the surrounding string into Identifier / Punctuator spans; nested
//! strings inside a template that contain `${` beyond depth 2 stay String.

use crate::token::{TokenRole, TokenSpan};
use crate::LexError;

/// Version of this lexer; part of parse and search cache identity.
pub const KOTLIN_LEXER_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
enum KotlinMode {
    #[default]
    Normal,
    BlockComment {
        depth: u8,
    },
    /// Outer `"..."` / `"""..."""`.
    String {
        raw: bool,
    },
    /// `${...}` of an outer string.
    Tpl {
        host_raw: bool,
        braces: u8,
    },
    /// String nested in [`KotlinMode::Tpl`].
    NestStr {
        raw: bool,
        host_raw: bool,
        outer_braces: u8,
    },
    /// `${...}` of a nested string. Strings inside this do not split on `${`.
    Tpl2 {
        str_raw: bool,
        host_raw: bool,
        braces: u8,
        outer_braces: u8,
    },
    /// String nested in [`KotlinMode::Tpl2`]; `${` stays in the string.
    DeepStr {
        raw: bool,
        str_raw: bool,
        host_raw: bool,
        braces: u8,
        outer_braces: u8,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct KotlinState {
    mode: KotlinMode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KotlinKind {
    Whitespace,
    Newline,
    Comment,
    Identifier,
    Keyword,
    Annotation,
    Label,
    Number,
    String,
    RawString,
    Char,
    Punctuator,
    Delimiter,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KotlinToken {
    pub start: u32,
    pub end: u32,
    pub kind: KotlinKind,
}

impl KotlinKind {
    pub fn role(self) -> TokenRole {
        match self {
            KotlinKind::Whitespace | KotlinKind::Newline => TokenRole::Whitespace,
            KotlinKind::Comment => TokenRole::Comment,
            KotlinKind::Identifier | KotlinKind::Label => TokenRole::Identifier,
            KotlinKind::Keyword => TokenRole::Keyword,
            KotlinKind::Annotation => TokenRole::Preprocessor,
            KotlinKind::Number => TokenRole::Number,
            KotlinKind::String | KotlinKind::RawString => TokenRole::String,
            KotlinKind::Char => TokenRole::Char,
            KotlinKind::Punctuator => TokenRole::Punctuator,
            KotlinKind::Delimiter => TokenRole::Delimiter,
            KotlinKind::Unknown => TokenRole::Unknown,
        }
    }
}

/// Hard keywords. Soft keywords and modifiers stay Identifier. Sorted.
const HARD_KEYWORDS: &[&str] = &[
    "as",
    "break",
    "class",
    "continue",
    "do",
    "else",
    "false",
    "for",
    "fun",
    "if",
    "in",
    "interface",
    "is",
    "null",
    "object",
    "package",
    "return",
    "super",
    "this",
    "throw",
    "true",
    "try",
    "typealias",
    "typeof",
    "val",
    "var",
    "when",
    "while",
];

/// Modifier identifiers. Stay Identifier kind; role Keyword in modifier position.
const MODIFIERS: &[&str] = &[
    "abstract",
    "actual",
    "annotation",
    "companion",
    "const",
    "crossinline",
    "data",
    "enum",
    "expect",
    "external",
    "final",
    "infix",
    "inline",
    "inner",
    "internal",
    "lateinit",
    "noinline",
    "open",
    "operator",
    "out",
    "override",
    "private",
    "protected",
    "public",
    "reified",
    "sealed",
    "suspend",
    "tailrec",
    "vararg",
];

const USE_SITE_TARGETS: &[&str] = &[
    "delegate",
    "field",
    "file",
    "get",
    "param",
    "property",
    "receiver",
    "set",
    "setparam",
];

pub fn is_hard_keyword(ident: &str) -> bool {
    HARD_KEYWORDS.binary_search(&ident).is_ok()
}

pub fn is_modifier_keyword(ident: &str) -> bool {
    MODIFIERS.binary_search(&ident).is_ok()
}

fn is_use_site_target(ident: &str) -> bool {
    USE_SITE_TARGETS.binary_search(&ident).is_ok()
}

fn classify_identifier(ident: &str, next_is_call: bool, next_is_mod_pos: bool) -> TokenRole {
    match ident {
        "if" | "else" | "when" | "try" | "return" | "throw" => TokenRole::BranchKeyword,
        "for" | "while" | "do" | "break" | "continue" => TokenRole::LoopKeyword,
        "true" | "false" | "null" => TokenRole::Constant,
        "class" | "interface" | "object" | "fun" | "val" | "var" | "typealias" | "package"
        | "import" => TokenRole::Keyword,
        other if is_hard_keyword(other) => TokenRole::Keyword,
        other if is_modifier_keyword(other) && next_is_mod_pos => TokenRole::Keyword,
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

fn push_token(
    tokens: &mut Vec<KotlinToken>,
    start: u32,
    end: usize,
    kind: KotlinKind,
) -> Result<(), LexError> {
    if end as u32 <= start {
        return Err(LexError::Nonprogress { at: start });
    }
    tokens.push(KotlinToken {
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

fn tok(start: u32, end: usize, kind: KotlinKind) -> KotlinToken {
    KotlinToken {
        start,
        end: end as u32,
        kind,
    }
}

fn string_kind(raw: bool) -> KotlinKind {
    if raw {
        KotlinKind::RawString
    } else {
        KotlinKind::String
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
    if b == b'u' {
        let mut n = 0;
        while *i < bytes.len() && n < 4 && bytes[*i].is_ascii_hexdigit() {
            *i += 1;
            n += 1;
        }
    }
}

fn looks_like_raw_open(bytes: &[u8], i: usize) -> bool {
    bytes.get(i) == Some(&b'"')
        && bytes.get(i + 1) == Some(&b'"')
        && bytes.get(i + 2) == Some(&b'"')
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

/// `$` interpolation: `$name` or `` $`name` ``. Leaves `i` at `$` if it is not
/// an interpolation start (`${` is handled by the caller).
fn interpolation_ident_end(bytes: &[u8], dollar: usize) -> Option<usize> {
    if bytes.get(dollar) != Some(&b'$') {
        return None;
    }
    let next = bytes.get(dollar + 1).copied().unwrap_or(0);
    if next == b'{' {
        return None;
    }
    if next == b'`' || is_ident_start(next) {
        let mut j = dollar + 1;
        consume_ident(bytes, &mut j);
        if j > dollar + 1 {
            return Some(j);
        }
    }
    None
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
        } else {
            while i < bytes.len() && (bytes[i] == b'0' || bytes[i] == b'1' || bytes[i] == b'_') {
                i += 1;
            }
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
        // `.` continues a number only when a fraction/exponent follows, so
        // `1.foo` is Number `1` + `.` + ident. End-of-input `1.` is a float.
        if next.is_ascii_digit() || next == b'e' || next == b'E' {
            i += 1;
            while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'_') {
                i += 1;
            }
        } else if next == 0 {
            i += 1;
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
    let Some(b) = bytes.get(i).copied() else {
        return i;
    };
    match b {
        b'f' | b'F' => i + 1,
        b'u' | b'U' => {
            i += 1;
            if matches!(bytes.get(i).copied(), Some(b'l' | b'L')) {
                i += 1;
            }
            i
        }
        b'l' | b'L' => {
            i += 1;
            if matches!(bytes.get(i).copied(), Some(b'u' | b'U')) {
                i += 1;
            }
            i
        }
        _ => i,
    }
}

fn punct_len(bytes: &[u8], i: usize) -> usize {
    let rest = &bytes[i..];
    if rest.starts_with(b"===") || rest.starts_with(b"!==") || rest.starts_with(b"..<") {
        return 3;
    }
    if matches!(
        rest.get(..2),
        Some(
            b"?." | b"?:"
                | b"!!"
                | b"->"
                | b".."
                | b"::"
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
        )
    ) {
        return 2;
    }
    1
}

fn close_string_parent(state: &mut KotlinState, raw: bool, nest: u8, restore: StringRestore) {
    match nest {
        0 => state.mode = KotlinMode::Normal,
        1 => {
            state.mode = KotlinMode::Tpl {
                host_raw: restore.host_raw,
                braces: restore.outer_braces,
            };
        }
        _ => {
            state.mode = KotlinMode::Tpl2 {
                str_raw: restore.str_raw,
                host_raw: restore.host_raw,
                braces: restore.braces,
                outer_braces: restore.outer_braces,
            };
        }
    }
    let _ = raw;
}

#[derive(Clone, Copy)]
struct StringRestore {
    host_raw: bool,
    str_raw: bool,
    braces: u8,
    outer_braces: u8,
}

fn string_context(mode: KotlinMode) -> Option<(bool, u8, StringRestore, bool)> {
    match mode {
        KotlinMode::String { raw } => Some((
            raw,
            0,
            StringRestore {
                host_raw: raw,
                str_raw: raw,
                braces: 0,
                outer_braces: 0,
            },
            true,
        )),
        KotlinMode::NestStr {
            raw,
            host_raw,
            outer_braces,
        } => Some((
            raw,
            1,
            StringRestore {
                host_raw,
                str_raw: raw,
                braces: 0,
                outer_braces,
            },
            true,
        )),
        KotlinMode::DeepStr {
            raw,
            str_raw,
            host_raw,
            braces,
            outer_braces,
        } => Some((
            raw,
            2,
            StringRestore {
                host_raw,
                str_raw,
                braces,
                outer_braces,
            },
            false,
        )),
        _ => None,
    }
}

fn open_template(state: &mut KotlinState, raw: bool, nest: u8, restore: StringRestore) {
    if nest == 0 {
        state.mode = KotlinMode::Tpl {
            host_raw: raw,
            braces: 0,
        };
    } else {
        state.mode = KotlinMode::Tpl2 {
            str_raw: raw,
            host_raw: restore.host_raw,
            braces: 0,
            outer_braces: restore.outer_braces,
        };
    }
}

fn enter_nested_string(state: &mut KotlinState, raw: bool) {
    match state.mode {
        KotlinMode::Tpl {
            host_raw,
            braces,
        } => {
            state.mode = KotlinMode::NestStr {
                raw,
                host_raw,
                outer_braces: braces,
            };
        }
        KotlinMode::Tpl2 {
            str_raw,
            host_raw,
            braces,
            outer_braces,
        } => {
            state.mode = KotlinMode::DeepStr {
                raw,
                str_raw,
                host_raw,
                braces,
                outer_braces,
            };
        }
        _ => {
            state.mode = KotlinMode::String { raw };
        }
    }
}

fn scan_string_token(
    bytes: &[u8],
    i: &mut usize,
    state: &mut KotlinState,
    token_start: u32,
) -> Result<KotlinToken, LexError> {
    let start_i = *i;
    let start = token_start;
    let Some((raw, nest, restore, allow_interp)) = string_context(state.mode) else {
        return Err(LexError::Nonprogress { at: start });
    };
    let kind = string_kind(raw);

    // Interpolation at the cursor is its own token only when this call
    // started on `$` (a continuation), not when an opener quote is pending.
    if allow_interp && (*i as u32) == start {
        if let Some(end) = interpolation_ident_end(bytes, *i) {
            *i = end;
            return Ok(tok(start, *i, KotlinKind::Identifier));
        }
        if bytes.get(*i) == Some(&b'$') && bytes.get(*i + 1) == Some(&b'{') {
            *i += 2;
            open_template(state, raw, nest, restore);
            return Ok(tok(start, *i, KotlinKind::Punctuator));
        }
    }

    if raw {
        if looks_like_raw_open(bytes, *i) {
            *i += 3;
            close_string_parent(state, raw, nest, restore);
            return Ok(tok(start, *i, kind));
        }
    } else if bytes.get(*i) == Some(&b'"') {
        *i += 1;
        close_string_parent(state, raw, nest, restore);
        return Ok(tok(start, *i, kind));
    }

    while *i < bytes.len() {
        if !raw && is_newline(bytes[*i]) {
            if *i == start_i {
                return Err(LexError::Nonprogress { at: start });
            }
            close_string_parent(state, raw, nest, restore);
            return Ok(tok(start, *i, kind));
        }
        if allow_interp {
            if interpolation_ident_end(bytes, *i).is_some()
                || (bytes.get(*i) == Some(&b'$') && bytes.get(*i + 1) == Some(&b'{'))
            {
                if *i == start_i {
                    break;
                }
                return Ok(tok(start, *i, kind));
            }
        }
        if raw {
            if looks_like_raw_open(bytes, *i) {
                *i += 3;
                close_string_parent(state, raw, nest, restore);
                return Ok(tok(start, *i, kind));
            }
            *i += 1;
            continue;
        }
        let b = bytes[*i];
        if b == b'\\' {
            *i += 1;
            consume_string_escape(bytes, i);
            continue;
        }
        if b == b'"' {
            *i += 1;
            close_string_parent(state, raw, nest, restore);
            return Ok(tok(start, *i, kind));
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
    Ok(tok(start, *i, kind))
}

fn template_mode(mode: KotlinMode) -> Option<(bool, u8, u8, bool, u8)> {
    match mode {
        KotlinMode::Tpl { host_raw, braces } => Some((host_raw, braces, 1, false, 0)),
        KotlinMode::Tpl2 {
            str_raw,
            host_raw,
            braces,
            outer_braces,
        } => Some((host_raw, braces, 2, str_raw, outer_braces)),
        _ => None,
    }
}

fn close_template(state: &mut KotlinState) {
    match state.mode {
        KotlinMode::Tpl { host_raw, .. } => {
            state.mode = KotlinMode::String { raw: host_raw };
        }
        KotlinMode::Tpl2 {
            str_raw,
            host_raw,
            outer_braces,
            ..
        } => {
            state.mode = KotlinMode::NestStr {
                raw: str_raw,
                host_raw,
                outer_braces,
            };
        }
        other => state.mode = other,
    }
}

fn inc_template_brace(state: &mut KotlinState) {
    match &mut state.mode {
        KotlinMode::Tpl { braces, .. } | KotlinMode::Tpl2 { braces, .. } => {
            *braces = braces.saturating_add(1);
        }
        _ => {}
    }
}

fn dec_template_brace(state: &mut KotlinState) -> bool {
    match &mut state.mode {
        KotlinMode::Tpl { braces, .. } | KotlinMode::Tpl2 { braces, .. } => {
            if *braces == 0 {
                return true;
            }
            *braces -= 1;
            false
        }
        _ => false,
    }
}

/// Shared automaton step. Emits one token covering `[start, *i)`.
fn lex_one(
    bytes: &[u8],
    i: &mut usize,
    state: &mut KotlinState,
    line_start: &mut bool,
) -> Result<KotlinToken, LexError> {
    let start_i = *i;
    let start = *i as u32;
    if *i >= bytes.len() {
        return Err(LexError::Nonprogress { at: start });
    }

    if let KotlinMode::BlockComment { depth } = state.mode {
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
                    state.mode = KotlinMode::Normal;
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
        if matches!(state.mode, KotlinMode::BlockComment { .. }) {
            state.mode = KotlinMode::BlockComment { depth };
        }
        if (*i as u32) <= start {
            return Err(LexError::Nonprogress { at: start });
        }
        return Ok(tok(start, *i, KotlinKind::Comment));
    }

    if string_context(state.mode).is_some() {
        let t = scan_string_token(bytes, i, state, start)?;
        *line_start = false;
        return Ok(t);
    }

    if template_mode(state.mode).is_some() {
        let b = bytes[*i];
        if b == b'}' {
            *i += 1;
            *line_start = false;
            if dec_template_brace(state) {
                close_template(state);
            }
            return Ok(tok(start, *i, KotlinKind::Punctuator));
        }
        if b == b'{' {
            *i += 1;
            *line_start = false;
            inc_template_brace(state);
            return Ok(tok(start, *i, KotlinKind::Delimiter));
        }
        if b == b'"' {
            let raw = looks_like_raw_open(bytes, *i);
            if raw {
                *i += 3;
            } else {
                *i += 1;
            }
            enter_nested_string(state, raw);
            *line_start = false;
            return scan_string_token(bytes, i, state, start);
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
        return Ok(tok(start, *i, KotlinKind::Newline));
    }
    if b == b'\n' {
        *i += 1;
        *line_start = true;
        return Ok(tok(start, *i, KotlinKind::Newline));
    }
    if is_space(b) {
        *i += 1;
        while *i < bytes.len() && is_space(bytes[*i]) {
            *i += 1;
        }
        return Ok(tok(start, *i, KotlinKind::Whitespace));
    }
    *line_start = false;
    if b == b'/' && bytes.get(*i + 1) == Some(&b'/') {
        *i += 2;
        while *i < bytes.len() && !is_newline(bytes[*i]) {
            *i += 1;
        }
        return Ok(tok(start, *i, KotlinKind::Comment));
    }
    if b == b'/' && bytes.get(*i + 1) == Some(&b'*') {
        *i += 2;
        let mut depth = 1u8;
        state.mode = KotlinMode::BlockComment { depth };
        while *i < bytes.len() {
            if bytes[*i] == b'/' && bytes.get(*i + 1) == Some(&b'*') {
                depth = depth.saturating_add(1);
                *i += 2;
                state.mode = KotlinMode::BlockComment { depth };
                continue;
            }
            if bytes[*i] == b'*' && bytes.get(*i + 1) == Some(&b'/') {
                *i += 2;
                if depth <= 1 {
                    state.mode = KotlinMode::Normal;
                    break;
                }
                depth -= 1;
                state.mode = KotlinMode::BlockComment { depth };
                continue;
            }
            *i += 1;
        }
        if matches!(state.mode, KotlinMode::BlockComment { .. }) {
            state.mode = KotlinMode::BlockComment { depth };
        }
        return Ok(tok(start, *i, KotlinKind::Comment));
    }
    if b == b'\'' {
        *i += 1;
        while *i < bytes.len() && !is_newline(bytes[*i]) {
            let c = bytes[*i];
            if c == b'\\' {
                *i += 1;
                consume_string_escape(bytes, i);
                continue;
            }
            if c == b'\'' {
                *i += 1;
                break;
            }
            *i += 1;
        }
        if *i as u32 <= start {
            if *i == start_i + 1 {
                return Ok(tok(start, *i, KotlinKind::Char));
            }
            return Err(LexError::Nonprogress { at: start });
        }
        return Ok(tok(start, *i, KotlinKind::Char));
    }
    if b == b'"' {
        let raw = looks_like_raw_open(bytes, *i);
        if raw {
            *i += 3;
        } else {
            *i += 1;
        }
        enter_nested_string(state, raw);
        return scan_string_token(bytes, i, state, start);
    }
    if b == b'@' {
        // `@` after an identifier-continue byte or closing backtick is a
        // label reference (`this@Outer`), not an annotation.
        let prev = start_i.checked_sub(1).and_then(|p| bytes.get(p).copied());
        if prev.is_some_and(|p| is_ident_continue(p) || p == b'`') {
            *i += 1;
            return Ok(tok(start, *i, KotlinKind::Punctuator));
        }
        let next = bytes.get(*i + 1).copied().unwrap_or(0);
        if is_ident_start(next) || next == b'`' {
            *i += 1;
            consume_ident(bytes, i);
            let ident = std::str::from_utf8(&bytes[start as usize + 1..*i]).unwrap_or("");
            if is_use_site_target(ident) && bytes.get(*i) == Some(&b':') {
                let after = bytes.get(*i + 1).copied().unwrap_or(0);
                if is_ident_start(after) || after == b'`' {
                    *i += 1;
                    consume_ident(bytes, i);
                }
            }
            return Ok(tok(start, *i, KotlinKind::Annotation));
        }
        *i += 1;
        return Ok(tok(start, *i, KotlinKind::Punctuator));
    }
    if b == b'`' {
        consume_ident(bytes, i);
        if bytes.get(*i) == Some(&b'@') {
            *i += 1;
            return Ok(tok(start, *i, KotlinKind::Label));
        }
        return Ok(tok(start, *i, KotlinKind::Identifier));
    }
    if is_ident_start(b) {
        *i += 1;
        while *i < bytes.len() && is_ident_continue(bytes[*i]) {
            *i += 1;
        }
        let ident = std::str::from_utf8(&bytes[start as usize..*i]).unwrap_or("");
        if !is_hard_keyword(ident) && bytes.get(*i) == Some(&b'@') {
            *i += 1;
            return Ok(tok(start, *i, KotlinKind::Label));
        }
        let kind = if is_hard_keyword(ident) {
            KotlinKind::Keyword
        } else {
            KotlinKind::Identifier
        };
        return Ok(tok(start, *i, kind));
    }
    if b.is_ascii_digit() || (b == b'.' && bytes.get(*i + 1).copied().unwrap_or(0).is_ascii_digit())
    {
        *i = scan_number(bytes, *i);
        return Ok(tok(start, *i, KotlinKind::Number));
    }
    let n = punct_len(bytes, *i);
    *i += n;
    let kind = if n == 1 && matches!(b, b'(' | b')' | b'{' | b'}' | b'[' | b']') {
        KotlinKind::Delimiter
    } else if b.is_ascii_graphic() {
        KotlinKind::Punctuator
    } else {
        KotlinKind::Unknown
    };
    if *i == start_i {
        return Err(LexError::Nonprogress { at: start });
    }
    Ok(tok(start, *i, kind))
}

fn reset_line_bounded(state: &mut KotlinState) {
    match state.mode {
        KotlinMode::String { raw } if !raw => state.mode = KotlinMode::Normal,
        KotlinMode::NestStr {
            raw,
            host_raw,
            outer_braces,
        } if !raw => {
            state.mode = KotlinMode::Tpl {
                host_raw,
                braces: outer_braces,
            };
        }
        KotlinMode::DeepStr {
            raw,
            str_raw,
            host_raw,
            braces,
            outer_braces,
        } if !raw => {
            state.mode = KotlinMode::Tpl2 {
                str_raw,
                host_raw,
                braces,
                outer_braces,
            };
        }
        _ => {}
    }
}

/// Tokenize an entire document. Spans are byte offsets into `bytes`.
pub fn lex_document_cancellable(
    bytes: &[u8],
    cancel: &dyn Fn() -> bool,
) -> Result<Vec<KotlinToken>, LexError> {
    let mut tokens = Vec::new();
    let mut state = KotlinState::default();
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
                    push_token(&mut tokens, start, i, KotlinKind::Newline)?;
                    continue;
                }
                if i == start_i && i < bytes.len() {
                    i += 1;
                    push_token(&mut tokens, at, i, KotlinKind::Unknown)?;
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

fn next_is_mod_pos_bytes(bytes: &[u8], end: usize) -> bool {
    let mut k = end;
    while k < bytes.len() && is_space(bytes[k]) {
        k += 1;
    }
    if k >= bytes.len() || is_newline(bytes[k]) {
        return false;
    }
    if bytes[k] == b'`' {
        return true;
    }
    if !is_ident_start(bytes[k]) {
        return false;
    }
    let mut j = k + 1;
    while j < bytes.len() && is_ident_continue(bytes[j]) {
        j += 1;
    }
    let ident = std::str::from_utf8(&bytes[k..j]).unwrap_or("");
    is_hard_keyword(ident) || is_ident_start(bytes[k])
}

/// Tokenize one display line (without the newline) given incoming continuation.
/// Each pair is `(end_index, role)` covering `[prev_end, end)` of `line`.
pub fn lex_line(line: &str, incoming: KotlinState) -> (KotlinState, Vec<(usize, TokenRole)>) {
    let bytes = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    let mut state = incoming;
    let mut line_start = matches!(state.mode, KotlinMode::Normal);
    while i < bytes.len() {
        let start = i;
        match lex_one(bytes, &mut i, &mut state, &mut line_start) {
            Ok(t) => {
                let end = (t.end as usize).min(bytes.len()).max(start);
                if end > start {
                    let role = if matches!(
                        t.kind,
                        KotlinKind::Keyword | KotlinKind::Identifier
                    ) {
                        let ident =
                            std::str::from_utf8(&bytes[t.start as usize..end]).unwrap_or("");
                        classify_identifier(
                            ident,
                            next_is_call_bytes(bytes, end),
                            next_is_mod_pos_bytes(bytes, end),
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
            KotlinMode::BlockComment { .. } => TokenRole::Comment,
            KotlinMode::String { .. }
            | KotlinMode::NestStr { .. }
            | KotlinMode::DeepStr { .. } => TokenRole::String,
            KotlinMode::Normal | KotlinMode::Tpl { .. } | KotlinMode::Tpl2 { .. } => {
                TokenRole::Unknown
            }
        };
        push_role(bytes.len(), role, &mut out);
    }
    (state, out)
}

fn same_line(tokens: &[KotlinToken], a: usize, b: usize) -> bool {
    tokens[a + 1..b]
        .iter()
        .all(|t| t.kind != KotlinKind::Newline)
}

/// Convert document tokens into public spans, classifying keywords and
/// function names with one-token lookahead.
pub fn document_spans(bytes: &[u8], tokens: &[KotlinToken]) -> Vec<TokenSpan> {
    let mut out = Vec::with_capacity(tokens.len());
    for (i, t) in tokens.iter().enumerate() {
        let mut role = t.kind.role();
        if t.kind == KotlinKind::Keyword || t.kind == KotlinKind::Identifier {
            let mut j = i + 1;
            while j < tokens.len() && matches!(tokens[j].kind, KotlinKind::Whitespace | KotlinKind::Comment)
            {
                j += 1;
            }
            let next_is_call = j < tokens.len()
                && tokens[j].kind == KotlinKind::Delimiter
                && matches!(bytes.get(tokens[j].start as usize), Some(&b'(' | &b'{'))
                && same_line(tokens, i, j);
            let next_is_mod_pos = if j < tokens.len() && tokens[j].kind != KotlinKind::Newline {
                let ident =
                    std::str::from_utf8(&bytes[tokens[j].start as usize..tokens[j].end as usize])
                        .unwrap_or("");
                (tokens[j].kind == KotlinKind::Identifier
                    || tokens[j].kind == KotlinKind::Keyword)
                    && same_line(tokens, i, j)
                    && (is_hard_keyword(ident)
                        || tokens[j].kind == KotlinKind::Identifier
                        || tokens[j].kind == KotlinKind::Keyword)
            } else {
                false
            };
            let ident = std::str::from_utf8(&bytes[t.start as usize..t.end as usize]).unwrap_or("");
            role = classify_identifier(ident, next_is_call, next_is_mod_pos);
        }
        out.push(TokenSpan::new(t.start, t.end, role));
    }
    out
}
