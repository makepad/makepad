//! The block index: top-level blocks as line ranges that tile the document,
//! rebuilt incrementally around each edit.

use pulldown_cmark::{BrokenLink, CowStr, Event, Options, Parser, Tag};
use std::collections::{HashMap, HashSet};
use std::ops::Range;

/// Lines per row of a top-level code block: a long block draws as several
/// rows so a frame lays out only the lines on screen.
pub const CODE_ROW_LINES: usize = 40;

/// The parse options every parse of an indexed document uses: the window
/// parses, the row parses and a full build alike. Front matter is not an
/// option here: the index recognises it at line 0 itself (the parser would
/// take a `---` block anywhere).
pub fn parse_options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_MATH
        | Options::ENABLE_GFM
}

/// What a top-level block is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockKind {
    /// Lines no block claims: blank lines or link reference definitions at
    /// the start of the document, or a document without blocks.
    Blank,
    /// A `---` ... `---` (or `...`) block at line 0.
    FrontMatter,
    Paragraph,
    Heading(u8),
    Quote,
    List { ordered: bool, start: u64, tight: bool },
    CodeFenced,
    CodeIndented,
    Html,
    Rule,
    Table,
    FootnoteDef,
    Other,
}

/// The document-wide definitions a block holds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BlockDefs {
    /// Link reference definitions: normalised label, destination, title.
    pub links: Vec<(String, String, String)>,
    /// Footnote definitions: normalised labels.
    pub footnotes: Vec<String>,
    /// Footnote references in source order, normalised labels, whether the
    /// document defines them or not (a definition elsewhere can come later).
    pub footnote_refs: Vec<String>,
}

/// One top-level block. Its lines run from `start` to the next block's start
/// (or the end of the document), blank lines after it included.
#[derive(Clone, Debug)]
pub struct Block {
    pub start: usize,
    pub kind: BlockKind,
    /// Changes whenever the block is reparsed: a row's cache key.
    pub gen: u64,
    /// A top-level list: the line of each direct item, from `start`.
    pub items: Vec<u32>,
    /// A top-level code block: its content lines, from `start`.
    pub code: Option<Range<u32>>,
    pub defs: Option<Box<BlockDefs>>,
}

impl Block {
    fn new(start: usize, kind: BlockKind) -> Self {
        Self {
            start,
            kind,
            gen: 0,
            items: Vec::new(),
            code: None,
            defs: None,
        }
    }

    /// Everything but the generation and the start line.
    fn same_shape(&self, other: &Block) -> bool {
        self.kind == other.kind
            && self.items == other.items
            && self.code == other.code
            && self.defs == other.defs
    }

    fn row_count(&self) -> usize {
        match self.kind {
            BlockKind::List { .. } => self.items.len().max(1),
            BlockKind::CodeFenced | BlockKind::CodeIndented => match &self.code {
                Some(code) => ((code.end - code.start) as usize)
                    .div_ceil(CODE_ROW_LINES)
                    .max(1),
                None => 1,
            },
            _ => 1,
        }
    }

    fn defs_mut(&mut self) -> &mut BlockDefs {
        self.defs.get_or_insert_with(Default::default)
    }
}

/// What part of its block a row draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowPart {
    Whole,
    /// One direct item of a top-level list.
    Item { index: usize, count: usize },
    /// A run of a top-level code block's lines (the first holds the opening
    /// fence, the last the closing one).
    Code { chunk: usize, count: usize },
}

/// A draw unit: a block, or a part of a long one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub block: usize,
    pub lines: Range<usize>,
    pub part: RowPart,
}

/// One edit's effect on lines, in the order the edits were applied: it
/// starts on `line`, removes `removed` line breaks and adds `added`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineEdit {
    pub line: usize,
    pub removed: usize,
    pub added: usize,
}

/// What an [`Index::apply`] replaced: these old blocks (and rows) became
/// these new ones; everything after them shifted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Splice {
    pub old_blocks: Range<usize>,
    pub new_blocks: Range<usize>,
    pub old_rows: Range<usize>,
    pub new_rows: Range<usize>,
    /// The link or footnote tables changed: rows that resolve against them
    /// may draw differently.
    pub globals_changed: bool,
}

