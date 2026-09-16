//! HTML tokeniser shared by the editor and the HTML frontend.
//! Byte offsets address the original document. Raw `<script>` / `<style>`
//! regions are recorded so highlighting can embed the JS and CSS lexers.

use crate::css_lex::{self, CssState};
use crate::script_lex::{self, ScriptDialect, ScriptState};
use crate::token::{TokenRole, TokenSpan};
use crate::LexError;

/// Version of this lexer; part of parse and search cache identity.
/// Stays 1: Html dialect output is byte-identical to the previous Html-only
/// lexer. Xml and Svg select raw-text elements at `finish_start_tag` only.
pub const HTML_LEXER_VERSION: u32 = 1;

/// Markup family sharing this tokeniser. Html is the default so existing
/// `.html` / `.htm` / `.xhtml` callers keep the previous raw-text rules.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum MarkupDialect {
    #[default]
    Html,
    Xml,
    Svg,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
enum HtmlMode {
    #[default]
    Data,
    Tag,
    AttrValue,
    Comment,
    Doctype,
    Cdata,
    Pi,
    Raw,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
enum TagHint {
    #[default]
    None,
    Script,
    Style,
    Textarea,
    Title,
    Other,
}

/// Provider-owned line continuation. Default is Data in the Html dialect.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct HtmlState {
    mode: HtmlMode,
    /// Quote for `AttrValue`: `"` or `'`; 0 means unquoted.
    attr_quote: u8,
    is_end_tag: bool,
    seen_name: bool,
    expect_value: bool,
    tag: TagHint,
    script_data: bool,
    attr_is_type: bool,
    raw: Option<RawKind>,
    tag_open_start: u32,
    raw_content_start: u32,
    script: ScriptState,
    css: CssState,
    dialect: MarkupDialect,
}

impl Default for HtmlState {
    fn default() -> Self {
        HtmlState {
            mode: HtmlMode::Data,
            attr_quote: 0,
            is_end_tag: false,
            seen_name: false,
            expect_value: false,
            tag: TagHint::None,
            script_data: false,
            attr_is_type: false,
            raw: None,
            tag_open_start: 0,
            raw_content_start: 0,
            script: ScriptState::default(),
            css: CssState::default(),
            dialect: MarkupDialect::Html,
        }
    }
}

