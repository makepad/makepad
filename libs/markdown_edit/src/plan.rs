//! Edits in rendered Markdown as source replacements: typing, Enter and
//! Shift+Enter here, Backspace, Delete and the cut every deletion uses in
//! `cut.rs`. Each takes whole top-level blocks and their parse.

use pulldown_cmark::{Event, Tag};
use std::ops::Range;

pub use crate::cut::{backspace, cut, delete_forward};
pub use crate::format::{
    indent_items, link_at, picture, picture_at, set_block, set_link, set_picture, toggle_inline, toggle_task,
    BlockStyle, Mark,
};
pub use crate::table::{
    block_text, cell_at, cell_below, cell_block, clear_block, delete_block, delete_column, delete_row, insert_column, insert_row,
    insert_table, table_from_header, table_step, Cell, CellBlock, Step,
};

/// An edit as source replacements: ascending, disjoint ranges in the old
/// text, and the caret's offset in the resulting text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub edits: Vec<(Range<usize>, String)>,
    pub caret: usize,
}

impl Plan {
    /// One insertion, with the caret after it.
    pub fn insert(at: usize, text: impl Into<String>) -> Self {
        let text = text.into();
        Self { caret: at + text.len(), edits: vec![(at..at, text)] }
    }

    /// The text after the plan's edits.
    pub fn apply(&self, text: &str) -> String {
        debug_assert!(self.valid(text));
        let mut out = String::new();
        let mut from = 0;
        for (range, replacement) in &self.edits {
            out.push_str(&text[from..range.start]);
            out.push_str(replacement);
            from = range.end;
        }
        out.push_str(&text[from..]);
        debug_assert!(out.is_char_boundary(self.caret));
        out
    }

    fn valid(&self, text: &str) -> bool {
        let mut end = 0;
        let mut len = text.len();
        for (range, replacement) in &self.edits {
            if range.start < end || range.start > range.end || range.end > text.len()
                || !text.is_char_boundary(range.start) || !text.is_char_boundary(range.end)
            {
                return false;
            }
            len = len - range.len() + replacement.len();
            end = range.end;
        }
        self.caret <= len
    }

    pub(crate) fn checked(self, text: &str) -> Self {
        debug_assert!(self.valid(text));
        self
    }
}

/// A container block around a position, outermost first in a [`Containers`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Container {
    Quote,
    /// A list item: its source marker, ordinal, indentation after the outer
    /// prefix, marker width including padding, list looseness and task state.
    /// Padding beyond four spaces counts as one space.
    Item {
        marker: String,
        number: Option<u64>,
        indent: usize,
        width: usize,
        loose: bool,
        task: Option<bool>,
    },
}

/// The leaf block holding a position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Leaf {
    Paragraph,
    Heading(u8),
    Code { fenced: bool },
    Table,
    Html,
    Other,
    None,
}

/// A position's containers, leaf block and source range, and source line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Containers {
    pub stack: Vec<Container>,
    pub leaf: Leaf,
    pub leaf_range: Range<usize>,
    /// The caret's line, ending before its newline.
    pub line: Range<usize>,
}

struct List {
    next: Option<u64>,
    loose: bool,
}

struct Frame {
    index: usize,
    list: Option<List>,
    container: Option<(usize, Container)>,
}