/// A run of the document's lines as one string to parse: every line ends in
/// `\n`. Offsets are bytes into `text`.
pub struct Slice {
    pub first_line: usize,
    pub text: String,
    line_starts: Vec<usize>,
}

impl Slice {
    pub fn new(lines: &[String], range: Range<usize>) -> Self {
        let lines = &lines[range.start..range.end];
        let mut text = String::with_capacity(lines.iter().map(|line| line.len() + 1).sum());
        let mut line_starts = Vec::with_capacity(lines.len() + 1);
        for line in lines {
            line_starts.push(text.len());
            text.push_str(line);
            text.push('\n');
        }
        line_starts.push(text.len());
        Self {
            first_line: range.start,
            text,
            line_starts,
        }
    }

    /// The number of document lines in the slice.
    pub fn line_count(&self) -> usize {
        self.line_starts.len() - 1
    }

    /// The document line holding `offset` (the line after the slice for its end).
    pub fn line_of(&self, offset: usize) -> usize {
        self.first_line + self.line_starts.partition_point(|&start| start <= offset) - 1
    }

    /// The document `(line, byte)` of an offset.
    pub fn pos(&self, offset: usize) -> (usize, usize) {
        let line = self.line_of(offset);
        (line, offset - self.line_starts[line - self.first_line])
    }

    /// The offset of a document `(line, byte)` inside the slice.
    pub fn offset(&self, line: usize, byte: usize) -> usize {
        self.line_starts[line - self.first_line] + byte
    }
}

/// Normalises a link or footnote label the way the parser compares them:
/// whitespace runs collapse, case folds.
fn normalize_label(label: &str) -> String {
    let mut out = String::with_capacity(label.len());
    for word in label.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&word.to_lowercase());
    }
    out
}

/// The closing line of a front matter block at line 0: a `---` line, a first
/// line that is neither blank nor a closer, and a `---` or `...` line.
pub fn front_matter_end(lines: &[String]) -> Option<usize> {
    let is_delimiter = |line: &str| line.trim_end() == "---";
    let first = lines.first()?;
    if !is_delimiter(first) {
        return None;
    }
    let second = lines.get(1)?;
    let is_closer = |line: &str| {
        let line = line.trim_end();
        line == "---" || line == "..."
    };
    if second.trim().is_empty() || is_closer(second) {
        return None;
    }
    (2..lines.len()).find(|&i| is_closer(&lines[i]))
}

