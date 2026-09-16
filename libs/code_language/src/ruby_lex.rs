//! Source-only Ruby lexer shared by the editor and the Ruby frontend.
//! Byte offsets address the original document. Continuation state is owned
//! here: `=begin`/`=end` comments, strings, percent literals, regex and one
//! pending heredoc survive line breaks. Source is never executed.

use crate::token::{TokenRole, TokenSpan};
use crate::LexError;

/// Version of this lexer; part of parse and search cache identity.
pub const RUBY_LEXER_VERSION: u32 = 1;

const HEREDOC_TAG_CAP: usize = 24;

/// Kind of a quote / percent / heredoc host.
const KIND_STRING: u8 = 0;
const KIND_REGEX: u8 = 1;
const KIND_SYMBOL: u8 = 2;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
struct StringHost {
    close: u8,
    open: u8,
    depth: u8,
    interpolate: bool,
    kind: u8,
    heredoc: bool,
    tag: [u8; HEREDOC_TAG_CAP],
    tag_len: u8,
    squiggly: bool,
    dash: bool,
}

impl StringHost {
    fn quote(close: u8, interpolate: bool, kind: u8) -> Self {
        StringHost {
            close,
            open: 0,
            depth: 1,
            interpolate,
            kind,
            heredoc: false,
            tag: [0; HEREDOC_TAG_CAP],
            tag_len: 0,
            squiggly: false,
            dash: false,
        }
    }

    fn percent(close: u8, open: u8, interpolate: bool, kind: u8) -> Self {
        StringHost {
            close,
            open,
            depth: 1,
            interpolate,
            kind,
            heredoc: false,
            tag: [0; HEREDOC_TAG_CAP],
            tag_len: 0,
            squiggly: false,
            dash: false,
        }
    }

    fn heredoc(
        tag: [u8; HEREDOC_TAG_CAP],
        tag_len: u8,
        squiggly: bool,
        dash: bool,
        interpolate: bool,
    ) -> Self {
        StringHost {
            close: 0,
            open: 0,
            depth: 1,
            interpolate,
            kind: KIND_STRING,
            heredoc: true,
            tag,
            tag_len,
            squiggly,
            dash,
        }
    }

