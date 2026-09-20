//! Source-only Markdown lexer shared by the editor and the text frontend.
//!
//! Two entry points share one role table. The document lexer (index time) runs
//! the repository CommonMark parser (`pulldown-cmark`, tables + math +
//! strikethrough) and maps event byte ranges. The line lexer (editor and code
//! view) is a bounded per-line highlighter with fence / front-matter /
//! HTML-block continuation. Neither path parses Markdown semantics.
//!
//! The two paths can classify the same bytes differently: lazy continuation
//! lines, link reference definitions, nested lists, setext vs thematic-break
//! ambiguity, and HTML blocks. Those differences are coverage, not a bug.

use crate::cpp_lex::LexError;
use crate::token::{TokenRole, TokenSpan};
use pulldown_cmark::{Event, Options, Parser, Tag};

/// Version of this lexer; part of parse and search cache identity.
pub const MARKDOWN_LEXER_VERSION: u32 = 1;

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct MarkdownState {
    /// Open fenced code block: fence character (`` ` `` or `~`) and opener count.
    fence: Option<(u8, u8)>,
    front_matter: bool,
    /// Previous non-blank line was a paragraph, so a `===` / `---` line is setext.
    prev_paragraph: bool,
    html_block: bool,
    /// False until the first `lex_line` call, including a leading empty line.
    started: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkdownKind {
    Whitespace,
    Text,
    Heading,
    CodeBlock,
    InlineCode,
    Emphasis,
    Link,
    Html,
    FrontMatter,
    BlockQuote,
    ListMarker,
    Rule,
    Math,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MarkdownToken {
    pub start: u32,
    pub end: u32,
    pub kind: MarkdownKind,
}

impl MarkdownKind {
    pub fn role(self) -> TokenRole {
        match self {
            MarkdownKind::Heading => TokenRole::Keyword,
            MarkdownKind::CodeBlock | MarkdownKind::InlineCode => TokenRole::String,
            MarkdownKind::Emphasis => TokenRole::Typename,
            MarkdownKind::Link => TokenRole::Function,
            MarkdownKind::Html | MarkdownKind::FrontMatter => TokenRole::Preprocessor,
            MarkdownKind::BlockQuote => TokenRole::Comment,
            MarkdownKind::ListMarker | MarkdownKind::Rule => TokenRole::Punctuator,
            MarkdownKind::Math => TokenRole::Number,
            MarkdownKind::Text | MarkdownKind::Unknown => TokenRole::Unknown,
            MarkdownKind::Whitespace => TokenRole::Whitespace,
        }
    }
}

fn is_rich(kind: MarkdownKind) -> bool {
    matches!(
        kind,
        MarkdownKind::Heading
            | MarkdownKind::CodeBlock
            | MarkdownKind::Link
            | MarkdownKind::Emphasis
            | MarkdownKind::InlineCode
            | MarkdownKind::Html
            | MarkdownKind::Math
            | MarkdownKind::FrontMatter
    )
}

fn container_kind(tag: &Tag<'_>) -> Option<MarkdownKind> {
    match tag {
        Tag::Heading { .. } => Some(MarkdownKind::Heading),
        Tag::CodeBlock(_) => Some(MarkdownKind::CodeBlock),
        Tag::BlockQuote(_) => Some(MarkdownKind::BlockQuote),
        Tag::List(_) | Tag::Item => Some(MarkdownKind::ListMarker),
        Tag::Emphasis | Tag::Strong | Tag::Strikethrough => Some(MarkdownKind::Emphasis),
        Tag::Link { .. } | Tag::Image { .. } => Some(MarkdownKind::Link),
        Tag::Table(_) | Tag::TableHead | Tag::TableRow | Tag::TableCell => {
            Some(MarkdownKind::ListMarker)
        }
        _ => None,
    }
}

struct Paint {
    start: u32,
    end: u32,
    kind: MarkdownKind,
    depth: u16,
}

fn parse_options() -> Options {
    Options::ENABLE_TABLES | Options::ENABLE_MATH | Options::ENABLE_STRIKETHROUGH
}

fn is_ws_byte(b: u8) -> bool {
    b == b' ' || b == b'\t' || b == b'\n' || b == b'\r' || b == 0x0b || b == 0x0c
}

fn trim_ascii(bytes: &[u8]) -> &[u8] {
    let mut a = 0usize;
    let mut z = bytes.len();
    while a < z && is_ws_byte(bytes[a]) {
        a += 1;
    }
    while z > a && is_ws_byte(bytes[z - 1]) {
        z -= 1;
    }
    &bytes[a..z]
}

/// Leading YAML-style `---` ... `---` (or `...`) block. Unclosed openers are
/// not front matter. Offsets are from the start of `text`.
fn split_front_matter(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut i = 0usize;
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        i = 3;
    }
    if !bytes[i..].starts_with(b"---") {
        return None;
    }
    let mut j = i + 3;
    while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'\t') {
        j += 1;
    }
    if j < bytes.len() && bytes[j] != b'\n' && bytes[j] != b'\r' {
        return None;
    }
    if bytes.get(j) == Some(&b'\r') {
        j += 1;
    }
    if bytes.get(j) == Some(&b'\n') {
        j += 1;
    }
    if j >= bytes.len() {
        return None;
    }
    while j < bytes.len() {
        let line_start = j;
        while j < bytes.len() && bytes[j] != b'\n' && bytes[j] != b'\r' {
            j += 1;
        }
        let line = trim_ascii(&bytes[line_start..j]);
        let at_close = line == b"---" || line == b"...";
        if bytes.get(j) == Some(&b'\r') {
            j += 1;
        }
        if bytes.get(j) == Some(&b'\n') {
            j += 1;
        }
        if at_close {
            return Some(j);
        }
        if j == line_start {
            break;
        }
    }
    None
}