/// Parses the lines of `range` (which starts at a top-level block boundary)
/// into blocks with absolute start lines. Every `[^label]` counts as a
/// footnote reference here, so a block's references do not depend on the
/// definitions elsewhere in the document.
fn parse_window(lines: &[String], range: Range<usize>) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut start = range.start;
    if start == 0 {
        if let Some(end) = front_matter_end(lines) {
            if end >= range.end {
                // The front matter runs past the window: the window is all of
                // it, so no boundary inside it can resync.
                blocks.push(Block::new(0, BlockKind::FrontMatter));
                return blocks;
            }
            blocks.push(Block::new(0, BlockKind::FrontMatter));
            start = end + 1;
        }
    }
    if start >= range.end {
        return blocks;
    }
    let first_new = blocks.len();
    let slice = Slice::new(lines, start..range.end);
    let mut parser = Parser::new_ext(&slice.text, parse_options());
    parser.assume_footnote_labels_defined();
    let mut iter = parser.into_offset_iter();
    let mut depth = 0usize;
    // The open top-level block is the last one pushed while depth > 0.
    let mut code_lines: Option<(usize, usize)> = None;
    for (event, range) in iter.by_ref() {
        match &event {
            Event::Start(tag) => {
                let line = slice.line_of(range.start);
                if depth == 0 {
                    let kind = match tag {
                        Tag::Paragraph => BlockKind::Paragraph,
                        Tag::Heading { level, .. } => BlockKind::Heading(*level as u8),
                        Tag::BlockQuote(_) => BlockKind::Quote,
                        Tag::CodeBlock(kind) if kind.is_fenced() => BlockKind::CodeFenced,
                        Tag::CodeBlock(_) => BlockKind::CodeIndented,
                        Tag::HtmlBlock => BlockKind::Html,
                        Tag::List(number) => BlockKind::List {
                            ordered: number.is_some(),
                            start: number.unwrap_or(1),
                            tight: true,
                        },
                        Tag::Table(_) => BlockKind::Table,
                        Tag::FootnoteDefinition(_) => BlockKind::FootnoteDef,
                        Tag::MetadataBlock(_) => BlockKind::FrontMatter,
                        _ => BlockKind::Other,
                    };
                    push_block(&mut blocks, first_new, Block::new(line, kind));
                    code_lines = None;
                    if let Tag::FootnoteDefinition(label) = tag {
                        if let Some(block) = blocks.last_mut() {
                            block.defs_mut().footnotes.push(normalize_label(label));
                        }
                    }
                } else if let Some(block) = blocks.last_mut() {
                    match (&mut block.kind, depth, tag) {
                        (BlockKind::List { .. }, 1, Tag::Item) => {
                            let offset = (line - block.start) as u32;
                            if block.items.last() != Some(&offset) {
                                block.items.push(offset);
                            }
                        }
                        (BlockKind::List { tight, .. }, 2, Tag::Paragraph) => *tight = false,
                        _ => {}
                    }
                }
                depth += 1;
            }
            Event::End(_) => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    if let Some(block) = blocks.last_mut() {
                        match block.kind {
                            BlockKind::CodeFenced => {
                                let first = block.start + 1;
                                let (from, to) = code_lines.unwrap_or((first, first));
                                block.code = Some(
                                    (from.max(first) - block.start) as u32
                                        ..(to.max(first) - block.start) as u32,
                                );
                            }
                            BlockKind::CodeIndented => {
                                let (from, to) = code_lines.unwrap_or((block.start, block.start));
                                block.code = Some(
                                    (from - block.start) as u32..(to - block.start) as u32,
                                );
                            }
                            _ => {}
                        }
                    }
                }
            }
            Event::FootnoteReference(label) => {
                if let Some(block) = blocks.last_mut() {
                    block.defs_mut().footnote_refs.push(normalize_label(label));
                }
            }
            Event::Rule if depth == 0 => {
                let line = slice.line_of(range.start);
                push_block(&mut blocks, first_new, Block::new(line, BlockKind::Rule));
            }
            Event::Text(_) if depth == 1 && !range.is_empty() => {
                if let Some(block) = blocks.last() {
                    if matches!(block.kind, BlockKind::CodeFenced | BlockKind::CodeIndented) {
                        let from = slice.line_of(range.start);
                        let to = slice.line_of(range.end - 1) + 1;
                        code_lines = Some(match code_lines {
                            Some((a, b)) => (a.min(from), b.max(to)),
                            None => (from, to),
                        });
                    }
                }
            }
            _ => {}
        }
    }
    // Lines before the first block (only at the start of the document or
    // after the front matter: every other window starts at a block).
    if blocks.len() == first_new || blocks[first_new].start > start {
        blocks.insert(first_new, Block::new(start, BlockKind::Blank));
    }
    let mut definitions: Vec<(usize, String, String, String)> = iter
        .reference_definitions()
        .iter()
        .map(|(label, def)| {
            (
                def.span.start,
                normalize_label(label),
                def.dest.to_string(),
                def.title.as_ref().map(|title| title.to_string()).unwrap_or_default(),
            )
        })
        .collect();
    definitions.sort_by_key(|def| def.0);
    for (offset, label, dest, title) in definitions {
        let line = slice.line_of(offset);
        let at = blocks.partition_point(|block| block.start <= line).saturating_sub(1);
        blocks[at].defs_mut().links.push((label, dest, title));
    }
    blocks
}

/// Pushes a block unless one already starts on its line (two top-level
/// blocks never share a line; the first one keeps it).
fn push_block(blocks: &mut Vec<Block>, first_new: usize, block: Block) {
    if blocks.len() > first_new && blocks.last().map(|last| last.start) == Some(block.start) {
        return;
    }
    blocks.push(block);
}