impl HtmlState {
    pub fn for_dialect(dialect: MarkupDialect) -> Self {
        let mut state = Self::default();
        state.dialect = dialect;
        state
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HtmlKind {
    Text,
    Whitespace,
    Comment,
    Doctype,
    TagOpen,
    TagName,
    TagEnd,
    AttrName,
    AttrEq,
    AttrValue,
    Cdata,
    ProcessingInstruction,
    RawText,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HtmlToken {
    pub start: u32,
    pub end: u32,
    pub kind: HtmlKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RawKind {
    Style,
    Script,
    ScriptData,
    Textarea,
    Title,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RawRegion {
    pub start: u32,
    pub end: u32,
    pub kind: RawKind,
    pub open_tag: (u32, u32),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HtmlLexed {
    pub tokens: Vec<HtmlToken>,
    pub regions: Vec<RawRegion>,
}

impl HtmlKind {
    pub fn role(self) -> TokenRole {
        match self {
            HtmlKind::Whitespace => TokenRole::Whitespace,
            HtmlKind::Comment => TokenRole::Comment,
            HtmlKind::Doctype | HtmlKind::Cdata | HtmlKind::ProcessingInstruction => {
                TokenRole::Preprocessor
            }
            HtmlKind::TagName => TokenRole::Keyword,
            HtmlKind::TagOpen | HtmlKind::TagEnd => TokenRole::Delimiter,
            HtmlKind::AttrEq => TokenRole::Punctuator,
            HtmlKind::AttrName => TokenRole::Identifier,
            HtmlKind::AttrValue => TokenRole::String,
            HtmlKind::Text | HtmlKind::RawText | HtmlKind::Unknown => TokenRole::Unknown,
        }
    }
}

pub fn lex_document_cancellable(
    bytes: &[u8],
    cancel: &dyn Fn() -> bool,
) -> Result<HtmlLexed, LexError> {
    lex_document_cancellable_dialect(bytes, MarkupDialect::Html, cancel)
}

pub fn lex_document_cancellable_dialect(
    bytes: &[u8],
    dialect: MarkupDialect,
    cancel: &dyn Fn() -> bool,
) -> Result<HtmlLexed, LexError> {
    let mut tokens = Vec::new();
    let mut regions = Vec::new();
    let mut state = HtmlState::for_dialect(dialect);
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
        let start = i as u32;
        match state.mode {
            HtmlMode::Comment => {
                while i < bytes.len() {
                    if i & 255 == 0 && cancel() {
                        return Err(LexError::Cancelled);
                    }
                    if bytes[i..].starts_with(b"-->") {
                        i += 3;
                        state.mode = HtmlMode::Data;
                        break;
                    }
                    i += 1;
                }
                push_token(&mut tokens, start, i, HtmlKind::Comment)?;
            }
            HtmlMode::Doctype => {
                while i < bytes.len() && bytes[i] != b'>' {
                    if i & 255 == 0 && cancel() {
                        return Err(LexError::Cancelled);
                    }
                    i += 1;
                }
                if bytes.get(i) == Some(&b'>') {
                    i += 1;
                    state.mode = HtmlMode::Data;
                }
                push_token(&mut tokens, start, i, HtmlKind::Doctype)?;
            }
            HtmlMode::Cdata => {
                while i < bytes.len() {
                    if i & 255 == 0 && cancel() {
                        return Err(LexError::Cancelled);
                    }
                    if bytes[i..].starts_with(b"]]>") {
                        i += 3;
                        state.mode = HtmlMode::Data;
                        break;
                    }
                    i += 1;
                }
                push_token(&mut tokens, start, i, HtmlKind::Cdata)?;
            }
            HtmlMode::Pi => {
                while i < bytes.len() {
                    if i & 255 == 0 && cancel() {
                        return Err(LexError::Cancelled);
                    }
                    if bytes[i] == b'?' && bytes.get(i + 1) == Some(&b'>') {
                        i += 2;
                        state.mode = HtmlMode::Data;
                        break;
                    }
                    if bytes[i] == b'>' {
                        i += 1;
                        state.mode = HtmlMode::Data;
                        break;
                    }
                    i += 1;
                }
                push_token(&mut tokens, start, i, HtmlKind::ProcessingInstruction)?;
            }
            HtmlMode::AttrValue => {
                consume_attr_value_doc(bytes, &mut i, &mut state, start, &mut tokens, cancel)?;
            }
            HtmlMode::Raw => {
                consume_raw_doc(bytes, &mut i, &mut state, start, &mut tokens, &mut regions, cancel)?;
            }
            HtmlMode::Tag => {
                consume_tag_doc(bytes, &mut i, &mut state, start, &mut tokens, cancel)?;
            }
            HtmlMode::Data => {
                consume_data_doc(bytes, &mut i, &mut state, start, &mut tokens, cancel)?;
            }
        }
        if i == start_i {
            return Err(LexError::Nonprogress { at: start });
        }
    }
    if state.mode == HtmlMode::Raw {
        if let Some(kind) = state.raw {
            let start = state.raw_content_start;
            let end = bytes.len() as u32;
            if end > start {
                // Unclosed raw region already emitted as we consumed to EOF in the loop.
                // If we exited because i==len without emitting, record the region.
                if regions.last().map(|r| r.start) != Some(start) {
                    regions.push(RawRegion {
                        start,
                        end,
                        kind,
                        open_tag: (state.tag_open_start, start),
                    });
                }
            }
        }
    }
    Ok(HtmlLexed { tokens, regions })
}

pub fn lex_line(line: &str, incoming: HtmlState) -> (HtmlState, Vec<(usize, TokenRole)>) {
    let bytes = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    let mut state = incoming;
    while i < bytes.len() {
        let start = i;
        match state.mode {
            HtmlMode::Comment => {
                if let Some(rel) = find_sub(bytes, i, b"-->") {
                    i = rel + 3;
                    push_role(i, TokenRole::Comment, &mut out);
                    state.mode = HtmlMode::Data;
                } else {
                    push_role(bytes.len(), TokenRole::Comment, &mut out);
                    i = bytes.len();
                }
            }
            HtmlMode::Doctype => {
                if let Some(rel) = find_byte(bytes, i, b'>') {
                    i = rel + 1;
                    push_role(i, TokenRole::Preprocessor, &mut out);
                    state.mode = HtmlMode::Data;
                } else {
                    push_role(bytes.len(), TokenRole::Preprocessor, &mut out);
                    i = bytes.len();
                }
            }
            HtmlMode::Cdata => {
                if let Some(rel) = find_sub(bytes, i, b"]]>") {
                    i = rel + 3;
                    push_role(i, TokenRole::Preprocessor, &mut out);
                    state.mode = HtmlMode::Data;
                } else {
                    push_role(bytes.len(), TokenRole::Preprocessor, &mut out);
                    i = bytes.len();
                }
            }
            HtmlMode::Pi => {
                if let Some(rel) = find_pi_end(bytes, i) {
                    i = rel;
                    push_role(i, TokenRole::Preprocessor, &mut out);
                    state.mode = HtmlMode::Data;
                } else {
                    push_role(bytes.len(), TokenRole::Preprocessor, &mut out);
                    i = bytes.len();
                }
            }
            HtmlMode::AttrValue => {
                consume_attr_value_line(bytes, &mut i, &mut state, &mut out);
            }
            HtmlMode::Raw => {
                consume_raw_line(bytes, &mut i, &mut state, &mut out);
            }
            HtmlMode::Tag => {
                consume_tag_line(bytes, &mut i, &mut state, &mut out);
            }
            HtmlMode::Data => {
                consume_data_line(bytes, &mut i, &mut state, &mut out);
            }
        }
        if i == start && i < bytes.len() {
            i += 1;
            push_role(i, TokenRole::Unknown, &mut out);
        }
    }
    (state, out)
}

pub fn document_spans(bytes: &[u8], lexed: &HtmlLexed) -> Vec<TokenSpan> {
    let mut spans: Vec<TokenSpan> = lexed
        .tokens
        .iter()
        .map(|t| TokenSpan::new(t.start, t.end, t.kind.role()))
        .collect();
    for region in &lexed.regions {
        if region.end <= region.start {
            continue;
        }
        match region.kind {
            RawKind::Style => splice_css(bytes, &mut spans, *region),
            RawKind::Script => splice_script(bytes, &mut spans, *region),
            RawKind::ScriptData => replace_range(&mut spans, region.start, region.end, TokenRole::Unknown),
            RawKind::Textarea | RawKind::Title => {
                replace_range(&mut spans, region.start, region.end, TokenRole::Unknown);
            }
        }
    }
    spans
}

/// Decode a handful of HTML attribute entities. Other named entities are Err
/// so the caller can mark the value unsupported instead of guessing.
pub fn decode_attribute(raw: &str) -> Result<String, ()> {
    let bytes = raw.as_bytes();
    if !bytes.contains(&b'&') {
        return Ok(raw.to_string());
    }
    let mut out = String::with_capacity(raw.len());
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] != b'&' {
            let ch = raw[i..].chars().next().ok_or(())?;
            out.push(ch);
            i += ch.len_utf8();
            continue;
        }
        let rest = &raw[i..];
        if rest.len() >= 5 && rest[..5].eq_ignore_ascii_case("&amp;") {
            out.push('&');
            i += 5;
            continue;
        }
        if rest.len() >= 4 && rest[..4].eq_ignore_ascii_case("&lt;") {
            out.push('<');
            i += 4;
            continue;
        }
        if rest.len() >= 4 && rest[..4].eq_ignore_ascii_case("&gt;") {
            out.push('>');
            i += 4;
            continue;
        }
        if rest.len() >= 6 && rest[..6].eq_ignore_ascii_case("&quot;") {
            out.push('"');
            i += 6;
            continue;
        }
        if rest.len() >= 6 && rest[..6].eq_ignore_ascii_case("&apos;") {
            out.push('\'');
            i += 6;
            continue;
        }
        if rest.starts_with("&#x") || rest.starts_with("&#X") {
            let restb = rest.as_bytes();
            let mut j = 3usize;
            while j < restb.len() && restb[j].is_ascii_hexdigit() {
                j += 1;
            }
            if j > 3 && j < restb.len() && restb[j] == b';' {
                let code = u32::from_str_radix(&rest[3..j], 16).map_err(|_| ())?;
                let ch = char::from_u32(code).ok_or(())?;
                out.push(ch);
                i += j + 1;
                continue;
            }
            return Err(());
        }
        if rest.starts_with("&#") {
            let restb = rest.as_bytes();
            let mut j = 2usize;
            while j < restb.len() && restb[j].is_ascii_digit() {
                j += 1;
            }
            if j > 2 && j < restb.len() && restb[j] == b';' {
                let code = rest[2..j].parse::<u32>().map_err(|_| ())?;
                let ch = char::from_u32(code).ok_or(())?;
                out.push(ch);
                i += j + 1;
                continue;
            }
            return Err(());
        }
        if rest.len() > 1 && rest.as_bytes()[1].is_ascii_alphabetic() {
            let restb = rest.as_bytes();
            let mut j = 1usize;
            while j < restb.len() && restb[j].is_ascii_alphanumeric() {
                j += 1;
            }
            if j < restb.len() && restb[j] == b';' {
                return Err(());
            }
        }
        out.push('&');
        i += 1;
    }
    Ok(out)
}

fn consume_data_doc(
    bytes: &[u8],
    i: &mut usize,
    state: &mut HtmlState,
    start: u32,
    tokens: &mut Vec<HtmlToken>,
    cancel: &dyn Fn() -> bool,
) -> Result<(), LexError> {
    let b = bytes[*i];
    if b == b'\r' {
        *i += 1;
        if bytes.get(*i) == Some(&b'\n') {
            *i += 1;
        }
        return push_token(tokens, start, *i, HtmlKind::Whitespace);
    }
    if b == b'\n' {
        *i += 1;
        return push_token(tokens, start, *i, HtmlKind::Whitespace);
    }
    if is_html_space(b) {
        *i += 1;
        while *i < bytes.len() && is_html_space(bytes[*i]) {
            *i += 1;
        }
        return push_token(tokens, start, *i, HtmlKind::Whitespace);
    }
    if b == b'<' {
        return open_markup_doc(bytes, i, state, start, tokens, cancel);
    }
    *i += 1;
    while *i < bytes.len() {
        let c = bytes[*i];
        if c == b'<' || c == b'\n' || c == b'\r' || is_html_space(c) {
            break;
        }
        *i += 1;
    }
    push_token(tokens, start, *i, HtmlKind::Text)
}

fn open_markup_doc(
    bytes: &[u8],
    i: &mut usize,
    state: &mut HtmlState,
    start: u32,
    tokens: &mut Vec<HtmlToken>,
    cancel: &dyn Fn() -> bool,
) -> Result<(), LexError> {
    let rest = &bytes[*i..];
    if rest.starts_with(b"<!--") {
        *i += 4;
        state.mode = HtmlMode::Comment;
        while *i < bytes.len() {
            if *i & 255 == 0 && cancel() {
                return Err(LexError::Cancelled);
            }
            if bytes[*i..].starts_with(b"-->") {
                *i += 3;
                state.mode = HtmlMode::Data;
                break;
            }
            *i += 1;
        }
        return push_token(tokens, start, *i, HtmlKind::Comment);
    }
    if rest.len() >= 9 && rest[..2] == *b"<!" && eq_ignore_ascii_case(&rest[2..9], b"DOCTYPE") {
        *i += 9;
        state.mode = HtmlMode::Doctype;
        while *i < bytes.len() && bytes[*i] != b'>' {
            if *i & 255 == 0 && cancel() {
                return Err(LexError::Cancelled);
            }
            *i += 1;
        }
        if bytes.get(*i) == Some(&b'>') {
            *i += 1;
            state.mode = HtmlMode::Data;
        }
        return push_token(tokens, start, *i, HtmlKind::Doctype);
    }
    if rest.starts_with(b"<![CDATA[") {
        *i += 9;
        state.mode = HtmlMode::Cdata;
        while *i < bytes.len() {
            if *i & 255 == 0 && cancel() {
                return Err(LexError::Cancelled);
            }
            if bytes[*i..].starts_with(b"]]>") {
                *i += 3;
                state.mode = HtmlMode::Data;
                break;
            }
            *i += 1;
        }
        return push_token(tokens, start, *i, HtmlKind::Cdata);
    }
    if rest.starts_with(b"<!") {
        *i += 2;
        state.mode = HtmlMode::Comment;
        while *i < bytes.len() {
            if *i & 255 == 0 && cancel() {
                return Err(LexError::Cancelled);
            }
            if bytes[*i] == b'>' {
                *i += 1;
                state.mode = HtmlMode::Data;
                break;
            }
            *i += 1;
        }
        return push_token(tokens, start, *i, HtmlKind::Comment);
    }
    if rest.starts_with(b"<?") {
        *i += 2;
        state.mode = HtmlMode::Pi;
        while *i < bytes.len() {
            if *i & 255 == 0 && cancel() {
                return Err(LexError::Cancelled);
            }
            if bytes[*i] == b'?' && bytes.get(*i + 1) == Some(&b'>') {
                *i += 2;
                state.mode = HtmlMode::Data;
                break;
            }
            if bytes[*i] == b'>' {
                *i += 1;
                state.mode = HtmlMode::Data;
                break;
            }
            *i += 1;
        }
        return push_token(tokens, start, *i, HtmlKind::ProcessingInstruction);
    }
    if rest.len() >= 2 && rest[1] == b'/' {
        *i += 2;
        state.mode = HtmlMode::Tag;
        state.is_end_tag = true;
        state.seen_name = false;
        state.expect_value = false;
        state.tag = TagHint::None;
        state.script_data = false;
        state.attr_is_type = false;
        state.tag_open_start = start;
        return push_token(tokens, start, *i, HtmlKind::TagOpen);
    }
    if rest.len() >= 2 && is_name_start(rest[1]) {
        *i += 1;
        state.mode = HtmlMode::Tag;
        state.is_end_tag = false;
        state.seen_name = false;
        state.expect_value = false;
        state.tag = TagHint::None;
        state.script_data = false;
        state.attr_is_type = false;
        state.tag_open_start = start;
        return push_token(tokens, start, *i, HtmlKind::TagOpen);
    }
    *i += 1;
    push_token(tokens, start, *i, HtmlKind::Text)
}

fn consume_tag_doc(
    bytes: &[u8],
    i: &mut usize,
    state: &mut HtmlState,
    start: u32,
    tokens: &mut Vec<HtmlToken>,
    cancel: &dyn Fn() -> bool,
) -> Result<(), LexError> {
    let b = bytes[*i];
    if b == b'\r' {
        *i += 1;
        if bytes.get(*i) == Some(&b'\n') {
            *i += 1;
        }
        return push_token(tokens, start, *i, HtmlKind::Whitespace);
    }
    if b == b'\n' {
        *i += 1;
        return push_token(tokens, start, *i, HtmlKind::Whitespace);
    }
    if is_html_space(b) {
        *i += 1;
        while *i < bytes.len() && is_html_space(bytes[*i]) {
            *i += 1;
        }
        return push_token(tokens, start, *i, HtmlKind::Whitespace);
    }
    if b == b'>' {
        *i += 1;
        push_token(tokens, start, *i, HtmlKind::TagEnd)?;
        finish_start_tag(state, *i as u32, false);
        return Ok(());
    }
    if b == b'/' && bytes.get(*i + 1) == Some(&b'>') {
        *i += 2;
        push_token(tokens, start, *i, HtmlKind::TagEnd)?;
        finish_start_tag(state, *i as u32, true);
        return Ok(());
    }
    if b == b'=' {
        *i += 1;
        state.expect_value = true;
        return push_token(tokens, start, *i, HtmlKind::AttrEq);
    }
    if state.expect_value && (b == b'"' || b == b'\'') {
        state.attr_quote = b;
        state.mode = HtmlMode::AttrValue;
        *i += 1;
        return consume_attr_value_doc(bytes, i, state, start, tokens, cancel);
    }
    if state.expect_value && !is_html_space(b) && b != b'>' && b != b'/' {
        state.attr_quote = 0;
        state.mode = HtmlMode::AttrValue;
        let before = *i;
        consume_attr_value_doc(bytes, i, state, start, tokens, cancel)?;
        if *i > before {
            return Ok(());
        }
        // Unquoted value was empty (terminator byte). Fall through to Unknown.
    }
    if !state.seen_name && is_name_start(b) {
        *i += 1;
        while *i < bytes.len() && is_name_continue(bytes[*i]) {
            *i += 1;
        }
        let name = &bytes[start as usize..*i];
        state.tag = tag_hint(name);
        state.seen_name = true;
        return push_token(tokens, start, *i, HtmlKind::TagName);
    }
    if state.seen_name && is_attr_name_start(b) {
        *i += 1;
        while *i < bytes.len() && is_attr_name_continue(bytes[*i]) {
            *i += 1;
        }
        let name = &bytes[start as usize..*i];
        state.attr_is_type = name.eq_ignore_ascii_case(b"type");
        state.expect_value = false;
        return push_token(tokens, start, *i, HtmlKind::AttrName);
    }
    *i += 1;
    push_token(tokens, start, *i, HtmlKind::Unknown)
}

fn consume_attr_value_doc(
    bytes: &[u8],
    i: &mut usize,
    state: &mut HtmlState,
    start: u32,
    tokens: &mut Vec<HtmlToken>,
    cancel: &dyn Fn() -> bool,
) -> Result<(), LexError> {
    let quote = state.attr_quote;
    if quote == 0 {
        while *i < bytes.len() {
            let c = bytes[*i];
            if is_html_space(c)
                || c == b'\n'
                || c == b'\r'
                || c == b'>'
                || c == b'/'
                || c == b'<'
                || c == b'"'
                || c == b'\''
                || c == b'='
                || c == b'`'
            {
                break;
            }
            *i += 1;
        }
        state.mode = HtmlMode::Tag;
        state.expect_value = false;
        state.attr_quote = 0;
        if (*i as u32) <= start {
            // Unquoted value started on a terminator (`<`, `` ` ``, `>`). Do not
            // emit a zero-length AttrValue; Tag mode will consume the byte.
            return Ok(());
        }
        classify_script_type(bytes, start, *i, state);
        return push_token(tokens, start, *i, HtmlKind::AttrValue);
    }
    while *i < bytes.len() {
        if *i & 255 == 0 && cancel() {
            return Err(LexError::Cancelled);
        }
        if bytes[*i] == quote {
            *i += 1;
            classify_script_type(bytes, start, *i, state);
            state.mode = HtmlMode::Tag;
            state.expect_value = false;
            state.attr_quote = 0;
            return push_token(tokens, start, *i, HtmlKind::AttrValue);
        }
        *i += 1;
    }
    // Unclosed quoted value: keep AttrValue mode for the next line / EOF.
    push_token(tokens, start, *i, HtmlKind::AttrValue)
}

fn consume_raw_doc(
    bytes: &[u8],
    i: &mut usize,
    state: &mut HtmlState,
    start: u32,
    tokens: &mut Vec<HtmlToken>,
    regions: &mut Vec<RawRegion>,
    cancel: &dyn Fn() -> bool,
) -> Result<(), LexError> {
    let kind = state.raw.unwrap_or(RawKind::Script);
    let tag = raw_close_tag(kind);
    if let Some(pos) = find_raw_close(bytes, *i, tag, cancel)? {
        if pos > *i {
            push_token(tokens, start, pos, HtmlKind::RawText)?;
        }
        regions.push(RawRegion {
            start: state.raw_content_start,
            end: pos as u32,
            kind,
            open_tag: (state.tag_open_start, state.raw_content_start),
        });
        *i = pos.max(*i);
        state.mode = HtmlMode::Data;
        state.raw = None;
        state.script = ScriptState::for_dialect(ScriptDialect::Js);
        state.css = CssState::default();
        if *i == start as usize && pos == *i {
            // Close tag at the start of the raw content: no RawText token.
            // Leave `i` at the close tag so Data consumes it next. The caller
            // treats a non-advancing step as nonprogress, so emit nothing and
            // switch to Data; the next iteration tokenises `</script>`.
            // Force progress by not returning a zero-length token: the close
            // tag is at `pos == start`, so we must consume markup now.
            return open_markup_doc(bytes, i, state, start, tokens, cancel);
        }
        return Ok(());
    }
    let end = bytes.len();
    if end > *i {
        push_token(tokens, start, end, HtmlKind::RawText)?;
    }
    regions.push(RawRegion {
        start: state.raw_content_start,
        end: end as u32,
        kind,
        open_tag: (state.tag_open_start, state.raw_content_start),
    });
    *i = end;
    Ok(())
}

fn consume_data_line(
    bytes: &[u8],
    i: &mut usize,
    state: &mut HtmlState,
    out: &mut Vec<(usize, TokenRole)>,
) {
    let b = bytes[*i];
    if is_html_space(b) || b == b'\n' || b == b'\r' {
        *i += 1;
        while *i < bytes.len() && (is_html_space(bytes[*i]) || bytes[*i] == b'\n' || bytes[*i] == b'\r')
        {
            *i += 1;
        }
        push_role(*i, TokenRole::Whitespace, out);
        return;
    }
    if b == b'<' {
        open_markup_line(bytes, i, state, out);
        return;
    }
    *i += 1;
    while *i < bytes.len() {
        let c = bytes[*i];
        if c == b'<' || is_html_space(c) || c == b'\n' || c == b'\r' {
            break;
        }
        *i += 1;
    }
    push_role(*i, TokenRole::Unknown, out);
}

fn open_markup_line(
    bytes: &[u8],
    i: &mut usize,
    state: &mut HtmlState,
    out: &mut Vec<(usize, TokenRole)>,
) {
    let rest = &bytes[*i..];
    if rest.starts_with(b"<!--") {
        *i += 4;
        state.mode = HtmlMode::Comment;
        if let Some(rel) = find_sub(bytes, *i, b"-->") {
            *i = rel + 3;
            state.mode = HtmlMode::Data;
        } else {
            *i = bytes.len();
        }
        push_role(*i, TokenRole::Comment, out);
        return;
    }
    if rest.len() >= 9 && rest[..2] == *b"<!" && eq_ignore_ascii_case(&rest[2..9.min(rest.len())], b"DOCTYPE")
    {
        *i += 9.min(rest.len());
        state.mode = HtmlMode::Doctype;
        if let Some(rel) = find_byte(bytes, *i, b'>') {
            *i = rel + 1;
            state.mode = HtmlMode::Data;
        } else {
            *i = bytes.len();
        }
        push_role(*i, TokenRole::Preprocessor, out);
        return;
    }
    if rest.starts_with(b"<![CDATA[") {
        *i += 9;
        state.mode = HtmlMode::Cdata;
        if let Some(rel) = find_sub(bytes, *i, b"]]>") {
            *i = rel + 3;
            state.mode = HtmlMode::Data;
        } else {
            *i = bytes.len();
        }
        push_role(*i, TokenRole::Preprocessor, out);
        return;
    }
    if rest.starts_with(b"<!") {
        *i += 2;
        state.mode = HtmlMode::Comment;
        if let Some(rel) = find_byte(bytes, *i, b'>') {
            *i = rel + 1;
            state.mode = HtmlMode::Data;
        } else {
            *i = bytes.len();
        }
        push_role(*i, TokenRole::Comment, out);
        return;
    }
    if rest.starts_with(b"<?") {
        *i += 2;
        state.mode = HtmlMode::Pi;
        if let Some(rel) = find_pi_end(bytes, *i) {
            *i = rel;
            state.mode = HtmlMode::Data;
        } else {
            *i = bytes.len();
        }
        push_role(*i, TokenRole::Preprocessor, out);
        return;
    }
    if rest.len() >= 2 && rest[1] == b'/' {
        *i += 2;
        state.mode = HtmlMode::Tag;
        state.is_end_tag = true;
        state.seen_name = false;
        state.expect_value = false;
        state.tag = TagHint::None;
        state.script_data = false;
        state.attr_is_type = false;
        push_role(*i, TokenRole::Delimiter, out);
        return;
    }
    if rest.len() >= 2 && is_name_start(rest[1]) {
        *i += 1;
        state.mode = HtmlMode::Tag;
        state.is_end_tag = false;
        state.seen_name = false;
        state.expect_value = false;
        state.tag = TagHint::None;
        state.script_data = false;
        state.attr_is_type = false;
        push_role(*i, TokenRole::Delimiter, out);
        return;
    }
    *i += 1;
    push_role(*i, TokenRole::Unknown, out);
}

fn consume_tag_line(
    bytes: &[u8],
    i: &mut usize,
    state: &mut HtmlState,
    out: &mut Vec<(usize, TokenRole)>,
) {
    let b = bytes[*i];
    if is_html_space(b) || b == b'\n' || b == b'\r' {
        *i += 1;
        while *i < bytes.len() && (is_html_space(bytes[*i]) || bytes[*i] == b'\n' || bytes[*i] == b'\r')
        {
            *i += 1;
        }
        push_role(*i, TokenRole::Whitespace, out);
        return;
    }
    if b == b'>' {
        *i += 1;
        push_role(*i, TokenRole::Delimiter, out);
        finish_start_tag(state, 0, false);
        return;
    }
    if b == b'/' && bytes.get(*i + 1) == Some(&b'>') {
        *i += 2;
        push_role(*i, TokenRole::Delimiter, out);
        finish_start_tag(state, 0, true);
        return;
    }
    if b == b'=' {
        *i += 1;
        state.expect_value = true;
        push_role(*i, TokenRole::Punctuator, out);
        return;
    }
    if state.expect_value && (b == b'"' || b == b'\'') {
        state.attr_quote = b;
        state.mode = HtmlMode::AttrValue;
        *i += 1;
        consume_attr_value_line(bytes, i, state, out);
        return;
    }
    if state.expect_value && !is_html_space(b) && b != b'>' && b != b'/' {
        state.attr_quote = 0;
        state.mode = HtmlMode::AttrValue;
        consume_attr_value_line(bytes, i, state, out);
        return;
    }
    if !state.seen_name && is_name_start(b) {
        let ns = *i;
        *i += 1;
        while *i < bytes.len() && is_name_continue(bytes[*i]) {
            *i += 1;
        }
        state.tag = tag_hint(&bytes[ns..*i]);
        state.seen_name = true;
        push_role(*i, TokenRole::Keyword, out);
        return;
    }
    if state.seen_name && is_attr_name_start(b) {
        let ns = *i;
        *i += 1;
        while *i < bytes.len() && is_attr_name_continue(bytes[*i]) {
            *i += 1;
        }
        state.attr_is_type = bytes[ns..*i].eq_ignore_ascii_case(b"type");
        state.expect_value = false;
        push_role(*i, TokenRole::Identifier, out);
        return;
    }
    *i += 1;
    push_role(*i, TokenRole::Unknown, out);
}

fn consume_attr_value_line(
    bytes: &[u8],
    i: &mut usize,
    state: &mut HtmlState,
    out: &mut Vec<(usize, TokenRole)>,
) {
    let quote = state.attr_quote;
    let value_start = *i;
    if quote == 0 {
        while *i < bytes.len() {
            let c = bytes[*i];
            if is_html_space(c)
                || c == b'\n'
                || c == b'\r'
                || c == b'>'
                || c == b'/'
                || c == b'<'
                || c == b'"'
                || c == b'\''
                || c == b'='
                || c == b'`'
            {
                break;
            }
            *i += 1;
        }
        state.mode = HtmlMode::Tag;
        state.expect_value = false;
        state.attr_quote = 0;
        if *i > value_start {
            classify_script_type(bytes, value_start as u32, *i, state);
            push_role(*i, TokenRole::String, out);
        }
        return;
    }
    while *i < bytes.len() {
        if bytes[*i] == quote {
            *i += 1;
            classify_script_type(bytes, value_start.saturating_sub(1) as u32, *i, state);
            state.mode = HtmlMode::Tag;
            state.expect_value = false;
            state.attr_quote = 0;
            push_role(*i, TokenRole::String, out);
            return;
        }
        *i += 1;
    }
    push_role(*i, TokenRole::String, out);
}

fn consume_raw_line(
    bytes: &[u8],
    i: &mut usize,
    state: &mut HtmlState,
    out: &mut Vec<(usize, TokenRole)>,
) {
    let kind = state.raw.unwrap_or(RawKind::Script);
    let tag = raw_close_tag(kind);
    if let Some(pos) = find_raw_close_in(bytes, *i, tag) {
        if pos > *i {
            emit_raw_roles(bytes, *i, pos, kind, state, out);
        }
        state.mode = HtmlMode::Data;
        state.raw = None;
        state.script = ScriptState::for_dialect(ScriptDialect::Js);
        state.css = CssState::default();
        // Tokenise the close tag as HTML from `pos`.
        *i = pos;
        if *i < bytes.len() {
            consume_data_line(bytes, i, state, out);
        }
        return;
    }
    emit_raw_roles(bytes, *i, bytes.len(), kind, state, out);
    *i = bytes.len();
}

fn emit_raw_roles(
    bytes: &[u8],
    from: usize,
    to: usize,
    kind: RawKind,
    state: &mut HtmlState,
    out: &mut Vec<(usize, TokenRole)>,
) {
    if to <= from {
        return;
    }
    let prefix = match std::str::from_utf8(&bytes[from..to]) {
        Ok(s) => s,
        Err(_) => {
            push_role(to, TokenRole::Unknown, out);
            return;
        }
    };
    match kind {
        RawKind::Script => {
            let (next, roles) = script_lex::lex_line(prefix, state.script.clone());
            state.script = next;
            for (end, role) in roles {
                push_role(from + end, role, out);
            }
        }
        RawKind::Style => {
            let (next, roles) = css_lex::lex_line(prefix, state.css.clone());
            state.css = next;
            for (end, role) in roles {
                push_role(from + end, role, out);
            }
        }
        RawKind::ScriptData | RawKind::Textarea | RawKind::Title => {
            push_role(to, TokenRole::Unknown, out);
        }
    }
}

fn raw_kind_for_dialect(state: &HtmlState) -> Option<RawKind> {
    match state.dialect {
        MarkupDialect::Xml => None,
        MarkupDialect::Svg => match state.tag {
            TagHint::Script => Some(if state.script_data {
                RawKind::ScriptData
            } else {
                RawKind::Script
            }),
            TagHint::Style => Some(RawKind::Style),
            _ => None,
        },
        MarkupDialect::Html => match state.tag {
            TagHint::Script => Some(if state.script_data {
                RawKind::ScriptData
            } else {
                RawKind::Script
            }),
            TagHint::Style => Some(RawKind::Style),
            TagHint::Textarea => Some(RawKind::Textarea),
            TagHint::Title => Some(RawKind::Title),
            _ => None,
        },
    }
}

fn finish_start_tag(state: &mut HtmlState, after_end: u32, self_closing: bool) {
    if state.is_end_tag || self_closing {
        state.mode = HtmlMode::Data;
        state.tag = TagHint::None;
        state.seen_name = false;
        state.expect_value = false;
        state.is_end_tag = false;
        state.script_data = false;
        state.attr_is_type = false;
        return;
    }
    let raw = raw_kind_for_dialect(state);
    if let Some(kind) = raw {
        state.mode = HtmlMode::Raw;
        state.raw = Some(kind);
        state.raw_content_start = after_end;
        state.script = ScriptState::for_dialect(ScriptDialect::Js);
        state.css = CssState::default();
    } else {
        state.mode = HtmlMode::Data;
    }
    state.seen_name = false;
    state.expect_value = false;
    state.is_end_tag = false;
    state.attr_is_type = false;
    // Keep `tag` / `script_data` only until raw is entered; then clear.
    if raw.is_none() {
        state.tag = TagHint::None;
        state.script_data = false;
    }
}

fn classify_script_type(bytes: &[u8], start: u32, end: usize, state: &mut HtmlState) {
    if !state.attr_is_type || state.tag != TagHint::Script {
        return;
    }
    let raw = match bytes.get(start as usize..end) {
        Some(s) => s,
        None => return,
    };
    let inner = strip_quotes(raw);
    let text = std::str::from_utf8(inner).unwrap_or("");
    let decoded = decode_attribute(text).unwrap_or_else(|_| text.to_string());
    if is_script_data_type(&decoded) {
        state.script_data = true;
    }
}

fn strip_quotes(raw: &[u8]) -> &[u8] {
    if raw.len() >= 2 {
        let a = raw[0];
        let b = raw[raw.len() - 1];
        if (a == b'"' && b == b'"') || (a == b'\'' && b == b'\'') {
            return &raw[1..raw.len() - 1];
        }
    }
    raw
}

fn is_script_data_type(value: &str) -> bool {
    let t = value.trim();
    let l = t.to_ascii_lowercase();
    l == "text/template"
        || l.starts_with("text/x-")
        || l == "application/json"
        || l == "importmap"
        || l == "application/ld+json"
}

fn tag_hint(name: &[u8]) -> TagHint {
    if name.eq_ignore_ascii_case(b"script") {
        TagHint::Script
    } else if name.eq_ignore_ascii_case(b"style") {
        TagHint::Style
    } else if name.eq_ignore_ascii_case(b"textarea") {
        TagHint::Textarea
    } else if name.eq_ignore_ascii_case(b"title") {
        TagHint::Title
    } else {
        TagHint::Other
    }
}

fn raw_close_tag(kind: RawKind) -> &'static [u8] {
    match kind {
        RawKind::Script | RawKind::ScriptData => b"script",
        RawKind::Style => b"style",
        RawKind::Textarea => b"textarea",
        RawKind::Title => b"title",
    }
}

fn find_raw_close(
    bytes: &[u8],
    start: usize,
    tag: &[u8],
    cancel: &dyn Fn() -> bool,
) -> Result<Option<usize>, LexError> {
    let mut i = start;
    while i < bytes.len() {
        if i & 255 == 0 && cancel() {
            return Err(LexError::Cancelled);
        }
        if bytes[i] == b'<' && bytes.get(i + 1) == Some(&b'/') && matches_close_tag(bytes, i + 2, tag)
        {
            return Ok(Some(i));
        }
        i += 1;
    }
    Ok(None)
}

fn find_raw_close_in(bytes: &[u8], start: usize, tag: &[u8]) -> Option<usize> {
    let mut i = start;
    while i < bytes.len() {
        if bytes[i] == b'<' && bytes.get(i + 1) == Some(&b'/') && matches_close_tag(bytes, i + 2, tag) {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn matches_close_tag(bytes: &[u8], name_at: usize, tag: &[u8]) -> bool {
    if name_at + tag.len() > bytes.len() {
        return false;
    }
    if !eq_ignore_ascii_case(&bytes[name_at..name_at + tag.len()], tag) {
        return false;
    }
    let after = name_at + tag.len();
    if after >= bytes.len() {
        return true;
    }
    matches!(
        bytes[after],
        b'>' | b'/' | b' ' | b'\t' | b'\n' | b'\r' | 0x0c
    )
}

fn splice_css(bytes: &[u8], spans: &mut Vec<TokenSpan>, region: RawRegion) {
    let slice = match bytes.get(region.start as usize..region.end as usize) {
        Some(s) => s,
        None => return,
    };
    let inner = match css_lex::lex_document_cancellable(slice, &|| false) {
        Ok(tokens) => css_lex::document_spans(slice, &tokens),
        Err(_) => vec![TokenSpan::new(0, slice.len() as u32, TokenRole::Unknown)],
    };
    splice_offset(spans, region.start, region.end, inner);
}

fn splice_script(bytes: &[u8], spans: &mut Vec<TokenSpan>, region: RawRegion) {
    let slice = match bytes.get(region.start as usize..region.end as usize) {
        Some(s) => s,
        None => return,
    };
    let inner = match script_lex::lex_document_cancellable(slice, ScriptDialect::Js, &|| false) {
        Ok(tokens) => script_lex::document_spans(slice, &tokens),
        Err(_) => vec![TokenSpan::new(0, slice.len() as u32, TokenRole::Unknown)],
    };
    splice_offset(spans, region.start, region.end, inner);
}

fn splice_offset(spans: &mut Vec<TokenSpan>, start: u32, end: u32, inner: Vec<TokenSpan>) {
    let mut offset: Vec<TokenSpan> = inner
        .into_iter()
        .map(|s| TokenSpan::new(s.start.saturating_add(start), s.end.saturating_add(start), s.role))
        .collect();
    if offset.is_empty() {
        offset.push(TokenSpan::new(start, end, TokenRole::Unknown));
    } else {
        if offset[0].start > start {
            offset.insert(0, TokenSpan::new(start, offset[0].start, TokenRole::Unknown));
        }
        if let Some(last) = offset.last() {
            if last.end < end {
                offset.push(TokenSpan::new(last.end, end, TokenRole::Unknown));
            }
        }
    }
    replace_overlapping(spans, start, end, offset);
}

fn replace_range(spans: &mut Vec<TokenSpan>, start: u32, end: u32, role: TokenRole) {
    replace_overlapping(spans, start, end, vec![TokenSpan::new(start, end, role)]);
}

fn replace_overlapping(
    spans: &mut Vec<TokenSpan>,
    start: u32,
    end: u32,
    inner: Vec<TokenSpan>,
) {
    let mut lo = None;
    let mut hi = None;
    for (i, s) in spans.iter().enumerate() {
        if s.end > start && s.start < end {
            if lo.is_none() {
                lo = Some(i);
            }
            hi = Some(i + 1);
        }
    }
    match (lo, hi) {
        (Some(lo), Some(hi)) => {
            spans.splice(lo..hi, inner);
        }
        _ => {}
    }
}

fn eq_ignore_ascii_case(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.eq_ignore_ascii_case(b)
}

fn find_sub(bytes: &[u8], start: usize, pat: &[u8]) -> Option<usize> {
    if pat.is_empty() || start >= bytes.len() {
        return None;
    }
    bytes[start..]
        .windows(pat.len())
        .position(|w| w == pat)
        .map(|p| start + p)
}

fn find_byte(bytes: &[u8], start: usize, b: u8) -> Option<usize> {
    bytes[start..].iter().position(|&c| c == b).map(|p| start + p)
}

fn find_pi_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut i = start;
    while i < bytes.len() {
        if bytes[i] == b'?' && bytes.get(i + 1) == Some(&b'>') {
            return Some(i + 2);
        }
        if bytes[i] == b'>' {
            return Some(i + 1);
        }
        i += 1;
    }
    None
}

fn is_html_space(b: u8) -> bool {
    b == b' ' || b == b'\t' || b == 0x0b || b == 0x0c
}

fn is_name_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || b == b':' || b >= 0x80
}

fn is_name_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b':' || b == b'.' || b >= 0x80
}

fn is_attr_name_start(b: u8) -> bool {
    !is_html_space(b)
        && b != b'='
        && b != b'/'
        && b != b'>'
        && b != b'<'
        && b != b'"'
        && b != b'\''
        && b != b'\n'
        && b != b'\r'
        && b != 0
}

fn is_attr_name_continue(b: u8) -> bool {
    is_attr_name_start(b)
}

fn push_token(
    tokens: &mut Vec<HtmlToken>,
    start: u32,
    end: usize,
    kind: HtmlKind,
) -> Result<(), LexError> {
    if end as u32 <= start {
        return Err(LexError::Nonprogress { at: start });
    }
    tokens.push(HtmlToken {
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
