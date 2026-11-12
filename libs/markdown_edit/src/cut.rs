//! Deletion plans over projected characters, with balanced source markers.

use crate::plan::{content_start, heading_start, line_at, Container, Containers, Leaf, Plan};
use crate::{Atom, PieceKind, Projection};
use pulldown_cmark::{Event, Tag};
use std::ops::Range;

type Edits = Vec<(Range<usize>, String)>;

struct Block {
    containers: Containers,
    raw: Range<usize>,
    body: Range<usize>,
    visible: Range<usize>,
}

fn line_end(text: &str, at: usize) -> usize {
    (line_at(text, at).end + 1).min(text.len())
}

fn previous_char(text: &str, at: usize) -> usize {
    text[..at].char_indices().next_back().map_or(0, |(at, _)| at)
}

fn body_start(text: &str, containers: &Containers) -> usize {
    let mut start = containers.content_start(text);
    if matches!(containers.leaf, Leaf::Heading(_)) {
        start = start.max(heading_start(text, containers));
    }
    if matches!(containers.stack.last(), Some(Container::Item { task: Some(_), .. })) {
        let source = &text[start..containers.line.end];
        if ["[ ]", "[x]", "[X]"].iter().any(|marker| source.starts_with(*marker)) {
            start += 3;
        }
    }
    start + text[start..containers.line.end].bytes().take_while(|b| matches!(b, b' ' | b'\t')).count()
}