fn flatten_paints(paints: &[Paint], len: u32, bytes: &[u8]) -> Vec<MarkdownToken> {
    if len == 0 {
        return Vec::new();
    }
    let mut events: Vec<(u32, u8, usize)> = Vec::with_capacity(paints.len().saturating_mul(2));
    for (i, p) in paints.iter().enumerate() {
        if p.end > p.start {
            events.push((p.start, 0, i));
            events.push((p.end, 1, i));
        }
    }
    events.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    let mut active: Vec<usize> = Vec::new();
    let mut tokens = Vec::new();
    let mut prev = 0u32;
    let mut cursor = 0usize;
    while cursor < events.len() {
        let pos = events[cursor].0;
        if pos > prev {
            emit_range(&mut tokens, paints, &active, bytes, prev, pos.min(len));
            prev = pos;
        }
        while cursor < events.len() && events[cursor].0 == pos {
            let (typ, idx) = (events[cursor].1, events[cursor].2);
            if typ == 0 {
                active.push(idx);
            } else {
                if let Some(at) = active.iter().position(|&j| j == idx) {
                    active.swap_remove(at);
                }
            }
            cursor += 1;
        }
    }
    if prev < len {
        emit_range(&mut tokens, paints, &active, bytes, prev, len);
    }
    tokens
}

fn emit_range(
    tokens: &mut Vec<MarkdownToken>,
    paints: &[Paint],
    active: &[usize],
    bytes: &[u8],
    start: u32,
    end: u32,
) {
    if end <= start {
        return;
    }
    let kind = best_kind(paints, active, bytes, start, end);
    if let Some(last) = tokens.last_mut() {
        if last.kind == kind && last.end == start {
            last.end = end;
            return;
        }
    }
    tokens.push(MarkdownToken {
        start,
        end,
        kind,
    });
}

