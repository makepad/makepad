//! Source-only C# lexer shared by the editor and the C# frontend.
//! Byte offsets address the original document. Continuation state is owned
//! here: verbatim, interpolated and raw strings are not packed into a
//! universal integer.

use crate::token::{TokenRole, TokenSpan};
use crate::LexError;

/// Version of this lexer; part of parse and search cache identity.
pub const CSHARP_LEXER_VERSION: u32 = 1;

/// Nested interpolated-string frames kept in line continuation. Capped so a
/// pathological file cannot grow unbounded state.
const FRAME_CAP: usize = 64;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
enum CSharpMode {
    #[default]
    Normal,
    BlockComment,
    RegularString,
    Char,
    Verbatim,
    Interpolated,
    InterpolatedVerbatim,
    Raw,
}

/// Quote kind open inside an interpolation hole.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
enum HoleQuote {
    #[default]
    None,
    Regular,
    Char,
    Verbatim,
}

/// Suspended outer interpolated / raw string when a hole contains another
/// interpolated or quoted literal that itself needs continuation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct InterpFrame {
    mode: CSharpMode,
    hole: u16,
    hole_code: u16,
    hole_quote: HoleQuote,
    hole_block_comment: bool,
    raw_quotes: u8,
    dollars: u8,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct CSharpState {
    mode: CSharpMode,
    /// Interpolation hole depth. 0 = in string text.
    hole: u16,
    /// `{` depth of C# code inside the current hole, not counting the hole's
    /// own opening brace.
    hole_code: u16,
    hole_quote: HoleQuote,
    hole_block_comment: bool,
    /// Opening quote count for a raw string (3+).
    raw_quotes: u8,
    /// `$` count that opened the current interpolated / raw string.
    dollars: u8,
    frames: Vec<InterpFrame>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CSharpKind {
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CSharpToken {
    pub start: u32,
    pub end: u32,
    pub kind: CSharpKind,
}

impl CSharpKind {
    pub fn role(self) -> TokenRole {
        match self {
            CSharpKind::Whitespace => TokenRole::Whitespace,
            CSharpKind::Comment => TokenRole::Comment,
            CSharpKind::Identifier => TokenRole::Identifier,
            CSharpKind::Keyword => TokenRole::Keyword,
            CSharpKind::Number => TokenRole::Number,
            CSharpKind::String => TokenRole::String,
            CSharpKind::Char => TokenRole::Char,
            CSharpKind::Punctuator => TokenRole::Punctuator,
            CSharpKind::Delimiter => TokenRole::Delimiter,
            CSharpKind::Preprocessor => TokenRole::Preprocessor,
            CSharpKind::Unknown => TokenRole::Unknown,
        }
    }
}

/// C# reserved keywords. Contextual keywords stay Identifier; the parser
/// decides. Sorted for binary search.
const KEYWORDS: &[&str] = &[
    "abstract",
    "as",
    "base",
    "bool",
    "break",
    "byte",
    "case",
    "catch",
    "char",
    "checked",
    "class",
    "const",
    "continue",
    "decimal",
    "default",
    "delegate",
    "do",
    "double",
    "else",
    "enum",
    "event",
    "explicit",
    "extern",
    "false",
    "finally",
    "fixed",
    "float",
    "for",
    "foreach",
    "goto",
    "if",
    "implicit",
    "in",
    "int",
    "interface",
    "internal",
    "is",
    "lock",
    "long",
    "namespace",
    "new",
    "null",
    "object",
    "operator",
    "out",
    "override",
    "params",
    "private",
    "protected",
    "public",
    "readonly",
    "ref",
    "return",
    "sbyte",
    "sealed",
    "short",
    "sizeof",
    "stackalloc",
    "static",
    "string",
    "struct",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "uint",
    "ulong",
    "unchecked",
    "unsafe",
    "ushort",
    "using",
    "virtual",
    "void",
    "volatile",
    "while",
];

fn is_keyword(ident: &str) -> bool {
    KEYWORDS.binary_search(&ident).is_ok()
}

fn classify_identifier(ident: &str, next_is_paren: bool) -> TokenRole {
    let name = ident.strip_prefix('@').unwrap_or(ident);
    match name {
        "if" | "else" | "switch" | "case" | "default" | "try" | "catch" | "finally" | "return"
        | "throw" | "goto" => TokenRole::BranchKeyword,
        "for" | "foreach" | "while" | "do" | "break" | "continue" => TokenRole::LoopKeyword,
        "true" | "false" | "null" => TokenRole::Constant,
        "bool" | "byte" | "char" | "decimal" | "double" | "float" | "int" | "long" | "object"
        | "sbyte" | "short" | "string" | "uint" | "ulong" | "ushort" | "void" | "var"
        | "dynamic" | "nint" | "nuint" => TokenRole::Typename,
        other if is_keyword(other) => TokenRole::Keyword,
        _ if next_is_paren => TokenRole::Function,
        _ if name
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
    b.is_ascii_alphabetic() || b == b'_' || b == b'@' || b >= 0x80
}

fn is_ident_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80
}

fn is_newline(b: u8) -> bool {
    b == b'\n' || b == b'\r'
}

fn push_token(
    tokens: &mut Vec<CSharpToken>,
    start: u32,
    end: usize,
    kind: CSharpKind,
) -> Result<(), LexError> {
    if end as u32 <= start {
        return Err(LexError::Nonprogress { at: start });
    }
    tokens.push(CSharpToken {
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

fn consume_u8_suffix(bytes: &[u8], i: &mut usize) {
    if bytes.get(*i).map(|b| b.to_ascii_lowercase()) == Some(b'u')
        && bytes.get(*i + 1) == Some(&b'8')
        && bytes
            .get(*i + 2)
            .map(|&c| !is_ident_continue(c))
            .unwrap_or(true)
    {
        *i += 2;
    }
}

impl CSharpState {
    fn reset_string(&mut self) {
        self.mode = CSharpMode::Normal;
        self.hole = 0;
        self.hole_code = 0;
        self.hole_quote = HoleQuote::None;
        self.hole_block_comment = false;
        self.raw_quotes = 0;
        self.dollars = 0;
        self.frames.clear();
    }

    fn push_frame(&mut self) -> bool {
        if self.frames.len() >= FRAME_CAP {
            return false;
        }
        self.frames.push(InterpFrame {
            mode: self.mode,
            hole: self.hole,
            hole_code: self.hole_code,
            hole_quote: self.hole_quote,
            hole_block_comment: self.hole_block_comment,
            raw_quotes: self.raw_quotes,
            dollars: self.dollars,
        });
        true
    }

    fn pop_frame(&mut self) {
        if let Some(f) = self.frames.pop() {
            self.mode = f.mode;
            self.hole = f.hole;
            self.hole_code = f.hole_code;
            self.hole_quote = f.hole_quote;
            self.hole_block_comment = f.hole_block_comment;
            self.raw_quotes = f.raw_quotes;
            self.dollars = f.dollars;
        } else {
            self.reset_string();
        }
    }

    fn line_bounded(&self) -> bool {
        matches!(self.mode, CSharpMode::RegularString | CSharpMode::Char)
            && self.hole == 0
            && self.frames.is_empty()
    }
}

/// Snapshot of how a string literal opened: `$` count, optional `@`, quote count.
struct StringOpen {
    dollars: u8,
    verbatim: bool,
    quotes: u8,
}

fn count_quotes(bytes: &[u8], i: usize) -> u8 {
    let mut n = 0u8;
    while bytes.get(i + n as usize) == Some(&b'"') && n < 16 {
        n += 1;
    }
    n
}

fn count_dollars(bytes: &[u8], i: usize) -> u8 {
    let mut n = 0u8;
    while bytes.get(i + n as usize) == Some(&b'$') && n < 16 {
        n += 1;
    }
    n
}

/// Opening at `i` if this is `"`, `@"`, `$"`, `$@"`, `@$"`, raw `"""`, `$"""`.
fn string_open(bytes: &[u8], i: usize) -> Option<StringOpen> {
    let b = *bytes.get(i)?;
    if b == b'\'' {
        return None;
    }
    let mut j = i;
    let dollars = count_dollars(bytes, j);
    j += dollars as usize;
    let mut verbatim = false;
    if bytes.get(j) == Some(&b'@') {
        verbatim = true;
        j += 1;
        if dollars == 0 && bytes.get(j) == Some(&b'$') {
            // `@$"`
            let extra = count_dollars(bytes, j);
            j += extra as usize;
            if extra == 0 {
                return None;
            }
            let quotes = count_quotes(bytes, j);
            if quotes == 0 {
                return None;
            }
            return Some(StringOpen {
                dollars: extra,
                verbatim: true,
                quotes: 1,
            });
        }
    }
    let quotes = count_quotes(bytes, j);
    if quotes == 0 {
        return None;
    }
    // `@"` / `$@"` / `@$"` are never raw: the opener is a single quote and
    // further quotes are content (`""` escape). Raw interpolations are `$"""`.
    if verbatim {
        return Some(StringOpen {
            dollars,
            verbatim: true,
            quotes: 1,
        });
    }
    if dollars == 0 && quotes == 1 {
        return Some(StringOpen {
            dollars: 0,
            verbatim: false,
            quotes: 1,
        });
    }
    if dollars > 0 {
        return Some(StringOpen {
            dollars,
            verbatim: false,
            quotes,
        });
    }
    if quotes >= 3 {
        return Some(StringOpen {
            dollars: 0,
            verbatim: false,
            quotes,
        });
    }
    None
}

fn apply_open(state: &mut CSharpState, open: &StringOpen) {
    state.hole = 0;
    state.hole_code = 0;
    state.hole_quote = HoleQuote::None;
    state.hole_block_comment = false;
    state.dollars = open.dollars;
    state.raw_quotes = if open.quotes >= 3 && !open.verbatim {
        open.quotes
    } else {
        0
    };
    state.mode = if open.quotes >= 3 && !open.verbatim {
        CSharpMode::Raw
    } else if open.dollars > 0 && open.verbatim {
        CSharpMode::InterpolatedVerbatim
    } else if open.dollars > 0 {
        CSharpMode::Interpolated
    } else if open.verbatim {
        CSharpMode::Verbatim
    } else {
        CSharpMode::RegularString
    };
}

fn open_len(open: &StringOpen) -> usize {
    let mut n = open.dollars as usize + open.quotes as usize;
    if open.verbatim {
        n += 1;
    }
    n
}

/// Scan from `i` inside the current string/char mode. Returns the index to
/// emit through and whether the literal closed.
fn scan_string_body(bytes: &[u8], i: &mut usize, state: &mut CSharpState) -> bool {
    while *i < bytes.len() {
        match state.mode {
            CSharpMode::RegularString => {
                if scan_regular_or_char(bytes, i, state, b'"') {
                    return true;
                }
            }
            CSharpMode::Char => {
                if scan_regular_or_char(bytes, i, state, b'\'') {
                    return true;
                }
            }
            CSharpMode::Verbatim => {
                if scan_verbatim(bytes, i, state) {
                    return true;
                }
            }
            CSharpMode::Interpolated => {
                if scan_interpolated(bytes, i, state, false) {
                    return true;
                }
            }
            CSharpMode::InterpolatedVerbatim => {
                if scan_interpolated(bytes, i, state, true) {
                    return true;
                }
            }
            CSharpMode::Raw => {
                if scan_raw(bytes, i, state) {
                    return true;
                }
            }
            CSharpMode::Normal | CSharpMode::BlockComment => return false,
        }
    }
    false
}

fn scan_regular_or_char(
    bytes: &[u8],
    i: &mut usize,
    state: &mut CSharpState,
    quote: u8,
) -> bool {
    let b = bytes[*i];
    if b == b'\\' {
        *i += 1;
        if *i < bytes.len() && !is_newline(bytes[*i]) {
            *i += 1;
        }
        return false;
    }
    if b == quote {
        *i += 1;
        consume_u8_suffix(bytes, i);
        if !state.frames.is_empty() {
            state.pop_frame();
            return false;
        }
        state.reset_string();
        return true;
    }
    if is_newline(b) {
        if !state.frames.is_empty() {
            state.pop_frame();
            return false;
        }
        state.reset_string();
        return true;
    }
    *i += 1;
    false
}

fn scan_verbatim(bytes: &[u8], i: &mut usize, state: &mut CSharpState) -> bool {
    let b = bytes[*i];
    if b == b'"' {
        if bytes.get(*i + 1) == Some(&b'"') {
            *i += 2;
            return false;
        }
        *i += 1;
        consume_u8_suffix(bytes, i);
        if !state.frames.is_empty() {
            state.pop_frame();
            return false;
        }
        state.reset_string();
        return true;
    }
    *i += 1;
    false
}

fn hole_needed(state: &CSharpState) -> u16 {
    state.dollars.max(1) as u16
}

/// Left-to-right `{` runs: `2N` braces are a literal `N` braces; `N` braces
/// open a hole. `N` is the `$` count (at least 1 for `$"..."`).
fn consume_interp_open_braces(bytes: &[u8], i: &mut usize, state: &mut CSharpState) -> bool {
    let need = hole_needed(state);
    let mut n = 0u16;
    while bytes.get(*i + n as usize) == Some(&b'{') {
        n += 1;
        if n > 16 {
            break;
        }
    }
    if n == 0 {
        *i += 1;
        return false;
    }
    if n >= need * 2 {
        *i += (need * 2) as usize;
        return false;
    }
    if n >= need {
        *i += need as usize;
        state.hole = 1;
        state.hole_code = 0;
        state.hole_quote = HoleQuote::None;
        state.hole_block_comment = false;
        return false;
    }
    *i += n as usize;
    false
}

fn scan_interpolated(bytes: &[u8], i: &mut usize, state: &mut CSharpState, verbatim: bool) -> bool {
    if state.hole > 0 {
        return scan_hole(bytes, i, state);
    }
    let b = bytes[*i];
    if !verbatim && b == b'\\' {
        *i += 1;
        if *i < bytes.len() && !is_newline(bytes[*i]) {
            *i += 1;
        }
        return false;
    }
    if b == b'"' {
        if verbatim && bytes.get(*i + 1) == Some(&b'"') {
            *i += 2;
            return false;
        }
        *i += 1;
        consume_u8_suffix(bytes, i);
        if !state.frames.is_empty() {
            state.pop_frame();
            return false;
        }
        state.reset_string();
        return true;
    }
    if !verbatim && is_newline(b) {
        state.reset_string();
        return true;
    }
    if b == b'{' {
        return consume_interp_open_braces(bytes, i, state);
    }
    if b == b'}' {
        let need = hole_needed(state);
        let mut n = 0u16;
        while bytes.get(*i + n as usize) == Some(&b'}') {
            n += 1;
            if n > 16 {
                break;
            }
        }
        let consume = if need > 1 {
            n.min(need) as usize
        } else if n >= 2 {
            2
        } else {
            1
        };
        *i += consume.max(1);
        return false;
    }
    *i += 1;
    false
}

fn scan_raw(bytes: &[u8], i: &mut usize, state: &mut CSharpState) -> bool {
    if state.hole > 0 {
        return scan_hole(bytes, i, state);
    }
    if bytes[*i] == b'"' {
        let n = count_quotes(bytes, *i);
        if n == state.raw_quotes {
            *i += n as usize;
            consume_u8_suffix(bytes, i);
            if !state.frames.is_empty() {
                state.pop_frame();
                return false;
            }
            state.reset_string();
            return true;
        }
        *i += n.max(1) as usize;
        return false;
    }
    if state.dollars > 0 && bytes[*i] == b'{' {
        return consume_interp_open_braces(bytes, i, state);
    }
    *i += 1;
    false
}

fn scan_hole(bytes: &[u8], i: &mut usize, state: &mut CSharpState) -> bool {
    if state.hole_block_comment {
        if bytes[*i] == b'*' && bytes.get(*i + 1) == Some(&b'/') {
            *i += 2;
            state.hole_block_comment = false;
        } else {
            *i += 1;
        }
        return false;
    }
    match state.hole_quote {
        HoleQuote::Regular => {
            let b = bytes[*i];
            if b == b'\\' {
                *i += 1;
                if *i < bytes.len() && !is_newline(bytes[*i]) {
                    *i += 1;
                }
                return false;
            }
            if b == b'"' {
                *i += 1;
                state.hole_quote = HoleQuote::None;
                return false;
            }
            if is_newline(b) {
                state.hole_quote = HoleQuote::None;
                return false;
            }
            *i += 1;
            return false;
        }
        HoleQuote::Char => {
            let b = bytes[*i];
            if b == b'\\' {
                *i += 1;
                if *i < bytes.len() && !is_newline(bytes[*i]) {
                    *i += 1;
                }
                return false;
            }
            if b == b'\'' {
                *i += 1;
                state.hole_quote = HoleQuote::None;
                return false;
            }
            if is_newline(b) {
                state.hole_quote = HoleQuote::None;
                return false;
            }
            *i += 1;
            return false;
        }
        HoleQuote::Verbatim => {
            if bytes[*i] == b'"' {
                if bytes.get(*i + 1) == Some(&b'"') {
                    *i += 2;
                } else {
                    *i += 1;
                    state.hole_quote = HoleQuote::None;
                }
                return false;
            }
            *i += 1;
            return false;
        }
        HoleQuote::None => {}
    }
    let b = bytes[*i];
    if b == b'/' && bytes.get(*i + 1) == Some(&b'/') {
        *i += 2;
        while *i < bytes.len() && !is_newline(bytes[*i]) {
            *i += 1;
        }
        return false;
    }
    if b == b'/' && bytes.get(*i + 1) == Some(&b'*') {
        *i += 2;
        state.hole_block_comment = true;
        return false;
    }
    if let Some(open) = string_open(bytes, *i) {
        if open.dollars == 0 && !open.verbatim && open.quotes == 1 {
            state.hole_quote = HoleQuote::Regular;
            *i += 1;
            return false;
        }
        if open.dollars == 0 && open.verbatim && open.quotes == 1 {
            state.hole_quote = HoleQuote::Verbatim;
            *i += open_len(&open);
            return false;
        }
        if state.push_frame() {
            *i += open_len(&open);
            apply_open(state, &open);
            return false;
        }
        *i += 1;
        return false;
    }
    if b == b'\'' {
        state.hole_quote = HoleQuote::Char;
        *i += 1;
        return false;
    }
    if b == b'{' {
        state.hole_code = state.hole_code.saturating_add(1);
        *i += 1;
        return false;
    }
    if b == b'}' {
        let need = hole_needed(state);
        if state.hole_code > 0 {
            state.hole_code -= 1;
            *i += 1;
            return false;
        }
        let mut n = 0u16;
        while bytes.get(*i + n as usize) == Some(&b'}') {
            n += 1;
            if n > 16 {
                break;
            }
        }
        if n >= need {
            *i += need as usize;
            state.hole = state.hole.saturating_sub(1);
            return false;
        }
        *i += n.max(1) as usize;
        return false;
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
        i += 2;
        while i < bytes.len() {
            let b = bytes[i];
            if b.is_ascii_hexdigit() || b == b'_' {
                i += 1;
                continue;
            }
            break;
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
        if next.is_ascii_digit()
            || matches!(next.to_ascii_lowercase(), b'e' | b'f' | b'd' | b'm')
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
    // u l ul lu f d m, any case, at most two letters.
    let a = bytes.get(i).map(|b| b.to_ascii_lowercase());
    let b = bytes.get(i + 1).map(|c| c.to_ascii_lowercase());
    match (a, b) {
        (Some(b'u'), Some(b'l')) | (Some(b'l'), Some(b'u')) => i += 2,
        (Some(b'u' | b'l' | b'f' | b'd' | b'm'), _) => i += 1,
        _ => {}
    }
    i
}

fn punct_len(bytes: &[u8], i: usize) -> usize {
    let rest = &bytes[i..];
    if rest.starts_with(b">>>=") {
        return 4;
    }
    if rest.starts_with(b">>>") || rest.starts_with(b">>=") || rest.starts_with(b"<<=") || rest.starts_with(b"??=")
    {
        return 3;
    }
    if matches!(
        rest.get(..2),
        Some(
            b"::" | b"->"
                | b"??"
                | b"?."
                | b"=>"
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
                | b".."
        )
    ) {
        return 2;
    }
    1
}

/// Shared automaton step. Emits one token covering `[start, *i)` and updates
/// continuation. `line_start` is true when the next non-blank on this physical
/// line has not yet been seen.
fn lex_one(
    bytes: &[u8],
    i: &mut usize,
    state: &mut CSharpState,
    line_start: &mut bool,
) -> Result<CSharpToken, LexError> {
    let start_i = *i;
    let start = *i as u32;
    if *i >= bytes.len() {
        return Err(LexError::Nonprogress { at: start });
    }

    match state.mode {
        CSharpMode::BlockComment => {
            while *i < bytes.len() {
                if bytes[*i] == b'*' && bytes.get(*i + 1) == Some(&b'/') {
                    *i += 2;
                    state.mode = CSharpMode::Normal;
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
            return Ok(CSharpToken {
                start,
                end: *i as u32,
                kind: CSharpKind::Comment,
            });
        }
        CSharpMode::RegularString
        | CSharpMode::Char
        | CSharpMode::Verbatim
        | CSharpMode::Interpolated
        | CSharpMode::InterpolatedVerbatim
        | CSharpMode::Raw => {
            let kind = if state.mode == CSharpMode::Char {
                CSharpKind::Char
            } else {
                CSharpKind::String
            };
            let closed = scan_string_body(bytes, i, state);
            if *i == start_i && *i < bytes.len() && !closed {
                // A newline recovered a line-bounded string without consuming it.
                if state.line_bounded() || matches!(state.mode, CSharpMode::Normal) {
                    // emit nothing; caller would loop. Force progress by
                    // treating leftover as the recovered empty remainder: the
                    // newline is not part of the string.
                    if is_newline(bytes[*i]) {
                        return Err(LexError::Nonprogress { at: start });
                    }
                    *i += 1;
                }
            }
            if *i as u32 <= start {
                // Recovered at a newline without consuming it: not a token.
                // The document lexer must emit the newline as whitespace, so
                // signal by producing a zero-width error the caller converts.
                return Err(LexError::Nonprogress { at: start });
            }
            *line_start = false;
            return Ok(CSharpToken {
                start,
                end: *i as u32,
                kind,
            });
        }
        CSharpMode::Normal => {}
    }

    let b = bytes[*i];
    if b == b'\r' {
        *i += 1;
        if bytes.get(*i) == Some(&b'\n') {
            *i += 1;
        }
        *line_start = true;
        return Ok(tok(start, *i, CSharpKind::Whitespace));
    }
    if b == b'\n' {
        *i += 1;
        *line_start = true;
        return Ok(tok(start, *i, CSharpKind::Whitespace));
    }
    if is_space(b) {
        *i += 1;
        while *i < bytes.len() && is_space(bytes[*i]) {
            *i += 1;
        }
        return Ok(tok(start, *i, CSharpKind::Whitespace));
    }
    if *line_start && b == b'#' {
        *i += 1;
        while *i < bytes.len() && !is_newline(bytes[*i]) {
            *i += 1;
        }
        *line_start = false;
        return Ok(tok(start, *i, CSharpKind::Preprocessor));
    }
    *line_start = false;
    if b == b'/' && bytes.get(*i + 1) == Some(&b'/') {
        *i += 2;
        while *i < bytes.len() && !is_newline(bytes[*i]) {
            *i += 1;
        }
        return Ok(tok(start, *i, CSharpKind::Comment));
    }
    if b == b'/' && bytes.get(*i + 1) == Some(&b'*') {
        *i += 2;
        state.mode = CSharpMode::BlockComment;
        while *i < bytes.len() {
            if bytes[*i] == b'*' && bytes.get(*i + 1) == Some(&b'/') {
                *i += 2;
                state.mode = CSharpMode::Normal;
                break;
            }
            *i += 1;
        }
        return Ok(tok(start, *i, CSharpKind::Comment));
    }
    if b == b'\'' {
        *i += 1;
        state.mode = CSharpMode::Char;
        let closed = scan_string_body(bytes, i, state);
        if *i as u32 <= start {
            if !closed && *i == start_i + 1 {
                // opening quote only, recovered at newline
                return Ok(tok(start, *i, CSharpKind::Char));
            }
            return Err(LexError::Nonprogress { at: start });
        }
        return Ok(tok(start, *i, CSharpKind::Char));
    }
    if let Some(open) = string_open(bytes, *i) {
        *i += open_len(&open);
        apply_open(state, &open);
        let _closed = scan_string_body(bytes, i, state);
        if *i as u32 <= start {
            return Err(LexError::Nonprogress { at: start });
        }
        return Ok(tok(start, *i, CSharpKind::String));
    }
    if is_ident_start(b) {
        *i += 1;
        while *i < bytes.len() && is_ident_continue(bytes[*i]) {
            *i += 1;
        }
        let ident = std::str::from_utf8(&bytes[start as usize..*i]).unwrap_or("");
        let kind = if ident.starts_with('@') {
            CSharpKind::Identifier
        } else if is_keyword(ident) {
            CSharpKind::Keyword
        } else {
            CSharpKind::Identifier
        };
        return Ok(tok(start, *i, kind));
    }
    if b.is_ascii_digit() || (b == b'.' && bytes.get(*i + 1).copied().unwrap_or(0).is_ascii_digit())
    {
        *i = scan_number(bytes, *i);
        return Ok(tok(start, *i, CSharpKind::Number));
    }
    let n = punct_len(bytes, *i);
    *i += n;
    let kind = if n == 1 && matches!(b, b'(' | b')' | b'{' | b'}' | b'[' | b']') {
        CSharpKind::Delimiter
    } else if b.is_ascii_graphic() {
        CSharpKind::Punctuator
    } else {
        CSharpKind::Unknown
    };
    if *i == start_i {
        return Err(LexError::Nonprogress { at: start });
    }
    Ok(tok(start, *i, kind))
}

fn tok(start: u32, end: usize, kind: CSharpKind) -> CSharpToken {
    CSharpToken {
        start,
        end: end as u32,
        kind,
    }
}

/// Tokenize an entire document. Spans are byte offsets into `bytes`.
pub fn lex_document_cancellable(
    bytes: &[u8],
    cancel: &dyn Fn() -> bool,
) -> Result<Vec<CSharpToken>, LexError> {
    let mut tokens = Vec::new();
    let mut state = CSharpState::default();
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
                // Line-bounded string recovered on a newline without a
                // remainder token. Emit the newline as whitespace.
                if i < bytes.len() && is_newline(bytes[i]) && i == start_i {
                    let start = i as u32;
                    i += 1;
                    if bytes.get(i - 1) == Some(&b'\r') && bytes.get(i) == Some(&b'\n') {
                        i += 1;
                    }
                    line_start = true;
                    push_token(&mut tokens, start, i, CSharpKind::Whitespace)?;
                    continue;
                }
                if i == start_i && i < bytes.len() {
                    i += 1;
                    push_token(&mut tokens, at, i, CSharpKind::Unknown)?;
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
pub fn lex_line(line: &str, incoming: CSharpState) -> (CSharpState, Vec<(usize, TokenRole)>) {
    let bytes = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    let mut state = incoming;
    let mut line_start = matches!(state.mode, CSharpMode::Normal);
    while i < bytes.len() {
        let start = i;
        match lex_one(bytes, &mut i, &mut state, &mut line_start) {
            Ok(t) => {
                let end = (t.end as usize).min(bytes.len()).max(start);
                if end > start {
                    let ident = if t.kind == CSharpKind::Keyword || t.kind == CSharpKind::Identifier
                    {
                        std::str::from_utf8(&bytes[t.start as usize..end]).unwrap_or("")
                    } else {
                        ""
                    };
                    let role = if t.kind == CSharpKind::Keyword || t.kind == CSharpKind::Identifier
                    {
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
    if matches!(
        state.mode,
        CSharpMode::RegularString | CSharpMode::Char
    ) && state.hole == 0
        && state.frames.is_empty()
    {
        // Regular strings and chars do not continue across lines.
        state.reset_string();
    }
    if i < bytes.len() {
        let role = match state.mode {
            CSharpMode::BlockComment => TokenRole::Comment,
            CSharpMode::Normal => TokenRole::Unknown,
            CSharpMode::Char => TokenRole::Char,
            _ => TokenRole::String,
        };
        push_role(bytes.len(), role, &mut out);
    }
    (state, out)
}

/// Convert document tokens into public spans, classifying keywords and
/// function names with one-token lookahead.
pub fn document_spans(bytes: &[u8], tokens: &[CSharpToken]) -> Vec<TokenSpan> {
    let mut out = Vec::with_capacity(tokens.len());
    for (i, t) in tokens.iter().enumerate() {
        let mut role = t.kind.role();
        if t.kind == CSharpKind::Keyword || t.kind == CSharpKind::Identifier {
            let mut j = i + 1;
            while j < tokens.len() && tokens[j].kind.role().is_trivia() {
                j += 1;
            }
            let next_is_paren = j < tokens.len()
                && tokens[j].kind == CSharpKind::Delimiter
                && bytes.get(tokens[j].start as usize) == Some(&b'(');
            let ident = std::str::from_utf8(&bytes[t.start as usize..t.end as usize]).unwrap_or("");
            role = classify_identifier(ident, next_is_paren);
        }
        out.push(TokenSpan::new(t.start, t.end, role));
    }
    out
}