/// The block index of one document.
#[derive(Clone)]
pub struct Index {
    blocks: Vec<Block>,
    line_count: usize,
    next_gen: u64,
    /// The first row of each block, and the row count at the end.
    row_starts: Vec<usize>,
    links: HashMap<String, (String, String)>,
    /// Defined footnote labels, in order of definition.
    footnotes: Vec<String>,
    /// The number of each defined label: in order of first reference, then
    /// the unreferenced ones in order of definition.
    footnote_numbers: HashMap<String, usize>,
}

impl Index {
    /// Indexes a whole document.
    pub fn new(lines: &[String]) -> Self {
        let mut index = Self {
            blocks: Vec::new(),
            line_count: 0,
            next_gen: 1,
            row_starts: vec![0],
            links: HashMap::new(),
            footnotes: Vec::new(),
            footnote_numbers: HashMap::new(),
        };
        index.rebuild(lines);
        index
    }

    fn rebuild(&mut self, lines: &[String]) {
        self.line_count = lines.len();
        // A whole-document parse sees every footnote definition itself.
        self.blocks = if lines.is_empty() {
            Vec::new()
        } else {
            parse_window(lines, 0..lines.len())
        };
        for block in &mut self.blocks {
            block.gen = self.next_gen;
            self.next_gen += 1;
        }
        self.rebuild_rows();
        self.rebuild_globals();
    }

    pub fn blocks(&self) -> &[Block] {
        &self.blocks
    }

    pub fn line_count(&self) -> usize {
        self.line_count
    }

    /// The lines block `index` covers.
    pub fn block_lines(&self, index: usize) -> Range<usize> {
        self.blocks[index].start..self.block_end(index)
    }

    fn block_end(&self, index: usize) -> usize {
        self.blocks
            .get(index + 1)
            .map(|block| block.start)
            .unwrap_or(self.line_count)
    }

    /// The block holding `line`.
    pub fn block_at_line(&self, line: usize) -> usize {
        self.blocks
            .partition_point(|block| block.start <= line)
            .saturating_sub(1)
    }

    pub fn row_count(&self) -> usize {
        *self.row_starts.last().unwrap_or(&0)
    }

    /// The rows of block `index`.
    pub fn block_rows(&self, index: usize) -> Range<usize> {
        self.row_starts[index]..self.row_starts[index + 1]
    }

    /// Row `index`: its block, lines and part.
    pub fn row(&self, index: usize) -> Row {
        let block_index = self
            .row_starts
            .partition_point(|&start| start <= index)
            .saturating_sub(1);
        let block = &self.blocks[block_index];
        let part_index = index - self.row_starts[block_index];
        let count = self.row_starts[block_index + 1] - self.row_starts[block_index];
        let end = self.block_end(block_index);
        match block.kind {
            BlockKind::List { .. } if !block.items.is_empty() => {
                let from = if part_index == 0 {
                    block.start
                } else {
                    block.start + block.items[part_index] as usize
                };
                let to = match block.items.get(part_index + 1) {
                    Some(&offset) => block.start + offset as usize,
                    None => end,
                };
                Row {
                    block: block_index,
                    lines: from..to,
                    part: RowPart::Item {
                        index: part_index,
                        count,
                    },
                }
            }
            BlockKind::CodeFenced | BlockKind::CodeIndented if block.code.is_some() => {
                let code_start = block.start + block.code.as_ref().map_or(0, |c| c.start as usize);
                let from = if part_index == 0 {
                    block.start
                } else {
                    code_start + part_index * CODE_ROW_LINES
                };
                let to = if part_index + 1 < count {
                    code_start + (part_index + 1) * CODE_ROW_LINES
                } else {
                    end
                };
                Row {
                    block: block_index,
                    lines: from..to,
                    part: RowPart::Code {
                        chunk: part_index,
                        count,
                    },
                }
            }
            _ => Row {
                block: block_index,
                lines: block.start..end,
                part: RowPart::Whole,
            },
        }
    }