fn best_kind(
    paints: &[Paint],
    active: &[usize],
    bytes: &[u8],
    start: u32,
    end: u32,
) -> MarkdownKind {
    let mut best: Option<(u16, MarkdownKind)> = None;
    for &i in active {
        let p = &paints[i];
        if p.start <= start && p.end >= end {
            if best.map(|(d, _)| p.depth >= d).unwrap_or(true) {
                best = Some((p.depth, p.kind));
            }
        }
    }
    match best {
        Some((_, k)) => k,
        None => {
            let slice = match bytes.get(start as usize..end as usize) {
                Some(s) => s,
                None => return MarkdownKind::Unknown,
            };
            if slice.iter().copied().all(is_ws_byte) {
                MarkdownKind::Whitespace
            } else {
                MarkdownKind::Text
            }
        }
    }
}

fn lex_markdown_text(
    text: &str,
    base: u32,
    cancel: &dyn Fn() -> bool,
) -> Result<Vec<Paint>, LexError> {
    let mut paints = Vec::new();
    let mut stack: Vec<Option<MarkdownKind>> = Vec::new();
    let mut depth = 0u16;
    let mut n = 0u32;
    let parser = Parser::new_ext(text, parse_options()).into_offset_iter();
    for (event, range) in parser {
        if n & 255 == 0 && cancel() {
            return Err(LexError::Cancelled);
        }
        n = n.saturating_add(1);
        let start = (range.start as u32).saturating_add(base);
        let end = (range.end as u32).saturating_add(base);
        match event {
            Event::Start(tag) => {
                depth = depth.saturating_add(1);
                let kind = container_kind(&tag);
                if let Some(kind) = kind {
                    if end > start {
                        paints.push(Paint {
                            start,
                            end,
                            kind,
                            depth,
                        });
                    }
                }
                stack.push(kind);
            }
            Event::End(_) => {
                let _ = stack.pop();
                depth = depth.saturating_sub(1);
            }
            Event::Text(_) | Event::SoftBreak | Event::HardBreak => {
                if !stack.iter().rev().flatten().copied().any(is_rich) && end > start {
                    paints.push(Paint {
                        start,
                        end,
                        kind: MarkdownKind::Text,
                        depth: depth.saturating_add(1),
                    });
                }
            }
            Event::Code(_) => {
                if end > start {
                    paints.push(Paint {
                        start,
                        end,
                        kind: MarkdownKind::InlineCode,
                        depth: depth.saturating_add(1),
                    });
                }
            }
            Event::InlineMath(_) | Event::DisplayMath(_) => {
                if end > start {
                    paints.push(Paint {
                        start,
                        end,
                        kind: MarkdownKind::Math,
                        depth: depth.saturating_add(1),
                    });
                }
            }
            Event::Html(_) | Event::InlineHtml(_) => {
                if end > start {
                    paints.push(Paint {
                        start,
                        end,
                        kind: MarkdownKind::Html,
                        depth: depth.saturating_add(1),
                    });
                }
            }
            Event::Rule => {
                if end > start {
                    paints.push(Paint {
                        start,
                        end,
                        kind: MarkdownKind::Rule,
                        depth: depth.saturating_add(1),
                    });
                }
            }
            Event::TaskListMarker(_) => {
                if end > start {
                    paints.push(Paint {
                        start,
                        end,
                        kind: MarkdownKind::ListMarker,
                        depth: depth.saturating_add(1),
                    });
                }
            }
            Event::FootnoteReference(_) => {
                if end > start && !stack.iter().rev().flatten().copied().any(is_rich) {
                    paints.push(Paint {
                        start,
                        end,
                        kind: MarkdownKind::Text,
                        depth: depth.saturating_add(1),
                    });
                }
            }
        }
    }
    Ok(paints)
}

