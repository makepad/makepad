//! Formatting commands over the source of rendered Markdown.

use crate::plan::{content_start, heading_start, line_at, Container, Containers, Leaf, Plan};
use crate::{Atom, Inline, InlineKind, PieceKind, Projection};
use pulldown_cmark::{Event, LinkType, Tag, TagEnd};
use std::ops::Range;

type Edits = Vec<(Range<usize>, String)>;

/// The inline marks a command toggles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    Emphasis,
    Strong,
    Strikethrough,
    Code,
}

/// What a block command makes of the blocks a range touches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockStyle {
    Paragraph,
    Heading(u8),
    Bullet,
    Numbered,
    Task,
    Quote,
    Code,
}

impl Plan {
    /// Maps an old source offset into the edited text, with right affinity.
    /// An offset inside a replacement lands at the replacement's end.
    pub fn map(&self, offset: usize) -> usize {
        let mut old = 0;
        let mut new = 0;
        for (range, replacement) in &self.edits {
            if offset < range.start {
                break;
            }
            new += range.start - old + replacement.len();
            old = range.end;
            if offset < range.end {
                return new;
            }
            // Include every insertion at this boundary, including one
            // immediately after a replacement ending here.
        }
        new + offset - old
    }
}

fn overlaps(a: &Range<usize>, b: &Range<usize>) -> bool {
    a.start < b.end && b.start < a.end
}

fn touched(block: &Range<usize>, range: &Range<usize>) -> bool {
    if range.is_empty() { block.contains(&range.start) } else { overlaps(block, range) }
}

fn line_end(text: &str, at: usize) -> usize {
    (line_at(text, at).end + 1).min(text.len())
}

fn last_char(text: &str, end: usize) -> usize {
    text[..end].char_indices().next_back().map_or(0, |(at, _)| at)
}

fn erase(edits: &mut Edits, range: Range<usize>) {
    if !range.is_empty() {
        edits.push((range, String::new()));
    }
}

fn insert(edits: &mut Edits, at: usize, value: impl Into<String>) {
    edits.push((at..at, value.into()));
}

fn finish(text: &str, mut edits: Edits, end: usize) -> Option<Plan> {
    edits.retain(|(range, value)| text[range.clone()] != *value);
    // Insertions precede erasures at the same offset. Stable sorting keeps
    // the order of opening/closing markers inserted at a shared boundary.
    edits.sort_by_key(|(range, _)| (range.start, range.end));
    let mut normalized: Edits = Vec::new();
    for (range, value) in edits {
        if let Some((previous, replacement)) = normalized.last_mut() {
            if range.start < previous.end {
                debug_assert!(value.is_empty() && replacement.is_empty());
                previous.end = previous.end.max(range.end);
                continue;
            }
            if range.is_empty() && *previous == range {
                replacement.push_str(&value);
                continue;
            }
        }
        normalized.push((range, value));
    }
    if normalized.is_empty() {
        return None;
    }
    let mut plan = Plan { edits: normalized, caret: 0 };
    plan.caret = plan.map(end);
    Some(plan.checked(text))
}

fn inline_tag(tag: &Tag<'_>) -> bool {
    matches!(tag, Tag::Emphasis | Tag::Strong | Tag::Strikethrough | Tag::Link { .. } | Tag::Image { .. })
}