    /// The row holding `line`.
    pub fn row_at_line(&self, line: usize) -> usize {
        let block = self.block_at_line(line);
        let rows = self.block_rows(block);
        let mut lo = rows.start;
        let mut hi = rows.end;
        // Rows of one block are few and in line order.
        while hi - lo > 1 {
            let mid = (lo + hi) / 2;
            if self.row(mid).lines.start <= line {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        lo
    }

    /// A link reference definition by label.
    pub fn link(&self, label: &str) -> Option<(&str, &str)> {
        self.links
            .get(&normalize_label(label))
            .map(|(dest, title)| (dest.as_str(), title.as_str()))
    }

    /// The number a footnote label shows as (1-based, in order of first
    /// reference; a label never referenced comes after, in definition order).
    pub fn footnote_number(&self, label: &str) -> Option<usize> {
        self.footnote_numbers.get(&normalize_label(label)).copied()
    }

    /// The defined footnote labels.
    pub fn footnote_labels(&self) -> &[String] {
        &self.footnotes
    }

    /// Parses a slice of the document the way the index sees it: links
    /// resolve through the document's reference definitions and footnote
    /// references through its footnote labels.
    pub fn parse_slice<'s>(&self, slice: &'s Slice) -> Vec<(Event<'s>, Range<usize>)> {
        let links = &self.links;
        let callback = move |link: BrokenLink<'s>| {
            links
                .get(&normalize_label(&link.reference))
                .map(|(dest, title)| (CowStr::from(dest.clone()), CowStr::from(title.clone())))
        };
        let mut parser =
            Parser::new_with_broken_link_callback(&slice.text, parse_options(), Some(callback));
        parser.declare_footnote_labels(self.footnotes.iter().map(String::as_str));
        parser.into_offset_iter().collect()
    }

    /// Brings the index up to date with `lines` after `edits` (in the order
    /// they were applied). Reparses a window from one block before the first
    /// touched block; it ends at the start of an untouched block the new
    /// parse also starts, checked with that whole block inside the window,
    /// and doubles the window past the edit until such a block or the end
    /// of the document is found.
    pub fn apply(&mut self, lines: &[String], edits: &[LineEdit]) -> Option<Splice> {
        if edits.is_empty() {
            return None;
        }
        let mut dirty: Option<(usize, usize)> = None;
        for edit in edits {
            let map = |line: usize| {
                if line <= edit.line {
                    line
                } else if line <= edit.line + edit.removed {
                    edit.line
                } else {
                    line - edit.removed + edit.added
                }
            };
            let touched = (edit.line, edit.line + edit.added);
            dirty = Some(match dirty {
                None => touched,
                Some((from, to)) => (map(from).min(touched.0), map(to).max(touched.1)),
            });
            if edit.removed != 0 || edit.added != 0 {
                let from = self.blocks.partition_point(|block| block.start <= edit.line);
                for block in &mut self.blocks[from..] {
                    block.start = map(block.start);
                }
            }
        }
        let line_count = lines.len();
        if self.blocks.is_empty() || line_count == 0 {
            let old_rows = 0..self.row_count();
            let old_blocks = 0..self.blocks.len();
            self.rebuild(lines);
            return Some(Splice {
                old_blocks,
                new_blocks: 0..self.blocks.len(),
                old_rows,
                new_rows: 0..self.row_count(),
                globals_changed: true,
            });
        }
        let (from_line, to_line) = dirty.unwrap_or((0, 0));
        let from_line = from_line.min(line_count - 1);
        let to_line = to_line.min(line_count - 1).max(from_line);
        let count = self.blocks.len();
        // The first touched block holds `from_line` or starts on it.
        let mut first = self.blocks.partition_point(|block| block.start < from_line);
        if first > 0 && (first == count || self.blocks[first].start > from_line) {
            first -= 1;
        }
        let last = self
            .blocks
            .partition_point(|block| block.start <= to_line)
            .saturating_sub(1)
            .max(first);
        // A closer line typed anywhere below an unclosed `---` at line 0
        // turns everything above it into front matter.
        if first > 0 && self.blocks[0].kind != BlockKind::FrontMatter {
            let opens = lines[0].trim_end() == "---";
            if opens
                && lines[from_line..=to_line]
                    .iter()
                    .any(|line| matches!(line.trim_end(), "---" | "..."))
            {
                first = 0;
            }
        }
        let window_block = first.saturating_sub(1);
        let window_start = if window_block == 0 {
            0
        } else {
            self.blocks[window_block].start
        };
        let guard = last + 1;
        let mut span = 1;
        let (resync, mut parsed) = loop {
            let end_block = (guard + span).min(count);
            let window_end = if end_block == count {
                line_count
            } else {
                self.blocks[end_block].start
            };
            let parsed = parse_window(lines, window_start..window_end);
            let resync = (guard..end_block).find(|&candidate| {
                let line = self.blocks[candidate].start;
                parsed
                    .binary_search_by_key(&line, |block| block.start)
                    .is_ok()
            });
            if let Some(candidate) = resync {
                let line = self.blocks[candidate].start;
                let keep = parsed.partition_point(|block| block.start < line);
                let mut parsed = parsed;
                parsed.truncate(keep);
                break (candidate, parsed);
            }
            if end_block == count {
                break (count, parsed);
            }
            span *= 2;
        };
        // The block before the edit is reparsed only to see whether the
        // edit joins it; unchanged, it keeps its generation and its rows.
        let mut replace_from = window_block;
        if window_block < first && !parsed.is_empty() {
            let new_end = parsed.get(1).map(|block| block.start).unwrap_or(if resync == count {
                line_count
            } else {
                self.blocks[resync].start
            });
            let old = &self.blocks[window_block];
            if parsed[0].start == old.start
                && new_end == self.blocks[window_block + 1].start
                && parsed[0].same_shape(old)
            {
                parsed.remove(0);
                replace_from += 1;
            }
        }
        let old_rows = self.row_starts[replace_from]..self.row_starts[resync];
        let touches_globals = self.blocks[replace_from..resync]
            .iter()
            .chain(parsed.iter())
            .any(|block| block.defs.is_some());
        for block in &mut parsed {
            block.gen = self.next_gen;
            self.next_gen += 1;
        }
        let new_len = parsed.len();
        self.blocks.splice(replace_from..resync, parsed);
        self.line_count = line_count;
        self.rebuild_rows();
        let globals_changed = touches_globals && self.rebuild_globals();
        Some(Splice {
            old_blocks: replace_from..resync,
            new_blocks: replace_from..replace_from + new_len,
            old_rows,
            new_rows: self.row_starts[replace_from]..self.row_starts[replace_from + new_len],
            globals_changed,
        })
    }