pub fn lex_document_cancellable(
    bytes: &[u8],
    cancel: &dyn Fn() -> bool,
) -> Result<Vec<MarkdownToken>, LexError> {
    let token_cap = bytes.len().saturating_add(8);
    match std::str::from_utf8(bytes) {
        Ok(text) => lex_utf8(text, bytes, token_cap, cancel),
        Err(err) => {
            let valid = err.valid_up_to();
            let mut tokens = if valid > 0 {
                let prefix = std::str::from_utf8(&bytes[..valid]).unwrap_or("");
                lex_utf8(prefix, &bytes[..valid], token_cap, cancel)?
            } else {
                Vec::new()
            };
            if valid < bytes.len() {
                tokens.push(MarkdownToken {
                    start: valid as u32,
                    end: bytes.len() as u32,
                    kind: MarkdownKind::Unknown,
                });
            }
            if tokens.len() > token_cap {
                return Err(LexError::Nonprogress {
                    at: valid as u32,
                });
            }
            Ok(tokens)
        }
    }
}

fn lex_utf8(
    text: &str,
    bytes: &[u8],
    token_cap: usize,
    cancel: &dyn Fn() -> bool,
) -> Result<Vec<MarkdownToken>, LexError> {
    let mut paints = Vec::new();
    let rest = if let Some(fm_end) = split_front_matter(text) {
        if fm_end > 0 {
            paints.push(Paint {
                start: 0,
                end: fm_end as u32,
                kind: MarkdownKind::FrontMatter,
                depth: 1,
            });
        }
        &text[fm_end..]
    } else {
        text
    };
    let base = (text.len() - rest.len()) as u32;
    if cancel() {
        return Err(LexError::Cancelled);
    }
    let mut body = lex_markdown_text(rest, base, cancel)?;
    paints.append(&mut body);
    let tokens = flatten_paints(&paints, bytes.len() as u32, bytes);
    if tokens.len() > token_cap {
        return Err(LexError::Nonprogress { at: 0 });
    }
    let mut at = 0u32;
    for t in &tokens {
        if t.start != at || t.end <= t.start {
            return Err(LexError::Nonprogress { at });
        }
        at = t.end;
    }
    if at != bytes.len() as u32 && !bytes.is_empty() {
        return Err(LexError::Nonprogress { at });
    }
    Ok(tokens)
}

pub fn lex_line(line: &str, incoming: MarkdownState) -> (MarkdownState, Vec<(usize, TokenRole)>) {
    let bytes = line.as_bytes();
    let mut out = Vec::new();
    let mut state = incoming;
    let first_line = !state.started;
    state.started = true;
    if bytes.is_empty() {
        state.prev_paragraph = false;
        return (state, out);
    }

    if let Some((ch, n)) = state.fence {
        if is_closing_fence(bytes, ch, n) {
            push_role(bytes.len(), MarkdownKind::CodeBlock.role(), &mut out);
            state.fence = None;
            state.prev_paragraph = false;
            return (state, out);
        }
        push_role(bytes.len(), MarkdownKind::CodeBlock.role(), &mut out);
        state.prev_paragraph = false;
        return (state, out);
    }

    if state.front_matter {
        if is_front_matter_close(bytes) {
            push_role(bytes.len(), MarkdownKind::FrontMatter.role(), &mut out);
            state.front_matter = false;
        } else {
            push_role(bytes.len(), MarkdownKind::FrontMatter.role(), &mut out);
        }
        state.prev_paragraph = false;
        return (state, out);
    }

    if state.html_block {
        if is_blank_line(bytes) {
            state.html_block = false;
        } else {
            push_role(bytes.len(), MarkdownKind::Html.role(), &mut out);
            state.prev_paragraph = false;
            return (state, out);
        }
    }

    if is_blank_line(bytes) {
        push_role(bytes.len(), TokenRole::Whitespace, &mut out);
        state.prev_paragraph = false;
        return (state, out);
    }

    if is_front_matter_open(bytes) && first_line {
        push_role(bytes.len(), MarkdownKind::FrontMatter.role(), &mut out);
        state.front_matter = true;
        state.prev_paragraph = false;
        return (state, out);
    }

    if let Some((ch, n, _)) = scan_open_fence(bytes) {
        push_role(bytes.len(), MarkdownKind::CodeBlock.role(), &mut out);
        state.fence = Some((ch, n));
        state.prev_paragraph = false;
        return (state, out);
    }

    if is_atx_heading(bytes) {
        push_role(bytes.len(), MarkdownKind::Heading.role(), &mut out);
        state.prev_paragraph = false;
        return (state, out);
    }

    if state.prev_paragraph && is_setext_underline(bytes) {
        push_role(bytes.len(), MarkdownKind::Heading.role(), &mut out);
        state.prev_paragraph = false;
        return (state, out);
    }

    if is_thematic_break(bytes) {
        push_role(bytes.len(), MarkdownKind::Rule.role(), &mut out);
        state.prev_paragraph = false;
        return (state, out);
    }

    let mut i = 0usize;
    while i < bytes.len() {
        let spaces = count_spaces(bytes, i);
        let j = i + spaces.0;
        if bytes.get(j) == Some(&b'>') {
            if spaces.1 <= 3 {
                push_role(j, TokenRole::Whitespace, &mut out);
                let mut k = j + 1;
                if bytes.get(k) == Some(&b' ') || bytes.get(k) == Some(&b'\t') {
                    k += 1;
                }
                push_role(k, MarkdownKind::BlockQuote.role(), &mut out);
                i = k;
                continue;
            }
        }
        break;
    }

    let spaces = count_spaces(bytes, i);
    if !state.prev_paragraph && spaces.1 >= 4 {
        push_role(bytes.len(), MarkdownKind::CodeBlock.role(), &mut out);
        state.prev_paragraph = false;
        return (state, out);
    }
    if spaces.0 > 0 {
        push_role(i + spaces.0, TokenRole::Whitespace, &mut out);
        i += spaces.0;
    }

    if let Some(end) = scan_list_marker(bytes, i) {
        push_role(end, MarkdownKind::ListMarker.role(), &mut out);
        i = end;
    }

    if looks_like_html_block(bytes, i) {
        push_role(bytes.len(), MarkdownKind::Html.role(), &mut out);
        state.html_block = !html_block_closed_on_line(bytes, i);
        state.prev_paragraph = false;
        return (state, out);
    }

    lex_inline(bytes, i, &mut out);
    state.prev_paragraph = true;
    (state, out)
}