impl Containers {
    /// The deepest blocks whose inclusive source ranges hold `at`.
    /// Tight-list text uses its item's range as a paragraph fallback.
    pub fn at(text: &str, events: &[(Event<'_>, Range<usize>)], at: usize) -> Self {
        debug_assert!(text.is_char_boundary(at));
        let mut out = Self {
            stack: Vec::new(), leaf: Leaf::None, leaf_range: at..at, line: line_at(text, at),
        };
        let mut frames: Vec<Frame> = Vec::new();
        let mut selected: Vec<(usize, Container)> = Vec::new();
        let mut depth = 0;
        for (index, (event, range)) in events.iter().enumerate() {
            match event {
                Event::Start(tag) => {
                    let holds = holds(text, range, at);
                    let mut frame = Frame { index, list: None, container: None };
                    match tag {
                        Tag::List(number) => frame.list = Some(List { next: *number, loose: false }),
                        Tag::Item => {
                            let parent = frames.last_mut().expect("an item belongs to a list");
                            let list = parent.list.as_mut().expect("an item's parent is a list");
                            let number = list.next;
                            list.next = number.map(|n| n + 1);
                            let loose = list.loose;
                            let list_index = parent.index;
                            if holds {
                                let outer_width = frames.iter().filter_map(|f| f.container.as_ref())
                                    .map(|(_, container)| container_width(container)).sum();
                                let item = item_at(text, events, index, outer_width, number, loose);
                                frame.container = Some((list_index, item));
                            }
                        }
                        Tag::BlockQuote(_) if holds => frame.container = Some((index, Container::Quote)),
                        Tag::Paragraph if frames.len() >= 2 => {
                            let last = frames.len() - 1;
                            if matches!(events[frames[last].index].0, Event::Start(Tag::Item)) {
                                if let Some(list) = &mut frames[last - 1].list {
                                    list.loose = true;
                                }
                            }
                        }
                        _ => {}
                    }
                    frames.push(frame);
                    let leaf = match tag {
                        Tag::Paragraph | Tag::Item => Some(Leaf::Paragraph),
                        Tag::Heading { level, .. } => Some(Leaf::Heading(*level as u8)),
                        Tag::CodeBlock(kind) => Some(Leaf::Code { fenced: kind.is_fenced() }),
                        Tag::Table(_) => Some(Leaf::Table),
                        Tag::HtmlBlock => Some(Leaf::Html),
                        Tag::BlockQuote(_) => Some(Leaf::None),
                        Tag::FootnoteDefinition(_) | Tag::MetadataBlock(_)
                            | Tag::DefinitionListTitle | Tag::DefinitionListDefinition => Some(Leaf::Other),
                        _ => None,
                    };
                    if let Some(leaf) = leaf.filter(|_| holds && frames.len() >= depth) {
                        depth = frames.len();
                        selected = frames.iter().filter_map(|f| f.container.clone()).collect();
                        out.leaf = leaf;
                        out.leaf_range = range.clone();
                    }
                }
                Event::End(_) => {
                    let Some(frame) = frames.pop() else { continue };
                    if let Some(list) = frame.list.filter(|_| holds(text, &events[frame.index].1, at)) {
                        for (index, container) in &mut selected {
                            if *index == frame.index {
                                if let Container::Item { loose, .. } = container {
                                    *loose = list.loose;
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        out.stack = selected.into_iter().map(|(_, container)| container).collect();
        out
    }

    /// A new line's prefix: `> ` per quote and indentation per item.
    pub fn continuation(&self) -> String {
        continuation(&self.stack)
    }

    /// The continuation prefix without trailing spaces, for a blank line.
    pub fn blank_continuation(&self) -> String {
        self.continuation().trim_end_matches(' ').to_string()
    }

    /// The line's first byte after its written container prefixes.
    pub fn content_start(&self, text: &str) -> usize {
        content_start(text, self.line.clone(), &self.stack)
    }
}

/// A block's range holds `at` from its start up to its end; its end too
/// when that is not the start of a line (a block's range takes its last
/// line's `\n`, and the next line belongs to the next block).
fn holds(text: &str, range: &Range<usize>, at: usize) -> bool {
    range.start <= at && (at < range.end || (at == range.end && !text[..at].ends_with('\n')))
}

pub(crate) fn line_at(text: &str, at: usize) -> Range<usize> {
    let start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let end = text[at..].find('\n').map_or(text.len(), |i| at + i);
    start..end
}

fn container_width(container: &Container) -> usize {
    match container {
        Container::Quote => 2,
        Container::Item { indent, width, .. } => indent + width,
    }
}

fn item_at(
    text: &str,
    events: &[(Event<'_>, Range<usize>)],
    index: usize,
    outer_width: usize,
    number: Option<u64>,
    loose: bool,
) -> Container {
    let start = events[index].1.start;
    let line = line_at(text, start);
    let leading = text[start..line.end].bytes().take_while(|&b| b == b' ').count();
    let marker_start = start + leading;
    let source = &text[marker_start..line.end];
    let digits = source.bytes().take_while(u8::is_ascii_digit).count();
    let marker_len = digits + 1;
    let marker = source[..marker_len].to_string();
    let padding = source[marker_len..].bytes().take_while(|b| matches!(b, b' ' | b'\t')).count();
    let padding = if padding == 0 || padding > 4 { 1 } else { padding };
    let mut next = index + 1;
    if matches!(events.get(next), Some((Event::Start(Tag::Paragraph), _))) {
        next += 1;
    }
    let task = match events.get(next) {
        Some((Event::TaskListMarker(checked), _)) => Some(*checked),
        _ => None,
    };
    Container::Item {
        marker, number, indent: (marker_start - line.start).saturating_sub(outer_width),
        width: marker_len + padding, loose, task,
    }
}

fn continuation(stack: &[Container]) -> String {
    let mut out = String::new();
    for container in stack {
        match container {
            Container::Quote => out.push_str("> "),
            Container::Item { indent, width, .. } => out.push_str(&" ".repeat(indent + width)),
        }
    }
    out
}

pub(crate) fn content_start(text: &str, line: Range<usize>, stack: &[Container]) -> usize {
    let mut at = line.start;
    for container in stack {
        let source = &text[at..line.end];
        match container {
            Container::Quote => {
                let spaces = source.bytes().take_while(|&b| b == b' ').count();
                if source.as_bytes().get(spaces) == Some(&b'>') {
                    at += spaces + 1;
                    if text.as_bytes().get(at) == Some(&b' ') {
                        at += 1;
                    }
                }
            }
            Container::Item { marker, indent, width, .. } => {
                let spaces = source.bytes().take_while(|&b| b == b' ').count();
                let first = source.get(*indent..).filter(|_| spaces >= *indent);
                let first = first.and_then(|s| s.strip_prefix(marker.as_str()));
                if first.is_some_and(|s| s.is_empty() || s.starts_with([' ', '\t'])) {
                    at += (indent + width).min(source.len());
                } else {
                    at += spaces.min(indent + width);
                }
            }
        }
    }
    at
}

fn next_item(container: &Container) -> String {
    let Container::Item { marker, number, indent, width, task, .. } = container else {
        return String::new();
    };
    let mut out = " ".repeat(*indent);
    match number {
        Some(n) => out.push_str(&format!("{}{}", n + 1, &marker[marker.len() - 1..])),
        None => out.push_str(marker),
    }
    out.push_str(&" ".repeat(width - marker.len()));
    if task.is_some() {
        out.push_str("[ ] ");
    }
    out
}

/// Raw typing, with escaped table pipes and closing fences.
pub fn type_text(text: &str, events: &[(Event<'_>, Range<usize>)], caret: usize, typed: &str) -> Plan {
    let containers = Containers::at(text, events, caret);
    let in_cell = events.iter().any(|(event, range)| {
        matches!(event, Event::Start(Tag::TableCell)) && holds(text, range, caret)
    });
    // Spaces at the start of a line's content would indent it (four make a
    // code block) and show nothing: there they do nothing.
    let blank = !typed.is_empty() && typed.trim_matches([' ', '\t']).is_empty();
    if blank && !matches!(containers.leaf, Leaf::Code { .. }) && caret == containers.content_start(text) {
        return Plan { edits: Vec::new(), caret }.checked(text);
    }
    let typed = if in_cell { typed.replace('|', "\\|") } else { typed.to_string() };
    // A blank line is where the final view shows an empty paragraph (the
    // line after the document's last one, say): text typed there is a
    // paragraph of its own, unless the block around it goes on below it
    // (the gap between two items of a list). Written next to a line with
    // text it would continue that block (a paragraph, an item or a quote
    // lazily, a table as a row), so a blank line goes between them.
    if !typed.trim().is_empty()
        && !matches!(containers.leaf, Leaf::Code { .. } | Leaf::Html)
        && text[containers.line.clone()].trim().is_empty()
        && !goes_on(text, events, caret, containers.line.end)
    {
        let filled = |at: usize| !text[line_at(text, at)].trim().is_empty();
        let above = containers.line.start > 0 && filled(containers.line.start - 1);
        let below = containers.line.end < text.len() && filled(containers.line.end + 1) && !typed.ends_with('\n');
        if above || below {
            let before = if above { "\n" } else { "" };
            let after = if below { "\n" } else { "" };
            return Plan {
                caret: caret + before.len() + typed.len(),
                edits: vec![(caret..caret, format!("{before}{typed}{after}"))],
            }
            .checked(text);
        }
    }
    let mut plan = Plan::insert(caret, typed.clone());
    if matches!(typed.as_str(), "`" | "~") && !matches!(containers.leaf, Leaf::Code { .. }) {
        let start = containers.content_start(text);
        // Only at the line's end: a closing fence typed into the middle of
        // the backticks would split them.
        if start <= caret && caret == containers.line.end && containers.line.end - start + typed.len() == 3 {
            let fence = typed.as_bytes()[0];
            if text[start..containers.line.end].bytes().all(|b| b == fence) {
                let closing = format!("\n{}{}", containers.continuation(), typed.repeat(3));
                plan.edits[0].1.push_str(&closing);
            }
        }
    }
    plan.checked(text)
}

/// Whether a block holding `at` also holds the first line with text after
/// `line_end`: the block goes on past a blank line at `at`.
fn goes_on(text: &str, events: &[(Event<'_>, Range<usize>)], at: usize, line_end: usize) -> bool {
    let mut next = line_end;
    while next < text.len() {
        let line = line_at(text, next + 1);
        if !text[line.clone()].trim().is_empty() {
            return events.iter().any(|(event, range)| {
                matches!(event, Event::Start(_)) && holds(text, range, at) && holds(text, range, line.start)
            });
        }
        next = line.end;
    }
    false
}

fn code_enter(text: &str, caret: usize, containers: &Containers) -> Plan {
    let start = containers.content_start(text);
    let indent = text[start..containers.line.end].bytes()
        .take_while(|b| matches!(b, b' ' | b'\t')).count();
    Plan::insert(caret, format!("\n{}{}", containers.continuation(), &text[start..start + indent]))
}

pub(crate) fn heading_start(text: &str, containers: &Containers) -> usize {
    let start = containers.leaf_range.start;
    let line = line_at(text, start);
    let source = &text[start..line.end];
    let hashes = source.bytes().take_while(|&b| b == b'#').count();
    if hashes > 0 && (source.len() == hashes || source[hashes..].starts_with([' ', '\t'])) {
        start + hashes + source[hashes..].bytes().take_while(|b| matches!(b, b' ' | b'\t')).count()
    } else {
        start
    }
}

fn empty_enter(text: &str, containers: &Containers) -> Option<Plan> {
    let last = containers.stack.len().checked_sub(1)?;
    let outer = &containers.stack[..last];
    match &containers.stack[last] {
        Container::Item { indent, .. } => {
            let parent = outer.iter().rposition(|container| matches!(container, Container::Item { .. }));
            let (start, replacement) = match parent {
                Some(parent) => (
                    content_start(text, containers.line.clone(), &outer[..parent]),
                    next_item(&outer[parent]),
                ),
                None => {
                    let start = content_start(text, containers.line.clone(), outer);
                    let spaces = text[start..containers.line.end].bytes().take_while(|&b| b == b' ').count();
                    (start + spaces.min(*indent), String::new())
                }
            };
            Some(Plan {
                caret: start + replacement.len(), edits: vec![(start..containers.line.end, replacement)],
            })
        }
        Container::Quote => {
            let replacement = continuation(outer);
            Some(Plan {
                caret: containers.line.start + replacement.len(),
                edits: vec![(containers.line.start..containers.content_start(text), replacement)],
            })
        }
    }
}

/// Enter: a code line, a new item, a paragraph split, or one level out.
pub fn enter(text: &str, events: &[(Event<'_>, Range<usize>)], caret: usize) -> Option<Plan> {
    let containers = Containers::at(text, events, caret);
    let content = text[containers.content_start(text)..containers.line.end].trim_matches([' ', '\t']);
    let empty_task = matches!(containers.stack.last(), Some(Container::Item { task: Some(_), .. }))
        && matches!(content, "[ ]" | "[x]" | "[X]");
    let plan = match containers.leaf {
        Leaf::Code { .. } => code_enter(text, caret, &containers),
        Leaf::Table | Leaf::Html => return None,
        _ if content.is_empty() || empty_task => empty_enter(text, &containers)?,
        Leaf::Heading(_) if caret <= heading_start(text, &containers) => {
            let inserted = format!("{}\n", containers.blank_continuation());
            let mut plan = Plan::insert(containers.line.start, inserted);
            plan.caret += caret - containers.line.start;
            plan
        }
        Leaf::Heading(_) => Plan::insert(caret, format!(
            "\n{}\n{}", containers.blank_continuation(), containers.continuation(),
        )),
        _ => match containers.stack.last() {
            Some(item @ Container::Item { loose, .. }) => {
                let outer = continuation(&containers.stack[..containers.stack.len() - 1]);
                let mut inserted = "\n".to_string();
                if *loose {
                    inserted.push_str(outer.trim_end_matches(' '));
                    inserted.push('\n');
                }
                inserted.push_str(&outer);
                inserted.push_str(&next_item(item));
                Plan::insert(caret, inserted)
            }
            _ => Plan::insert(caret, format!(
                "\n{}\n{}", containers.blank_continuation(), containers.continuation(),
            )),
        },
    };
    Some(plan.checked(text))
}

/// Shift+Enter: a backslash hard break, or an indented newline in code.
pub fn hard_break(text: &str, events: &[(Event<'_>, Range<usize>)], caret: usize) -> Option<Plan> {
    let containers = Containers::at(text, events, caret);
    let plan = match containers.leaf {
        Leaf::Code { .. } => code_enter(text, caret, &containers),
        Leaf::Heading(_) | Leaf::Table | Leaf::Html | Leaf::None => return None,
        _ => Plan::insert(caret, format!("\\\n{}", containers.continuation())),
    };
    Some(plan.checked(text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_options;
    use pulldown_cmark::Parser;

    fn case(
        rule: &str,
        before: &str,
        expected: &str,
        edit: impl FnOnce(&str, &[(Event<'_>, Range<usize>)], usize) -> Option<Plan>,
    ) {
        let caret = before.find('‸').expect("case has a caret");
        let text = before.replace('‸', "");
        assert!(text.ends_with('\n'), "{rule}: slice lines end in a newline");
        let events: Vec<_> = Parser::new_ext(&text, parse_options()).into_offset_iter().collect();
        let plan = edit(&text, &events, caret);
        let mut actual = match &plan {
            Some(plan) => {
                assert!(plan.valid(&text), "{rule}: invalid plan {plan:?}");
                plan.apply(&text)
            }
            None => text.clone(),
        };
        actual.insert_str(plan.as_ref().map_or(caret, |plan| plan.caret), "‸");
        assert_eq!(actual, expected, "{rule}\nevents: {events:?}");
        if before == expected {
            assert!(plan.is_none(), "{rule}: the key must do nothing");
        }
    }

    #[test]
    fn typing_is_raw_except_in_cells_and_fences() {
        for (rule, before, typed, after) in [
            ("raw paragraph", "a‸b\n", "*|é", "a*|é‸b\n"),
            ("header cell pipes", "| a‸ | b |\n| - | - |\n", "||", "| a\\|\\|‸ | b |\n| - | - |\n"),
            ("body cell pipe", "| a |\n| - |\n| b‸ |\n", "|", "| a |\n| - |\n| b\\|‸ |\n"),
            ("top-level fence", "``‸\n", "`", "```‸\n```\n"),
            ("tilde fence", "~~‸\n", "~", "~~~‸\n~~~\n"),
            ("item fence", "- ``‸\n", "`", "- ```‸\n  ```\n"),
            ("quote fence", "> ``‸\n", "`", "> ```‸\n> ```\n"),
            ("existing code fence", "```\na\n``‸\n", "`", "```\na\n```‸\n"),
            ("text before fence", "a``‸\n", "`", "a```‸\n"),
            ("only one typed delimiter", "‸\n", "```", "```‸\n"),
            ("exactly three delimiters", "```‸\n", "`", "````‸\n"),
            ("fence only at the line end", "`‸`\n", "`", "``‸`\n"),
            ("leading space does nothing", "‸a\n", " ", "‸a\n"),
            ("leading space in an item does nothing", "- ‸a\n", " ", "- ‸a\n"),
            ("space on a blank line does nothing", "a\n\n‸\n", " ", "a\n\n‸\n"),
            ("space after text", "a‸\n", " ", "a ‸\n"),
            ("indent in code", "```\n‸a\n```\n", " ", "```\n ‸a\n```\n"),
            // Text typed on a blank line is its own paragraph, never a lazy
            // continuation of a neighbour; in code it stays as typed.
            ("after the last paragraph", "a\n‸\n", "b", "a\n\nb‸\n"),
            ("after a list", "- a\n‸\n", "b", "- a\n\nb‸\n"),
            ("after a quote", "> a\n‸\n", "b", "> a\n\nb‸\n"),
            ("after a table", "| a |\n| - |\n‸\n", "b", "| a |\n| - |\n\nb‸\n"),
            ("before a paragraph", "‸\nb\n", "a", "a‸\n\nb\n"),
            ("between paragraphs", "a\n‸\nb\n", "x", "a\n\nx‸\n\nb\n"),
            ("pasted lines before a paragraph", "‸\nb\n", "a\n", "a\n‸\nb\n"),
            ("between blank lines stays", "a\n\n‸\n\nb\n", "x", "a\n\nx‸\n\nb\n"),
            ("blank code line stays", "```\na\n‸\nb\n```\n", "x", "```\na\nx‸\nb\n```\n"),
        ] {
            case(rule, before, after, |text, events, caret| {
                Some(type_text(text, events, caret, typed)).filter(|plan| !plan.edits.is_empty())
            });
        }
    }

    #[test]
    fn enter_continues_the_containers() {
        for (rule, before, after) in [
            ("paragraph split", "a‸b\n", "a\n\n‸b\n"),
            ("quoted paragraph", "> a‸b\n", "> a\n>\n> ‸b\n"),
            ("tight bullet", "- a‸\n", "- a\n- ‸\n"),
            ("star bullet", "* a‸\n", "* a\n* ‸\n"),
            ("plus bullet", "+ a‸\n", "+ a\n+ ‸\n"),
            ("ordered dot", "7. a‸\n", "7. a\n8. ‸\n"),
            ("ordered parenthesis", "7) a‸\n", "7) a\n8) ‸\n"),
            ("ordered sibling ordinal", "7. a\n2. b‸\n", "7. a\n2. b\n9. ‸\n"),
            ("loose list", "- a‸\n\n- b\n", "- a\n\n- ‸\n\n- b\n"),
            ("loose quoted list", "> - a‸\n>\n> - b\n", "> - a\n>\n> - ‸\n>\n> - b\n"),
            ("task resets", "- [x] a‸\n", "- [x] a\n- [ ] ‸\n"),
            ("nested item", "- a\n  - b‸\n", "- a\n  - b\n  - ‸\n"),
            ("list in quote", "> - a‸\n", "> - a\n> - ‸\n"),
            ("suffix moves", "- a‸b\n", "- a\n- ‸b\n"),
            ("marker padding", "  -   a‸\n", "  -   a\n  -   ‸\n"),
            ("later item line", "- a\n  b‸\n", "- a\n  b\n- ‸\n"),
            ("lazy item line", "- a\nb‸\n", "- a\nb\n- ‸\n"),
        ] {
            case(rule, before, after, enter);
        }
    }

    #[test]
    fn enter_on_an_empty_line_steps_out() {
        for (rule, before, after) in [
            ("empty item", "- a\n- ‸\n", "- a\n‸\n"),
            ("empty task", "- [ ] ‸\n", "‸\n"),
            ("empty checked task", "- [x] ‸\n", "‸\n"),
            ("empty nested item", "- a\n  - b\n  - ‸\n", "- a\n  - b\n- ‸\n"),
            // A lone `-` under a paragraph line is a heading underline, so
            // the empty nested items follow a sibling.
            ("outer ordered marker", "7. a\n   - b\n   - ‸\n", "7. a\n   - b\n8. ‸\n"),
            ("outer task marker", "- [x] a\n  - b\n  - ‸\n", "- [x] a\n  - b\n- [ ] ‸\n"),
            ("quote level", "> a\n> ‸\n", "> a\n‸\n"),
            ("nested quote level", "> > a\n> > ‸\n", "> > a\n> ‸\n"),
            ("list inside quote", "> - a\n> - ‸\n", "> - a\n> ‸\n"),
            ("trailing item spaces", "- ‸  \n", "‸\n"),
            ("empty marker without padding", "-‸\n", "‸\n"),
            ("blank top-level", "a\n\n‸\nb\n", "a\n\n‸\nb\n"),
        ] {
            case(rule, before, after, enter);
        }
    }

    #[test]
    fn enter_in_headings_and_code() {
        for (rule, before, after) in [
            ("heading split", "# Ti‸tle\n", "# Ti\n\n‸tle\n"),
            ("heading moves down", "# ‸Title\n", "\n# ‸Title\n"),
            ("heading marker caret", "‸# Title\n", "\n‸# Title\n"),
            ("quoted heading moves", "> # ‸Title\n", ">\n> # ‸Title\n"),
            ("item heading splits", "- # Ti‸tle\n", "- # Ti\n\n  ‸tle\n"),
            ("setext heading moves", "‸Title\n===\n", "\n‸Title\n===\n"),
            ("code indentation", "```\n    a‸\n```\n", "```\n    a\n    ‸\n```\n"),
            ("code tabs", "```\n\ta‸\n```\n", "```\n\ta\n\t‸\n```\n"),
            ("opening fence", "```rust‸\n```\n", "```rust\n‸\n```\n"),
            ("quoted code", "> ```\n>   a‸\n> ```\n", "> ```\n>   a\n>   ‸\n> ```\n"),
            ("item code", "- ```\n  a‸\n  ```\n", "- ```\n  a\n  ‸\n  ```\n"),
            ("indented code", "    a‸\n", "    a\n    ‸\n"),
            ("wide marker padding becomes code", "-     a‸\n", "-     a\n      ‸\n"),
            ("table inert", "| a‸ |\n| - |\n", "| a‸ |\n| - |\n"),
            ("html inert", "<div>\na‸\n</div>\n", "<div>\na‸\n</div>\n"),
        ] {
            case(rule, before, after, enter);
        }
    }

    #[test]
    fn hard_break_continues_the_line() {
        for (rule, before, after) in [
            ("paragraph hard break", "a‸b\n", "a\\\n‸b\n"),
            ("item hard break", "- a‸\n", "- a\\\n  ‸\n"),
            ("quote hard break", "> a‸\n", "> a\\\n> ‸\n"),
            ("heading inert", "# a‸\n", "# a‸\n"),
            ("table inert", "| a‸ |\n| - |\n", "| a‸ |\n| - |\n"),
            ("html inert", "<div>\na‸\n</div>\n", "<div>\na‸\n</div>\n"),
            ("blank inert", "‸\n", "‸\n"),
            ("code newline", "```\na‸\n```\n", "```\na\n‸\n```\n"),
            ("code indentation", "```\n  a‸\n```\n", "```\n  a\n  ‸\n```\n"),
        ] {
            case(rule, before, after, hard_break);
        }
    }

    #[test]
    fn containers_describe_source_and_boundaries() {
        let text = "> 7. a\n>\n> 2. [x] b\n>    more\n\n\ntail\n";
        let events: Vec<_> = Parser::new_ext(text, parse_options()).into_offset_iter().collect();
        let at = text.find("more").unwrap();
        let containers = Containers::at(text, &events, at);
        assert_eq!(containers.stack, vec![Container::Quote, Container::Item {
            marker: "2.".into(), number: Some(8), indent: 0, width: 3, loose: true, task: Some(true),
        }], "source marker, list ordinal, looseness and task state");
        assert_eq!(containers.leaf, Leaf::Paragraph, "direct paragraph is the leaf");
        assert_eq!(containers.continuation(), ">    ", "outer-to-inner continuation");
        assert_eq!(containers.blank_continuation(), ">", "blank continuation trims spaces");
        assert_eq!(containers.content_start(text), at, "continuation prefix as written");
        assert!(containers.leaf_range.contains(&at), "leaf range holds its content");
        assert_eq!(&text[containers.line], ">    more", "line excludes its newline");
        let blank = Containers::at(text, &events, text.find("\n\n\n").unwrap() + 2);
        assert_eq!((blank.stack, blank.leaf), (vec![], Leaf::None), "unclaimed blank line");

        let text = "é\n";
        let events: Vec<_> = Parser::new_ext(text, parse_options()).into_offset_iter().collect();
        let range = events[0].1.clone();
        for at in [range.start, 2] {
            let containers = Containers::at(text, &events, at);
            assert_eq!(containers.leaf, Leaf::Paragraph, "a block holds its start and its line's end");
            assert_eq!(containers.leaf_range, range, "leaf keeps the parser range");
        }
        let list = "- a\n\nb\n";
        let events: Vec<_> = Parser::new_ext(list, parse_options()).into_offset_iter().collect();
        let containers = Containers::at(list, &events, list.find('b').unwrap());
        assert_eq!((containers.stack, containers.leaf), (vec![], Leaf::Paragraph), "the next line is the next block's");
        let plan = Plan { edits: vec![(0..2, "ø".into()), (3..3, "fin".into())], caret: 6 };
        assert_eq!(plan.apply(text), "ø\nfin", "disjoint edits use old UTF-8 offsets");
    }
}