    fn rebuild_rows(&mut self) {
        self.row_starts.clear();
        let mut row = 0;
        for block in &self.blocks {
            self.row_starts.push(row);
            row += block.row_count();
        }
        self.row_starts.push(row);
    }

    /// Rebuilds the link and footnote tables from the blocks' definitions
    /// and references: one walk over the blocks. Returns whether they changed.
    fn rebuild_globals(&mut self) -> bool {
        let mut links = HashMap::new();
        let mut footnotes = Vec::new();
        let mut defined = HashSet::new();
        for block in &self.blocks {
            let Some(defs) = &block.defs else { continue };
            for (label, dest, title) in &defs.links {
                links
                    .entry(label.clone())
                    .or_insert_with(|| (dest.clone(), title.clone()));
            }
            for label in &defs.footnotes {
                if defined.insert(label.as_str()) {
                    footnotes.push(label.clone());
                }
            }
        }
        let mut numbers = HashMap::new();
        let referenced = self
            .blocks
            .iter()
            .filter_map(|block| block.defs.as_ref())
            .flat_map(|defs| defs.footnote_refs.iter());
        for label in referenced.chain(footnotes.iter()) {
            if defined.contains(label.as_str()) && !numbers.contains_key(label) {
                numbers.insert(label.clone(), numbers.len() + 1);
            }
        }
        let changed =
            links != self.links || footnotes != self.footnotes || numbers != self.footnote_numbers;
        self.links = links;
        self.footnotes = footnotes;
        self.footnote_numbers = numbers;
        changed
    }

    /// The blocks' shapes, for comparing two indexes of one text.
    pub fn same_blocks(&self, other: &Index) -> bool {
        self.line_count == other.line_count
            && self.blocks.len() == other.blocks.len()
            && self
                .blocks
                .iter()
                .zip(&other.blocks)
                .all(|(a, b)| a.start == b.start && a.same_shape(b))
            && self.row_starts == other.row_starts
            && self.links == other.links
            && self.footnotes == other.footnotes
            && self.footnote_numbers == other.footnote_numbers
    }
}