fn push_role(end: usize, role: TokenRole, out: &mut Vec<(usize, TokenRole)>) {
    if end == 0 {
        return;
    }
    if out.last().map(|(e, _)| *e) == Some(end) {
        return;
    }
    if out.last().map(|(_, r)| *r) == Some(role) {
        out.last_mut().unwrap().0 = end;
    } else {
        out.push((end, role));
    }
}

fn is_blank_line(bytes: &[u8]) -> bool {
    bytes.iter().all(|&b| b == b' ' || b == b'\t' || b == b'\r')
}

fn is_front_matter_open(bytes: &[u8]) -> bool {
    trim_ascii(bytes) == b"---"
}

fn is_front_matter_close(bytes: &[u8]) -> bool {
    let t = trim_ascii(bytes);
    t == b"---" || t == b"..."
}

fn is_closing_fence(bytes: &[u8], ch: u8, n: u8) -> bool {
    let mut i = 0usize;
    let spaces = count_spaces(bytes, i);
    if spaces.1 > 3 {
        return false;
    }
    i += spaces.0;
    let mut count = 0u8;
    while i < bytes.len() && bytes[i] == ch && count < 255 {
        count += 1;
        i += 1;
    }
    if count < n {
        return false;
    }
    while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t' || bytes[i] == b'\r') {
        i += 1;
    }
    i >= bytes.len()
}

fn scan_open_fence(bytes: &[u8]) -> Option<(u8, u8, usize)> {
    let spaces = count_spaces(bytes, 0);
    if spaces.1 > 3 {
        return None;
    }
    let mut i = spaces.0;
    let ch = *bytes.get(i)?;
    if ch != b'`' && ch != b'~' {
        return None;
    }
    let mut count = 0u8;
    while i < bytes.len() && bytes[i] == ch && count < 255 {
        count += 1;
        i += 1;
    }
    if count < 3 {
        return None;
    }
    if ch == b'`' && bytes[i..].contains(&b'`') {
        return None;
    }
    Some((ch, count, i))
}