fn block(text: &str, events: &[(Event<'_>, Range<usize>)], p: &Projection, at: usize) -> Block {
    let mut containers = Containers::at(text, events, at);
    let range = containers.leaf_range.clone();
    containers.line = line_at(text, range.start);
    let raw = containers.line.start..if range.is_empty() {
        line_end(text, at)
    } else {
        line_end(text, previous_char(text, range.end))
    };
    let mut start = body_start(text, &containers);
    if matches!(containers.leaf, Leaf::Code { fenced: true }) {
        start = line_end(text, range.start);
    }
    let mut first = None;
    let mut last = None;
    for piece in &p.pieces {
        if piece.src.end <= start
            || piece.src.start >= range.end
            || matches!(piece.kind, PieceKind::Atomic(Atom::BlockBreak | Atom::Rule))
        {
            continue;
        }
        let from = piece.src.start.max(start);
        let mut end = piece.src.end.min(range.end);
        if matches!(containers.leaf, Leaf::Code { .. } | Leaf::Html) {
            end -= text[from..end].len() - text[from..end].trim_end_matches('\n').len();
        }
        if from >= end {
            continue;
        }
        let visible = match piece.kind {
            PieceKind::Text => {
                piece.visible.start + from - piece.src.start..piece.visible.start + end - piece.src.start
            }
            PieceKind::Atomic(_) => piece.visible.clone(),
        };
        first.get_or_insert((visible.start, from));
        last = Some((visible.end, end));
    }
    if containers.leaf == Leaf::Table {
        start = first.map_or(start, |(_, from)| from);
    }
    let visible_start = first.map_or_else(|| p.to_visible(start), |(visible, _)| visible);
    let end = last.map_or(start, |(_, end)| line_at(text, end).end);
    let visible = visible_start..last.map_or(visible_start, |(end, _)| end);
    Block { containers, raw, body: start..end, visible }
}

// Edits are collected in source order only after nested marker removals have
// been added. A containing deletion absorbs replacements (such as a soft
// break's joining space) that would otherwise survive inside an empty span.
fn normalize(mut edits: Edits) -> Edits {
    edits.retain(|(range, replacement)| !range.is_empty() || !replacement.is_empty());
    edits.sort_by(|a, b| a.0.start.cmp(&b.0.start).then(b.0.end.cmp(&a.0.end)));
    let mut out: Edits = Vec::new();
    for (range, replacement) in edits {
        if let Some((previous, value)) = out.last_mut() {
            if range.start < previous.end {
                if range.end > previous.end {
                    previous.end = range.end;
                    value.push_str(&replacement);
                }
                continue;
            }
            if range.start == previous.end && value.is_empty() && replacement.is_empty() {
                previous.end = range.end;
                value.push_str(&replacement);
                continue;
            }
        }
        out.push((range, replacement));
    }
    out
}

fn mapped(edits: &Edits, at: usize) -> usize {
    let mut old = 0;
    let mut new = 0;
    for (range, replacement) in edits {
        if at < range.start {
            break;
        }
        new += range.start - old;
        if at <= range.end {
            return new + replacement.len();
        }
        new += replacement.len();
        old = range.end;
    }
    new + at - old
}

fn finish(text: &str, edits: Edits, caret: usize) -> Plan {
    let edits = normalize(edits);
    let caret = mapped(&edits, caret);
    Plan { edits, caret }.checked(text)
}

fn erase(range: Range<usize>) -> (Range<usize>, String) {
    (range, String::new())
}

fn overlaps(a: &Range<usize>, b: &Range<usize>) -> bool {
    a.start < b.end && b.start < a.end
}

fn within(text: &str, p: &Projection, scope: &Block, range: Range<usize>) -> Edits {
    let mut edits = Vec::new();
    let mut selected = None::<Range<usize>>;
    let mut soft = Vec::new();
    for piece in &p.pieces {
        if !overlaps(&piece.src, &range) || matches!(piece.kind, PieceKind::Atomic(Atom::BlockBreak)) {
            continue;
        }
        let (mut source, visible) = match piece.kind {
            PieceKind::Text => {
                let source = piece.src.start.max(range.start)..piece.src.end.min(range.end);
                let visible = piece.visible.start + source.start - piece.src.start
                    ..piece.visible.start + source.end - piece.src.start;
                (source, visible)
            }
            PieceKind::Atomic(_) => (piece.src.clone(), piece.visible.clone()),
        };
        match &mut selected {
            Some(selected) => selected.end = visible.end,
            None => selected = Some(visible.clone()),
        }
        if piece.kind == PieceKind::Atomic(Atom::SoftBreak) {
            let line = line_at(text, source.end);
            source.end = content_start(text, line.clone(), &scope.containers.stack);
            source.end +=
                text[source.end..line.end].bytes().take_while(|b| matches!(b, b' ' | b'\t')).count();
            soft.push((source.clone(), visible));
        }
        edits.push(erase(source));
    }
    let Some(selected) = selected else { return edits };
    let mut emptied = Vec::new();
    for inline in p.inlines.iter().rev() {
        let start = p.to_visible(inline.open.end);
        let end = p.to_visible(inline.close.start);
        if selected.start <= start && end <= selected.end && start < end {
            let range = inline.open.start..inline.close.end;
            edits.push(erase(range.clone()));
            emptied.push(range);
        }
    }
    let left = p.text[..selected.start].chars().next_back();
    let right = p.text[selected.end..].chars().next();
    if left.is_some_and(|c| !c.is_whitespace()) && right.is_some_and(|c| !c.is_whitespace()) {
        for (source, _) in soft {
            // A space the source keeps on either side (one the parse hid at
            // the line's end) joins the lines already.
            let spaced = text[..source.start].ends_with([' ', '\t']) || text[source.end..].starts_with([' ', '\t']);
            if !spaced && !emptied.iter().any(|range| range.start <= source.start && source.end <= range.end) {
                if let Some((_, replacement)) = edits.iter_mut().find(|(range, _)| *range == source) {
                    replacement.push(' ');
                    break;
                }
            }
        }
    }
    edits
}

fn prefix(text: &str, scope: &Block) -> Option<Range<usize>> {
    let containers = &scope.containers;
    if matches!(containers.leaf, Leaf::Heading(_)) {
        let end = heading_start(text, containers);
        if end > containers.leaf_range.start {
            return Some(containers.leaf_range.start..end);
        }
    }
    for (index, container) in containers.stack.iter().enumerate().rev() {
        let start = content_start(text, containers.line.clone(), &containers.stack[..index]);
        if let Container::Item { indent, marker, .. } = container {
            let start = start + indent;
            if text.get(start..containers.line.end).is_some_and(|s| s.starts_with(marker)) {
                return Some(start..scope.body.start);
            }
        }
    }
    for (index, container) in containers.stack.iter().enumerate().rev() {
        if matches!(container, Container::Quote) {
            let start = content_start(text, containers.line.clone(), &containers.stack[..index]);
            let start = start + text[start..containers.line.end].bytes().take_while(|&b| b == b' ').count();
            if text.as_bytes().get(start) == Some(&b'>') {
                let end = start + 1 + usize::from(text.as_bytes().get(start + 1) == Some(&b' '));
                return Some(start..end);
            }
        }
    }
    None
}

struct Top {
    range: Range<usize>,
    rule: bool,
}

fn top_blocks(events: &[(Event<'_>, Range<usize>)]) -> Vec<Top> {
    let mut blocks = Vec::new();
    let mut depth = 0usize;
    for (event, range) in events {
        match event {
            Event::Start(_) => {
                if depth == 0 {
                    blocks.push(Top { range: range.clone(), rule: false });
                }
                depth += 1;
            }
            Event::End(_) => depth = depth.saturating_sub(1),
            Event::Rule if depth == 0 => blocks.push(Top { range: range.clone(), rule: true }),
            _ => {}
        }
    }
    blocks
}

fn island(scope: &Block) -> bool {
    matches!(scope.containers.leaf, Leaf::Code { .. } | Leaf::Table | Leaf::Html)
}

fn join(
    text: &str,
    events: &[(Event<'_>, Range<usize>)],
    p: &Projection,
    scope: &Block,
    caret: usize,
    backward: bool,
) -> Option<Plan> {
    let tops = top_blocks(events);
    let index = tops.iter().position(|top| {
        top.range.start <= scope.containers.leaf_range.start
            && scope.containers.leaf_range.end <= top.range.end
    })?;
    let next = if backward { index.checked_sub(1)? } else { index + 1 };
    let top = tops.get(next)?;
    // A leaf boundary inside a list or quote is not the edge of its
    // top-level block. Do not erase its unselected sibling leaves.
    if p.pieces.iter().any(|piece| {
        piece.kind != PieceKind::Atomic(Atom::BlockBreak)
            && piece.src.start >= tops[index].range.start
            && piece.src.end <= tops[index].range.end
            && if backward {
                piece.src.end <= scope.containers.leaf_range.start
            } else {
                piece.src.start >= scope.containers.leaf_range.end
            }
    }) {
        return None;
    }
    if top.rule {
        let start = line_at(text, top.range.start).start;
        let end = tops.get(next + 1).map_or(text.len(), |next| line_at(text, next.range.start).start);
        return Some(finish(text, vec![erase(start..end)], caret));
    }
    let piece = if backward {
        p.pieces.iter().rev().find(|piece| {
            piece.src.start < top.range.end
                && piece.src.start >= top.range.start
                && piece.kind != PieceKind::Atomic(Atom::BlockBreak)
        })
    } else {
        p.pieces.iter().find(|piece| {
            piece.src.start >= top.range.start
                && piece.src.start < top.range.end
                && piece.kind != PieceKind::Atomic(Atom::BlockBreak)
        })
    };
    let at = piece.map_or(top.range.start, |piece| piece.src.start);
    let other = block(text, events, p, at);
    if island(&other) {
        let caret = if backward {
            other.body.end
        } else if other.visible.is_empty() {
            other.body.start
        } else {
            p.to_source(other.visible.start, crate::Affinity::Right)
        };
        return Some(Plan { edits: vec![], caret }.checked(text));
    }
    let range = if backward { other.body.end..scope.body.start } else { scope.body.end..other.body.start };
    Some(finish(text, vec![erase(range)], caret))
}

/// A key on a blank line (the empty paragraph Enter leaves): Backspace takes
/// the blank lines back to the text above, except the line break after a
/// code block, table or HTML block, which its last line needs; Delete takes
/// the line out.
fn blank_line(text: &str, events: &[(Event<'_>, Range<usize>)], caret: usize, backward: bool) -> Option<Plan> {
    let line = line_at(text, caret);
    if backward {
        let content_end = text[..line.start].trim_end_matches(['\n', ' ', '\t']).len();
        let after_island = content_end > 0
            && events.iter().any(|(event, range)| {
                matches!(event, Event::Start(Tag::CodeBlock(_) | Tag::Table(_) | Tag::HtmlBlock))
                    && range.start < content_end
                    && content_end <= range.end
            });
        let start = if after_island { content_end + 1 } else { content_end };
        (start < caret).then(|| finish(text, vec![erase(start..caret)], start))
    } else {
        let end = (line.end + 1).min(text.len());
        (caret < end && end < text.len()).then(|| finish(text, vec![erase(caret..end)], caret))
    }
}

fn key(text: &str, events: &[(Event<'_>, Range<usize>)], caret: usize, backward: bool) -> Option<Plan> {
    debug_assert!(text.is_char_boundary(caret));
    if text[line_at(text, caret)].trim().is_empty() {
        return blank_line(text, events, caret, backward);
    }
    let p = Projection::new(text, events);
    let scope = block(text, events, &p, caret);
    // A table cell's edge holds: the keys never reach into the next cell.
    if scope.containers.leaf == Leaf::Table {
        if let Some(cell) = crate::table::cell_at(text, events, caret) {
            if (backward && caret <= cell.content.start) || (!backward && caret >= cell.content.end) {
                return None;
            }
        }
    }
    let visible = p.to_visible(caret).clamp(scope.visible.start, scope.visible.end);
    if backward && visible == scope.visible.start {
        if let Some(range) = prefix(text, &scope) {
            return Some(finish(text, vec![erase(range)], caret));
        }
    }
    let character = if backward && visible > scope.visible.start {
        p.text[..visible].char_indices().next_back().map(|(at, _)| at..visible)
    } else if !backward && visible < scope.visible.end {
        p.text[visible..].chars().next().map(|c| visible..visible + c.len_utf8())
    } else {
        None
    };
    if let Some(character) = character {
        let piece = p.pieces.iter().find(|piece| overlaps(&piece.visible, &character))?;
        if piece.kind == PieceKind::Atomic(Atom::BlockBreak) {
            return None;
        }
        let source = match piece.kind {
            PieceKind::Text => {
                piece.src.start + character.start - piece.visible.start
                    ..piece.src.start + character.end - piece.visible.start
            }
            PieceKind::Atomic(_) => piece.src.clone(),
        };
        return Some(finish(text, within(text, &p, &scope, source), caret));
    }
    join(text, events, &p, &scope, caret, backward)
}

/// Deletes the preceding projected character, removes a kind prefix, or joins the preceding block.
pub fn backspace(text: &str, events: &[(Event<'_>, Range<usize>)], caret: usize) -> Option<Plan> {
    key(text, events, caret, true)
}

/// Deletes the following projected character or joins the following block, preserving kind prefixes.
pub fn delete_forward(text: &str, events: &[(Event<'_>, Range<usize>)], caret: usize) -> Option<Plan> {
    key(text, events, caret, false)
}

fn fragment(text: &str, p: &Projection, range: Range<usize>) -> String {
    let mut out = String::new();
    for inline in &p.inlines {
        if inline.open.start < range.start
            && range.start <= inline.close.start
            && inline.open.end <= range.start
        {
            out.push_str(&text[inline.open.clone()]);
        }
    }
    out.push_str(&text[range.clone()]);
    for inline in p.inlines.iter().rev() {
        if inline.open.end <= range.end && range.end <= inline.close.start && range.start < inline.close.end {
            out.push_str(&text[inline.close.clone()]);
        }
    }
    out
}

fn continuation_edits(text: &str, last: &Block, from: usize, continuation: &str) -> Edits {
    let mut edits = Vec::new();
    let mut at = line_end(text, from);
    while at < last.raw.end {
        let line = line_at(text, at);
        let start = content_start(text, line.clone(), &last.containers.stack);
        if text[at..start] != *continuation {
            edits.push((at..start, continuation.to_string()));
        }
        at = (line.end + 1).min(text.len());
    }
    edits
}

// A table cannot lose part of a row, and code fences are structural. The
// selection's source interval expands to rows/lines, but fences remain in
// the document unless both fences were selected.
fn island_range(text: &str, scope: &Block, range: &Range<usize>) -> Range<usize> {
    if range.start <= scope.raw.start && scope.raw.end <= range.end {
        return scope.raw.clone();
    }
    let start = line_at(text, range.start.max(scope.body.start)).start;
    let end = line_end(text, previous_char(text, range.end).min(scope.body.end));
    if scope.containers.leaf == Leaf::Table {
        if start <= line_at(text, scope.body.start).start {
            scope.raw.clone()
        } else {
            start..end.min(scope.raw.end)
        }
    } else {
        let end = end.min(line_end(text, scope.body.end));
        start.max(scope.body.start)..end
    }
}

/// Cuts a source selection, balances inline markers, and returns repaired Markdown for the clipboard.
pub fn cut(text: &str, events: &[(Event<'_>, Range<usize>)], range: Range<usize>) -> (Plan, String) {
    debug_assert!(
        range.start <= range.end && text.is_char_boundary(range.start) && text.is_char_boundary(range.end)
    );
    if range.is_empty() {
        return (Plan { edits: vec![], caret: range.start }.checked(text), String::new());
    }
    let p = Projection::new(text, events);
    let first = block(text, events, &p, range.start);
    let last_at = previous_char(text, range.end);
    let last = block(text, events, &p, last_at);
    let same = first.containers.leaf_range == last.containers.leaf_range;
    let protected =
        |scope: &Block| matches!(scope.containers.leaf, Leaf::Table | Leaf::Code { fenced: true });
    // Text inside one table cell cuts as text; anything wider takes rows.
    let in_cell = first.containers.leaf == Leaf::Table
        && crate::table::cell_at(text, events, range.start).is_some_and(|cell| {
            cell.content.start <= range.start
                && range.end <= cell.content.end
                && crate::table::cell_at(text, events, range.end).as_ref() == Some(&cell)
        });
    let mut removed = range.clone();
    let mut edits;
    if same && protected(&first) && !in_cell {
        removed = island_range(text, &first, &range);
        edits = vec![erase(removed.clone())];
    } else if same {
        edits = within(text, &p, &first, range.clone());
        let all =
            p.to_visible(range.start) <= first.visible.start && first.visible.end <= p.to_visible(range.end);
        if all && !first.visible.is_empty() {
            edits = vec![erase(first.raw.start..first.raw.end.saturating_sub(1))];
        }
    } else {
        let first_island = protected(&first);
        let last_island = protected(&last);
        let first_end = if first_island { first.raw.end } else { first.body.end };
        let last_start = if last_island { last.raw.start } else { last.body.start };
        edits = if first_island {
            let selected = island_range(text, &first, &range);
            removed.start = selected.start;
            vec![erase(selected)]
        } else {
            within(text, &p, &first, range.start..first.body.end)
        };
        if last_island {
            let selected = island_range(text, &last, &range);
            removed.end = selected.end;
            edits.push(erase(selected));
        } else {
            edits.extend(within(text, &p, &last, last.body.start..range.end));
            edits.extend(continuation_edits(text, &last, range.end, &first.containers.continuation()));
        }
        // Keep the line boundary around a surviving island. A fully selected
        // island is raw middle material and needs no such boundary.
        let keep_first = first_island && removed.start > first.raw.start;
        let keep_last = last_island && removed.end < last.raw.end;
        if first_end <= last_start {
            let gap = if keep_first {
                "\n"
            } else if keep_last {
                "\n\n"
            } else {
                ""
            };
            edits.push((first_end..last_start, gap.to_string()));
        }
        if !first_island && range.start < first.body.start {
            edits.push(erase(first.raw.start..first.body.start));
        }
    }
    // Expanding an atomic selection repairs its clipboard source as well.
    for piece in &p.pieces {
        if matches!(piece.kind, PieceKind::Atomic(atom) if atom != Atom::BlockBreak)
            && overlaps(&piece.src, &removed)
        {
            removed.start = removed.start.min(piece.src.start);
            removed.end = removed.end.max(piece.src.end);
        }
    }
    let mut clipboard = fragment(text, &p, removed.clone());
    if !same && p.to_visible(range.start) == first.visible.start && range.start >= first.body.start {
        clipboard.insert_str(0, &text[first.raw.start..first.body.start]);
    }
    (finish(text, edits, range.start), clipboard)
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

    fn key_case(rule: &str, before: &str, expected: &str, backward: bool) {
        let caret = before.find('‸').expect("case has a caret");
        let text = before.replace('‸', "");
        assert!(text.ends_with('\n'), "{rule}: slice lines end in a newline");
        let mut parser = Parser::new_ext(&text, parse_options());
        parser.assume_footnote_labels_defined();
        let events: Vec<_> = parser.into_offset_iter().collect();
        let plan =
            if backward { backspace(&text, &events, caret) } else { delete_forward(&text, &events, caret) };
        let mut actual = plan.as_ref().map_or_else(|| text.clone(), |plan| plan.apply(&text));
        actual.insert_str(plan.as_ref().map_or(caret, |plan| plan.caret), "‸");
        assert_eq!(actual, expected, "{rule}\nevents: {events:?}");
        if before == expected {
            assert!(plan.is_none(), "{rule}: the key must do nothing");
        }
        if before.replace('‸', "") == expected.replace('‸', "") && before != expected {
            assert!(plan.unwrap().edits.is_empty(), "{rule}: moving into an island makes no edits");
        }
    }

    fn cut_case(rule: &str, before: &str, expected: &str, clipboard: &str) {
        let start = before.find('‹').expect("case has a selection start");
        let end = before.find('›').expect("case has a selection end") - '‹'.len_utf8();
        let text = before.replace(['‹', '›'], "");
        assert!(text.ends_with('\n'), "{rule}: slice lines end in a newline");
        let mut parser = Parser::new_ext(&text, parse_options());
        parser.assume_footnote_labels_defined();
        let events: Vec<_> = parser.into_offset_iter().collect();
        let (plan, fragment) = cut(&text, &events, start..end);
        let mut actual = plan.apply(&text);
        actual.insert_str(plan.caret, "‸");
        assert_eq!(actual, expected, "{rule}\nevents: {events:?}");
        assert_eq!(fragment, clipboard, "{rule}: clipboard\nevents: {events:?}");
    }

    #[test]
    fn backspace_and_delete_inside_a_block() {
        let cases = [
            ("plain character", "ab‸c\n", "a‸c\n"),
            ("UTF-8 character", "aé‸c\n", "a‸c\n"),
            ("hidden closing marker and caret", "**ab**‸c\n", "**a**‸c\n"),
            ("empty strong", "**a**‸\n", "‸\n"),
            ("empty nested emphasis", "***a***‸\n", "‸\n"),
            ("entity is atomic", "a&amp;‸b\n", "a‸b\n"),
            ("image is atomic", "a![x](u)‸b\n", "a‸b\n"),
            ("footnote is atomic", "a[^n]‸b\n", "a‸b\n"),
            ("math is atomic", "a$x$‸b\n", "a‸b\n"),
            ("hard break is atomic", "a\\\n‸b\n", "a‸b\n"),
            ("space hard break is atomic", "a  \n‸b\n", "a‸b\n"),
            ("soft break adds a space", "a\n‸b\n", "a ‸b\n"),
            ("soft break removes quote prefix", "> a\n> ‸b\n", "> a ‸b\n"),
            ("soft break removes list continuation", "- a\n  ‸b\n", "- a ‸b\n"),
            ("soft break keeps existing space", "a \n‸b\n", "a ‸b\n"),
            ("inline code empties", "`a`‸\n", "‸\n"),
            ("link empties", "[a](u)‸\n", "‸\n"),
        ];
        all(cases, |(rule, before, after)| key_case(rule, before, after, true));
        let cases = [
            ("delete plain character", "a‸bc\n", "a‸c\n"),
            ("delete UTF-8 character", "a‸éb\n", "a‸b\n"),
            ("delete across hidden opening marker", "a‸**bc**\n", "a‸**c**\n"),
            ("delete across hidden closing marker", "**a‸**b\n", "**a‸**\n"),
            ("delete empty strong", "‸**a**\n", "‸\n"),
            ("delete nested emphasis", "‸***a***\n", "‸\n"),
            ("delete entity", "a‸&amp;b\n", "a‸b\n"),
            ("delete image", "a‸![x](u)b\n", "a‸b\n"),
            ("delete footnote", "a‸[^n]b\n", "a‸b\n"),
            ("delete math", "a‸$x$b\n", "a‸b\n"),
            ("delete hard break", "a‸\\\nb\n", "a‸b\n"),
            ("delete soft break", "a‸\nb\n", "a ‸b\n"),
            ("delete quoted soft break", "> a‸\n> b\n", "> a ‸b\n"),
            ("delete soft break keeps space", "a ‸\nb\n", "a ‸b\n"),
            ("delete inline code", "‸`a`\n", "‸\n"),
            ("delete link", "‸[a](u)\n", "‸\n"),
        ];
        all(cases, |(rule, before, after)| key_case(rule, before, after, false));
    }

    #[test]
    fn backspace_at_a_block_start() {
        let cases = [
            ("heading becomes paragraph", "###   ‸Title\n", "‸Title\n"),
            ("heading before quote level", "> # ‸Title\n", "> ‸Title\n"),
            ("item loses marker", "- ‸a\n", "‸a\n"),
            ("ordered item loses marker", "12. ‸a\n", "‸a\n"),
            ("item keeps indent", "  -   ‸a\n", "  ‸a\n"),
            ("task item loses marker and box", "- [x] ‸a\n", "‸a\n"),
            ("unchecked task loses box", "- [ ] ‸a\n", "‸a\n"),
            ("item before quote level", "> - ‸a\n", "> ‸a\n"),
            ("quote loses one level", "> > ‸a\n", "> ‸a\n"),
            ("paragraph joins paragraph", "a\n\n‸b\n", "a‸b\n"),
            ("paragraph joins list", "- a\n\n‸b\n", "- a‸b\n"),
            ("previous rule goes", "***\n\n‸b\n", "‸b\n"),
            ("fenced code takes caret", "```\na\n```\n\n‸b\n", "```\na‸\n```\n\nb\n"),
            ("indented code takes caret", "    a\n\n‸b\n", "    a‸\n\nb\n"),
            ("table takes caret", "| a |\n| - |\n| b |\n\n‸c\n", "| a |\n| - |\n| b |‸\n\nc\n"),
            ("a cell's start holds", "| a | ‸b |\n| - | - |\n", "| a | ‸b |\n| - | - |\n"),
            ("an empty cell holds", "| a | ‸ |\n| - | - |\n", "| a | ‸ |\n| - | - |\n"),
            ("in a cell, a character", "| a | bc‸ |\n| - | - |\n", "| a | b‸ |\n| - | - |\n"),
            ("HTML takes caret", "<div>\na\n</div>\n\n‸b\n", "<div>\na\n</div>‸\n\nb\n"),
            ("first block does nothing", "‸a\n", "‸a\n"),
            ("join keeps inline opening marker", "a\n\n**‸b**\n", "a**‸b**\n"),
            ("setext has no hash prefix", "‸Title\n===\n", "‸Title\n===\n"),
            ("blank line goes back to the text", "a\n\n‸\n", "a‸\n"),
            ("blank line after code keeps its break", "```\nx\n```\n\n‸\n", "```\nx\n```\n‸\n"),
            ("first blank line does nothing", "‸\na\n", "‸\na\n"),
        ];
        all(cases, |(rule, before, after)| key_case(rule, before, after, true));
        let cases = [
            ("delete joins paragraph", "a‸\n\nb\n", "a‸b\n"),
            ("delete joins into list", "- a‸\n\nb\n", "- a‸b\n"),
            ("delete joins following heading", "a‸\n\n# b\n", "a‸b\n"),
            ("delete joins following item", "a‸\n\n- b\n", "a‸b\n"),
            ("delete preserves current heading", "# a‸\n", "# a‸\n"),
            ("delete following rule", "a‸\n\n***\n\n", "a‸\n\n"),
            ("delete enters fenced code", "a‸\n\n```\nb\n```\n", "a\n\n```\n‸b\n```\n"),
            ("delete enters table", "a‸\n\n| b |\n| - |\n", "a\n\n| ‸b |\n| - |\n"),
            ("a cell's end holds", "| a‸ | b |\n| - | - |\n", "| a‸ | b |\n| - | - |\n"),
            ("delete enters HTML", "a‸\n\n<div>\nb\n</div>\n", "a\n\n‸<div>\nb\n</div>\n"),
            ("last block does nothing", "a‸\n", "a‸\n"),
            ("delete takes a blank line out", "a\n\n‸\n\nb\n", "a\n\n‸\nb\n"),
            ("join keeps inline closing marker", "**a‸**\n\nb\n", "**a‸**b\n"),
        ];
        all(cases, |(rule, before, after)| key_case(rule, before, after, false));
    }

    #[test]
    fn cut_keeps_markers_balanced() {
        let cases = [
            (
                "partial strong and emphasis",
                "**bo‹ld** and *it›alic*\n",
                "**bo‸***alic*\n",
                "**ld** and *it*",
            ),
            ("whole strong content removes markers", "a**‹b›**c\n", "a‸c\n", "**b**"),
            ("whole nested container", "a***‹b›***c\n", "a‸c\n", "***b***"),
            ("whole container including markers", "a‹**b**›c\n", "a‸c\n", "**b**"),
            ("partial link text", "[ab‹cd›ef](u)\n", "[ab‸ef](u)\n", "[cd](u)"),
            ("empty link removes destination", "a[‹b›](u)c\n", "a‸c\n", "[b](u)"),
            ("cut whole entity", "a‹&amp;›b\n", "a‸b\n", "&amp;"),
            ("cut partial entity expands", "a&‹am›p;b\n", "a‸b\n", "&amp;"),
            ("cut whole image", "a‹![x](u)›b\n", "a‸b\n", "![x](u)"),
            ("cut hard break", "a‹\\\n›b\n", "a‸b\n", "\\\n"),
            ("cut quoted soft break", "> a‹\n> ›b\n", "> a ‸b\n", "\n> "),
            ("whole heading loses prefix", "# ‹ab›\n", "‸\n", "ab"),
            ("whole item loses prefix", "- ‹ab›\n", "‸\n", "ab"),
            ("whole quote loses prefix", "> ‹ab›\n", "‸\n", "ab"),
            ("empty selection preserves source", "**a‹›b**\n", "**a‸b**\n", ""),
        ];
        all(cases, |(rule, before, after, clipboard)| cut_case(rule, before, after, clipboard));
    }

    #[test]
    fn cut_across_blocks() {
        let cases = [
            ("two paragraphs", "a‹b\n\nc›d\n", "a‸d\n", "b\n\nc"),
            ("heading to paragraph", "# a‹b\n\nc›d\n", "# a‸d\n", "b\n\nc"),
            ("quote to paragraph rebases tail", "> a‹b\n\nc›d\ne\n", "> a‸d\n> e\n", "b\n\nc"),
            ("paragraph to quote strips tail prefix", "a‹b\n\n> c›d\n> e\n", "a‸d\ne\n", "b\n\n> c"),
            ("list to paragraph rebases tail", "- a‹b\n\nc›d\ne\n", "- a‸d\n  e\n", "b\n\nc"),
            ("paragraph to list strips indent", "a‹b\n\n- c›d\n  e\n", "a‸d\ne\n", "b\n\n- c"),
            (
                "whole middle blocks go raw",
                "a‹b\n\n# middle\n\n***\n\nc›d\n",
                "a‸d\n",
                "b\n\n# middle\n\n***\n\nc",
            ),
            ("endpoint inline markers balance", "**a‹b**\n\n*c›d*\n", "**a‸***d*\n", "**b**\n\n*c*"),
            ("first heading prefix stays and copies", "# ‹ab\n\nc›d\n", "# ‸d\n", "# ab\n\nc"),
            ("first quote prefix stays and copies", "> ‹ab\n\nc›d\n", "> ‸d\n", "> ab\n\nc"),
            ("first list prefix stays and copies", "- ‹ab\n\nc›d\n", "- ‸d\n", "- ab\n\nc"),
            ("last whole heading loses prefix", "a‹b\n\n# cd›\n", "a‸\n", "b\n\n# cd"),
            (
                "whole fenced middle block goes raw",
                "a‹b\n\n```\nx\n```\n\nc›d\n",
                "a‸d\n",
                "b\n\n```\nx\n```\n\nc",
            ),
            (
                "table body takes whole rows",
                "| h |\n| - |\n| a‹b |\n| c›d |\n| e |\n",
                "| h |\n| - |\n‸| e |\n",
                "| ab |\n| cd |\n",
            ),
            ("table header takes whole table", "‹| h› |\n| - |\n| b |\n", "‸", "| h |\n| - |\n| b |\n"),
            ("header cell text cuts alone", "| ‹h› | i |\n| - | - |\n", "| ‸ | i |\n| - | - |\n", "h"),
            ("body cell text cuts alone", "| h |\n| - |\n| b‹c› |\n", "| h |\n| - |\n| b‸ |\n", "c"),
            ("fenced cut takes whole lines", "```\na‹b\nc›d\ne\n```\n", "```\n‸e\n```\n", "ab\ncd\n"),
            (
                "into fenced code keeps fences",
                "a‹b\n\n```\nx›y\nz\n```\n",
                "a‸\n\n```\nz\n```\n",
                "b\n\n```\nxy\n",
            ),
            ("whole fenced block takes fences", "‹```\nx\n```\n›", "‸", "```\nx\n```\n"),
            (
                "into table touches header and takes table",
                "a‹b\n\n| h |\n| - |\n| c›d |\n\ntail\n",
                "a‸\ntail\n",
                "b\n\n| h |\n| - |\n| cd |\n",
            ),
        ];
        all(cases, |(rule, before, after, clipboard)| cut_case(rule, before, after, clipboard));
    }
}