// Tight lists omit Paragraph events. Their direct inline children form a
// leaf ending before a nested list or another block, never the whole item.
fn leaf_ranges(text: &str, events: &[(Event<'_>, Range<usize>)]) -> Vec<(Range<usize>, Leaf)> {
    let mut leaves = Vec::new();
    for (index, (event, range)) in events.iter().enumerate() {
        let leaf = match event {
            Event::Start(Tag::Paragraph) => Leaf::Paragraph,
            Event::Start(Tag::Heading { level, .. }) => Leaf::Heading(*level as u8),
            Event::Start(Tag::CodeBlock(kind)) => Leaf::Code { fenced: kind.is_fenced() },
            Event::Start(Tag::Table(_)) => Leaf::Table,
            Event::Start(Tag::HtmlBlock) => Leaf::Html,
            Event::Rule => Leaf::Other,
            Event::Start(Tag::Item) => {
                let mut depth = 0usize;
                let mut end = None;
                for (next, source) in &events[index + 1..] {
                    match next {
                        Event::Start(tag) if inline_tag(tag) => depth += 1,
                        Event::Start(_) => break,
                        Event::End(_) if depth == 0 => break,
                        Event::End(_) => depth -= 1,
                        _ => {}
                    }
                    end = Some(end.map_or(source.end, |end: usize| end.max(source.end)));
                }
                if let Some(end) = end {
                    leaves.push((range.start..line_end(text, last_char(text, end)), Leaf::Paragraph));
                } else if matches!(events.get(index + 1), Some((Event::End(TagEnd::Item), _))) {
                    leaves.push((range.clone(), Leaf::Paragraph));
                }
                continue;
            }
            _ => continue,
        };
        leaves.push((range.clone(), leaf));
    }
    leaves
}

struct Selection {
    source: Range<usize>,
    visible: Range<usize>,
    block: Range<usize>,
}

fn selections(
    text: &str,
    events: &[(Event<'_>, Range<usize>)],
    projection: &Projection,
    range: &Range<usize>,
    trim: bool,
) -> Vec<Selection> {
    let mut out = Vec::new();
    if range.is_empty() {
        return out;
    }
    for (block, leaf) in leaf_ranges(text, events) {
        if !overlaps(&block, range) || matches!(leaf, Leaf::Code { .. } | Leaf::Html | Leaf::Other) {
            continue;
        }
        let mut selected: Option<Selection> = None;
        let from = projection.pieces.partition_point(|piece| piece.src.end <= block.start);
        for piece in projection.pieces[from..].iter().take_while(|piece| piece.src.start < block.end) {
            if !overlaps(&piece.src, &block) || !overlaps(&piece.src, range)
                || matches!(piece.kind, PieceKind::Atomic(Atom::BlockBreak | Atom::TaskMarker(_) | Atom::Rule))
            {
                continue;
            }
            let (mut source, mut visible) = match piece.kind {
                PieceKind::Text => {
                    let source = piece.src.start.max(range.start)..piece.src.end.min(range.end);
                    let visible = piece.visible.start + source.start - piece.src.start
                        ..piece.visible.start + source.end - piece.src.start;
                    (source, visible)
                }
                PieceKind::Atomic(_) => (piece.src.clone(), piece.visible.clone()),
            };
            if trim {
                let value = &projection.text[visible.clone()];
                if value.trim().is_empty() {
                    continue;
                }
                if piece.kind == PieceKind::Text {
                    let left = value.len() - value.trim_start().len();
                    let right = value.len() - value.trim_end().len();
                    source.start += left;
                    source.end -= right;
                    visible.start += left;
                    visible.end -= right;
                }
            }
            if let Some(selected) = &mut selected {
                selected.source.end = source.end;
                selected.visible.end = visible.end;
            } else {
                selected = Some(Selection { source, visible, block: block.clone() });
            }
        }
        out.extend(selected);
    }
    out
}

fn visible_inline(p: &Projection, inline: &Inline) -> Range<usize> {
    p.to_visible(inline.open.end)..p.to_visible(inline.close.start)
}

fn marked(p: &Projection, range: &Range<usize>, kind: InlineKind) -> bool {
    let mut end = range.start;
    for inline in p.inlines.iter().filter(|inline| inline.kind == kind) {
        let span = visible_inline(p, inline);
        if span.start <= end {
            end = end.max(span.end);
        }
        if end >= range.end {
            return true;
        }
    }
    false
}

fn slide_left(text: &str, at: usize, limit: usize) -> usize {
    limit + text[limit..at].trim_end().len()
}

fn slide_right(text: &str, at: usize, limit: usize) -> usize {
    limit - text[at..limit].trim_start().len()
}

fn intraword(text: &str, at: usize) -> bool {
    text[..at].chars().next_back().is_some_and(char::is_alphanumeric)
        && text[at..].chars().next().is_some_and(char::is_alphanumeric)
}

fn split_marker(text: &str, inline: &Inline, at: usize, open: bool) -> String {
    if inline.kind != InlineKind::Code {
        let marker = &text[if open { inline.open.clone() } else { inline.close.clone() }];
        return if marker.starts_with('_') && intraword(text, at) {
            "*".repeat(marker.len())
        } else {
            marker.to_string()
        };
    }
    let ticks = text[inline.open.clone()].bytes().take_while(|&b| b == b'`').count();
    let pad = if open { text[at..].starts_with('`') } else { text[..at].ends_with('`') };
    let marker = "`".repeat(ticks);
    match (open, pad) {
        (true, true) => marker + " ",
        (false, true) => " ".to_string() + &marker,
        _ => marker,
    }
}

fn remove_mark(text: &str, p: &Projection, selection: &Selection, kind: InlineKind, edits: &mut Edits) {
    for inline in p.inlines.iter().filter(|inline| inline.kind == kind) {
        let span = visible_inline(p, inline);
        if !overlaps(&span, &selection.visible) {
            continue;
        }
        if span.start < selection.visible.start {
            let at = slide_left(text, selection.source.start, inline.open.end);
            let marker = split_marker(text, inline, at, false);
            if inline.kind != InlineKind::Code && marker != text[inline.close.clone()] {
                edits.push((inline.open.clone(), marker.clone()));
            }
            insert(edits, at, marker);
        } else {
            erase(edits, inline.open.clone());
        }
        if selection.visible.end < span.end {
            let at = slide_right(text, selection.source.end, inline.close.start);
            let marker = split_marker(text, inline, at, true);
            if inline.kind != InlineKind::Code && marker != text[inline.open.clone()] {
                edits.push((inline.close.clone(), marker.clone()));
            }
            insert(edits, at, marker);
        } else {
            erase(edits, inline.close.clone());
        }
    }
}

fn absorb(p: &Projection, selection: &mut Selection, kind: InlineKind, edits: &mut Edits) {
    // A newly included outer container can expose another overlapping one.
    loop {
        let before = selection.visible.clone();
        for inline in p.inlines.iter().filter(|inline| inline.kind == kind) {
            let span = visible_inline(p, inline);
            if overlaps(&span, &selection.visible) {
                selection.visible.start = selection.visible.start.min(span.start);
                selection.visible.end = selection.visible.end.max(span.end);
                selection.source.start = selection.source.start.min(inline.open.start);
                selection.source.end = selection.source.end.max(inline.close.end);
            }
        }
        if before == selection.visible {
            break;
        }
    }
    for inline in p.inlines.iter().filter(|inline| inline.kind == kind) {
        if overlaps(&visible_inline(p, inline), &selection.visible) {
            erase(edits, inline.open.clone());
            erase(edits, inline.close.clone());
        }
    }
}

fn include_markers(p: &Projection, selection: &mut Selection) {
    for inline in &p.inlines {
        let span = visible_inline(p, inline);
        if !span.is_empty() && selection.visible.start <= span.start && span.end <= selection.visible.end
            && selection.block.start <= inline.open.start && inline.close.end <= selection.block.end
        {
            selection.source.start = selection.source.start.min(inline.open.start);
            selection.source.end = selection.source.end.max(inline.close.end);
        }
    }
}

fn longest_ticks(text: &str) -> usize {
    let mut longest = 0;
    let mut run = 0;
    for byte in text.bytes() {
        run = if byte == b'`' { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    longest
}

fn source_without(text: &str, range: Range<usize>, edits: &Edits) -> String {
    let mut erased: Vec<_> = edits.iter().filter_map(|(source, value)| {
        (value.is_empty() && overlaps(source, &range)).then_some(source.clone())
    }).collect();
    erased.sort_by_key(|range| range.start);
    let mut out = String::new();
    let mut at = range.start;
    for erased in erased {
        let start = erased.start.max(at);
        out.push_str(&text[at..start]);
        at = at.max(erased.end.min(range.end));
    }
    out.push_str(&text[at..range.end]);
    out
}

fn mark_delimiters(text: &str, p: &Projection, selection: &Selection, mark: Mark, edits: &Edits)
    -> (String, String)
{
    if mark == Mark::Code {
        let content = source_without(text, selection.source.clone(), edits);
        let ticks = "`".repeat(longest_ticks(&content) + 1);
        return if content.starts_with('`') || content.ends_with('`') {
            (ticks.clone() + " ", " ".to_string() + &ticks)
        } else {
            (ticks.clone(), ticks)
        };
    }
    let underscore = !intraword(text, selection.source.start) && !intraword(text, selection.source.end)
        && p.inlines.iter().any(|inline| {
            inline.kind == mark_kind(mark) && overlaps(&selection.block, &inline.open)
                && text[inline.open.clone()].starts_with('_')
        });
    let marker = match mark {
        Mark::Emphasis => if underscore { "_" } else { "*" },
        Mark::Strong => if underscore { "__" } else { "**" },
        Mark::Strikethrough => "~~",
        Mark::Code => unreachable!(),
    };
    (marker.to_string(), marker.to_string())
}

fn mark_kind(mark: Mark) -> InlineKind {
    match mark {
        Mark::Emphasis => InlineKind::Emphasis,
        Mark::Strong => InlineKind::Strong,
        Mark::Strikethrough => InlineKind::Strikethrough,
        Mark::Code => InlineKind::Code,
    }
}

/// Toggles a mark on visible characters, independently within each leaf block.
/// Returns `None` for a caret or a selection without visible content.
pub fn toggle_inline(
    text: &str, events: &[(Event<'_>, Range<usize>)], range: Range<usize>, mark: Mark,
) -> Option<Plan> {
    let p = Projection::new(text, events);
    let selected = selections(text, events, &p, &range, true);
    let kind = mark_kind(mark);
    let remove = selected.iter().all(|selection| marked(&p, &selection.visible, kind));
    let mut edits = Vec::new();
    for mut selection in selected {
        include_markers(&p, &mut selection);
        if remove {
            remove_mark(text, &p, &selection, kind, &mut edits);
            continue;
        }
        absorb(&p, &mut selection, kind, &mut edits);
        if mark == Mark::Code {
            for kind in [InlineKind::Emphasis, InlineKind::Strong, InlineKind::Strikethrough] {
                remove_mark(text, &p, &selection, kind, &mut edits);
            }
        }
        include_markers(&p, &mut selection);
        let (open, close) = mark_delimiters(text, &p, &selection, mark, &edits);
        insert(&mut edits, selection.source.start, open);
        let at = selection.source.end;
        let before = edits.iter().position(|(range, _)| range.is_empty() && range.start == at)
            .unwrap_or(edits.len());
        edits.insert(before, (at..at, close));
    }
    finish(text, edits, range.end)
}

fn link_event(events: &[(Event<'_>, Range<usize>)], at: usize) -> Option<usize> {
    events.iter().position(|(event, range)| {
        matches!(event, Event::Start(Tag::Link { .. })) && range.contains(&at)
    })
}

/// The parser-resolved URL of the inline or reference link holding `at`.
pub fn link_at(text: &str, events: &[(Event<'_>, Range<usize>)], at: usize) -> Option<String> {
    debug_assert!(text.is_char_boundary(at));
    let index = link_event(events, at)?;
    let Event::Start(Tag::Link { dest_url, .. }) = &events[index].0 else { return None };
    Some(dest_url.to_string())
}

fn destination(url: &str) -> String {
    let mut out = String::new();
    let angle = url.chars().any(char::is_whitespace);
    if angle {
        out.push('<');
    }
    for ch in url.chars() {
        match ch {
            '\n' => out.push_str("%0A"),
            '\r' => out.push_str("%0D"),
            '&' => out.push_str("&amp;"),
            '\\' | '(' | ')' | '<' | '>' => { out.push('\\'); out.push(ch); }
            _ => out.push(ch),
        }
    }
    if angle {
        out.push('>');
    }
    out
}

// Scan only the existing destination token; the parser has already
// validated the link. Its separator, title and title delimiter stay exact.
fn destination_range(text: &str, close: &Range<usize>) -> Range<usize> {
    let mut at = close.start + 2;
    while at < close.end && text.as_bytes()[at].is_ascii_whitespace() {
        at += 1;
    }
    let start = at;
    let angle = text.as_bytes().get(at) == Some(&b'<');
    if angle {
        at += 1;
    }
    let mut depth = 0usize;
    while at < close.end {
        let byte = text.as_bytes()[at];
        if byte == b'\\' && at + 1 < close.end && text.as_bytes()[at + 1].is_ascii_punctuation() {
            at += 2;
            continue;
        }
        if angle {
            at += 1;
            if byte == b'>' {
                break;
            }
            continue;
        }
        match byte {
            b'(' => depth += 1,
            b')' if depth > 0 => depth -= 1,
            b')' => break,
            b if b.is_ascii_whitespace() => break,
            _ => {}
        }
        at += 1;
    }
    start..at
}

/// Adds or updates a link, or removes the link holding the selection start.
/// Reference replacements affect this occurrence, keeping its resolved title.
pub fn set_link(
    text: &str, events: &[(Event<'_>, Range<usize>)], range: Range<usize>, url: Option<&str>,
) -> Option<Plan> {
    let p = Projection::new(text, events);
    let existing = link_event(events, range.start);
    let mut edits = Vec::new();
    if let Some(index) = existing {
        let (Event::Start(Tag::Link { link_type, title, .. }), source) = &events[index] else {
            return None;
        };
        let inline = p.inlines.iter().find(|inline| {
            inline.kind == InlineKind::Link && inline.open.start == source.start
        })?;
        // A collapsed reference's `[]` lies outside the link's range.
        let mut close = inline.close.clone();
        let collapsed = matches!(link_type, LinkType::Collapsed | LinkType::CollapsedUnknown);
        if collapsed && text[close.end..].starts_with("[]") {
            close.end += 2;
        }
        if url.is_none() {
            erase(&mut edits, inline.open.clone());
            erase(&mut edits, close);
            return finish(text, edits, range.end);
        }
        if range.end <= source.end {
            let url = destination(url?);
            if *link_type == LinkType::Inline {
                edits.push((destination_range(text, &inline.close), url));
            } else {
                let title = if title.is_empty() { String::new() } else {
                    format!(" \"{}\"", title.replace('&', "&amp;").replace('\\', "\\\\").replace('"', "\\\""))
                };
                if matches!(link_type, LinkType::Autolink | LinkType::Email) {
                    edits.push((inline.open.clone(), "[".to_string()));
                    erase(&mut edits, inline.close.clone());
                    insert(&mut edits, inline.close.end, format!("]({url}{title})"));
                } else {
                    let suffix = close.start + 1..close.end;
                    edits.push((suffix, format!("({url}{title})")));
                }
            }
            return finish(text, edits, range.end);
        }
    }
    let url = destination(url?);
    for mut selection in selections(text, events, &p, &range, false) {
        absorb(&p, &mut selection, InlineKind::Link, &mut edits);
        include_markers(&p, &mut selection);
        insert(&mut edits, selection.source.start, "[");
        insert(&mut edits, selection.source.end, format!("]({url})"));
    }
    finish(text, edits, range.end)
}

struct Item {
    range: Range<usize>,
    event: usize,
    parent: Option<usize>,
    previous: Option<usize>,
}

fn items(events: &[(Event<'_>, Range<usize>)]) -> Vec<Item> {
    let mut out = Vec::new();
    let mut parents = Vec::new();
    let mut lists = Vec::new();
    for (index, (event, range)) in events.iter().enumerate() {
        match event {
            Event::Start(Tag::List(_)) => lists.push(None),
            Event::Start(Tag::Item) => {
                let previous = lists.last_mut().expect("an item belongs to a list");
                let item = out.len();
                out.push(Item {
                    range: range.clone(), event: index, parent: parents.last().copied(), previous: *previous,
                });
                *previous = Some(item);
                parents.push(item);
            }
            Event::End(TagEnd::Item) => { parents.pop(); }
            Event::End(TagEnd::List(_)) => { lists.pop(); }
            _ => {}
        }
    }
    out
}

fn item_at(items: &[Item], at: usize) -> Option<usize> {
    items.iter().rposition(|item| item.range.contains(&at))
}

fn task_marker(events: &[(Event<'_>, Range<usize>)], item: &Item) -> Option<(bool, Range<usize>)> {
    let mut at = item.event + 1;
    if matches!(events.get(at), Some((Event::Start(Tag::Paragraph), _))) {
        at += 1;
    }
    match events.get(at) {
        Some((Event::TaskListMarker(checked), range)) => Some((*checked, range.clone())),
        _ => None,
    }
}

/// Flips the one byte inside the task box of the item holding `at`.
pub fn toggle_task(text: &str, events: &[(Event<'_>, Range<usize>)], at: usize) -> Option<Plan> {
    let items = items(events);
    let item = &items[item_at(&items, at)?];
    let (checked, marker) = task_marker(events, item)?;
    let byte = marker.end - 2;
    finish(text, vec![(byte..byte + 1, if checked { " " } else { "x" }.to_string())], at)
}

struct Block {
    range: Range<usize>,
    raw: Range<usize>,
    containers: Containers,
    item: Option<usize>,
    quote: Option<usize>,
}

fn blocks(
    text: &str, events: &[(Event<'_>, Range<usize>)], range: &Range<usize>, items: &[Item],
) -> Vec<Block> {
    leaf_ranges(text, events).into_iter().filter_map(|(source, leaf)| {
        let raw = line_at(text, source.start).start..line_end(text, last_char(text, source.end));
        if !touched(&raw, range) {
            return None;
        }
        let mut containers = Containers::at(text, events, source.start);
        containers.leaf = leaf;
        containers.leaf_range = source.clone();
        containers.line = line_at(text, source.start);
        let item = item_at(items, source.start);
        let quote = events.iter().rposition(|(event, range)| {
            matches!(event, Event::Start(Tag::BlockQuote(_))) && range.contains(&source.start)
        });
        Some(Block { range: source, raw, containers, item, quote })
    }).collect()
}

fn lines(text: &str, range: Range<usize>) -> impl Iterator<Item = Range<usize>> + '_ {
    let mut at = range.start;
    std::iter::from_fn(move || {
        if at >= range.end {
            return None;
        }
        let line = line_at(text, at);
        at = (line.end + 1).min(text.len());
        Some(line)
    })
}

fn atx_prefix(text: &str, block: &Block) -> Range<usize> {
    let start = block.containers.content_start(text).max(block.range.start);
    if matches!(block.containers.leaf, Leaf::Heading(_)) {
        start..heading_start(text, &block.containers).max(start)
    } else {
        start..start
    }
}

fn remove_setext(text: &str, block: &Block, edits: &mut Edits) {
    if matches!(block.containers.leaf, Leaf::Heading(_)) && atx_prefix(text, block).is_empty() {
        let line = line_at(text, last_char(text, block.range.end));
        if line.start > block.raw.start {
            erase(edits, line.start..line_end(text, line.start));
        }
    }
}

fn join_soft_lines(text: &str, events: &[(Event<'_>, Range<usize>)], block: &Block, edits: &mut Edits) {
    for (event, range) in events {
        if !matches!(event, Event::SoftBreak | Event::HardBreak) || !overlaps(range, &block.range) {
            continue;
        }
        let first = line_at(text, range.start);
        let next = line_at(text, range.end);
        let start = first.start + text[first.start..range.start].trim_end_matches([' ', '\t']).len();
        let end = content_start(text, next.clone(), &block.containers.stack);
        let end = end + text[end..next.end].bytes().take_while(|b| matches!(b, b' ' | b'\t')).count();
        edits.push((start..end, " ".to_string()));
    }
}

fn item_prefix(text: &str, events: &[(Event<'_>, Range<usize>)], item: &Item) -> (Range<usize>, Containers) {
    let containers = Containers::at(text, events, item.range.start);
    let last = containers.stack.iter().rposition(|container| matches!(container, Container::Item { .. }))
        .expect("item has a container");
    let Container::Item { indent, .. } = containers.stack[last] else { unreachable!() };
    let start = content_start(text, containers.line.clone(), &containers.stack[..last]) + indent;
    let mut end = content_start(text, containers.line.clone(), &containers.stack[..=last]);
    if let Some((_, marker)) = task_marker(events, item) {
        end = marker.end;
        end += text[end..containers.line.end].bytes().take_while(|b| matches!(b, b' ' | b'\t')).count();
    }
    (start..end, containers)
}

fn list_style(events: &[(Event<'_>, Range<usize>)], item: &Item, containers: &Containers) -> BlockStyle {
    if task_marker(events, item).is_some() {
        BlockStyle::Task
    } else if containers.stack.iter().rev().find_map(|container| {
        if let Container::Item { number, .. } = container { Some(number.is_some()) } else { None }
    }) == Some(true) {
        BlockStyle::Numbered
    } else {
        BlockStyle::Bullet
    }
}

fn separate_items(
    text: &str, events: &[(Event<'_>, Range<usize>)], items: &[Item], removed: &[usize], edits: &mut Edits,
) {
    for pair in removed.windows(2) {
        let previous = &items[pair[0]];
        let next = &items[pair[1]];
        if next.previous != Some(pair[0]) || previous.parent != next.parent {
            continue;
        }
        let (_, mut containers) = item_prefix(text, events, next);
        containers.stack.pop();
        let start = containers.line.start;
        let preceding = line_at(text, last_char(text, start));
        let content = content_start(text, preceding.clone(), &containers.stack);
        if !text[content..preceding.end].trim().is_empty() {
            insert(edits, start, containers.blank_continuation() + "\n");
        }
    }
}

fn quote_blocks(text: &str, selected: &[Block], edits: &mut Edits) {
    let remove = selected.iter().all(|block| block.containers.stack.contains(&Container::Quote));
    for block in selected {
        let stack = &block.containers.stack;
        let level = if remove {
            stack.iter().rposition(|container| *container == Container::Quote).unwrap()
        } else {
            stack.iter().rposition(|container| matches!(container, Container::Item { .. })).unwrap_or(stack.len())
        };
        for line in lines(text, block.raw.clone()) {
            let start = content_start(text, line.clone(), &stack[..level]);
            if remove {
                let at = start + text[start..line.end].bytes().take_while(|&b| b == b' ').count();
                if text.as_bytes().get(at) == Some(&b'>') {
                    let end = at + 1 + usize::from(text.as_bytes().get(at + 1) == Some(&b' '));
                    erase(edits, at..end);
                }
            } else {
                insert(edits, start, "> ");
            }
        }
    }
}

fn unindent_code(text: &str, block: &Block, edits: &mut Edits) {
    for line in lines(text, block.raw.clone()) {
        let start = content_start(text, line.clone(), &block.containers.stack);
        let mut end = start;
        let mut width = 0;
        while end < line.end && width < 4 {
            match text.as_bytes()[end] {
                b' ' => width += 1,
                b'\t' => width = 4,
                _ => break,
            }
            end += 1;
        }
        erase(edits, start..end);
    }
}

fn unwrap_code(text: &str, block: &Block, edits: &mut Edits) {
    if block.containers.leaf == (Leaf::Code { fenced: false }) {
        unindent_code(text, block, edits);
        return;
    }
    let first = block.containers.line.clone();
    let start = block.containers.content_start(text);
    let fence = text[start..first.end].trim_start_matches(' ');
    let Some(byte) = fence.bytes().next() else { return };
    let width = fence.bytes().take_while(|&b| b == byte).count();
    let last = line_at(text, last_char(text, block.raw.end));
    let end = content_start(text, last.clone(), &block.containers.stack);
    let closing = text[end..last.end].trim();
    let closed = last.start > first.start && closing.len() >= width
        && closing.bytes().all(|b| b == byte);
    let body_end = if closed { last.start } else { block.raw.end };
    let next = line_end(text, first.start);
    if next < body_end {
        let next = line_at(text, next);
        let content = content_start(text, next, &block.containers.stack);
        erase(edits, start..content);
    } else {
        edits.push((start..body_end, "\n".to_string()));
    }
    if closed {
        erase(edits, last.start..block.raw.end);
    }
}

fn wrap_code(text: &str, selected: &[Block], edits: &mut Edits) {
    let mut from = 0;
    while from < selected.len() {
        let first = &selected[from];
        let mut to = from + 1;
        while to < selected.len() && selected[to].containers.stack == first.containers.stack
            && selected[to].item == first.item && selected[to].quote == first.quote
        {
            to += 1;
        }
        let last = &selected[to - 1];
        let mut width = 3;
        for block in &selected[from..to] {
            if block.containers.leaf == (Leaf::Code { fenced: false }) {
                unindent_code(text, block, edits);
            }
        }
        for line in lines(text, first.raw.start..last.raw.end) {
            let start = content_start(text, line.clone(), &first.containers.stack);
            let run = text[start..line.end].trim_start_matches([' ', '\t']).bytes()
                .take_while(|&b| b == b'`').count();
            width = width.max(run + 1);
        }
        let fence = "`".repeat(width);
        let prefix = first.containers.continuation();
        let start = first.containers.content_start(text);
        insert(edits, start, format!("{fence}\n{prefix}"));
        insert(edits, last.raw.end, format!("{prefix}{fence}\n"));
        from = to;
    }
}

/// Sets the touched leaf blocks' prefixes; quote, fenced code and existing list kinds toggle off.
pub fn set_block(
    text: &str, events: &[(Event<'_>, Range<usize>)], range: Range<usize>, style: BlockStyle,
) -> Option<Plan> {
    if matches!(style, BlockStyle::Heading(level) if !(1..=6).contains(&level)) {
        return None;
    }
    let items = items(events);
    let selected = blocks(text, events, &range, &items);
    if selected.is_empty() || selected.iter().any(|block| {
        matches!(block.containers.leaf, Leaf::Table | Leaf::Html | Leaf::Other)
    }) {
        return None;
    }
    let mut edits = Vec::new();
    if style == BlockStyle::Quote {
        quote_blocks(text, &selected, &mut edits);
    } else if style == BlockStyle::Code {
        if selected.iter().all(|block| block.containers.leaf == (Leaf::Code { fenced: true })) {
            for block in &selected {
                unwrap_code(text, block, &mut edits);
            }
        } else {
            wrap_code(text, &selected, &mut edits);
        }
    } else {
        let list = matches!(style, BlockStyle::Bullet | BlockStyle::Numbered | BlockStyle::Task);
        let remove = list && selected.iter().all(|block| block.item.is_some_and(|item| {
            list_style(events, &items[item], &block.containers) == style
        }));
        let mut changed_items = Vec::new();
        let mut removed_items = Vec::new();
        let mut number = 1usize;
        for block in &selected {
            if matches!(block.containers.leaf, Leaf::Code { .. }) {
                if style == BlockStyle::Paragraph {
                    unwrap_code(text, block, &mut edits);
                }
                continue;
            }
            remove_setext(text, block, &mut edits);
            let mut heading = atx_prefix(text, block);
            if let BlockStyle::Heading(level) = style {
                if let Some((_, marker)) = block.item.and_then(|item| task_marker(events, &items[item])) {
                    if heading.start <= marker.start && marker.end <= block.containers.line.end {
                        let after = &text[marker.end..block.containers.line.end];
                        heading.end = marker.end + after.len() - after.trim_start_matches([' ', '\t']).len();
                    }
                }
                edits.push((heading, "#".repeat(level as usize) + " "));
                join_soft_lines(text, events, block, &mut edits);
                continue;
            }
            erase(&mut edits, heading);
            let marker = match style {
                BlockStyle::Bullet => "- ".to_string(),
                BlockStyle::Numbered => format!("{number}. "),
                BlockStyle::Task => "- [ ] ".to_string(),
                _ => String::new(),
            };
            if let Some(item) = block.item {
                if changed_items.contains(&item) {
                    continue;
                }
                changed_items.push(item);
                let (prefix, _) = item_prefix(text, events, &items[item]);
                if remove || style == BlockStyle::Paragraph {
                    erase(&mut edits, prefix);
                    removed_items.push(item);
                } else {
                    let marker = if style == BlockStyle::Task {
                        task_marker(events, &items[item]).map_or(marker, |(checked, _)| {
                            if checked { "- [x] ".to_string() } else { "- [ ] ".to_string() }
                        })
                    } else { marker };
                    edits.push((prefix, marker));
                }
            } else if list {
                insert(&mut edits, block.containers.content_start(text), marker);
            }
            number += 1;
        }
        separate_items(text, events, &items, &removed_items, &mut edits);
    }
    finish(text, edits, range.end)
}

/// Indents selected items under their preceding sibling, or outdents to the parent marker column.
/// Descendant lines move with their selected ancestor, exactly once.
pub fn indent_items(
    text: &str, events: &[(Event<'_>, Range<usize>)], range: Range<usize>, outdent: bool,
) -> Option<Plan> {
    let items = items(events);
    let mut selected = Vec::new();
    if range.is_empty() {
        selected.push(item_at(&items, range.start)?);
    } else {
        for block in blocks(text, events, &range, &items) {
            let item = block.item?;
            if !selected.contains(&item) {
                selected.push(item);
            }
        }
    }
    // An ancestor already carries all of a selected descendant's lines.
    let roots: Vec<_> = selected.iter().copied().filter(|&index| {
        let mut parent = items[index].parent;
        while let Some(index) = parent {
            if selected.contains(&index) {
                return false;
            }
            parent = items[index].parent;
        }
        true
    }).collect();
    let mut edits = Vec::new();
    for &index in &roots {
        let item = &items[index];
        let (_, containers) = item_prefix(text, events, item);
        let stack = &containers.stack;
        let level = stack.iter().rposition(|container| matches!(container, Container::Item { .. }))?;
        let Container::Item { indent, .. } = stack[level] else { return None };
        let (outer, width) = if outdent {
            item.parent?;
            let parent = stack[..level].iter().rposition(|container| matches!(container, Container::Item { .. }))?;
            let Container::Item { width, .. } = stack[parent] else { return None };
            (&stack[..parent], width + indent)
        } else {
            let mut previous = item.previous?;
            while roots.contains(&previous) {
                previous = items[previous].previous?;
            }
            let (_, sibling) = item_prefix(text, events, &items[previous]);
            let Container::Item { indent: padding, width, .. } = sibling.stack.last()? else { return None };
            (&stack[..level], (padding + width).saturating_sub(indent))
        };
        let raw = containers.line.start..line_end(text, last_char(text, item.range.end));
        for line in lines(text, raw) {
            let start = content_start(text, line.clone(), outer);
            if outdent {
                let spaces = text[start..line.end].bytes().take_while(|&b| b == b' ').count();
                erase(&mut edits, start..start + spaces.min(width));
            } else if !text[start..line.end].trim().is_empty() {
                // A blank line between the item's blocks stays blank.
                insert(&mut edits, start, " ".repeat(width));
            }
        }
    }
    finish(text, edits, range.end)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_options;
    use pulldown_cmark::Parser;

    /// Runs `check` on every case and fails once, naming every case that broke.
    fn all<T>(cases: impl IntoIterator<Item = T>, check: impl Fn(T)) {
        let mut failed = Vec::new();
        for case in cases {
            if let Err(panic) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| check(case))) {
                let message = panic
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
                    .unwrap_or_default();
                failed.push(message.split("\nevents:").next().unwrap_or("").to_string()
                    + &message.find("  left:").map_or(String::new(), |at| message[at..].to_string()));
            }
        }
        assert!(failed.is_empty(), "{} cases broke:\n{}", failed.len(), failed.join("\n---\n"));
    }

    fn selection(before: &str) -> (String, Range<usize>) {
        if let Some(at) = before.find('‸') {
            (before.replace('‸', ""), at..at)
        } else {
            let start = before.find('‹').expect("case has a selection start");
            let end = before.find('›').expect("case has a selection end") - '‹'.len_utf8();
            (before.replace(['‹', '›'], ""), start..end)
        }
    }

    fn case(
        rule: &str, before: &str, expected: &str,
        edit: impl FnOnce(&str, &[(Event<'_>, Range<usize>)], Range<usize>) -> Option<Plan>,
    ) {
        let (text, range) = selection(before);
        assert!(text.ends_with('\n'), "{rule}: slice lines end in a newline");
        let mut parser = Parser::new_ext(&text, parse_options());
        parser.assume_footnote_labels_defined();
        let events: Vec<_> = parser.into_offset_iter().collect();
        let plan = edit(&text, &events, range.clone());
        let mut actual = plan.as_ref().map_or_else(|| text.clone(), |plan| plan.apply(&text));
        if let Some(plan) = &plan {
            assert_eq!(plan.caret, plan.map(range.end), "{rule}: caret maps the original selection end");
            let mut previous = 0;
            for offset in text.char_indices().map(|(at, _)| at).chain([text.len()]) {
                let mapped = plan.map(offset);
                assert!(mapped >= previous && actual.is_char_boundary(mapped), "{rule}: valid ordered map");
                previous = mapped;
            }
        }
        actual.insert_str(plan.as_ref().map_or(range.end, |plan| plan.caret), "‸");
        assert_eq!(actual, expected, "{rule}\nevents: {events:?}");
        if text == expected.replace('‸', "") {
            assert!(plan.is_none(), "{rule}: an inert command returns None");
        }
    }

    #[test]
    fn inline_marks_toggle() {
        let cases = [
            ("bold a word", "a ‹word› z\n", "a **word**‸ z\n", Mark::Strong),
            ("unbold a word", "**‹word›**\n", "word‸\n", Mark::Strong),
            ("split strong in the middle", "**ab‹c›de**\n", "**ab**c**‸de**\n", Mark::Strong),
            ("underscore split stays marked intraword", "__ab‹c›de__\n", "**ab**c**‸de**\n", Mark::Strong),
            ("absorb straddling strong", "**ab‹cd** ef›\n", "**abcd ef**‸\n", Mark::Strong),
            ("remove contained strong before wrapping", "‹a **b** c›\n", "**a b c**‸\n", Mark::Strong),
            ("trim selection whitespace", "‹ ab ›\n", " **ab** ‸\n", Mark::Strong),
            ("split markers flank outside spaces", "**ab ‹cd› ef**\n", "**ab** cd‸ **ef**\n", Mark::Strong),
            ("split outside a complete nested mark", "**ab *‹cd›* ef**\n",
                "**ab** *cd‸* **ef**\n", Mark::Strong),
            ("intraword emphasis uses stars", "a‹b›c _x_\n", "a*b*‸c _x_\n", Mark::Emphasis),
            ("reuse underscore emphasis", "_a_ ‹b›\n", "_a_ _b_‸\n", Mark::Emphasis),
            ("reuse underscore strong", "__a__ ‹b›\n", "__a__ __b__‸\n", Mark::Strong),
            ("strike a word", "‹gone›\n", "~~gone~~‸\n", Mark::Strikethrough),
            ("code exceeds inner tick run", "‹a`b›\n", "``a`b``‸\n", Mark::Code),
            ("code pads edge backtick", "‹`a›\n", "`` `a ``‸\n", Mark::Code),
            ("code removes inner strong", "‹a **b**›\n", "`a b`‸\n", Mark::Code),
            ("code removes all inner marks", "‹*a* **b** ~~c~~›\n", "`a b c`‸\n", Mark::Code),
            ("code splits outside strong", "**ab‹c›de**\n", "**ab**`c`**‸de**\n", Mark::Code),
            ("code toggles off", "`‹ab›`\n", "ab‸\n", Mark::Code),
            ("code removes padding too", "`` ‹`a› ``\n", "`a‸\n", Mark::Code),
            ("mark each paragraph separately", "‹one\n\ntwo›\n", "**one**\n\n**two**‸\n", Mark::Strong),
            ("unmark across paragraphs", "**‹one**\n\n**two›**\n", "one\n\ntwo‸\n", Mark::Strong),
            ("tight list leaves stay separate", "- ‹a\n- b›\n", "- **a**\n- **b**‸\n", Mark::Strong),
            ("wrap whole link markers", "‹[a](u)›\n", "**[a](u)**‸\n", Mark::Strong),
            ("atomic entity selection snaps", "x&‹am›p;y\n", "x**&am‸p;**y\n", Mark::Strong),
            ("UTF-8 source boundaries", "‹éø›\n", "**éø**‸\n", Mark::Strong),
            ("caret does nothing", "a‸b\n", "a‸b\n", Mark::Strong),
            ("whitespace does nothing", "a‹  ›b\n", "a  ‸b\n", Mark::Strong),
        ];
        all(cases, |(rule, before, after, mark)| {
            case(rule, before, after, |text, events, range| toggle_inline(text, events, range, mark));
        });
        let plan = Plan { edits: vec![(1..3, "é".into()), (3..3, "!".into())], caret: 4 };
        all([
            ("before replacement", 0, 0),
            ("replacement start", 1, 3),
            ("inside replacement", 2, 3),
            ("replacement end includes boundary insertion", 3, 4),
            ("after replacement", 4, 5),
        ], |(rule, before, after)| assert_eq!(plan.map(before), after, "{rule}"));
    }

    #[test]
    fn links_set_replace_remove() {
        let cases = [
            ("add a link", "‹a›\n", "[a](u)‸\n", Some("u")),
            ("preserve exact title", "[a‸](old 'title')\n", "[a‸](new 'title')\n", Some("new")),
            ("replace balanced destination", "[a‸](old(x) \"title\")\n",
                "[a‸](new \"title\")\n", Some("new")),
            ("replace angle destination", "[‹a›](<old path> \"title\")\n",
                "[a‸](new \"title\")\n", Some("new")),
            ("remove inline link", "[‹a›](u)\n", "a‸\n", None),
            ("caret removes inline link", "[a‸](u)\n", "a‸\n", None),
            ("remove reference link", "[‹a›][ref]\n\n[ref]: /old\n", "a‸\n\n[ref]: /old\n", None),
            ("remove collapsed reference", "[‹a›][]\n\n[a]: /old\n", "a‸\n\n[a]: /old\n", None),
            ("remove shortcut reference", "[‹a›]\n\n[a]: /old\n", "a‸\n\n[a]: /old\n", None),
            ("replace collapsed reference", "[a‸][]\n\n[a]: /old\n",
                "[a‸](new)\n\n[a]: /old\n", Some("new")),
            ("replace one reference occurrence", "[a‸][ref]\n\n[ref]: /old \"title\"\n",
                "[a‸](new \"title\")\n\n[ref]: /old \"title\"\n", Some("new")),
            ("no nested links", "‹a [b](old) c›\n", "[a b c](new)‸\n", Some("new")),
            ("keep label formatting balanced", "‹**a**›\n", "[**a**](u)‸\n", Some("u")),
            ("encode URL syntax", "‹a›\n", "[a](a\\(b\\)&amp;c)‸\n", Some("a(b)&c")),
            ("angle URL containing spaces", "‹a›\n", "[a](<a b>)‸\n", Some("a b")),
            ("caret adds nothing", "a‸\n", "a‸\n", Some("u")),
            ("remove outside a link", "a‸\n", "a‸\n", None),
        ];
        all(cases, |(rule, before, after, url)| {
            case(rule, before, after, |text, events, range| set_link(text, events, range, url));
        });
        all([
            ("inline URL", "[a‸](url)\n", Some("url")),
            ("resolved reference URL", "[a‸][r]\n\n[r]: /resolved\n", Some("/resolved")),
            ("decoded destination", "[a‸](a&amp;b)\n", Some("a&b")),
            ("autolink destination", "<https://a‸.test>\n", Some("https://a.test")),
            ("after link is outside", "[a](u)‸ x\n", None),
        ], |(rule, before, expected)| {
            let (text, range) = selection(before);
            let events: Vec<_> = Parser::new_ext(&text, parse_options()).into_offset_iter().collect();
            assert_eq!(link_at(&text, &events, range.start).as_deref(), expected, "{rule}\nevents: {events:?}");
        });
    }

    #[test]
    fn block_styles() {
        let cases = [
            ("set heading level", "‹title›\n", "## title‸\n", BlockStyle::Heading(2)),
            ("change heading level", "# ‹title›\n", "### title‸\n", BlockStyle::Heading(3)),
            ("remove heading prefix", "## ‹title›\n", "title‸\n", BlockStyle::Paragraph),
            ("setext becomes ATX", "‹title›\n===\n", "## title‸\n", BlockStyle::Heading(2)),
            ("setext underline removed", "‹title›\n-\n", "title‸\n", BlockStyle::Paragraph),
            ("heading joins soft lines", "‹one\ntwo›\n", "## one two‸\n", BlockStyle::Heading(2)),
            ("quoted heading joins prefixes", "> ‹one\n> two›\n", "> # one two‸\n", BlockStyle::Heading(1)),
            ("add bullet", "‹a›\n", "- a‸\n", BlockStyle::Bullet),
            ("number three paragraphs", "‹a\n\nb\n\nc›\n", "1. a\n\n2. b\n\n3. c‸\n", BlockStyle::Numbered),
            ("add task", "‹a›\n", "- [ ] a‸\n", BlockStyle::Task),
            ("heading becomes list", "# ‹a›\n", "- a‸\n", BlockStyle::Bullet),
            ("bullet to numbered", "- ‹a›\n", "1. a‸\n", BlockStyle::Numbered),
            ("numbered to task", "7. ‹a›\n", "- [ ] a‸\n", BlockStyle::Task),
            ("task to bullet", "- [x] ‹a›\n", "- a‸\n", BlockStyle::Bullet),
            ("task to numbered", "- [ ] ‹a›\n", "1. a‸\n", BlockStyle::Numbered),
            ("numbered to bullet", "7. ‹a›\n", "- a‸\n", BlockStyle::Bullet),
            ("bullet to task", "- ‹a›\n", "- [ ] a‸\n", BlockStyle::Task),
            ("remove bullet kind and separate paragraphs", "- ‹a\n- b›\n", "a\n\nb‸\n", BlockStyle::Bullet),
            ("remove numbered kind", "1. ‹a\n2. b›\n", "a\n\nb‸\n", BlockStyle::Numbered),
            ("remove task kind and boxes", "- [ ] ‹a\n- [x] b›\n", "a\n\nb‸\n", BlockStyle::Task),
            ("paragraph keeps item indent", "  - [x] ‹a›\n", "  a‸\n", BlockStyle::Paragraph),
            ("continuation lines stay unchanged", "- ‹a\n  b›\n", "1. a\n  b‸\n", BlockStyle::Numbered),
            ("quote every line", "‹a\nb›\n", "> a\n> b‸\n", BlockStyle::Quote),
            ("remove quote from every line", "> ‹a\n> b›\n", "a\nb‸\n", BlockStyle::Quote),
            ("quote before inner list marker", "- ‹a›\n", "> - a‸\n", BlockStyle::Quote),
            ("quote inside outer list", "- > ‹a›\n", "- a‸\n", BlockStyle::Quote),
            ("paragraph retains quote", "> # ‹a›\n", "> a‸\n", BlockStyle::Paragraph),
            ("wrap code", "‹a›\n", "```\na‸\n```\n", BlockStyle::Code),
            ("unwrap fenced code", "```\n‹a›\n```\n", "a‸\n", BlockStyle::Code),
            ("paragraph unwraps code", "~~~rust\n‹a›\n~~~\n", "a‸\n", BlockStyle::Paragraph),
            ("indented code first becomes fenced", "    ‹a›\n", "```\na‸\n```\n", BlockStyle::Code),
            ("paragraph removes code indentation", "    ‹a›\n", "a‸\n", BlockStyle::Paragraph),
            ("one fence around selected blocks", "‹a\n\nb›\n", "```\na\n\nb‸\n```\n", BlockStyle::Code),
            ("fence exceeds contained line-start run", "    ‹```\n    x›\n",
                "````\n```\nx‸\n````\n", BlockStyle::Code),
            ("quoted fences carry prefix", "> ‹a›\n", "> ```\n> a‸\n> ```\n", BlockStyle::Code),
            ("item fences carry prefix", "- ‹a›\n", "- ```\n  a‸\n  ```\n", BlockStyle::Code),
            ("sibling items get separate fences", "- ‹a\n- b›\n",
                "- ```\n  a\n  ```\n- ```\n  b‸\n  ```\n", BlockStyle::Code),
            ("unwrapping preserves item marker", "- ```\n  ‹a›\n  ```\n", "- a‸\n", BlockStyle::Code),
            ("heading follows item prefix", "- ‹a›\n", "- ## a‸\n", BlockStyle::Heading(2)),
            ("heading replaces task box prefix", "- [ ] ‹a›\n", "- ## a‸\n", BlockStyle::Heading(2)),
            ("caret selects its block", "‸a\n", "# ‸a\n", BlockStyle::Heading(1)),
            ("exclusive selection end skips next block", "‹a\n\n›b\n", "# a\n\n‸b\n", BlockStyle::Heading(1)),
            ("invalid heading level does nothing", "a‸\n", "a‸\n", BlockStyle::Heading(7)),
        ];
        all(cases, |(rule, before, after, style)| {
            case(rule, before, after, |text, events, range| set_block(text, events, range, style));
        });
    }

    #[test]
    fn tasks_and_indentation() {
        #[derive(Clone, Copy)]
        enum Command { Task, Indent, Outdent }
        let cases = [
            ("check task", "- [ ] a‸\n", "- [x] a‸\n", Command::Task),
            ("uncheck task", "- [x] a‸\n", "- [ ] a‸\n", Command::Task),
            ("uppercase checked task", "- [X] a‸\n", "- [ ] a‸\n", Command::Task),
            ("task from continuation", "- [ ] a\n  b‸\n", "- [x] a\n  b‸\n", Command::Task),
            ("not a task", "- a‸\n", "- a‸\n", Command::Task),
            ("parent task does not toggle for child", "- [ ] a\n  - b‸\n",
                "- [ ] a\n  - b‸\n", Command::Task),
            ("indent second item", "- a\n- b‸\n", "- a\n  - b‸\n", Command::Indent),
            ("outdent nested item", "- a\n  - b‸\n", "- a\n- b‸\n", Command::Outdent),
            ("first item cannot indent", "- a‸\n- b\n", "- a‸\n- b\n", Command::Indent),
            ("top-level item cannot outdent", "- a‸\n", "- a‸\n", Command::Outdent),
            ("outside list does nothing", "a‸\n", "a‸\n", Command::Indent),
            ("indent carries children", "- a\n- b‸\n  - c\n", "- a\n  - b‸\n    - c\n", Command::Indent),
            ("selected descendants move once", "- a\n- ‹b\n  - c›\n",
                "- a\n  - b\n    - c‸\n", Command::Indent),
            ("selected siblings share preceding parent", "- a\n- ‹b\n- c›\n",
                "- a\n  - b\n  - c‸\n", Command::Indent),
            ("ordered parent content width", "10. a\n11. b‸\n", "10. a\n    11. b‸\n", Command::Indent),
            ("ordered parent outdent width", "10. a\n    - b‸\n", "10. a\n- b‸\n", Command::Outdent),
            ("indent after quote prefix", "> - a\n> - b‸\n", "> - a\n>   - b‸\n", Command::Indent),
            ("outdent after quote prefix", "> - a\n>   - b‸\n", "> - a\n> - b‸\n", Command::Outdent),
        ];
        all(cases, |(rule, before, after, command)| {
            case(rule, before, after, |text, events, range| match command {
                Command::Task => {
                    let plan = toggle_task(text, events, range.end);
                    if let Some(plan) = &plan {
                        assert_eq!(plan.edits.len(), 1, "{rule}: one task edit");
                        assert_eq!(plan.edits[0].0.len(), 1, "{rule}: one source byte");
                        assert_eq!(plan.edits[0].1.len(), 1, "{rule}: one replacement byte");
                    }
                    plan
                }
                Command::Indent => indent_items(text, events, range, false),
                Command::Outdent => indent_items(text, events, range, true),
            });
        });
    }
}