fn is_atx_heading(bytes: &[u8]) -> bool {
    let spaces = count_spaces(bytes, 0);
    if spaces.1 > 3 {
        return false;
    }
    let mut i = spaces.0;
    let mut n = 0u8;
    while i < bytes.len() && bytes[i] == b'#' && n < 6 {
        n += 1;
        i += 1;
    }
    if n == 0 || n > 6 {
        return false;
    }
    i >= bytes.len() || bytes[i] == b' ' || bytes[i] == b'\t' || bytes[i] == b'\r'
}

fn is_setext_underline(bytes: &[u8]) -> bool {
    let t = trim_ascii(bytes);
    if t.is_empty() {
        return false;
    }
    let ch = t[0];
    if ch != b'=' && ch != b'-' {
        return false;
    }
    t.iter().all(|&c| c == ch)
}

fn is_thematic_break(bytes: &[u8]) -> bool {
    let spaces = count_spaces(bytes, 0);
    if spaces.1 > 3 {
        return false;
    }
    let mut i = spaces.0;
    let Some(&ch) = bytes.get(i) else {
        return false;
    };
    if ch != b'*' && ch != b'-' && ch != b'_' {
        return false;
    }
    let mut n = 0u8;
    while i < bytes.len() {
        let c = bytes[i];
        if c == ch {
            n = n.saturating_add(1);
            i += 1;
        } else if c == b' ' || c == b'\t' || c == b'\r' {
            i += 1;
        } else {
            return false;
        }
    }
    n >= 3
}

/// `(byte_count, visual_indent)` with tab = 4 columns.
fn count_spaces(bytes: &[u8], mut i: usize) -> (usize, usize) {
    let start = i;
    let mut cols = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b' ' => {
                cols += 1;
                i += 1;
            }
            b'\t' => {
                cols += 4 - (cols % 4);
                i += 1;
            }
            _ => break,
        }
    }
    (i - start, cols)
}

fn scan_list_marker(bytes: &[u8], i: usize) -> Option<usize> {
    let mut j = i;
    if matches!(bytes.get(j), Some(&b'-' | &b'*' | &b'+')) {
        j += 1;
        if bytes.get(j) == Some(&b' ') || bytes.get(j) == Some(&b'\t') || j >= bytes.len() {
            return Some(j.min(bytes.len()).max(i + 1));
        }
        return None;
    }
    if bytes.get(j).map(|c| c.is_ascii_digit()).unwrap_or(false) {
        while j < bytes.len() && bytes[j].is_ascii_digit() {
            j += 1;
        }
        if matches!(bytes.get(j), Some(&b'.' | &b')')) {
            j += 1;
            if bytes.get(j) == Some(&b' ') || bytes.get(j) == Some(&b'\t') || j >= bytes.len() {
                return Some(j.min(bytes.len()));
            }
        }
    }
    None
}

fn looks_like_html_block(bytes: &[u8], i: usize) -> bool {
    let rest = &bytes[i..];
    if rest.starts_with(b"<!--")
        || rest.starts_with(b"<?")
        || rest.starts_with(b"<![CDATA[")
        || rest.len() >= 2 && rest[0] == b'<' && rest[1] == b'!'
    {
        return true;
    }
    if rest.len() >= 2 && rest[0] == b'<' {
        let mut k = 1usize;
        if rest.get(k) == Some(&b'/') {
            k += 1;
        }
        rest.get(k).map(|c| c.is_ascii_alphabetic()).unwrap_or(false)
            && !rest[k..].starts_with(b"http")
    } else {
        false
    }
}

fn html_block_closed_on_line(bytes: &[u8], i: usize) -> bool {
    let rest = &bytes[i..];
    rest.windows(3).any(|w| w == b"-->")
        || rest.windows(2).any(|w| w == b"?>")
        || rest.windows(3).any(|w| w == b"]]>")
        || rest.contains(&b'>')
}