    fn token_kind(self) -> RubyKind {
        match self.kind {
            KIND_REGEX => RubyKind::Regex,
            KIND_SYMBOL => RubyKind::Symbol,
            _ => RubyKind::String,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
enum RubyMode {
    #[default]
    Normal,
    BlockComment,
    String(StringHost),
    /// `#{ ... }` of an outer string (nest 1).
    Tpl {
        host: StringHost,
        braces: u8,
    },
    /// String nested in [`RubyMode::Tpl`].
    NestStr {
        inner: StringHost,
        host: StringHost,
        outer_braces: u8,
    },
    /// `#{ ... }` of a nested string. Strings inside this do not split on `#{`.
    Tpl2 {
        inner: StringHost,
        host: StringHost,
        braces: u8,
        outer_braces: u8,
    },
    /// String nested in [`RubyMode::Tpl2`]; `#{` stays in the string.
    DeepStr {
        inner: StringHost,
        nest: StringHost,
        host: StringHost,
        braces: u8,
        outer_braces: u8,
    },
    /// After `__END__`; remaining bytes are Comment.
    Data,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct RubyState {
    mode: RubyMode,
    pending_heredoc: bool,
    pending_tag: [u8; HEREDOC_TAG_CAP],
    pending_tag_len: u8,
    pending_squiggly: bool,
    pending_dash: bool,
    pending_interpolate: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RubyKind {
    Whitespace,
    Newline,
    Comment,
    Identifier,
    Constant,
    Variable,
    Symbol,
    Keyword,
    Number,
    String,
    Regex,
    Punctuator,
    Delimiter,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RubyToken {
    pub start: u32,
    pub end: u32,
    pub kind: RubyKind,
}

impl RubyKind {
    pub fn role(self) -> TokenRole {
        match self {
            RubyKind::Whitespace | RubyKind::Newline => TokenRole::Whitespace,
            RubyKind::Comment => TokenRole::Comment,
            RubyKind::Identifier | RubyKind::Variable => TokenRole::Identifier,
            RubyKind::Constant => TokenRole::Typename,
            RubyKind::Symbol => TokenRole::Constant,
            RubyKind::Keyword => TokenRole::Keyword,
            RubyKind::Number => TokenRole::Number,
            RubyKind::String | RubyKind::Regex => TokenRole::String,
            RubyKind::Punctuator => TokenRole::Punctuator,
            RubyKind::Delimiter => TokenRole::Delimiter,
            RubyKind::Unknown => TokenRole::Unknown,
        }
    }
}

/// MRI hard keywords plus `__method__` as specified. Sorted.
const KEYWORDS: &[&str] = &[
    "BEGIN",
    "END",
    "__ENCODING__",
    "__FILE__",
    "__LINE__",
    "__method__",
    "alias",
    "and",
    "begin",
    "break",
    "case",
    "class",
    "def",
    "defined?",
    "do",
    "else",
    "elsif",
    "end",
    "ensure",
    "false",
    "for",
    "if",
    "in",
    "module",
    "next",
    "nil",
    "not",
    "or",
    "redo",
    "rescue",
    "retry",
    "return",
    "self",
    "super",
    "then",
    "true",
    "undef",
    "unless",
    "until",
    "when",
    "while",
    "yield",
];

pub fn is_keyword(ident: &str) -> bool {
    KEYWORDS.binary_search(&ident).is_ok()
}

fn classify_identifier(ident: &str, next_is_call: bool) -> TokenRole {
    match ident {
        "if" | "elsif" | "else" | "unless" | "case" | "when" | "then" | "return" | "yield"
        | "begin" | "rescue" | "ensure" | "retry" | "raise" | "defined?" => TokenRole::BranchKeyword,
        "while" | "until" | "for" | "do" | "break" | "next" | "redo" | "loop" => {
            TokenRole::LoopKeyword
        }
        "true" | "false" | "nil" | "self" => TokenRole::Constant,
        other if is_keyword(other) => TokenRole::Keyword,
        _ if next_is_call => TokenRole::Function,
        _ => TokenRole::Identifier,
    }
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

fn is_constant_start(b: u8) -> bool {
    b.is_ascii_uppercase()
}

fn pair_close(open: u8) -> Option<u8> {
    match open {
        b'(' => Some(b')'),
        b'[' => Some(b']'),
        b'{' => Some(b'}'),
        b'<' => Some(b'>'),
        _ => None,
    }
}

fn is_percent_delim(b: u8) -> bool {
    !is_ident_continue(b) && !is_space(b) && !is_newline(b)
}

fn percent_kind(flag: u8) -> (bool, u8) {
    match flag {
        b'q' | b'w' | b'i' => (false, KIND_STRING),
        b's' => (false, KIND_SYMBOL),
        b'r' => (true, KIND_REGEX),
        b'Q' | b'W' | b'I' | b'x' => (true, KIND_STRING),
        _ => (true, KIND_STRING),
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

fn push_token(
    tokens: &mut Vec<RubyToken>,
    start: u32,
    end: usize,
    kind: RubyKind,
) -> Result<(), LexError> {
    if end as u32 <= start {
        return Err(LexError::Nonprogress { at: start });
    }
    tokens.push(RubyToken {
        start,
        end: end as u32,
        kind,
    });
    Ok(())
}

fn tok(start: u32, end: usize, kind: RubyKind) -> RubyToken {
    RubyToken {
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

fn ident_kind(first: u8) -> RubyKind {
    if is_constant_start(first) {
        RubyKind::Constant
    } else {
        RubyKind::Identifier
    }
}

fn scan_ident_end(bytes: &[u8], mut i: usize) -> usize {
    if i >= bytes.len() || !is_ident_start(bytes[i]) {
        return i;
    }
    i += 1;
    while i < bytes.len() && is_ident_continue(bytes[i]) {
        i += 1;
    }
    if i < bytes.len() && matches!(bytes[i], b'?' | b'!') {
        let next = bytes.get(i + 1).copied().unwrap_or(0);
        if next != b'=' && !is_ident_continue(next) {
            i += 1;
        }
    }
    i
}

fn keyword_or_ident(bytes: &[u8], start: usize, end: usize, after_dot: bool) -> RubyKind {
    if after_dot {
        return ident_kind(bytes[start]);
    }
    let ident = std::str::from_utf8(&bytes[start..end]).unwrap_or("");
    if is_keyword(ident) {
        RubyKind::Keyword
    } else {
        ident_kind(bytes[start])
    }
}

fn scan_number(bytes: &[u8], mut i: usize) -> usize {
    let start = i;
    if bytes[i] == b'0' {
        let tag = bytes.get(i + 1).map(|b| b.to_ascii_lowercase());
        if matches!(tag, Some(b'x' | b'b' | b'o')) {
            i += 2;
            while i < bytes.len() {
                let b = bytes[i];
                let ok = match tag {
                    Some(b'x') => b.is_ascii_hexdigit() || b == b'_',
                    Some(b'b') => b == b'0' || b == b'1' || b == b'_',
                    Some(b'o') => (b'0'..=b'7').contains(&b) || b == b'_',
                    _ => false,
                };
                if !ok {
                    break;
                }
                i += 1;
            }
            return consume_numeric_suffix(bytes, i.max(start + 1));
        }
        if bytes.get(i + 1).is_some_and(|b| (b'0'..=b'7').contains(b) || *b == b'_') {
            i += 1;
            while i < bytes.len() && ((b'0'..=b'7').contains(&bytes[i]) || bytes[i] == b'_') {
                i += 1;
            }
            return consume_numeric_suffix(bytes, i);
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
    consume_numeric_suffix(bytes, i.max(start + 1))
}

fn consume_numeric_suffix(bytes: &[u8], mut i: usize) -> usize {
    if matches!(bytes.get(i), Some(&b'r')) {
        i += 1;
    }
    if matches!(bytes.get(i), Some(&b'i')) {
        i += 1;
    }
    i
}

fn punct_len(bytes: &[u8], i: usize) -> usize {
    let rest = &bytes[i..];
    if rest.starts_with(b"**=")
        || rest.starts_with(b"<=>")
        || rest.starts_with(b"===")
        || rest.starts_with(b"<<=")
        || rest.starts_with(b">>=")
        || rest.starts_with(b"&&=")
        || rest.starts_with(b"||=")
        || rest.starts_with(b"...")
    {
        return 3;
    }
    if matches!(
        rest.get(..2),
        Some(
            b"=~" | b"!~"
                | b"**"
                | b"=="
                | b"!="
                | b"<="
                | b">="
                | b"<<"
                | b">>"
                | b"&&"
                | b"||"
                | b".."
                | b"::"
                | b"->"
                | b"=>"
                | b"+="
                | b"-="
                | b"*="
                | b"/="
                | b"%="
                | b"|="
                | b"&="
                | b"^="
                | b"&."
        )
    ) {
        return 2;
    }
    1
}

fn is_delim_byte(b: u8) -> bool {
    matches!(b, b'(' | b')' | b'{' | b'}' | b'[' | b']')
}

fn is_single_punct(b: u8) -> bool {
    matches!(
        b,
        b'=' | b'+'
            | b'-'
            | b'*'
            | b'/'
            | b'%'
            | b'&'
            | b'|'
            | b'^'
            | b'!'
            | b'~'
            | b'<'
            | b'>'
            | b'?'
            | b':'
            | b'.'
            | b','
            | b';'
            | b'\\'
    )
}

/// Previous significant token is an operand (value) rather than an operator
/// or expression opener. Line-local: the caller resets this at each newline.
#[derive(Clone, Copy, Debug, Default)]
struct LineCtx {
    at_line_start: bool,
    prev_operand: bool,
    prev_ident: bool,
    space_since_prev: bool,
    after_dot: bool,
}

fn note_emitted(ctx: &mut LineCtx, kind: RubyKind, bytes: &[u8], start: usize, end: usize) {
    ctx.at_line_start = false;
    ctx.space_since_prev = false;
    ctx.after_dot = false;
    match kind {
        RubyKind::Whitespace | RubyKind::Comment => {
            ctx.space_since_prev = true;
        }
        RubyKind::Newline => {
            ctx.at_line_start = true;
            ctx.prev_operand = false;
            ctx.prev_ident = false;
            ctx.space_since_prev = false;
            ctx.after_dot = false;
        }
        RubyKind::Identifier | RubyKind::Constant | RubyKind::Variable => {
            ctx.prev_operand = true;
            ctx.prev_ident = true;
        }
        RubyKind::Symbol | RubyKind::Number | RubyKind::String | RubyKind::Regex => {
            ctx.prev_operand = true;
            ctx.prev_ident = false;
        }
        RubyKind::Keyword => {
            let ident = std::str::from_utf8(&bytes[start..end]).unwrap_or("");
            let value = matches!(
                ident,
                "true"
                    | "false"
                    | "nil"
                    | "self"
                    | "end"
                    | "super"
                    | "__FILE__"
                    | "__LINE__"
                    | "__ENCODING__"
                    | "__method__"
            );
            ctx.prev_operand = value;
            ctx.prev_ident = value;
        }
        RubyKind::Delimiter => {
            let b = bytes.get(start).copied().unwrap_or(0);
            ctx.prev_operand = matches!(b, b')' | b']' | b'}');
            ctx.prev_ident = false;
            ctx.after_dot = false;
        }
        RubyKind::Punctuator => {
            let t = &bytes[start..end];
            ctx.after_dot = t == b"." || t == b"&.";
            ctx.prev_operand = false;
            ctx.prev_ident = false;
        }
        RubyKind::Unknown => {
            ctx.prev_operand = false;
            ctx.prev_ident = false;
        }
    }
}

fn slash_is_regex(bytes: &[u8], i: usize, ctx: &LineCtx) -> bool {
    if !ctx.prev_operand {
        return true;
    }
    if ctx.prev_ident && ctx.space_since_prev {
        let next = bytes.get(i + 1).copied().unwrap_or(0);
        if !is_space(next) && !is_newline(next) {
            return true;
        }
    }
    false
}

/// `%` is a percent literal, not modulo. After an identifier plus whitespace,
/// `%` immediately followed by a flag letter and a delimiter, or by
/// `( [ { < | !`, is a literal. A space after `%`, or no space before it,
/// stays modulo (`x % 3`, `x %3`, `x%3`).
fn percent_is_literal(bytes: &[u8], i: usize, ctx: &LineCtx) -> bool {
    if !ctx.prev_operand {
        return true;
    }
    if !(ctx.prev_ident && ctx.space_since_prev) {
        return false;
    }
    let next = bytes.get(i + 1).copied().unwrap_or(0);
    if is_space(next) || is_newline(next) {
        return false;
    }
    if matches!(
        next,
        b'q' | b'Q' | b'w' | b'W' | b'i' | b'I' | b's' | b'r' | b'x'
    ) {
        let delim = bytes.get(i + 2).copied().unwrap_or(0);
        return is_percent_delim(delim);
    }
    matches!(next, b'(' | b'[' | b'{' | b'<' | b'|' | b'!')
}

fn looks_like_char_literal(bytes: &[u8], i: usize, ctx: &LineCtx) -> bool {
    if ctx.prev_operand {
        return false;
    }
    if bytes.get(i) != Some(&b'?') {
        return false;
    }
    let next = bytes.get(i + 1).copied();
    match next {
        None => false,
        Some(b) if is_space(b) || is_newline(b) => false,
        Some(_) => true,
    }
}

fn scan_char_literal(bytes: &[u8], mut i: usize) -> usize {
    // `?` already at i.
    i += 1;
    if i >= bytes.len() {
        return i;
    }
    if bytes[i] == b'\\' {
        i += 1;
        if i < bytes.len() && !is_newline(bytes[i]) {
            i += 1;
            if bytes.get(i - 1) == Some(&b'M') || bytes.get(i - 1) == Some(&b'C') {
                if bytes.get(i) == Some(&b'-') {
                    i += 1;
                    if bytes.get(i) == Some(&b'\\') {
                        i += 1;
                    }
                    if i < bytes.len() && !is_newline(bytes[i]) {
                        i += 1;
                    }
                }
            }
        }
        return i;
    }
    i += 1;
    i
}

struct HeredocOpen {
    tag: [u8; HEREDOC_TAG_CAP],
    tag_len: u8,
    squiggly: bool,
    dash: bool,
    interpolate: bool,
    end: usize,
}

fn parse_heredoc_open(bytes: &[u8], i: usize, ctx: &LineCtx) -> Option<HeredocOpen> {
    if bytes.get(i) != Some(&b'<') || bytes.get(i + 1) != Some(&b'<') {
        return None;
    }
    if bytes.get(i + 2) == Some(&b'=') {
        return None;
    }
    let mut j = i + 2;
    let mut dash = false;
    let mut squiggly = false;
    if bytes.get(j) == Some(&b'-') {
        dash = true;
        j += 1;
    } else if bytes.get(j) == Some(&b'~') {
        squiggly = true;
        j += 1;
    }
    let quote = bytes.get(j).copied();
    let quoted = matches!(quote, Some(b'\'' | b'"' | b'`'));
    if ctx.prev_operand && bytes.get(i + 2).is_some_and(|b| is_space(*b)) {
        return None;
    }
    if quoted {
        let mut k = j + 1;
        let q = quote.unwrap();
        let tag_start = k;
        while k < bytes.len() && bytes[k] != q && !is_newline(bytes[k]) {
            k += 1;
        }
        if k >= bytes.len() || bytes[k] != q {
            return None;
        }
        let tag_bytes = &bytes[tag_start..k];
        if tag_bytes.is_empty() || tag_bytes.len() > HEREDOC_TAG_CAP {
            return None;
        }
        let mut tag = [0u8; HEREDOC_TAG_CAP];
        tag[..tag_bytes.len()].copy_from_slice(tag_bytes);
        let interpolate = q == b'"' || q == b'`';
        return Some(HeredocOpen {
            tag,
            tag_len: tag_bytes.len() as u8,
            squiggly,
            dash,
            interpolate,
            end: k + 1,
        });
    }
    if j >= bytes.len() || !is_ident_start(bytes[j]) {
        return None;
    }
    let lower = bytes[j].is_ascii_lowercase();
    if !dash && !squiggly && ctx.prev_operand && lower {
        return None;
    }
    let tag_start = j;
    let tag_end = scan_ident_end(bytes, j);
    let tag_bytes = &bytes[tag_start..tag_end];
    if tag_bytes.is_empty() || tag_bytes.len() > HEREDOC_TAG_CAP {
        return None;
    }
    let mut tag = [0u8; HEREDOC_TAG_CAP];
    tag[..tag_bytes.len()].copy_from_slice(tag_bytes);
    Some(HeredocOpen {
        tag,
        tag_len: tag_bytes.len() as u8,
        squiggly,
        dash,
        interpolate: true,
        end: tag_end,
    })
}

/// Emit the heredoc terminator line (leading whitespace included, newline
/// excluded) as a String token so highlighting and the frontend see it as
/// part of the literal.
fn emit_heredoc_terminator(
    bytes: &[u8],
    i: &mut usize,
    state: &mut RubyState,
    start: u32,
) -> Result<RubyToken, LexError> {
    while *i < bytes.len() && !is_newline(bytes[*i]) {
        *i += 1;
    }
    close_string_parent(state);
    if (*i as u32) <= start {
        return Err(LexError::Nonprogress { at: start });
    }
    Ok(tok(start, *i, RubyKind::String))
}

fn line_is_heredoc_end(bytes: &[u8], i: usize, host: &StringHost) -> bool {
    if !host.heredoc || host.tag_len == 0 {
        return false;
    }
    let mut j = i;
    if host.dash || host.squiggly {
        while j < bytes.len() && is_space(bytes[j]) {
            j += 1;
        }
    }
    let tag = &host.tag[..host.tag_len as usize];
    if j + tag.len() > bytes.len() {
        return false;
    }
    if &bytes[j..j + tag.len()] != tag {
        return false;
    }
    j += tag.len();
    j == bytes.len() || is_newline(bytes[j])
}

fn activate_pending(state: &mut RubyState) {
    if !state.pending_heredoc {
        return;
    }
    state.mode = RubyMode::String(StringHost::heredoc(
        state.pending_tag,
        state.pending_tag_len,
        state.pending_squiggly,
        state.pending_dash,
        state.pending_interpolate,
    ));
    state.pending_heredoc = false;
    state.pending_tag = [0; HEREDOC_TAG_CAP];
    state.pending_tag_len = 0;
}

fn set_pending(state: &mut RubyState, open: &HeredocOpen) {
    if state.pending_heredoc {
        return;
    }
    state.pending_heredoc = true;
    state.pending_tag = open.tag;
    state.pending_tag_len = open.tag_len;
    state.pending_squiggly = open.squiggly;
    state.pending_dash = open.dash;
    state.pending_interpolate = open.interpolate;
}

fn string_context(mode: RubyMode) -> Option<(StringHost, u8, bool)> {
    match mode {
        RubyMode::String(host) => Some((host, 0, true)),
        RubyMode::NestStr {
            inner,
            host: _,
            outer_braces: _,
        } => Some((inner, 1, true)),
        RubyMode::DeepStr { inner, .. } => Some((inner, 2, false)),
        _ => None,
    }
}

fn close_string_parent(state: &mut RubyState) {
    match state.mode {
        RubyMode::String(_) => state.mode = RubyMode::Normal,
        RubyMode::NestStr {
            host,
            outer_braces,
            ..
        } => {
            state.mode = RubyMode::Tpl {
                host,
                braces: outer_braces,
            };
        }
        RubyMode::DeepStr {
            nest,
            host,
            braces,
            outer_braces,
            ..
        } => {
            state.mode = RubyMode::Tpl2 {
                inner: nest,
                host,
                braces,
                outer_braces,
            };
        }
        _ => state.mode = RubyMode::Normal,
    }
}

fn open_template(state: &mut RubyState, host: StringHost, nest: u8) {
    match nest {
        0 => {
            state.mode = RubyMode::Tpl { host, braces: 0 };
        }
        _ => match state.mode {
            RubyMode::NestStr {
                inner,
                host: outer,
                outer_braces,
            } => {
                state.mode = RubyMode::Tpl2 {
                    inner,
                    host: outer,
                    braces: 0,
                    outer_braces,
                };
            }
            _ => {
                state.mode = RubyMode::Tpl { host, braces: 0 };
            }
        },
    }
}

fn enter_nested_string(state: &mut RubyState, inner: StringHost) {
    match state.mode {
        RubyMode::Tpl { host, braces } => {
            state.mode = RubyMode::NestStr {
                inner,
                host,
                outer_braces: braces,
            };
        }
        RubyMode::Tpl2 {
            inner: nest,
            host,
            braces,
            outer_braces,
        } => {
            state.mode = RubyMode::DeepStr {
                inner,
                nest,
                host,
                braces,
                outer_braces,
            };
        }
        _ => {
            state.mode = RubyMode::String(inner);
        }
    }
}

fn scan_string_body(
    bytes: &[u8],
    i: &mut usize,
    host: &mut StringHost,
    allow_interp: bool,
) -> StringStop {
    let start = *i;
    let mut in_class = false;
    while *i < bytes.len() {
        let b = bytes[*i];
        if host.heredoc && *i == start && line_is_heredoc_end(bytes, *i, host) {
            return StringStop::HeredocEnd;
        }
        if allow_interp && host.interpolate && b == b'#' && bytes.get(*i + 1) == Some(&b'{') {
            if *i > start {
                return StringStop::Content;
            }
            return StringStop::Interp;
        }
        if host.kind == KIND_REGEX && host.close == b'/' {
            if b == b'[' && !in_class {
                in_class = true;
                *i += 1;
                continue;
            }
            if b == b']' && in_class {
                in_class = false;
                *i += 1;
                continue;
            }
        }
        if b == b'\\' {
            *i += 1;
            if *i >= bytes.len() {
                continue;
            }
            if !host.interpolate && host.close == b'\'' {
                if matches!(bytes[*i], b'\\' | b'\'') {
                    *i += 1;
                }
                continue;
            }
            if is_newline(bytes[*i]) {
                consume_newline(bytes, i);
            } else {
                *i += 1;
            }
            continue;
        }
        if host.heredoc {
            if is_newline(b) {
                *i += 1;
                if bytes.get(*i - 1) == Some(&b'\r') && bytes.get(*i) == Some(&b'\n') {
                    *i += 1;
                }
                return StringStop::Content;
            }
            *i += 1;
            continue;
        }
        if host.open != 0 && b == host.open {
            host.depth = host.depth.saturating_add(1);
            *i += 1;
            continue;
        }
        if b == host.close {
            if host.open != 0 && host.depth > 1 {
                host.depth -= 1;
                *i += 1;
                continue;
            }
            return StringStop::Close;
        }
        *i += 1;
    }
    StringStop::Content
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum StringStop {
    Content,
    Close,
    Interp,
    HeredocEnd,
}

fn consume_regex_flags(bytes: &[u8], i: &mut usize) {
    while *i < bytes.len() {
        match bytes[*i] {
            b'i' | b'm' | b'o' | b'x' | b'u' | b'n' | b'e' => *i += 1,
            _ => break,
        }
    }
}

fn scan_operator_symbol(bytes: &[u8], i: usize) -> usize {
    let rest = &bytes[i..];
    if rest.starts_with(b"[]=") {
        return i + 3;
    }
    if rest.starts_with(b"[]") {
        return i + 2;
    }
    if rest.starts_with(b"+@") || rest.starts_with(b"-@") {
        return i + 2;
    }
    let n = punct_len(bytes, i);
    if n >= 1 && is_single_punct(bytes[i]) {
        return i + n;
    }
    i
}

fn line_starts_with(bytes: &[u8], i: usize, word: &[u8]) -> bool {
    if i + word.len() > bytes.len() {
        return false;
    }
    if &bytes[i..i + word.len()] != word {
        return false;
    }
    let after = bytes.get(i + word.len()).copied().unwrap_or(b'\n');
    is_space(after) || is_newline(after) || i + word.len() == bytes.len()
}

fn lex_one(
    bytes: &[u8],
    i: &mut usize,
    state: &mut RubyState,
    ctx: &mut LineCtx,
) -> Result<RubyToken, LexError> {
    if *i >= bytes.len() {
        return Err(LexError::Nonprogress { at: *i as u32 });
    }
    let start = *i as u32;
    match state.mode {
        RubyMode::Data => {
            *i = bytes.len();
            return Ok(tok(start, *i, RubyKind::Comment));
        }
        RubyMode::BlockComment => {
            while *i < bytes.len() {
                if ctx.at_line_start && line_starts_with(bytes, *i, b"=end") {
                    while *i < bytes.len() && !is_newline(bytes[*i]) {
                        *i += 1;
                    }
                    state.mode = RubyMode::Normal;
                    return Ok(tok(start, *i, RubyKind::Comment));
                }
                if is_newline(bytes[*i]) {
                    consume_newline(bytes, i);
                    ctx.at_line_start = true;
                    continue;
                }
                ctx.at_line_start = false;
                *i += 1;
            }
            return Ok(tok(start, *i, RubyKind::Comment));
        }
        RubyMode::Tpl { .. } | RubyMode::Tpl2 { .. } => {
            return lex_interp(bytes, i, state, ctx);
        }
        RubyMode::String(_) | RubyMode::NestStr { .. } | RubyMode::DeepStr { .. } => {
            return lex_in_string_from(bytes, i, state, *i as u32);
        }
        RubyMode::Normal => {}
    }

    let b = bytes[*i];
    if is_newline(b) {
        consume_newline(bytes, i);
        let t = tok(start, *i, RubyKind::Newline);
        note_emitted(ctx, RubyKind::Newline, bytes, start as usize, *i);
        activate_pending(state);
        return Ok(t);
    }
    if is_space(b) {
        while *i < bytes.len() && is_space(bytes[*i]) {
            *i += 1;
        }
        ctx.at_line_start = false;
        ctx.space_since_prev = true;
        return Ok(tok(start, *i, RubyKind::Whitespace));
    }

    if ctx.at_line_start && line_starts_with(bytes, *i, b"=begin") {
        state.mode = RubyMode::BlockComment;
        while *i < bytes.len() && !is_newline(bytes[*i]) {
            *i += 1;
        }
        return Ok(tok(start, *i, RubyKind::Comment));
    }
    if ctx.at_line_start && line_starts_with(bytes, *i, b"__END__") {
        let end = *i + 7;
        if end == bytes.len()
            || is_space(bytes[end])
            || is_newline(bytes[end])
        {
            *i = end;
            let t = tok(start, *i, RubyKind::Keyword);
            state.mode = RubyMode::Data;
            return Ok(t);
        }
    }

    if b == b'#' {
        while *i < bytes.len() && !is_newline(bytes[*i]) {
            *i += 1;
        }
        return Ok(tok(start, *i, RubyKind::Comment));
    }

    if looks_like_char_literal(bytes, *i, ctx) {
        *i = scan_char_literal(bytes, *i);
        return Ok(tok(start, *i, RubyKind::String));
    }

    if b == b'\'' || b == b'"' || b == b'`' {
        let interpolate = b != b'\'';
        let host = StringHost::quote(b, interpolate, KIND_STRING);
        let token_start = *i as u32;
        *i += 1;
        state.mode = RubyMode::String(host);
        return lex_in_string_from(bytes, i, state, token_start);
    }

    if b == b':' && bytes.get(*i + 1) != Some(&b':') {
        return lex_symbol(bytes, i, state);
    }

    if b == b'$' {
        return lex_global(bytes, i);
    }
    if b == b'@' {
        return lex_ivar(bytes, i);
    }

    if b == b'/' && slash_is_regex(bytes, *i, ctx) {
        let token_start = *i as u32;
        *i += 1;
        let host = StringHost::quote(b'/', true, KIND_REGEX);
        state.mode = RubyMode::String(host);
        return lex_in_string_from(bytes, i, state, token_start);
    }

    if b == b'%' && percent_is_literal(bytes, *i, ctx) {
        let token_start = *i as u32;
        if let Some(host) = parse_percent(bytes, i) {
            state.mode = RubyMode::String(host);
            return lex_in_string_from(bytes, i, state, token_start);
        }
    }

    if b == b'<' {
        if let Some(open) = parse_heredoc_open(bytes, *i, ctx) {
            *i = open.end;
            set_pending(state, &open);
            return Ok(tok(start, *i, RubyKind::Punctuator));
        }
    }

    if b.is_ascii_digit() {
        *i = scan_number(bytes, *i);
        return Ok(tok(start, *i, RubyKind::Number));
    }
    if b == b'.'
        && bytes.get(*i + 1).is_some_and(|d| d.is_ascii_digit())
        && !ctx.prev_operand
    {
        *i = scan_number(bytes, *i);
        return Ok(tok(start, *i, RubyKind::Number));
    }

    if is_ident_start(b) {
        let end = scan_ident_end(bytes, *i);
        let kind = keyword_or_ident(bytes, *i, end, ctx.after_dot);
        *i = end;
        if kind != RubyKind::Keyword
            && bytes.get(*i) == Some(&b':')
            && bytes.get(*i + 1) != Some(&b':')
        {
            let after = bytes.get(*i + 1).copied().unwrap_or(b'\n');
            if is_space(after) || is_newline(after) || *i + 1 == bytes.len() {
                *i += 1;
                return Ok(tok(start, *i, RubyKind::Symbol));
            }
        }
        return Ok(tok(start, *i, kind));
    }

    if is_delim_byte(b) {
        *i += 1;
        return Ok(tok(start, *i, RubyKind::Delimiter));
    }

    if is_single_punct(b) {
        let n = punct_len(bytes, *i);
        *i += n;
        return Ok(tok(start, *i, RubyKind::Punctuator));
    }

    *i += 1;
    Ok(tok(start, *i, RubyKind::Unknown))
}

fn parse_percent(bytes: &[u8], i: &mut usize) -> Option<StringHost> {
    let start = *i;
    if bytes.get(start) != Some(&b'%') {
        return None;
    }
    let mut j = start + 1;
    if j >= bytes.len() {
        return None;
    }
    let mut flag = 0u8;
    if matches!(
        bytes[j],
        b'q' | b'Q' | b'w' | b'W' | b'i' | b'I' | b's' | b'r' | b'x'
    ) {
        flag = bytes[j];
        j += 1;
    }
    if j >= bytes.len() || !is_percent_delim(bytes[j]) {
        return None;
    }
    let delim = bytes[j];
    j += 1;
    let (interpolate, kind) = if flag == 0 {
        (true, KIND_STRING)
    } else {
        percent_kind(flag)
    };
    let (open, close) = match pair_close(delim) {
        Some(c) => (delim, c),
        None => (0u8, delim),
    };
    *i = j;
    Some(StringHost::percent(close, open, interpolate, kind))
}

fn lex_symbol(
    bytes: &[u8],
    i: &mut usize,
    state: &mut RubyState,
) -> Result<RubyToken, LexError> {
    let start = *i as u32;
    *i += 1;
    if *i >= bytes.len() {
        return Ok(tok(start, *i, RubyKind::Punctuator));
    }
    let b = bytes[*i];
    if b == b'\'' || b == b'"' {
        let interpolate = b == b'"';
        *i += 1;
        state.mode = RubyMode::String(StringHost::quote(b, interpolate, KIND_SYMBOL));
        return lex_in_string_from(bytes, i, state, start);
    }
    if is_ident_start(b) {
        *i = scan_ident_end(bytes, *i);
        return Ok(tok(start, *i, RubyKind::Symbol));
    }
    let end = scan_operator_symbol(bytes, *i);
    if end > *i {
        *i = end;
        return Ok(tok(start, *i, RubyKind::Symbol));
    }
    Ok(tok(start, *i, RubyKind::Punctuator))
}

fn lex_global(bytes: &[u8], i: &mut usize) -> Result<RubyToken, LexError> {
    let start = *i as u32;
    *i += 1;
    if *i >= bytes.len() {
        return Ok(tok(start, *i, RubyKind::Unknown));
    }
    let b = bytes[*i];
    if b.is_ascii_digit() {
        while *i < bytes.len() && bytes[*i].is_ascii_digit() {
            *i += 1;
        }
        return Ok(tok(start, *i, RubyKind::Variable));
    }
    if is_ident_start(b) {
        *i = scan_ident_end(bytes, *i);
        return Ok(tok(start, *i, RubyKind::Variable));
    }
    if b == b'-' && bytes.get(*i + 1).is_some_and(|c| c.is_ascii_alphabetic()) {
        *i += 2;
        return Ok(tok(start, *i, RubyKind::Variable));
    }
    if matches!(
        b,
        b'!' | b'@'
            | b'&'
            | b'`'
            | b'\''
            | b'+'
            | b'~'
            | b'='
            | b'/'
            | b'\\'
            | b','
            | b';'
            | b'.'
            | b'<'
            | b'>'
            | b'*'
            | b'?'
            | b'$'
            | b':'
            | b'"'
    ) {
        *i += 1;
        return Ok(tok(start, *i, RubyKind::Variable));
    }
    Ok(tok(start, *i, RubyKind::Unknown))
}

fn lex_ivar(bytes: &[u8], i: &mut usize) -> Result<RubyToken, LexError> {
    let start = *i as u32;
    *i += 1;
    if bytes.get(*i) == Some(&b'@') {
        *i += 1;
    }
    if *i < bytes.len() && is_ident_start(bytes[*i]) {
        *i = scan_ident_end(bytes, *i);
        return Ok(tok(start, *i, RubyKind::Variable));
    }
    Ok(tok(start, *i, RubyKind::Unknown))
}

fn lex_in_string_from(
    bytes: &[u8],
    i: &mut usize,
    state: &mut RubyState,
    start: u32,
) -> Result<RubyToken, LexError> {
    let Some((mut host, nest, allow_interp)) = string_context(state.mode) else {
        return Err(LexError::Nonprogress { at: start });
    };
    if *i >= bytes.len() {
        if (*i as u32) > start {
            return Ok(tok(start, *i, host.token_kind()));
        }
        return Err(LexError::Nonprogress { at: start });
    }
    if allow_interp && host.interpolate && bytes[*i] == b'#' && bytes.get(*i + 1) == Some(&b'{') {
        if start < *i as u32 {
            return Ok(tok(start, *i, host.token_kind()));
        }
        *i += 2;
        open_template(state, host, nest);
        return Ok(tok(start, *i, RubyKind::Punctuator));
    }
    if !host.heredoc && bytes.get(*i) == Some(&host.close) && (host.open == 0 || host.depth <= 1) {
        *i += 1;
        if host.kind == KIND_REGEX && host.close == b'/' {
            consume_regex_flags(bytes, i);
        }
        close_string_parent(state);
        return Ok(tok(start, *i, host.token_kind()));
    }
    if host.heredoc && line_is_heredoc_end(bytes, *i, &host) {
        return emit_heredoc_terminator(bytes, i, state, start);
    }
    let stop = scan_string_body(bytes, i, &mut host, allow_interp);
    match state.mode {
        RubyMode::String(_) => state.mode = RubyMode::String(host),
        RubyMode::NestStr {
            host: outer,
            outer_braces,
            ..
        } => {
            state.mode = RubyMode::NestStr {
                inner: host,
                host: outer,
                outer_braces,
            };
        }
        RubyMode::DeepStr {
            nest,
            host: outer,
            braces,
            outer_braces,
            ..
        } => {
            state.mode = RubyMode::DeepStr {
                inner: host,
                nest,
                host: outer,
                braces,
                outer_braces,
            };
        }
        other => state.mode = other,
    }
    match stop {
        StringStop::Interp => {
            if *i as u32 > start {
                return Ok(tok(start, *i, host.token_kind()));
            }
            *i += 2;
            open_template(state, host, nest);
            Ok(tok(start, *i, RubyKind::Punctuator))
        }
        StringStop::Close => {
            *i += 1;
            if host.kind == KIND_REGEX && host.close == b'/' {
                consume_regex_flags(bytes, i);
            }
            close_string_parent(state);
            if (*i as u32) <= start {
                return Err(LexError::Nonprogress { at: start });
            }
            Ok(tok(start, *i, host.token_kind()))
        }
        StringStop::HeredocEnd => {
            if *i as u32 > start {
                return Ok(tok(start, *i, host.token_kind()));
            }
            emit_heredoc_terminator(bytes, i, state, start)
        }
        StringStop::Content => {
            if *i as u32 <= start {
                if *i < bytes.len() {
                    *i += 1;
                    return Ok(tok(start, *i, host.token_kind()));
                }
                // Unclosed opener at EOF: emit the opener as a String/Regex/Symbol
                // token instead of Nonprogress so the document still lexes.
                close_string_parent(state);
                if start < bytes.len() as u32 {
                    *i = bytes.len();
                    return Ok(tok(start, *i, host.token_kind()));
                }
                return Err(LexError::Nonprogress { at: start });
            }
            Ok(tok(start, *i, host.token_kind()))
        }
    }
}

fn lex_interp(
    bytes: &[u8],
    i: &mut usize,
    state: &mut RubyState,
    ctx: &mut LineCtx,
) -> Result<RubyToken, LexError> {
    if *i >= bytes.len() {
        return Err(LexError::Nonprogress { at: *i as u32 });
    }
    let b = bytes[*i];
    if b == b'}' {
        let braces = match state.mode {
            RubyMode::Tpl { braces, .. } | RubyMode::Tpl2 { braces, .. } => braces,
            _ => 0,
        };
        if braces == 0 {
            let start = *i as u32;
            *i += 1;
            match state.mode {
                RubyMode::Tpl { host, .. } => state.mode = RubyMode::String(host),
                RubyMode::Tpl2 {
                    inner,
                    host,
                    outer_braces,
                    ..
                } => {
                    state.mode = RubyMode::NestStr {
                        inner,
                        host,
                        outer_braces,
                    };
                }
                other => state.mode = other,
            }
            return Ok(tok(start, *i, RubyKind::Punctuator));
        }
    }
    if b == b'{' {
        match &mut state.mode {
            RubyMode::Tpl { braces, .. } | RubyMode::Tpl2 { braces, .. } => {
                *braces = braces.saturating_add(1);
            }
            _ => {}
        }
        let start = *i as u32;
        *i += 1;
        return Ok(tok(start, *i, RubyKind::Delimiter));
    }
    if b == b'}' {
        match &mut state.mode {
            RubyMode::Tpl { braces, .. } | RubyMode::Tpl2 { braces, .. } => {
                *braces = braces.saturating_sub(1);
            }
            _ => {}
        }
        let start = *i as u32;
        *i += 1;
        return Ok(tok(start, *i, RubyKind::Delimiter));
    }
    if matches!(b, b'\'' | b'"' | b'`') {
        let interpolate = b != b'\'';
        let inner = StringHost::quote(b, interpolate, KIND_STRING);
        let start = *i as u32;
        *i += 1;
        enter_nested_string(state, inner);
        return lex_in_string_from(bytes, i, state, start);
    }
    if b == b'%' && percent_is_literal(bytes, *i, ctx) {
        let save = *i;
        if let Some(inner) = parse_percent(bytes, i) {
            enter_nested_string(state, inner);
            return lex_in_string_from(bytes, i, state, save as u32);
        }
        *i = save;
    }
    let saved = state.mode;
    state.mode = RubyMode::Normal;
    let t = lex_one(bytes, i, state, ctx)?;
    match state.mode {
        RubyMode::Normal => state.mode = saved,
        RubyMode::String(inner) => {
            state.mode = saved;
            enter_nested_string(state, inner);
        }
        _ => {}
    }
    Ok(t)
}

/// Tokenize an entire document. Spans are byte offsets into `bytes`.
pub fn lex_document_cancellable(
    bytes: &[u8],
    cancel: &dyn Fn() -> bool,
) -> Result<Vec<RubyToken>, LexError> {
    let mut tokens = Vec::new();
    let mut state = RubyState::default();
    let mut ctx = LineCtx {
        at_line_start: true,
        ..LineCtx::default()
    };
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
        match lex_one(bytes, &mut i, &mut state, &mut ctx) {
            Ok(t) => {
                if t.end <= t.start {
                    return Err(LexError::Nonprogress { at: t.start });
                }
                note_emitted(&mut ctx, t.kind, bytes, t.start as usize, t.end as usize);
                tokens.push(t);
            }
            Err(LexError::Nonprogress { at }) => {
                if i == start_i && i < bytes.len() {
                    i += 1;
                    push_token(&mut tokens, at, i, RubyKind::Unknown)?;
                    ctx.at_line_start = false;
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
    bytes.get(k) == Some(&b'(')
}

/// Tokenize one display line (without the newline) given incoming continuation.
/// Each pair is `(end_index, role)` covering `[prev_end, end)` of `line`.
pub fn lex_line(line: &str, incoming: RubyState) -> (RubyState, Vec<(usize, TokenRole)>) {
    let bytes = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    let mut state = incoming;
    let mut ctx = LineCtx {
        at_line_start: matches!(
            state.mode,
            RubyMode::Normal | RubyMode::BlockComment | RubyMode::Data
        ),
        ..LineCtx::default()
    };
    while i < bytes.len() {
        let start = i;
        match lex_one(bytes, &mut i, &mut state, &mut ctx) {
            Ok(t) => {
                let end = (t.end as usize).min(bytes.len()).max(start);
                if end > start {
                    let role = match t.kind {
                        RubyKind::Keyword | RubyKind::Identifier | RubyKind::Constant => {
                            let ident =
                                std::str::from_utf8(&bytes[t.start as usize..end]).unwrap_or("");
                            if t.kind == RubyKind::Constant {
                                TokenRole::Typename
                            } else {
                                classify_identifier(ident, next_is_call_bytes(bytes, end))
                            }
                        }
                        RubyKind::Symbol => TokenRole::Constant,
                        _ => t.kind.role(),
                    };
                    push_role(end, role, &mut out);
                }
                note_emitted(&mut ctx, t.kind, bytes, t.start as usize, end);
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
            RubyMode::BlockComment | RubyMode::Data => TokenRole::Comment,
            RubyMode::String(h) | RubyMode::NestStr { inner: h, .. } | RubyMode::DeepStr { inner: h, .. } => {
                match h.kind {
                    KIND_REGEX => TokenRole::String,
                    KIND_SYMBOL => TokenRole::Constant,
                    _ => TokenRole::String,
                }
            }
            RubyMode::Normal | RubyMode::Tpl { .. } | RubyMode::Tpl2 { .. } => TokenRole::Unknown,
        };
        push_role(bytes.len(), role, &mut out);
    }
    (state, out)
}

fn same_line(tokens: &[RubyToken], a: usize, b: usize) -> bool {
    tokens[a + 1..b]
        .iter()
        .all(|t| t.kind != RubyKind::Newline)
}

/// Convert document tokens into public spans, classifying keywords and
/// function names with one-token lookahead.
pub fn document_spans(bytes: &[u8], tokens: &[RubyToken]) -> Vec<TokenSpan> {
    let mut out = Vec::with_capacity(tokens.len());
    for (i, t) in tokens.iter().enumerate() {
        let mut role = t.kind.role();
        match t.kind {
            RubyKind::Keyword | RubyKind::Identifier => {
                let mut j = i + 1;
                while j < tokens.len()
                    && matches!(tokens[j].kind, RubyKind::Whitespace | RubyKind::Comment)
                {
                    j += 1;
                }
                let next_is_call = j < tokens.len()
                    && tokens[j].kind == RubyKind::Delimiter
                    && bytes.get(tokens[j].start as usize) == Some(&b'(')
                    && same_line(tokens, i, j);
                let ident =
                    std::str::from_utf8(&bytes[t.start as usize..t.end as usize]).unwrap_or("");
                role = classify_identifier(ident, next_is_call);
            }
            RubyKind::Constant => role = TokenRole::Typename,
            RubyKind::Symbol => role = TokenRole::Constant,
            _ => {}
        }
        out.push(TokenSpan::new(t.start, t.end, role));
    }
    out
}