fn lex_inline(bytes: &[u8], mut i: usize, out: &mut Vec<(usize, TokenRole)>) {
    while i < bytes.len() {
        let start = i;
        let b = bytes[i];
        if b == b' ' || b == b'\t' {
            i += 1;
            while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
                i += 1;
            }
            push_role(i, TokenRole::Whitespace, out);
            continue;
        }
        if b == b'\\' && i + 1 < bytes.len() {
            i += 2;
            push_role(i, MarkdownKind::Text.role(), out);
            continue;
        }
        if b == b'`' {
            if let Some(end) = scan_code_span(bytes, i) {
                push_role(end, MarkdownKind::InlineCode.role(), out);
                i = end;
                continue;
            }
        }
        if b == b'$' {
            if let Some(end) = scan_math(bytes, i) {
                push_role(end, MarkdownKind::Math.role(), out);
                i = end;
                continue;
            }
        }
        if b == b'~' && bytes.get(i + 1) == Some(&b'~') {
            if let Some(end) = scan_delimited(bytes, i, b"~~") {
                push_role(end, MarkdownKind::Emphasis.role(), out);
                i = end;
                continue;
            }
        }
        if b == b'*' || b == b'_' {
            let marker = if bytes.get(i + 1) == Some(&b) {
                &bytes[i..i + 2]
            } else {
                &bytes[i..i + 1]
            };
            let left_ok = b != b'_' || i == 0 || !bytes[i - 1].is_ascii_alphanumeric();
            if left_ok {
                if let Some(end) = scan_delimited(bytes, i, marker) {
                    let right_ok =
                        b != b'_' || end >= bytes.len() || !bytes[end].is_ascii_alphanumeric();
                    if right_ok {
                        push_role(end, MarkdownKind::Emphasis.role(), out);
                        i = end;
                        continue;
                    }
                }
            }
        }
        if b == b'!' && bytes.get(i + 1) == Some(&b'[') {
            if let Some(end) = scan_link(bytes, i + 1) {
                push_role(end, MarkdownKind::Link.role(), out);
                i = end;
                continue;
            }
        }
        if b == b'[' {
            if let Some(end) = scan_link(bytes, i) {
                push_role(end, MarkdownKind::Link.role(), out);
                i = end;
                continue;
            }
        }
        if b == b'<' {
            if let Some(end) = scan_autolink_or_html(bytes, i) {
                let role = if bytes.get(i + 1).map(|c| *c == b'/' || c.is_ascii_alphabetic() || *c == b'!' || *c == b'?').unwrap_or(false)
                    && !is_autolink_start(&bytes[i..])
                {
                    MarkdownKind::Html.role()
                } else {
                    MarkdownKind::Link.role()
                };
                push_role(end, role, out);
                i = end;
                continue;
            }
        }
        i += 1;
        while i < bytes.len() {
            let c = bytes[i];
            if c == b' '
                || c == b'\t'
                || c == b'`'
                || c == b'$'
                || c == b'*'
                || c == b'_'
                || c == b'~'
                || c == b'['
                || c == b'!'
                || c == b'<'
                || c == b'\\'
            {
                break;
            }
            i += 1;
        }
        if i > start {
            push_role(i, MarkdownKind::Text.role(), out);
        }
    }
}

fn scan_code_span(bytes: &[u8], i: usize) -> Option<usize> {
    let mut n = 0usize;
    let mut j = i;
    while j < bytes.len() && bytes[j] == b'`' {
        n += 1;
        j += 1;
    }
    if n == 0 {
        return None;
    }
    let mut k = j;
    while k < bytes.len() {
        if bytes[k] == b'`' {
            let mut m = 0usize;
            while k + m < bytes.len() && bytes[k + m] == b'`' {
                m += 1;
            }
            if m == n {
                return Some(k + m);
            }
            k += m;
            continue;
        }
        k += 1;
    }
    None
}

fn scan_math(bytes: &[u8], i: usize) -> Option<usize> {
    let display = bytes.get(i + 1) == Some(&b'$');
    let open = if display { 2 } else { 1 };
    let mut j = i + open;
    if j >= bytes.len() {
        return None;
    }
    while j < bytes.len() {
        if bytes[j] == b'\\' && j + 1 < bytes.len() {
            j += 2;
            continue;
        }
        if bytes[j] == b'$' {
            if display {
                if bytes.get(j + 1) == Some(&b'$') {
                    return Some(j + 2);
                }
            } else {
                return Some(j + 1);
            }
        }
        j += 1;
    }
    None
}

fn scan_delimited(bytes: &[u8], i: usize, marker: &[u8]) -> Option<usize> {
    if marker.is_empty() || !bytes[i..].starts_with(marker) {
        return None;
    }
    let mut j = i + marker.len();
    if j >= bytes.len() {
        return None;
    }
    while j + marker.len() <= bytes.len() {
        if bytes[j] == b'\\' && j + 1 < bytes.len() {
            j += 2;
            continue;
        }
        if bytes[j..].starts_with(marker) && j > i + marker.len() {
            return Some(j + marker.len());
        }
        j += 1;
    }
    None
}

fn scan_link(bytes: &[u8], i: usize) -> Option<usize> {
    if bytes.get(i) != Some(&b'[') {
        return None;
    }
    let mut j = i + 1;
    let mut depth = 1i32;
    while j < bytes.len() && depth > 0 {
        match bytes[j] {
            b'\\' if j + 1 < bytes.len() => j += 2,
            b'[' => {
                depth += 1;
                j += 1;
            }
            b']' => {
                depth -= 1;
                j += 1;
            }
            _ => j += 1,
        }
    }
    if depth != 0 {
        return None;
    }
    if bytes.get(j) == Some(&b'(') {
        j += 1;
        let mut p = 1i32;
        while j < bytes.len() && p > 0 {
            match bytes[j] {
                b'\\' if j + 1 < bytes.len() => j += 2,
                b'(' => {
                    p += 1;
                    j += 1;
                }
                b')' => {
                    p -= 1;
                    j += 1;
                }
                _ => j += 1,
            }
        }
        if p == 0 {
            return Some(j);
        }
        return None;
    }
    if bytes.get(j) == Some(&b'[') {
        j += 1;
        while j < bytes.len() && bytes[j] != b']' {
            if bytes[j] == b'\\' && j + 1 < bytes.len() {
                j += 2;
            } else {
                j += 1;
            }
        }
        if bytes.get(j) == Some(&b']') {
            return Some(j + 1);
        }
        return None;
    }
    None
}

fn is_autolink_start(rest: &[u8]) -> bool {
    if rest.len() < 4 || rest[0] != b'<' {
        return false;
    }
    let inner = &rest[1..];
    inner.starts_with(b"http://")
        || inner.starts_with(b"https://")
        || inner.starts_with(b"mailto:")
        || inner.starts_with(b"ftp://")
        || inner.iter().any(|&c| c == b'@')
}

fn scan_autolink_or_html(bytes: &[u8], i: usize) -> Option<usize> {
    if bytes.get(i) != Some(&b'<') {
        return None;
    }
    if bytes[i..].starts_with(b"<!--") {
        return find_sub(bytes, i + 4, b"-->").map(|p| p + 3);
    }
    let mut j = i + 1;
    if j >= bytes.len() {
        return None;
    }
    let start_c = bytes[j];
    if start_c != b'/' && start_c != b'!' && start_c != b'?' && !start_c.is_ascii_alphabetic() {
        return None;
    }
    while j < bytes.len() {
        if bytes[j] == b'>' {
            return Some(j + 1);
        }
        if bytes[j] == b'<' {
            return None;
        }
        j += 1;
    }
    None
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

pub fn document_spans(_bytes: &[u8], tokens: &[MarkdownToken]) -> Vec<TokenSpan> {
    tokens
        .iter()
        .map(|t| TokenSpan::new(t.start, t.end, t.kind.role()))
        .collect()
}
