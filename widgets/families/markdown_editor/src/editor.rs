//! The final view of a Markdown document: its blocks drawn as rich text in a
//! virtual list, over the same `CodeDocument` the source view edits. The view
//! joins the document's edit broadcast as a session of its own, so every
//! change (typed in the source view, reloaded, written by an agent) reaches
//! its block index as line edits, and only the touched blocks parse again.
//!
//! The caret and the selection are source positions. A row maps them to its
//! flow's text through the runs the markdown walk recorded, so a caret stands
//! between visible characters and markers stay hidden.
//!
//! Editing writes the source: a key becomes a plan of source replacements
//! (`makepad_markdown_edit::plan`), applied as this view's own edit in the
//! document's one history, so undo steps the same in both views.

use crate::highlight::{Highlights, RowBlocks, RowHighlight, WholeBlock};
use crate::print::{PrintJob, PrintUpdate};
use crate::rowmap::{caret_src, flat_of, line_around, next_char, next_word, prev_char, prev_word, word_around};
use makepad_code_editor::{
    code_view::find_matches,
    history::EditKind,
    selection::{Cursor, Selection, SelectionSet},
    session::SessionId,
    text::{Change, Edit, Position, Text},
    CodeDocument, TokenColors,
};
use makepad_markdown_edit::{
    plan::{self, BlockStyle, Mark, Plan},
    pulldown_cmark::{CodeBlockKind, CowStr, Event as MdEvent, Tag, TagEnd},
    BlockKind, Index, LineEdit, Row, RowPart, Slice,
};
use makepad_widgets_core::makepad_platform::{
    clipboard_read::{ClipboardContent, ClipboardImage, ClipboardWrite},
    thread::{Class, SignalToUI, TaskHandle},
};
use makepad_widgets_core::text_input::TextMark;
use makepad_widgets_core::*;
use makepad_widgets_rich_text::{
    markdown::{resolve_source, Markdown, MarkdownHost, MarkdownPiece, MarkdownWidgetRefExt},
    text_flow::{CellSelection, TextFlow},
};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    ops::Range,
    path::{Path, PathBuf},
    sync::{
        mpsc::{channel, Receiver},
        Arc,
    },
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.MarkdownEditorBase = #(MarkdownEditor::register_widget(vm))

    mod.widgets.MarkdownEditor = set_type_default() do mod.widgets.MarkdownEditorBase{
        width: Fill
        height: Fill
        flow: Down
        draw_caret +: { color: theme.color_text }
        draw_page_number +: {
            color: #x707070
            text_style: theme.font_regular{font_size: 8}
        }
        list := PortalList{
            width: Fill
            height: Fill
            flow: Down
            grab_key_focus: false
            drag_scrolling: false
            capture_overload: false
            Row := Markdown{
                width: Fill
                height: Fit
                padding: Inset{left: 24 right: 24 top: 0 bottom: 0}
                host_selection: true
                inline_links: true
                body: ""
            }
        }
        // The picture a move drags, riding the pointer: drawn by the view
        // in an overlay, never here.
        chip_box := View{
            visible: false
            width: 0
            height: 0
            chip := Image{width: 96 height: 96}
        }
        // The rows as they print: drawn off screen by a print job, never here.
        print_page := View{
            visible: false
            width: 0
            height: 0
            row := Markdown{
                width: Fill
                height: Fit
                padding: Inset{left: 0 right: 0 top: 0 bottom: 0}
                host_selection: true
                inline_links: true
                body: ""
                font_color: #x1a1a1a
                draw_text +: { color: #x1a1a1a }
                link_color: #x1a4fb8
                draw_block +: {
                    line_color: #x1a1a1a
                    sep_color: #xb8b8b8
                    quote_bg_color: #xf1f1f1
                    quote_fg_color: #x4a4a4a
                    code_color: #xf1f1f1
                    table_header_bg_color: #xe9e9e9
                    table_border_color: #xb8b8b8
                }
            }
        }
    }
}

/// What the final view tells its host.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum MarkdownEditorAction {
    /// The caret or the selection moved.
    Caret,
    /// The view edited the document (the host's other views take it in).
    Changed,
    /// A key the view did not use (Cmd/Ctrl+S and the like), for the host.
    UnhandledKey(KeyEvent),
    /// Cmd/Ctrl+K: the host asks for a URL (starting from the one of the
    /// link the caret is in) and hands it to `set_link`.
    EditLink(Option<String>),
    /// Cmd/Ctrl+click on a link: the host follows its destination.
    OpenLink(String),
    /// Something the person must know about: a pasted picture that could
    /// not be saved, and why.
    Problem(String),
    /// The view wrote a file beside the document (a pasted picture, or a
    /// dropped one copied in): a list of the folder's files shows it now.
    WroteFile(PathBuf),
    /// A double-click on a picture (or Cmd/Ctrl+K with one selected): the
    /// host edits its alt text and source, then calls `set_picture`.
    EditPicture { alt: String, src: String },
    #[default]
    None,
}

/// What the caret stands in, for a format bar to light its keys.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FormatState {
    /// The kind of the innermost block (None while the caret's row is not drawn).
    pub block: Option<BlockStyle>,
    /// The block sits in a quote.
    pub quote: bool,
    /// The marks the next typed text gets: those around the caret, with the
    /// pending ones toggled.
    pub marks: Vec<Mark>,
    /// The caret is in a link.
    pub link: bool,
    /// The caret is in a table.
    pub table: bool,
}

/// What a table command does, at the caret's cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableEdit {
    /// A new table after the caret's block (on a blank line, in its place).
    Insert,
    RowAbove,
    RowBelow,
    DeleteRow,
    ColumnLeft,
    ColumnRight,
    DeleteColumn,
}

/// Where the selection goes after a plan.
#[derive(Clone, Copy, Debug)]
enum After {
    /// A caret at the plan's caret.
    Caret,
    /// A selection, from offsets of the new text.
    Select { anchor: usize, caret: usize },
    /// Where the edits moved the caret and the anchor (an edit away from
    /// the caret, like a task box clicked).
    Keep,
}

/// The document the view shows: its edit broadcast and its block index.
struct Attached {
    document: CodeDocument,
    session: SessionId,
    edits: Receiver<(Option<SelectionSet>, Vec<Edit>)>,
    index: Index,
}

impl Drop for Attached {
    fn drop(&mut self) {
        // The document sends every edit to every session it knows and
        // panics on one whose receiver is gone.
        self.document.remove_session(self.session);
    }
}

/// A picture dragged over the view: where it would go and its file.
#[derive(Clone, Debug, PartialEq)]
struct DropTarget {
    at: Position,
    /// The markdown the ghost shows.
    source: String,
}

/// A picture in the document: its source span and where it reads from.
struct Picture {
    start: Position,
    end: Position,
    alt: String,
    dest: String,
}

/// A picture being dragged to another place in the document.
struct Moving {
    start: Position,
    end: Position,
    /// Its markdown, as it goes in at the drop.
    source: String,
    /// Its file, for the chip on the pointer.
    path: String,
}

/// The `internal_id` of a picture moved within the document.
const PICTURE_MOVE: LiveId = live_id!(markdown_picture_move);

/// How far the pointer moves from a press on a picture before it drags it.
const DRAG_START: f64 = 6.0;

/// The longest side of the chip a moved picture shows on the pointer.
const CHIP: f64 = 96.0;

/// The row a drop would go in, drawn as if the picture were there (its
/// ghost): the copy's parse, and where the picture's source sits in it.
struct GhostRow {
    target: DropTarget,
    row: usize,
    cache: RowCache,
    /// The ghost's source range in the copy's slice.
    ghost: Range<usize>,
}

impl GhostRow {
    fn new(index: &Index, lines: &[String], target: DropTarget) -> Option<Self> {
        let row_id = index.row_at_line(target.at.line_index);
        let row = index.row(row_id);
        if matches!(row.part, RowPart::Code { .. }) || !row.lines.contains(&target.at.line_index) {
            return None;
        }
        let mut copy = lines[row.lines.clone()].to_vec();
        let line = &mut copy[target.at.line_index - row.lines.start];
        let byte = target.at.byte_index.min(line.len());
        if !line.is_char_boundary(byte) {
            return None;
        }
        let source = target.source.clone();
        line.insert_str(byte, &source);
        let mut slice = Slice::new(&copy, 0..copy.len());
        slice.first_line = row.lines.start;
        let start = slice.offset(target.at.line_index, byte);
        let cache = RowCache::of_slice(index, &row, slice);
        Some(Self { target, row: row_id, cache, ghost: start..start + source.len() })
    }

    /// The document position of `flat` in the ghost row's flow: the drop
    /// place itself anywhere on the ghost.
    fn position_at(&self, flat: usize) -> Position {
        let offset = caret_src(&self.cache.pieces, flat, &self.cache.links);
        if offset <= self.ghost.start {
            self.cache.position_of(offset)
        } else if offset < self.ghost.end {
            self.target.at
        } else {
            let mut position = self.cache.position_of(offset);
            if position.line_index == self.target.at.line_index {
                position.byte_index -= self.ghost.len();
            }
            position
        }
    }
}

/// A picture on its way into the document (pasted, or dropped from outside
/// its folder), being written beside the document on a worker.
struct Saving {
    task: TaskHandle<Result<String, String>>,
    /// Where it goes: a drop's place, or (a paste) the caret when it is saved.
    at: Option<Position>,
    alt: String,
}

/// A row's parse, kept while the row is drawn: its block's generation and
/// part name it, so an edit elsewhere keeps it.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
struct RowKey {
    gen: u64,
    part: (u8, usize),
}

impl RowKey {
    fn of(index: &Index, row: &Row) -> Self {
        let part = match row.part {
            RowPart::Whole => (0, 0),
            RowPart::Item { index, .. } => (1, index),
            RowPart::Code { chunk, .. } => (2, chunk),
        };
        Self {
            gen: index.blocks()[row.block].gen,
            part,
        }
    }
}

pub(crate) struct RowCache {
    pub(crate) slice: Slice,
    pub(crate) events: Vec<(MdEvent<'static>, Range<usize>)>,
    pub(crate) list_start: Option<u64>,
    /// The source ranges of the row's links (the caret leaves a link at its end).
    links: Vec<Range<usize>>,
    /// The source ranges of the row's task boxes (a click flips one).
    tasks: Vec<Range<usize>>,
    /// What the flow drew, from the row's last draw.
    pieces: Vec<MarkdownPiece>,
    /// Where the caret stood in the last draw when its line drew no text (a
    /// blank line, an empty item): the walk made room for it there.
    anchor: Option<(usize, Rect)>,
    /// The draw this row was last drawn in.
    drawn_in: u64,
}

impl RowCache {
    pub(crate) fn build(index: &Index, lines: &[String], row: &Row) -> Self {
        let slice = Slice::new(lines, row.lines.clone());
        let block = &index.blocks()[row.block];
        let events = match row.part {
            RowPart::Code { count, .. } if count > 1 => code_events(lines, &slice, block.start, &block.kind, block.code.clone()),
            _ => index
                .parse_slice(&slice)
                .into_iter()
                .map(|(event, range)| (event.into_static(), range))
                .collect(),
        };
        Self::of_events(index, row, slice, events)
    }

    /// Row `row` drawn from `slice`, a copy of its lines with something put
    /// in (a drop's ghost); not for a code chunk.
    fn of_slice(index: &Index, row: &Row, slice: Slice) -> Self {
        let events = index.parse_slice(&slice).into_iter().map(|(event, range)| (event.into_static(), range)).collect();
        Self::of_events(index, row, slice, events)
    }

    fn of_events(index: &Index, row: &Row, slice: Slice, events: Vec<(MdEvent<'static>, Range<usize>)>) -> Self {
        let block = &index.blocks()[row.block];
        let list_start = match (row.part, &block.kind) {
            (RowPart::Item { index, .. }, BlockKind::List { ordered: true, start, .. }) => Some(start + index as u64),
            _ => None,
        };
        let links = events
            .iter()
            .filter(|(event, _)| matches!(event, MdEvent::Start(Tag::Link { .. })))
            .map(|(_, range)| range.clone())
            .collect();
        let tasks = events
            .iter()
            .filter(|(event, _)| matches!(event, MdEvent::TaskListMarker(_)))
            .map(|(_, range)| range.clone())
            .collect();
        Self {
            slice,
            events,
            list_start,
            links,
            tasks,
            pieces: Vec::new(),
            anchor: None,
            drawn_in: 0,
        }
    }

    /// The row's source range as document positions: its first line's start
    /// to its last line's end.
    fn span(&self) -> (Position, Position) {
        let lines = self.slice.line_count();
        let last = self.slice.first_line + lines.saturating_sub(1);
        let end = self.slice.text.len().saturating_sub(1);
        (
            Position { line_index: self.slice.first_line, byte_index: 0 },
            Position { line_index: last, byte_index: end - self.slice.offset(last, 0) },
        )
    }

    fn offset_of(&self, position: Position) -> usize {
        let (start, end) = self.span();
        let position = position.clamp(start, end);
        self.slice.offset(position.line_index, position.byte_index)
    }

    fn position_of(&self, offset: usize) -> Position {
        let offset = offset.min(self.slice.text.len().saturating_sub(1));
        let (line, byte) = self.slice.pos(offset);
        Position { line_index: line, byte_index: byte }
    }

    /// Where the caret stands at flow index `flat`.
    fn caret_at(&self, flat: usize) -> Position {
        self.position_of(caret_src(&self.pieces, flat, &self.links))
    }

    /// How many spaces (and tabs) the source holds between the last text the
    /// flow drew on `line` and `offset`, when nothing else is there.
    fn trailing_spaces(&self, line: usize, offset: usize) -> usize {
        let line_start = self.slice.offset(line, 0);
        let drawn_end = self
            .pieces
            .iter()
            .filter(|piece| piece.src.end >= line_start && piece.src.end <= offset)
            .map(|piece| piece.src.end)
            .max();
        match drawn_end {
            Some(end) if end < offset && self.slice.text[end..offset].bytes().all(|b| b == b' ' || b == b'\t') => offset - end,
            _ => 0,
        }
    }

    /// The width of a space as the row drew one, measured between the caret
    /// places on its two sides.
    fn space_advance(&self, flow: &TextFlow) -> Option<f64> {
        let piece = self.pieces.iter().find(|piece| piece.identity && self.slice.text[piece.src.clone()].contains(' '))?;
        let flat = piece.flat.start + self.slice.text[piece.src.clone()].find(' ')?;
        let (before, after) = (flow.caret_rect(flat, false)?, flow.caret_rect(flat + 1, false)?);
        let advance = after.pos.x - before.pos.x;
        (advance > 0.0 && (after.pos.y - before.pos.y).abs() < 0.5).then_some(advance)
    }

    /// The caret's line has no text the flow draws: only container prefixes
    /// (quote marks, a list or task marker, heading marks) or nothing.
    fn line_is_blank(&self, line: usize) -> bool {
        let start = self.slice.offset(line, 0);
        let end = self.slice.text[start..].find('\n').map_or(self.slice.text.len(), |at| start + at);
        prefix_only(&self.slice.text[start..end])
    }
}

/// A line that holds only block prefixes: quote marks, a list marker (and a
/// task box), or heading marks, with spaces.
fn prefix_only(line: &str) -> bool {
    let mut rest = line.trim_start();
    loop {
        if let Some(after) = rest.strip_prefix('>') {
            rest = after.trim_start();
            continue;
        }
        let digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        let marker = if rest.starts_with(['-', '*', '+']) {
            1
        } else if digits > 0 && rest[digits..].starts_with(['.', ')']) {
            digits + 1
        } else {
            0
        };
        if marker > 0 && rest[marker..].chars().next().map_or(true, char::is_whitespace) {
            rest = rest[marker..].trim_start();
            for task in ["[ ]", "[x]", "[X]"] {
                if let Some(after) = rest.strip_prefix(task) {
                    rest = after.trim_start();
                }
            }
            continue;
        }
        break;
    }
    let hashes = rest.len() - rest.trim_start_matches('#').len();
    if (1..=6).contains(&hashes) && rest[hashes..].chars().next().map_or(true, char::is_whitespace) {
        rest = rest[hashes..].trim_start();
    }
    rest.trim().is_empty()
}

/// The events of a run of a long code block's lines, made here: a run cut
/// from the middle of a block does not parse as code on its own. Each run
/// is a code block of its lines; the fences show nowhere.
/// A block keeps the spacing above it; the next item of a list or run of a
/// code block continues the one above.
pub(crate) fn follows_block(row_id: usize, row: &Row) -> bool {
    row_id > 0
        && match row.part {
            RowPart::Whole => true,
            RowPart::Item { index, .. } => index == 0,
            RowPart::Code { chunk, .. } => chunk == 0,
        }
}

fn code_events(
    lines: &[String],
    slice: &Slice,
    block_start: usize,
    kind: &BlockKind,
    code: Option<Range<u32>>,
) -> Vec<(MdEvent<'static>, Range<usize>)> {
    let code = code.unwrap_or(0..0);
    let content = block_start + code.start as usize..block_start + code.end as usize;
    let first = slice.first_line.max(content.start);
    let last = (slice.first_line + slice.line_count()).min(content.end);
    let tag_kind = match kind {
        BlockKind::CodeFenced => {
            let info = lines[block_start].trim_start().trim_start_matches(['`', '~']).trim();
            let language = info.split_whitespace().next().unwrap_or("");
            CodeBlockKind::Fenced(CowStr::from(language.to_string()))
        }
        _ => CodeBlockKind::Indented,
    };
    let whole = 0..slice.text.len();
    let mut events = vec![(MdEvent::Start(Tag::CodeBlock(tag_kind)), whole.clone())];
    if first < last {
        let from = slice.offset(first, 0);
        let to = slice.offset(last, 0);
        events.push((MdEvent::Text(CowStr::from(slice.text[from..to].to_string())), from..to));
    }
    events.push((MdEvent::End(TagEnd::CodeBlock), whole));
    events
}

/// Where `offset` of a plan's old text lands in its new text: an offset
/// inside a replaced range at the replacement's end; at text inserted right
/// there, after it, or with `before` in front of it.
fn map_offset(plan: &Plan, offset: usize, before: bool) -> usize {
    let mut edits: Vec<&(Range<usize>, String)> = plan.edits.iter().collect();
    edits.sort_by_key(|(range, _)| (range.start, range.end));
    let mut mapped = offset as isize;
    for (range, text) in edits {
        let grows = text.len() as isize - range.len() as isize;
        if range.end < offset || (range.end == offset && !(before && range.is_empty())) {
            mapped += grows;
        } else if range.start < offset {
            mapped += range.start as isize - offset as isize + text.len() as isize;
        }
    }
    mapped.max(0) as usize
}

/// The anchor of the heading whose first line is `line` (`# Title #` or a
/// setext title).
pub(crate) fn heading_anchor(line: &str) -> String {
    slug(line.trim_start().trim_start_matches('#').trim_end().trim_end_matches('#'))
}

/// A heading's anchor: its text in lower case, letters, digits, `-` and
/// `_` kept, spaces as `-`, inline markers and other punctuation dropped.
pub(crate) fn slug(text: &str) -> String {
    text.trim()
        .chars()
        .filter_map(|c| match c {
            ' ' => Some('-'),
            c if c.is_alphanumeric() || c == '-' || c == '_' => Some(c),
            _ => None,
        })
        .flat_map(char::to_lowercase)
        .collect()
}

/// What a press hits besides text.
enum Target {
    /// A task box, at the position of its `[`.
    Task(Position),
    /// A link, to this destination.
    Link(String),
}

/// Where a row drew, for hit tests between draws.
struct DrawnRow {
    row: usize,
    key: RowKey,
    item: WidgetRef,
}

/// Scroll a source line to a place in the view: the row that holds it, at
/// `y` from the view's top.
#[derive(Clone, Copy, Debug)]
struct Place {
    line: usize,
    y: f64,
}

/// Find in the final view: the query's matches in the document's source
/// (the search the source view runs), washed where the view shows them.
#[derive(Default)]
struct Finding {
    query: String,
    /// Typing searches from here: where the selection started when find
    /// opened.
    from: Position,
    matches: Vec<(Position, Position)>,
    current: Option<usize>,
}

impl Finding {
    /// Counts the matches in `lines`, keeping the current one where it was
    /// (or the next after it), else the first after `from`.
    fn count(&mut self, lines: &[String]) {
        let from = self.current.and_then(|current| self.matches.get(current)).map_or(self.from, |m| m.0);
        self.matches = find_matches(lines, &self.query);
        self.current = if self.matches.is_empty() {
            None
        } else {
            Some(self.matches.iter().position(|m| m.0 >= from).unwrap_or(0))
        };
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct MarkdownEditor {
    #[deref]
    view: View,
    #[live]
    draw_caret: DrawColor,
    /// A printed page's number, under its text.
    #[live]
    draw_page_number: DrawText,
    /// The colours of code blocks' tokens: those of the code editor.
    #[live]
    token_colors: TokenColors,
    #[rust]
    doc: Option<Attached>,
    /// Code blocks' tokens, lexed once per version of a block.
    #[rust]
    highlights: Highlights,
    #[rust]
    rows: HashMap<RowKey, RowCache>,
    #[rust]
    drawn: Vec<DrawnRow>,
    #[rust]
    draws: u64,
    #[rust]
    caret: Position,
    #[rust]
    anchor: Position,
    /// At a wrap, the caret stands at the start of the next line.
    #[rust]
    prefer_next_row: bool,
    #[rust]
    selecting: bool,
    #[rust]
    place: Option<Place>,
    /// Where the caret should stand after a placement: the first draw puts
    /// its row there, the next moves the row so the caret itself is.
    #[rust]
    place_caret_y: Option<f64>,
    /// The caret's top from the view's top, as last drawn.
    #[rust]
    caret_y: Option<f64>,
    /// The caret's rect as last drawn, where it is now (for Up and Down).
    #[rust]
    caret_rect: Option<Rect>,
    /// The x Up and Down keep while they move through lines.
    #[rust]
    preferred_x: Option<f64>,
    /// The caret moved to a row not drawn yet: once drawn, it moves to the
    /// nearest place a caret can stand (never inside hidden markers).
    #[rust]
    snap: bool,
    /// The source range the last typed input went into, for input that
    /// replaces it (a dead key's accent, a press-and-hold variant).
    #[rust]
    last_input: Option<(Position, Position)>,
    /// Numbers the history groups an edit made of several steps shares.
    #[rust]
    groups: u64,
    /// A key moved the caret: the next draw scrolls it into view.
    #[rust]
    reveal: bool,
    /// Marks toggled with no selection (Cmd+B and the like): the next text
    /// typed takes them, or loses them.
    #[rust]
    pending: Vec<Mark>,
    /// The link a Cmd/Ctrl+click would follow, underlined.
    #[rust]
    hovered_link: Option<String>,
    /// The pages being made for print or export.
    #[rust]
    print: Option<PrintJob>,
    /// The document's file: its folder holds the document's relative images.
    #[rust]
    file: Option<PathBuf>,
    /// The clipboard reads pastes asked for, in order, each with the text
    /// its paste came with: a read's Markdown flavour pastes in its place,
    /// and a picture alone on the clipboard is saved beside the document.
    #[rust]
    paste_reads: Vec<(u64, Option<String>)>,
    /// A paste's text came as input and its read is on the way: the
    /// Cmd/Ctrl+V key behind it asks for no second one.
    #[rust]
    paste_by_text: bool,
    /// Pictures being written beside the document, then linked.
    #[rust]
    saving: Vec<Saving>,
    /// A picture dragged over the view, and the row drawn with its ghost.
    #[rust]
    drop_target: Option<DropTarget>,
    #[rust]
    ghost_row: Option<GhostRow>,
    /// A press on a picture, and where: a move starts once the pointer has
    /// gone `DRAG_START` from there.
    #[rust]
    press_picture: Option<(Picture, DVec2)>,
    #[rust]
    moving: Option<Moving>,
    /// Where the pointer is while a picture moves: the chip rides it.
    #[rust]
    chip_at: Option<DVec2>,
    #[live]
    chip_list: DrawList2d,
    /// A wash (the ghost, found text) was due in a row drawn before its
    /// pieces were known: one more draw washes it.
    #[rust]
    rewash: bool,
    /// Find, while it is open.
    #[rust]
    find: Option<Finding>,
    /// The caret a find moved scrolls in to the view's upper middle, not
    /// to the edge it left by.
    #[rust]
    reveal_middle: bool,
    /// The start of the picture whose alt text and source the host edits.
    #[rust]
    editing_picture: Option<Position>,
}

impl MarkdownEditor {
    /// Shows `document`, from its start. The view joins the document's edit
    /// broadcast until another document replaces it or `detach`.
    pub fn attach(&mut self, cx: &mut Cx, mut document: CodeDocument) {
        self.doc = None;
        self.rows.clear();
        self.highlights.clear();
        let session = SessionId::new();
        let (sender, edits) = channel();
        document.add_session(session, sender);
        let index = Index::new(document.as_text().as_lines());
        self.doc = Some(Attached { document, session, edits, index });
        self.caret = Position::default();
        self.anchor = Position::default();
        self.place = None;
        self.view.redraw(cx);
    }

    pub fn detach(&mut self, cx: &mut Cx) {
        self.doc = None;
        self.rows.clear();
        self.highlights.clear();
        self.view.redraw(cx);
    }

    /// Starts making the document's pages as they read on paper, into a
    /// PDF, from a copy of the document as it is now. The host drives the
    /// job whether the view shows or not: `draw_print` in its draw and
    /// `print_event` with its events, which answers the progress and the
    /// PDF. The host redraws to start it. False with no document or a job
    /// already running.
    pub fn print(&mut self, cx: &mut Cx, title: &str) -> bool {
        self.sync(cx);
        if self.print.is_some() {
            return false;
        }
        let Some(doc) = self.doc.as_ref() else { return false };
        let lines = doc.document.as_text().as_lines().to_vec();
        let base_dir = self.file.as_deref().and_then(Path::parent).map(Path::to_path_buf);
        self.print = Some(PrintJob::new(cx, title, doc.index.clone(), lines, base_dir));
        true
    }

    /// The document's file: relative image sources are read from its
    /// folder (`None`: from the working directory).
    pub fn set_file(&mut self, cx: &mut Cx, file: Option<&Path>) {
        if self.file.as_deref() != file {
            self.file = file.map(Path::to_path_buf);
            self.view.redraw(cx);
        }
    }

    /// The title of the job running, if one is.
    pub fn printing(&self) -> Option<&str> {
        self.print.as_ref().map(|job| job.title())
    }

    pub fn cancel_print(&mut self) {
        self.print = None;
    }

    /// The print job's draw (see `print`).
    pub fn draw_print(&mut self, cx: &mut Cx2d) {
        let Some(job) = self.print.as_mut() else { return };
        let row = self.view.widget(cx, ids!(print_page.row));
        let Some(mut md) = row.borrow_mut::<Markdown>() else { return };
        md.set_base_dir(cx, self.file.as_deref().and_then(Path::parent));
        job.draw(cx, &mut md, &mut self.draw_page_number);
    }

    /// The print job's events (see `print`): where it stands, when that changed.
    pub fn print_event(&mut self, cx: &mut Cx, event: &Event) -> Option<PrintUpdate> {
        let update = self.print.as_mut()?.handle_event(cx, event)?;
        if matches!(update, PrintUpdate::Done(_) | PrintUpdate::Failed(_)) {
            self.print = None;
        }
        Some(update)
    }

    pub fn is_attached(&self) -> bool {
        self.doc.is_some()
    }

    /// Where the caret is, in the source.
    pub fn caret(&self) -> Position {
        self.caret
    }

    /// The selection, anchor first.
    pub fn selection(&self) -> (Position, Position) {
        (self.anchor, self.caret)
    }

    /// Puts the caret (and an empty selection) at `caret`, and scrolls its
    /// line to `view_y` from the view's top when given.
    pub fn set_caret(&mut self, cx: &mut Cx, caret: Position, view_y: Option<f64>) {
        let caret = self.clamp(caret);
        self.caret = caret;
        self.anchor = caret;
        self.prefer_next_row = false;
        if let Some(y) = view_y {
            self.place = Some(Place { line: caret.line_index, y });
        }
        self.view.redraw(cx);
    }

    /// The caret's top from the view's top as last drawn (None when its row
    /// was not drawn).
    pub fn caret_view_y(&self) -> Option<f64> {
        self.caret_y
    }

    pub fn set_key_focus(&self, cx: &mut Cx) {
        cx.set_key_focus(self.view.area());
    }

    fn clamp(&self, position: Position) -> Position {
        let Some(doc) = &self.doc else { return Position::default() };
        let text = doc.document.as_text();
        let lines = text.as_lines();
        let line = position.line_index.min(lines.len().saturating_sub(1));
        let len = lines.get(line).map_or(0, String::len);
        let mut byte = position.byte_index.min(len);
        while byte > 0 && !lines[line].is_char_boundary(byte) {
            byte -= 1;
        }
        Position { line_index: line, byte_index: byte }
    }

    /// Takes the edits the document sent since the last look, for a host
    /// whose source view edits while this view is hidden: hidden, the view
    /// gets no events of its own, and its index would fall behind.
    pub fn follow_edits(&mut self, cx: &mut Cx) {
        self.sync(cx);
    }

    /// Takes the edits the document sent since the last look: the index
    /// follows them, the list moves its rows, the caret moves with the text,
    /// and an undo or redo of this view's edit puts back its selection.
    fn sync(&mut self, cx: &mut Cx) {
        let Some(doc) = self.doc.as_mut() else { return };
        let mut line_edits = Vec::new();
        while let Ok((restored, edits)) = doc.edits.try_recv() {
            if !edits.is_empty() {
                self.last_input = None;
            }
            for edit in &edits {
                self.caret = self.caret.apply_edit(edit);
                self.anchor = self.anchor.apply_edit(edit);
                line_edits.push(match &edit.change {
                    Change::Insert(at, text) => LineEdit {
                        line: at.line_index,
                        removed: 0,
                        added: text.length().line_count,
                    },
                    Change::Delete(at, length) => LineEdit {
                        line: at.line_index,
                        removed: length.line_count,
                        added: 0,
                    },
                });
            }
            if let Some(selection) = restored.as_ref().and_then(|set| set.first().copied()) {
                self.caret = selection.cursor.position;
                self.anchor = selection.anchor;
                self.snap = true;
            }
        }
        if line_edits.is_empty() {
            return;
        }
        let splice = {
            let text = doc.document.as_text();
            doc.index.apply(text.as_lines(), &line_edits)
        };
        // A row the edit did not reparse keeps its cache, and the lines
        // added or removed above it move it.
        for edit in &line_edits {
            let delta = edit.added as isize - edit.removed as isize;
            if delta != 0 {
                for cache in self.rows.values_mut() {
                    if cache.slice.first_line > edit.line {
                        cache.slice.first_line = cache.slice.first_line.saturating_add_signed(delta);
                    }
                }
            }
        }
        if let Some(splice) = &splice {
            // The rows drawn after the edit renumber; the replaced ones are
            // gone until the next draw.
            let moved = splice.new_rows.len() as isize - splice.old_rows.len() as isize;
            self.drawn.retain_mut(|drawn| {
                if drawn.row >= splice.old_rows.end {
                    drawn.row = drawn.row.saturating_add_signed(moved);
                    true
                } else {
                    drawn.row < splice.old_rows.start
                }
            });
        }
        if let Some(splice) = splice {
            let list = self.view.portal_list(cx, ids!(list));
            list.splice_items(
                cx,
                splice.old_rows.start,
                splice.old_rows.len(),
                splice.new_rows.len(),
            );
            if splice.globals_changed {
                // Rows resolve links and footnote numbers against the tables.
                self.rows.clear();
            }
        }
        self.caret = self.clamp(self.caret);
        self.anchor = self.clamp(self.anchor);
        if let (Some(find), Some(doc)) = (self.find.as_mut(), self.doc.as_ref()) {
            find.count(doc.document.as_text().as_lines());
        }
        self.view.redraw(cx);
    }

    fn ordered_selection(&self) -> (Position, Position) {
        (self.anchor.min(self.caret), self.anchor.max(self.caret))
    }

    fn draw_rows(&mut self, cx: &mut Cx2d, list: &mut PortalList) {
        let Some(doc) = self.doc.as_ref() else {
            list.set_item_range(cx, 0, 0);
            return;
        };
        let index = &doc.index;
        let text = doc.document.as_text();
        let lines = text.as_lines();
        let row_count = index.row_count();
        list.set_item_range(cx, 0, row_count);
        if let Some(place) = self.place {
            if row_count > 0 {
                let row = index.row_at_line(place.line.min(index.line_count().saturating_sub(1)));
                list.set_first_id_and_scroll(row, place.y);
            }
        }
        let (sel_start, sel_end) = self.ordered_selection();
        let footnote = |label: &str| index.footnote_number(label);
        // A picture dragged over the view: its row draws from a copy with
        // the picture in it.
        match &self.drop_target {
            Some(target) if self.ghost_row.as_ref().map_or(true, |ghost| ghost.target != *target) => {
                self.ghost_row = GhostRow::new(index, lines, target.clone());
            }
            Some(_) => {}
            None => self.ghost_row = None,
        }
        while let Some(row_id) = list.next_visible_item(cx) {
            if row_id >= row_count {
                continue;
            }
            let row = index.row(row_id);
            let key = RowKey::of(index, &row);
            let ghost = self.ghost_row.as_mut().filter(|ghost| ghost.row == row_id);
            let ghost_range = ghost.as_ref().map(|ghost| ghost.ghost.clone());
            let cache = match ghost {
                Some(ghost) => &mut ghost.cache,
                None => self.rows.entry(key).or_insert_with(|| RowCache::build(index, lines, &row)),
            };
            cache.drawn_in = self.draws;
            let item = list.item(cx, row_id, id!(Row));
            let walk = item.walk(cx);
            let Some(mut md) = item.borrow_mut::<Markdown>() else { continue };
            md.set_base_dir(cx, self.file.as_deref().and_then(Path::parent));
            // The selection in the flow's text, from what this row drew last
            // time (a row drawn for the first time shows it from the next draw).
            let (start, end) = cache.span();
            let (a, b) = if let Some(ghost) = &ghost_range {
                // The ghost shows washed, as a selected picture does.
                if cache.pieces.is_empty() {
                    // its pieces come with this draw: wash it in the next
                    self.rewash = true;
                    (0, 0)
                } else {
                    (flat_of(&cache.pieces, ghost.start), flat_of(&cache.pieces, ghost.end))
                }
            } else if sel_start < sel_end && sel_start <= end && sel_end >= start && !cache.pieces.is_empty() {
                let a = if sel_start <= start { 0 } else { flat_of(&cache.pieces, cache.offset_of(sel_start)) };
                let b = if sel_end >= end { usize::MAX } else { flat_of(&cache.pieces, cache.offset_of(sel_end)) };
                (a, b)
            } else {
                (0, 0)
            };
            // A selection from one cell of a table to another shows whole
            // cells, not the text between.
            let within = ghost_range.is_none() && sel_start < sel_end && sel_start >= start && sel_end <= end;
            let cells = if within { cell_selection(cache, self.anchor, self.caret) } else { None };
            let (a, b) = if cells.is_some() { (0, 0) } else { (a, b) };
            md.text_flow.set_selection(a, b);
            md.text_flow.set_cell_selection(cells);
            let found = match &self.find {
                Some(find) => found_in_row(&find.matches, cache, start, end).unwrap_or_else(|| {
                    // its pieces come with this draw: wash them in the next
                    self.rewash = true;
                    Vec::new()
                }),
                None => Vec::new(),
            };
            // Marks redraw the flow when set: set only when they change.
            if md.text_flow.marks() != found.as_slice() {
                md.text_flow.set_marks(cx, found);
            }
            let follows_block = follows_block(row_id, &row);
            // A caret on a line that draws no text gets a place made for it.
            let caret_line = self.caret.line_index;
            let anchors = if ghost_range.is_none() && caret_line >= start.line_index && caret_line <= end.line_index && cache.line_is_blank(caret_line) {
                vec![(cache.offset_of(self.caret), None)]
            } else {
                Vec::new()
            };
            // Code blocks take the code editor's colours; a long one split in
            // rows is lexed whole.
            let whole = match row.part {
                RowPart::Code { count, .. } if count > 1 => {
                    let block = &index.blocks()[row.block];
                    let code = block.code.clone().unwrap_or(0..0);
                    let content = block.start + code.start as usize..block.start + code.end as usize;
                    let first = cache.slice.first_line.max(content.start) - content.start;
                    Some(WholeBlock { lines: &lines[content], first })
                }
                _ => None,
            };
            let highlight = RowHighlight {
                blocks: RowBlocks {
                    highlights: RefCell::new(&mut self.highlights),
                    gen: key.gen,
                    part: key.part,
                    whole,
                    calls: Cell::new(0),
                },
                colors: &self.token_colors,
            };
            let mut host = MarkdownHost {
                footnote_number: Some(&footnote),
                list_start: cache.list_start,
                follows_block,
                anchors,
                highlight: Some(&highlight),
                ..Default::default()
            };
            md.draw_events(cx, walk, &cache.slice.text, cache.events.iter().cloned(), &mut host);
            cache.pieces = host.pieces;
            cache.anchor = host.anchors.first().and_then(|(at, rect)| rect.map(|rect| (*at, rect)));
            drop(md);
            self.drawn.push(DrawnRow { row: row_id, key, item: item.clone() });
        }
    }

    /// The caret over the rows, in the view's own pass: the rows do not draw
    /// again when it moves.
    fn draw_caret(&mut self, cx: &mut Cx2d) {
        self.caret_y = None;
        let Some(doc) = self.doc.as_ref() else { return };
        if doc.index.row_count() == 0 {
            return;
        }
        let row = doc.index.row_at_line(self.caret.line_index.min(doc.index.line_count().saturating_sub(1)));
        let Some(drawn) = self.drawn.iter().find(|drawn| drawn.row == row) else { return };
        let Some(cache) = self.rows.get(&drawn.key) else { return };
        let Some(md) = drawn.item.borrow::<Markdown>() else { return };
        let offset = cache.offset_of(self.caret);
        let shift = md.text_flow.drawn_shift(cx);
        let drawn_rect = match cache.anchor {
            Some((at, rect)) if at == offset => Some(rect),
            _ => md.text_flow.caret_rect(flat_of(&cache.pieces, offset), self.prefer_next_row),
        };
        let mut rect = match drawn_rect {
            Some(rect) => Rect { pos: rect.pos + shift, size: rect.size },
            None => {
                let area = drawn.item.area().rect(cx);
                Rect { pos: area.pos, size: dvec2(0.0, 16.0) }
            }
        };
        // Spaces typed at a line's end, which the parse drops until text
        // follows them: the caret stands after them all the same.
        let trailing = cache.trailing_spaces(self.caret.line_index, offset);
        if trailing > 0 {
            rect.pos.x += trailing as f64 * cache.space_advance(&md.text_flow).unwrap_or(rect.size.y * 0.25);
        }
        drop(md);
        let view = self.view.area().rect(cx);
        self.caret_y = Some(rect.pos.y - view.pos.y);
        self.caret_rect = Some(rect);
        if rect.pos.y + rect.size.y < view.pos.y || rect.pos.y > view.pos.y + view.size.y {
            return;
        }
        if cx.has_key_focus(self.view.area()) {
            cx.show_text_ime(self.view.area(), rect.pos - view.pos + dvec2(0.0, rect.size.y));
            if self.anchor == self.caret {
                self.draw_caret.draw_abs(cx, Rect { pos: rect.pos, size: dvec2(1.5, rect.size.y) });
            }
        }
    }

    /// After a draw: a caret that moved into a row drawn for the first time
    /// steps out of hidden source to where a caret stands.
    fn snap_caret(&mut self) {
        if !std::mem::take(&mut self.snap) {
            return;
        }
        let Some((drawn, cache)) = self.caret_row() else { return };
        if cache.line_is_blank(self.caret.line_index) || cache.pieces.is_empty() {
            return;
        }
        let _ = drawn;
        let snapped = cache.caret_at(flat_of(&cache.pieces, cache.offset_of(self.caret)));
        if self.anchor == self.caret {
            self.anchor = snapped;
        }
        self.caret = snapped;
    }

    /// After a draw: a caret a key moved out of view scrolls back in, to
    /// the edge it left by.
    fn reveal_caret(&mut self, cx: &mut Cx2d) {
        if !std::mem::take(&mut self.reveal) {
            return;
        }
        let view = self.view.area().rect(cx);
        let height = self.caret_rect.map_or(16.0, |rect| rect.size.y);
        let middle = std::mem::take(&mut self.reveal_middle).then(|| ((view.size.y - height) * 0.4).max(0.0));
        let target = match self.caret_y {
            Some(y) if y >= 0.0 && y + height <= view.size.y => None,
            _ if middle.is_some() => middle,
            Some(y) if y < 0.0 => Some(0.0),
            Some(_) => Some((view.size.y - height).max(0.0)),
            None => {
                let below = self
                    .doc
                    .as_ref()
                    .map(|doc| doc.index.row_at_line(self.caret.line_index.min(doc.index.line_count().saturating_sub(1))))
                    .is_some_and(|row| self.drawn.iter().all(|drawn| drawn.row < row));
                Some(if below { (view.size.y - height).max(0.0) } else { 0.0 })
            }
        };
        if let Some(y) = target {
            self.place = Some(Place { line: self.caret.line_index, y });
            self.view.redraw(cx);
        }
    }

    /// The source position under `abs`: the drawn row nearest to it, and the
    /// character of its flow there.
    fn position_at(&self, cx: &Cx, abs: DVec2) -> Option<Position> {
        let drawn = self.nearest_row(cx, abs)?;
        let cache = self.rows.get(&drawn.key)?;
        let md = drawn.item.borrow::<Markdown>()?;
        let flat = md.text_flow.selection_point_to_char_index(cx, abs).unwrap_or(0);
        Some(cache.caret_at(flat))
    }

    /// The drawn row nearest `abs` up or down.
    fn nearest_row(&self, cx: &Cx, abs: DVec2) -> Option<&DrawnRow> {
        let mut best: Option<(&DrawnRow, f64)> = None;
        for drawn in &self.drawn {
            let rect = drawn.item.area().rect(cx);
            let dist = if abs.y < rect.pos.y {
                rect.pos.y - abs.y
            } else if abs.y > rect.pos.y + rect.size.y {
                abs.y - rect.pos.y - rect.size.y
            } else {
                0.0
            };
            if best.map_or(true, |(_, d)| dist < d) {
                best = Some((drawn, dist));
            }
        }
        best.map(|(drawn, _)| drawn)
    }

    /// The drawn row under `abs`.
    fn row_at(&self, cx: &Cx, abs: DVec2) -> Option<usize> {
        self.drawn.iter().position(|drawn| drawn.item.area().rect(cx).contains(abs))
    }

    /// The flow index of the character under `abs` in drawn row `at`: the
    /// one left of the nearest boundary when the pointer is left of it.
    fn char_at(&self, cx: &Cx, at: usize, abs: DVec2) -> Option<usize> {
        let md = self.drawn[at].item.borrow::<Markdown>()?;
        let mut flat = md.text_flow.selection_point_to_char_index(cx, abs)?;
        if let Some(rect) = md.text_flow.caret_rect(flat, false) {
            if flat > 0 && abs.x < rect.pos.x + md.text_flow.drawn_shift(cx).x {
                flat = prev_char(md.text_flow.tracked_text(), flat);
            }
        }
        Some(flat)
    }

    /// A double click selects the word under the pointer, a triple click
    /// its line of text; the ends map to the source as the caret does.
    fn select_around(&mut self, cx: &mut Cx, abs: DVec2, line: bool) -> bool {
        let Some(at) = self.row_at(cx, abs) else { return false };
        let Some(flat) = self.char_at(cx, at, abs) else { return false };
        let drawn = &self.drawn[at];
        let Some(cache) = self.rows.get(&drawn.key) else { return false };
        let Some(md) = drawn.item.borrow::<Markdown>() else { return false };
        let text = md.text_flow.tracked_text();
        let range = if line { line_around(text, flat) } else { word_around(text, flat) };
        if range.is_empty() {
            return false;
        }
        let (anchor, caret) = (cache.caret_at(range.start), cache.caret_at(range.end));
        drop(md);
        self.anchor = anchor;
        self.caret = caret;
        self.prefer_next_row = false;
        self.preferred_x = None;
        self.last_input = None;
        if let Some(doc) = &self.doc {
            doc.document.force_new_group();
        }
        self.moved(cx);
        true
    }

    /// The picture drawn under `abs`, if a press there lands on one: its
    /// whole `![alt](src)` and its destination.
    fn object_at(&self, cx: &Cx, abs: DVec2) -> Option<Picture> {
        let at = self.row_at(cx, abs)?;
        let drawn = &self.drawn[at];
        let cache = self.rows.get(&drawn.key)?;
        let md = drawn.item.borrow::<Markdown>()?;
        let flat = md.text_flow.object_at(cx, abs)?;
        let end = flat + '\u{FFFC}'.len_utf8();
        let piece = cache.pieces.iter().find(|piece| !piece.identity && piece.flat == (flat..end))?;
        let (alt, dest) = plan::picture_at(&cache.events, piece.src.start)?;
        Some(Picture { start: cache.position_of(piece.src.start), end: cache.position_of(piece.src.end), alt, dest })
    }

    /// What a press at `abs` would hit besides text: a task box (the
    /// position of its `[`) or a link (its destination).
    fn target_at(&self, cx: &Cx, abs: DVec2) -> Option<Target> {
        let at = self.row_at(cx, abs)?;
        let cache = self.rows.get(&self.drawn[at].key)?;
        if cache.tasks.is_empty() && cache.links.is_empty() {
            return None;
        }
        let flat = self.char_at(cx, at, abs)?;
        let piece = cache.pieces.iter().find(|piece| piece.flat.start <= flat && flat < piece.flat.end)?;
        let offset = if piece.identity { piece.src.start + (flat - piece.flat.start) } else { piece.src.start };
        if let Some(task) = cache.tasks.iter().find(|task| task.contains(&offset)) {
            return Some(Target::Task(cache.position_of(task.start)));
        }
        if cache.links.iter().any(|link| link.contains(&offset)) {
            return plan::link_at(&cache.slice.text, &cache.events, offset).map(Target::Link);
        }
        None
    }

    /// Underlines the link a Cmd/Ctrl+click would follow.
    fn hover_link(&mut self, cx: &mut Cx, url: Option<String>) {
        if self.hovered_link == url {
            return;
        }
        self.hovered_link = url;
        for drawn in &self.drawn {
            drawn.item.as_markdown().set_hovered_link(cx, self.hovered_link.as_deref());
        }
    }

    /// The selection as markdown, repaired at its ends (markers open at the
    /// start and still open at the end closed), as the cut rules take it.
    /// The selected source when it lies on one line: what a find opened
    /// now looks for.
    pub fn find_seed(&self) -> Option<String> {
        let (start, end) = self.ordered_selection();
        if start.line_index != end.line_index || start == end {
            return None;
        }
        let doc = self.doc.as_ref()?;
        let text = doc.document.as_text();
        text.as_lines().get(start.line_index)?.get(start.byte_index..end.byte_index).map(str::to_string)
    }

    /// Finds `query` in the document: every match washed, the first at or
    /// after where the selection started when find opened selected and
    /// scrolled to. Typing on keeps searching from there.
    pub fn set_find(&mut self, cx: &mut Cx, query: &str) {
        let Some(doc) = &self.doc else { return };
        let from = self.ordered_selection().0;
        let find = self.find.get_or_insert_with(|| Finding { from, ..Default::default() });
        find.query = query.to_string();
        find.current = None;
        find.count(doc.document.as_text().as_lines());
        self.select_found(cx);
    }

    /// Steps to the next (+1) or previous (-1) match, wrapping.
    pub fn find_step(&mut self, cx: &mut Cx, step: isize) {
        let Some(find) = self.find.as_mut() else { return };
        let count = find.matches.len();
        if count == 0 {
            return;
        }
        find.current = Some(match find.current {
            Some(current) => (current as isize + step).rem_euclid(count as isize) as usize,
            None => 0,
        });
        self.select_found(cx);
    }

    /// Ends find: the washes go, the last match stays selected.
    pub fn close_find(&mut self, cx: &mut Cx) {
        if self.find.take().is_some() {
            self.view.redraw(cx);
        }
    }

    /// While find is open: the current match (from 0) and how many there are.
    pub fn find_count(&self) -> Option<(Option<usize>, usize)> {
        self.find.as_ref().filter(|find| !find.query.is_empty()).map(|find| (find.current, find.matches.len()))
    }

    /// Selects the current match and scrolls it in when it is out of view.
    fn select_found(&mut self, cx: &mut Cx) {
        let Some((start, end)) = self.find.as_ref().and_then(|find| find.matches.get(find.current?)).copied() else {
            self.view.redraw(cx);
            return;
        };
        self.anchor = start;
        self.caret = end;
        self.prefer_next_row = false;
        self.preferred_x = None;
        self.reveal = true;
        self.reveal_middle = true;
        self.view.redraw(cx);
        cx.widget_action(self.widget_uid(), MarkdownEditorAction::Caret);
    }

    /// What a copy of `markdown` (the selection's source) puts on the
    /// clipboard: the text as it reads (no markers), the Markdown as its
    /// Markdown flavour, which a paste back here takes, and its HTML. A
    /// selection that reads as nothing (a picture without a description)
    /// copies its Markdown as the text.
    fn copied(&self, markdown: String) -> Option<ClipboardWrite> {
        if markdown.is_empty() {
            return None;
        }
        let doc = self.doc.as_ref()?;
        let flavours = doc.index.flavours(markdown);
        let text = if flavours.text.is_empty() { flavours.markdown.clone() } else { flavours.text };
        Some(ClipboardWrite { text, html: Some(flavours.html), markdown: Some(flavours.markdown) })
    }

    /// Pastes what a read found: the Markdown flavour as it is, else the
    /// text the paste came with (or the read's), else a picture.
    fn paste(&mut self, cx: &mut Cx, content: &ClipboardContent, text: Option<String>) {
        let markdown = content.markdown.as_deref().map(typed_text).filter(|m| !m.is_empty());
        let text = text.or_else(|| content.text.as_deref().map(typed_text)).filter(|t| !t.is_empty());
        if let Some(typed) = markdown.or(text) {
            self.type_text(cx, &typed, true);
        } else if let Some(image) = &content.image {
            self.save_picture(cx, image.clone());
        }
    }

    fn selected_text(&self) -> String {
        let (start, end) = self.ordered_selection();
        if start == end {
            return String::new();
        }
        let Some((slice, events)) = self.blocks_around(start.line_index, end.line_index, 0, 0).and_then(|lines| self.parse(lines)) else {
            return String::new();
        };
        let (anchor, caret) = (Self::slice_offset(&slice, self.anchor), Self::slice_offset(&slice, self.caret));
        if let Some(block) = plan::cell_block(&slice.text, &events, anchor, caret) {
            return plan::block_text(&slice.text, &events, &block).unwrap_or_default();
        }
        let range = Self::slice_offset(&slice, start)..Self::slice_offset(&slice, end);
        plan::cut(&slice.text, &events, range).1
    }

    fn select_all(&mut self, cx: &mut Cx) {
        let Some(doc) = &self.doc else { return };
        let text = doc.document.as_text();
        let lines = text.as_lines();
        let last = lines.len().saturating_sub(1);
        self.anchor = Position::default();
        self.caret = Position { line_index: last, byte_index: lines.get(last).map_or(0, String::len) };
        self.view.redraw(cx);
    }

    fn moved(&mut self, cx: &mut Cx) {
        self.view.redraw(cx);
        cx.widget_action(self.widget_uid(), MarkdownEditorAction::Caret);
    }

    // Editing.

    /// The selection as the history records it, in source positions.
    fn history_selection(&self) -> SelectionSet {
        let mut set = SelectionSet::new();
        set.set_selection(Selection {
            cursor: Cursor { position: self.caret, ..Default::default() },
            anchor: self.anchor,
        });
        set
    }

    /// The lines of the top-level blocks from the one holding `first` to the
    /// one holding `last`, with `before` blocks before and `after` after them.
    fn blocks_around(&self, first: usize, last: usize, before: usize, after: usize) -> Option<Range<usize>> {
        let index = &self.doc.as_ref()?.index;
        let blocks = index.blocks().len();
        if blocks == 0 {
            return None;
        }
        let at = |line: usize| index.block_at_line(line.min(index.line_count().saturating_sub(1)));
        let from = at(first).saturating_sub(before);
        let to = (at(last) + after).min(blocks - 1);
        Some(index.block_lines(from).start..index.block_lines(to).end)
    }

    /// A slice of the document's lines and its parse as the index sees it.
    fn parse(&self, lines: Range<usize>) -> Option<(Slice, Vec<(MdEvent<'static>, Range<usize>)>)> {
        let doc = self.doc.as_ref()?;
        let text = doc.document.as_text();
        let slice = Slice::new(text.as_lines(), lines);
        let events = doc.index.parse_slice(&slice).into_iter().map(|(event, range)| (event.into_static(), range)).collect();
        Some((slice, events))
    }

    fn slice_offset(slice: &Slice, position: Position) -> usize {
        let last = slice.first_line + slice.line_count().saturating_sub(1);
        let line = position.line_index.clamp(slice.first_line, last);
        let byte = if line == position.line_index { position.byte_index } else { 0 };
        let line_len = slice.offset(line + 1, 0) - slice.offset(line, 0) - 1;
        slice.offset(line, byte.min(line_len))
    }

    /// The document position of `offset` in `slice`. The `\n` a slice adds
    /// after the document's last line maps to the document's end.
    fn doc_position(&self, slice: &Slice, offset: usize) -> Position {
        let (line, byte) = slice.pos(offset.min(slice.text.len()));
        let Some(doc) = &self.doc else { return Position::default() };
        let text = doc.document.as_text();
        let lines = text.as_lines();
        let last = lines.len().saturating_sub(1);
        if line > last {
            return Position { line_index: last, byte_index: lines.get(last).map_or(0, String::len) };
        }
        Position { line_index: line, byte_index: byte.min(lines[line].len()) }
    }

    /// Applies `plan`, made over `slice`, as this view's edit of `kind`: one
    /// step in the document's history that remembers the selection before it.
    fn apply_plan(&mut self, cx: &mut Cx, slice: &Slice, plan: &Plan, kind: EditKind, after: After) {
        let new_text = plan.apply(&slice.text);
        let position = |offset: usize| {
            let head = &new_text[..offset.min(new_text.len())];
            Position {
                line_index: slice.first_line + head.matches('\n').count(),
                byte_index: head.len() - head.rfind('\n').map_or(0, |at| at + 1),
            }
        };
        let (caret, anchor) = match after {
            After::Select { anchor, caret } => (position(caret), position(anchor)),
            After::Caret | After::Keep => (position(plan.caret), position(plan.caret)),
        };
        if !plan.edits.is_empty() {
            let ranges: Vec<(Position, Position, Text)> = plan
                .edits
                .iter()
                .map(|(range, text)| {
                    (self.doc_position(slice, range.start), self.doc_position(slice, range.end), Text::from(text.as_str()))
                })
                .collect();
            let Some(doc) = &self.doc else { return };
            let before = self.history_selection();
            doc.document.apply_ranges(doc.session, kind, &before, &ranges);
            self.sync(cx);
            cx.widget_action(self.widget_uid(), MarkdownEditorAction::Changed);
        }
        if !matches!(after, After::Keep) {
            self.caret = self.clamp(caret);
            self.anchor = self.clamp(anchor);
        }
        self.preferred_x = None;
        self.prefer_next_row = false;
        self.reveal = true;
        self.moved(cx);
    }

    /// A history group of its own for an edit made in steps (a selection
    /// replaced by typing undoes in one step).
    fn new_group(&mut self) -> EditKind {
        self.groups += 1;
        EditKind::Group(self.groups)
    }

    /// Removes the selection by the cut rules (markers kept balanced, tables
    /// and code kept whole): the markdown it removed and the history group
    /// it started. None without a selection. Whole cells of a table are
    /// emptied, or with `cells_go` removed where they are whole rows or
    /// columns (a Delete, a cut), and the caret goes to the first.
    fn delete_selection(&mut self, cx: &mut Cx, cells_go: bool) -> Option<(EditKind, String)> {
        let (start, end) = self.ordered_selection();
        if start == end {
            return None;
        }
        let lines = self.blocks_around(start.line_index, end.line_index, 0, 0)?;
        let (slice, events) = self.parse(lines)?;
        let (anchor, caret) = (Self::slice_offset(&slice, self.anchor), Self::slice_offset(&slice, self.caret));
        if let Some(block) = plan::cell_block(&slice.text, &events, anchor, caret) {
            let fragment = plan::block_text(&slice.text, &events, &block).unwrap_or_default();
            let step = if cells_go { plan::delete_block(&slice.text, &events, &block) } else { plan::clear_block(&slice.text, &events, &block) }?;
            let kind = self.new_group();
            self.apply_step(cx, &slice, step, kind);
            return Some((kind, fragment));
        }
        let range = Self::slice_offset(&slice, start)..Self::slice_offset(&slice, end);
        let (plan, fragment) = plan::cut(&slice.text, &events, range);
        let kind = self.new_group();
        self.apply_plan(cx, &slice, &plan, kind, After::Caret);
        Some((kind, fragment))
    }

    /// Types `typed` at the caret, over the selection. Pasted text keeps the
    /// caret's container: each line after the first takes its prefix.
    fn type_text(&mut self, cx: &mut Cx, typed: &str, was_paste: bool) {
        // Marks toggled with no selection take the first text typed after
        // them that can carry a mark (not spaces, not a new line).
        let pending = std::mem::take(&mut self.pending);
        let wrap = !pending.is_empty() && !typed.contains('\n') && !typed.trim().is_empty();
        if !wrap {
            self.pending = pending.clone();
        }
        let kind = match self.delete_selection(cx, false) {
            Some((group, _)) => group,
            None if wrap => self.new_group(),
            None if was_paste => EditKind::Other,
            None if typed == " " => EditKind::InsertSpace,
            None => EditKind::Insert,
        };
        let start = self.caret;
        // A blank line takes the blocks on both sides along: text typed
        // there must not continue either one.
        let blank_line = self.doc.as_ref().is_some_and(|doc| {
            doc.document.as_text().as_lines().get(start.line_index).map_or(true, |line| line.trim().is_empty())
        });
        let around = usize::from(blank_line);
        let Some(lines) = self.blocks_around(start.line_index, start.line_index, around, around) else { return };
        // Only a table cell (`|`), a fence (backticks, tildes), spaces at a
        // line's start, a paste into a container and text on a blank line
        // type differently: other text needs no parse.
        let (slice, planned) = if typed.contains(['|', '`', '~', '\n']) || typed.trim().is_empty() || blank_line {
            let Some((slice, events)) = self.parse(lines) else { return };
            let at = Self::slice_offset(&slice, start);
            let typed = if typed.contains('\n') {
                let prefix = plan::Containers::at(&slice.text, &events, at).continuation();
                typed.replace('\n', &format!("\n{prefix}"))
            } else {
                typed.to_string()
            };
            let planned = plan::type_text(&slice.text, &events, at, &typed);
            (slice, planned)
        } else {
            let Some(doc) = &self.doc else { return };
            let slice = Slice::new(doc.document.as_text().as_lines(), lines);
            let at = Self::slice_offset(&slice, start);
            (slice, Plan::insert(at, typed))
        };
        self.apply_plan(cx, &slice, &planned, kind, After::Caret);
        self.last_input = Some((start, self.caret));
        if wrap {
            self.anchor = start;
            for mark in pending {
                self.format(cx, kind, true, |text, events, range| plan::toggle_inline(text, events, range, mark));
            }
            // The caret stays inside the new markers, so the text typed next
            // carries the marks too.
            self.anchor = self.caret;
            self.last_input = None;
        }
    }

    /// Writes a pasted picture beside the document on a worker, as
    /// `<document>-image-N.png` (the first free N); `insert_saved` links it.
    fn save_picture(&mut self, cx: &mut Cx, image: ClipboardImage) {
        let Some(file) = self.file.clone() else {
            cx.widget_action(self.widget_uid(), MarkdownEditorAction::Problem("save the document first: a pasted picture is saved beside it".into()));
            return;
        };
        let job = move || {
            let result = write_picture(&file, image);
            SignalToUI::set_ui_signal();
            result
        };
        match cx.task_pool().submit(Class::Visible, job) {
            Ok(task) => self.saving.push(Saving { task, at: None, alt: String::new() }),
            Err(error) => cx.widget_action(self.widget_uid(), MarkdownEditorAction::Problem(format!("the pasted picture was not saved: {error}"))),
        }
    }

    /// A picture dropped at `at`: one under the document's folder is linked
    /// where it is; any other is copied beside the document first, on a
    /// worker, under its own name (or the first free `name-N`).
    fn drop_picture(&mut self, cx: &mut Cx, at: Position, path: &str) {
        let Some(folder) = self.file.as_deref().and_then(Path::parent).map(Path::to_path_buf) else {
            cx.widget_action(self.widget_uid(), MarkdownEditorAction::Problem("save the document first: a dropped picture is linked from its folder".into()));
            return;
        };
        let source = PathBuf::from(path);
        let alt = picture_alt(&source);
        if let Ok(relative) = source.strip_prefix(&folder) {
            let relative: Vec<String> = relative.components().map(|part| part.as_os_str().to_string_lossy().into_owned()).collect();
            self.anchor = self.clamp(at);
            self.caret = self.anchor;
            self.insert_source(cx, &plan::picture(&alt, &relative.join("/")));
            return;
        }
        let job = move || {
            let result = copy_picture(&folder, &source);
            SignalToUI::set_ui_signal();
            result
        };
        match cx.task_pool().submit(Class::Visible, job) {
            Ok(task) => self.saving.push(Saving { task, at: Some(at), alt }),
            Err(error) => cx.widget_action(self.widget_uid(), MarkdownEditorAction::Problem(format!("the dropped picture was not copied: {error}"))),
        }
    }

    /// Links each picture saved since the last call, as one undo step: a
    /// dropped one at its drop place, a pasted one at the caret, over the
    /// selection.
    fn insert_saved(&mut self, cx: &mut Cx) {
        let mut i = 0;
        while i < self.saving.len() {
            let Some(result) = self.saving[i].task.try_take() else {
                i += 1;
                continue;
            };
            // finished and taken: nothing is left to detach
            let Saving { task, at, alt } = self.saving.remove(i);
            drop(task);
            let error = match result {
                Ok(Ok(name)) => {
                    if let Some(folder) = self.file.as_deref().and_then(Path::parent) {
                        cx.widget_action(self.widget_uid(), MarkdownEditorAction::WroteFile(folder.join(&name)));
                    }
                    if let Some(at) = at {
                        self.anchor = self.clamp(at);
                        self.caret = self.anchor;
                    }
                    self.insert_source(cx, &plan::picture(&alt, &name));
                    continue;
                }
                Ok(Err(error)) => error,
                Err(error) => format!("{error:?}"),
            };
            let what = if at.is_some() { "dropped picture was not copied" } else { "pasted picture was not saved" };
            cx.widget_action(self.widget_uid(), MarkdownEditorAction::Problem(format!("the {what}: {error}")));
        }
    }

    /// Pictures dragged over the view: a ghost where the drop would go, the
    /// text reflowing around it; the drop links the picture there.
    fn handle_drag(&mut self, cx: &mut Cx, event: &Event) {
        let moving = self.moving.as_ref().map(|moving| moving.source.clone());
        // What a drag carries for the view: a picture file, or the picture
        // this view is moving (its markdown as it is).
        let picture = |items: &[DragItem]| {
            items.iter().find_map(|item| match item {
                DragItem::FilePath { internal_id: Some(id), .. } if *id == PICTURE_MOVE => moving.clone().map(|source| (source, None)),
                DragItem::FilePath { path, internal_id: None } if is_picture(path) => {
                    Some((plan::picture(&picture_alt(Path::new(path)), path), Some(path.clone())))
                }
                _ => None,
            })
        };
        // The chip rides the pointer wherever it goes.
        if self.moving.is_some() {
            match event {
                Event::Drag(e) => {
                    self.chip_at = Some(e.abs);
                    self.view.redraw(cx);
                }
                Event::DragEnd => {
                    self.moving = None;
                    self.chip_at = None;
                    self.view.redraw(cx);
                }
                _ => {}
            }
        }
        let ours = match event {
            Event::Drag(e) => picture(&e.items).is_some(),
            Event::Drop(e) => picture(&e.items).is_some(),
            Event::DragEnd => self.drop_target.is_some(),
            _ => false,
        };
        if !ours || self.doc.is_none() {
            return;
        }
        match event.drag_hits(cx, self.view.area()) {
            DragHit::Drag(d) => {
                let target = match (&d.state, picture(&d.items)) {
                    (DragState::Out, _) | (_, None) => None,
                    (_, Some((source, _))) => self.drop_position_at(cx, d.abs).map(|at| DropTarget { at, source }),
                };
                // A moved picture dropped on its own place stays.
                let target = target.filter(|target| !self.moving.as_ref().is_some_and(|m| m.start <= target.at && target.at <= m.end));
                if target.is_some() {
                    *d.response.lock().unwrap_or_else(|e| e.into_inner()) = DragResponse::Copy;
                }
                if target != self.drop_target {
                    self.drop_target = target;
                    self.view.redraw(cx);
                }
            }
            DragHit::Drop(d) => {
                let target = self.drop_target.take();
                self.ghost_row = None;
                let moving = self.moving.take();
                self.chip_at = None;
                if let Some((_, path)) = picture(&d.items) {
                    let at = match &moving {
                        Some(_) => target.map(|target| target.at),
                        None => target.map(|target| target.at).or_else(|| self.drop_position_at(cx, d.abs)),
                    };
                    if let Some(at) = at {
                        cx.set_key_focus(self.view.area());
                        match (moving, path) {
                            (Some(moving), _) => self.move_picture(cx, moving, at),
                            (None, Some(path)) => self.drop_picture(cx, at, &path),
                            (None, None) => {}
                        }
                    }
                }
                self.view.redraw(cx);
            }
            DragHit::DragEnd | DragHit::NoHit => {
                if self.drop_target.take().is_some() {
                    self.ghost_row = None;
                    self.view.redraw(cx);
                }
            }
        }
    }

    /// Starts dragging `picture` to another place: the view's own drag, its
    /// chip on the pointer.
    fn start_move(&mut self, cx: &mut Cx, picture: Picture, abs: DVec2) {
        let Some(lines) = self.blocks_around(picture.start.line_index, picture.end.line_index, 0, 0) else { return };
        let Some(doc) = &self.doc else { return };
        let slice = Slice::new(doc.document.as_text().as_lines(), lines);
        let source = slice.text[Self::slice_offset(&slice, picture.start)..Self::slice_offset(&slice, picture.end)].to_string();
        let path = resolve_source(&picture.dest, self.file.as_deref().and_then(Path::parent));
        let path = path.strip_prefix("file://").unwrap_or(&path).to_string();
        self.selecting = false;
        self.chip_at = Some(abs);
        self.moving = Some(Moving { start: picture.start, end: picture.end, source, path: path.clone() });
        cx.start_dragging(vec![DragItem::FilePath { path, internal_id: Some(PICTURE_MOVE) }]);
        self.view.redraw(cx);
    }

    /// Moves a picture to `at`: its removal and its insertion are one undo
    /// step, and it stays selected.
    fn move_picture(&mut self, cx: &mut Cx, moving: Moving, at: Position) {
        if moving.start <= at && at <= moving.end {
            return;
        }
        let first = moving.start.line_index.min(at.line_index);
        let last = moving.end.line_index.max(at.line_index);
        let Some(lines) = self.blocks_around(first, last, 0, 0) else { return };
        let Some(doc) = &self.doc else { return };
        let slice = Slice::new(doc.document.as_text().as_lines(), lines);
        let (start, end, to) = (Self::slice_offset(&slice, moving.start), Self::slice_offset(&slice, moving.end), Self::slice_offset(&slice, at));
        let source = slice.text[start..end].to_string();
        let len = source.len();
        let (edits, placed) = if to < start {
            (vec![(to..to, source), (start..end, String::new())], to)
        } else {
            (vec![(start..end, String::new()), (to..to, source)], to - (end - start))
        };
        let plan = Plan { edits, caret: placed + len };
        let kind = self.new_group();
        self.apply_plan(cx, &slice, &plan, kind, After::Select { anchor: placed, caret: placed + len });
    }

    /// The chip of a moved picture, on the pointer, over everything.
    fn draw_chip(&mut self, cx: &mut Cx2d, scope: &mut Scope) {
        let (Some(at), Some(moving)) = (self.chip_at, self.moving.as_ref()) else {
            self.chip_list.begin_always(cx);
            self.chip_list.end(cx);
            return;
        };
        let chip = self.view.widget(cx, ids!(chip_box.chip));
        let image = chip.as_image();
        let _ = image.load_image_file_by_path_async(cx, Path::new(&moving.path));
        self.chip_list.begin_overlay_last(cx);
        let size = cx.current_pass_size();
        cx.begin_root_turtle(size, Layout::default());
        if let Some((width, height)) = image.size_in_pixels(cx).filter(|(width, height)| *width > 0 && *height > 0) {
            let scale = (CHIP / width.max(height) as f64).min(1.0);
            let walk = Walk {
                abs_pos: Some(at + dvec2(14.0, 14.0)),
                width: Size::Fixed(width as f64 * scale),
                height: Size::Fixed(height as f64 * scale),
                ..Walk::default()
            };
            chip.draw_walk_all(cx, scope, walk);
        }
        cx.end_pass_sized_turtle();
        self.chip_list.end(cx);
    }

    /// Where a drop at `abs` would go: the caret place there, with the
    /// ghost's row read as if the ghost were not in it.
    fn drop_position_at(&self, cx: &Cx, abs: DVec2) -> Option<Position> {
        let drawn = self.nearest_row(cx, abs)?;
        let md = drawn.item.borrow::<Markdown>()?;
        let flat = md.text_flow.selection_point_to_char_index(cx, abs).unwrap_or(0);
        match self.ghost_row.as_ref().filter(|ghost| ghost.row == drawn.row) {
            Some(ghost) => Some(ghost.position_at(flat)),
            None => Some(self.rows.get(&drawn.key)?.caret_at(flat)),
        }
    }

    /// Puts `source` in at the caret as it is (no escaping), over the
    /// selection: one undo step.
    fn insert_source(&mut self, cx: &mut Cx, source: &str) {
        let kind = match self.delete_selection(cx, false) {
            Some((group, _)) => group,
            None => self.new_group(),
        };
        let Some(lines) = self.blocks_around(self.caret.line_index, self.caret.line_index, 0, 0) else { return };
        let Some(doc) = &self.doc else { return };
        let slice = Slice::new(doc.document.as_text().as_lines(), lines);
        let at = Self::slice_offset(&slice, self.caret);
        self.apply_plan(cx, &slice, &Plan::insert(at, source), kind, After::Caret);
    }

    /// Enter (a new block, or one container level out), or with `hard` a
    /// line break inside the block.
    fn enter(&mut self, cx: &mut Cx, hard: bool) {
        let kind = self.delete_selection(cx, false).map_or(EditKind::Other, |(group, _)| group);
        let Some(lines) = self.blocks_around(self.caret.line_index, self.caret.line_index, 0, 0) else { return };
        let Some((slice, events)) = self.parse(lines) else { return };
        let at = Self::slice_offset(&slice, self.caret);
        // In a table Enter goes to the cell below; after a pipe row it makes
        // the row a table's header.
        if !hard {
            let step = plan::cell_below(&slice.text, &events, at).or_else(|| plan::table_from_header(&slice.text, &events, at));
            if let Some(step) = step {
                self.apply_step(cx, &slice, step, kind);
                return;
            }
        }
        let plan = if hard {
            plan::hard_break(&slice.text, &events, at)
        } else {
            plan::enter(&slice.text, &events, at)
        };
        if let Some(plan) = plan {
            self.apply_plan(cx, &slice, &plan, kind, After::Caret);
        }
    }

    /// Backspace or forward Delete; `to` names how far: a character (None),
    /// or the caret's word or line edge (a position the keys found).
    fn delete(&mut self, cx: &mut Cx, forward: bool, to: Option<Position>) {
        if self.delete_selection(cx, true).is_some() {
            return;
        }
        if let Some(to) = to {
            if to != self.caret {
                self.anchor = to;
                self.delete_selection(cx, true);
            }
            return;
        }
        let line = self.caret.line_index;
        let lines = if forward { self.blocks_around(line, line, 0, 1) } else { self.blocks_around(line, line, 1, 0) };
        let Some((slice, events)) = lines.and_then(|lines| self.parse(lines)) else { return };
        let at = Self::slice_offset(&slice, self.caret);
        let plan = if forward {
            plan::delete_forward(&slice.text, &events, at)
        } else {
            plan::backspace(&slice.text, &events, at)
        };
        if let Some(plan) = plan {
            self.apply_plan(cx, &slice, &plan, EditKind::Delete, After::Caret);
        }
    }

    fn undo(&mut self, cx: &mut Cx, redo: bool) {
        let Some(doc) = &self.doc else { return };
        let selections = self.history_selection();
        let done = if redo {
            doc.document.redo(doc.session, &selections)
        } else {
            doc.document.undo(doc.session, &selections)
        };
        if done {
            self.sync(cx);
            self.reveal = true;
            self.moved(cx);
            cx.widget_action(self.widget_uid(), MarkdownEditorAction::Changed);
        }
    }

    // Formatting.

    /// Runs a format command over the top-level blocks the selection
    /// touches; a selection keeps covering the same text. With `inside`
    /// the caret stays before markers the command put at the selection's
    /// end (so text typed there takes the new mark). False when the command
    /// had nothing to do.
    fn format(
        &mut self,
        cx: &mut Cx,
        kind: EditKind,
        inside: bool,
        command: impl FnOnce(&str, &[(MdEvent<'static>, Range<usize>)], Range<usize>) -> Option<Plan>,
    ) -> bool {
        let (start, end) = self.ordered_selection();
        let Some((slice, events)) = self.blocks_around(start.line_index, end.line_index, 0, 0).and_then(|lines| self.parse(lines))
        else {
            return false;
        };
        let range = Self::slice_offset(&slice, start)..Self::slice_offset(&slice, end);
        let Some(planned) = command(&slice.text, &events, range.clone()) else { return false };
        let after = if range.is_empty() {
            After::Caret
        } else {
            After::Select {
                anchor: map_offset(&planned, range.start, false),
                caret: if inside { map_offset(&planned, range.end, true) } else { planned.caret },
            }
        };
        self.apply_plan(cx, &slice, &planned, kind, after);
        true
    }

    /// Toggles `mark` over the selection. With no selection the mark is
    /// pending: the next text typed takes it (or, inside the mark, loses it).
    pub fn toggle_mark(&mut self, cx: &mut Cx, mark: Mark) {
        if self.anchor == self.caret {
            match self.pending.iter().position(|pending| *pending == mark) {
                Some(at) => {
                    self.pending.remove(at);
                }
                None => self.pending.push(mark),
            }
            cx.widget_action(self.widget_uid(), MarkdownEditorAction::Caret);
            return;
        }
        self.format(cx, EditKind::Other, false, |text, events, range| plan::toggle_inline(text, events, range, mark));
    }

    /// Gives the blocks the selection touches `style` (Quote and Code
    /// toggle).
    pub fn set_block_style(&mut self, cx: &mut Cx, style: BlockStyle) {
        self.format(cx, EditKind::Other, false, |text, events, range| plan::set_block(text, events, range, style));
    }

    /// Links the selection to `url`; a caret or selection in a link changes
    /// its destination; None (or an empty URL) removes the link.
    pub fn set_link(&mut self, cx: &mut Cx, url: Option<&str>) {
        let url = url.map(str::trim).filter(|url| !url.is_empty());
        self.format(cx, EditKind::Other, false, |text, events, range| plan::set_link(text, events, range, url));
    }

    /// The destination of the link the caret is in.
    pub fn link_at_caret(&self) -> Option<String> {
        self.link_at(self.caret)
    }

    fn link_at(&self, position: Position) -> Option<String> {
        let line = position.line_index;
        let (slice, events) = self.blocks_around(line, line, 0, 0).and_then(|lines| self.parse(lines))?;
        plan::link_at(&slice.text, &events, Self::slice_offset(&slice, position))
    }

    /// Asks the host to edit `picture`'s alt text and source.
    fn edit_picture(&mut self, cx: &mut Cx, picture: &Picture) {
        self.editing_picture = Some(picture.start);
        let action = MarkdownEditorAction::EditPicture { alt: picture.alt.clone(), src: picture.dest.clone() };
        cx.widget_action(self.widget_uid(), action);
    }

    /// The picture the selection is, whole.
    fn selected_picture(&self) -> Option<Picture> {
        let (start, end) = self.ordered_selection();
        let (slice, events) = self.blocks_around(start.line_index, end.line_index, 0, 0).and_then(|lines| self.parse(lines))?;
        let (from, to) = (Self::slice_offset(&slice, start), Self::slice_offset(&slice, end));
        let image = events.iter().any(|(event, range)| matches!(event, MdEvent::Start(Tag::Image { .. })) && *range == (from..to));
        let (alt, dest) = plan::picture_at(&events, from).filter(|_| image)?;
        Some(Picture { start, end, alt, dest })
    }

    /// Gives the picture `EditPicture` named the alt text `alt` and the
    /// source `src` (an empty source keeps it as it is): one undo step, the
    /// picture selected after it.
    pub fn set_picture(&mut self, cx: &mut Cx, alt: &str, src: &str) {
        let Some(start) = self.editing_picture.take() else { return };
        let src = src.trim();
        let src = src.strip_prefix('<').and_then(|src| src.strip_suffix('>')).unwrap_or(src);
        if src.is_empty() {
            return;
        }
        let line = start.line_index;
        let Some((slice, events)) = self.blocks_around(line, line, 0, 0).and_then(|lines| self.parse(lines)) else { return };
        let at = Self::slice_offset(&slice, start);
        let Some(planned) = plan::set_picture(&slice.text, &events, at..at, alt.trim(), src) else { return };
        let end = planned.caret;
        let kind = self.new_group();
        self.apply_plan(cx, &slice, &planned, kind, After::Select { anchor: at, caret: end });
    }

    /// Flips the task box of the item at `position`; the selection stays.
    fn toggle_task(&mut self, cx: &mut Cx, position: Position) -> bool {
        let line = position.line_index;
        let Some((slice, events)) = self.blocks_around(line, line, 0, 0).and_then(|lines| self.parse(lines)) else {
            return false;
        };
        let at = Self::slice_offset(&slice, position);
        let Some(planned) = plan::toggle_task(&slice.text, &events, at) else { return false };
        self.apply_plan(cx, &slice, &planned, EditKind::Other, After::Keep);
        true
    }

    /// Runs a table command at the caret: its edits as one history step,
    /// then its selection. False when it does not apply here.
    fn table(
        &mut self,
        cx: &mut Cx,
        command: impl FnOnce(&str, &[(MdEvent<'static>, Range<usize>)], usize) -> Option<plan::Step>,
    ) -> bool {
        let line = self.caret.line_index;
        let Some((slice, events)) = self.blocks_around(line, line, 0, 0).and_then(|lines| self.parse(lines)) else {
            return false;
        };
        let Some(step) = command(&slice.text, &events, Self::slice_offset(&slice, self.caret)) else { return false };
        self.apply_step(cx, &slice, step, EditKind::Other);
        true
    }

    /// Applies a table step made over `slice`: its plan, then its selection.
    fn apply_step(&mut self, cx: &mut Cx, slice: &Slice, step: plan::Step, kind: EditKind) {
        let (anchor, caret) = (step.select.start, step.select.end);
        let planned = step.plan.unwrap_or(Plan { edits: Vec::new(), caret });
        if !planned.edits.is_empty() {
            self.pending.clear();
        }
        self.apply_plan(cx, slice, &planned, kind, After::Select { anchor, caret });
    }

    /// A table key: inserts a table, or adds or removes the caret's row or
    /// column. False when it does not apply (a row key outside a table).
    pub fn edit_table(&mut self, cx: &mut Cx, edit: TableEdit) -> bool {
        self.table(cx, |text, events, at| match edit {
            TableEdit::Insert => plan::insert_table(text, events, at, 3, 2),
            TableEdit::RowAbove => plan::insert_row(text, events, at, true),
            TableEdit::RowBelow => plan::insert_row(text, events, at, false),
            TableEdit::DeleteRow => plan::delete_row(text, events, at),
            TableEdit::ColumnLeft => plan::insert_column(text, events, at, true),
            TableEdit::ColumnRight => plan::insert_column(text, events, at, false),
            TableEdit::DeleteColumn => plan::delete_column(text, events, at),
        })
    }

    /// Tab and Shift+Tab: the list items the selection touches move in or
    /// out a level. Outside a list Tab types a tab (a code block keeps it;
    /// elsewhere the leading-space rule drops it).
    fn indent(&mut self, cx: &mut Cx, outdent: bool) {
        if self.table(cx, |text, events, at| plan::table_step(text, events, at, outdent)) {
            return;
        }
        let done =
            self.format(cx, EditKind::Other, false, |text, events, range| plan::indent_items(text, events, range, outdent));
        if !done && !outdent && self.anchor == self.caret && self.in_list().is_none() {
            self.type_text(cx, "\t", false);
        }
    }

    /// The caret is in a list item (Some(ordered)), as the caret's row parsed.
    fn in_list(&self) -> Option<bool> {
        match self.format_state().block? {
            BlockStyle::Bullet | BlockStyle::Task => Some(false),
            BlockStyle::Numbered => Some(true),
            _ => None,
        }
    }

    /// What the caret stands in, from its row's parse (empty while the row
    /// is not drawn): for a format bar. Costs one walk over the row's events.
    pub fn format_state(&self) -> FormatState {
        let mut state = FormatState::default();
        let Some((_, cache)) = self.caret_row() else { return state };
        let at = cache.offset_of(self.caret);
        let holds = |range: &Range<usize>| range.start <= at && at < range.end;
        let inside = |range: &Range<usize>| range.start < at && at < range.end;
        // The lists (ordered or not) and the items the walk is in.
        let mut lists: Vec<bool> = Vec::new();
        let mut items: Vec<Range<usize>> = Vec::new();
        for (event, range) in &cache.events {
            match event {
                MdEvent::Start(Tag::List(start)) => lists.push(start.is_some()),
                MdEvent::End(TagEnd::List(_)) => {
                    lists.pop();
                }
                MdEvent::Start(Tag::Item) => {
                    if holds(range) {
                        let ordered = lists.last().copied().unwrap_or(false);
                        state.block = Some(if ordered { BlockStyle::Numbered } else { BlockStyle::Bullet });
                    }
                    items.push(range.clone());
                }
                MdEvent::End(TagEnd::Item) => {
                    items.pop();
                }
                MdEvent::TaskListMarker(_) if items.last().is_some_and(&holds) => {
                    state.block = Some(BlockStyle::Task);
                }
                MdEvent::Start(Tag::BlockQuote(_)) if holds(range) => state.quote = true,
                MdEvent::Start(Tag::Heading { level, .. }) if holds(range) => {
                    state.block = Some(BlockStyle::Heading(*level as u8));
                }
                MdEvent::Start(Tag::CodeBlock(_)) if holds(range) => state.block = Some(BlockStyle::Code),
                MdEvent::Start(Tag::Table(_)) if holds(range) => state.table = true,
                MdEvent::Start(Tag::Paragraph) if holds(range) && state.block.is_none() => {
                    state.block = Some(BlockStyle::Paragraph);
                }
                MdEvent::Start(Tag::Emphasis) if inside(range) => state.marks.push(Mark::Emphasis),
                MdEvent::Start(Tag::Strong) if inside(range) => state.marks.push(Mark::Strong),
                MdEvent::Start(Tag::Strikethrough) if inside(range) => state.marks.push(Mark::Strikethrough),
                MdEvent::Code(_) if inside(range) => state.marks.push(Mark::Code),
                MdEvent::Start(Tag::Link { .. }) if inside(range) || (range.start < at && at == range.end) => {
                    state.link = true;
                }
                _ => {}
            }
        }
        if state.block.is_none() && cache.line_is_blank(self.caret.line_index) {
            state.block = Some(BlockStyle::Paragraph);
        }
        state.marks.dedup();
        for mark in &self.pending {
            match state.marks.iter().position(|have| have == mark) {
                Some(at) => {
                    state.marks.remove(at);
                }
                None => state.marks.push(*mark),
            }
        }
        state
    }

    /// Scrolls the heading whose anchor (its text as a slug: lower case,
    /// spaces as `-`, punctuation dropped) is `anchor` to the top, with the
    /// caret at its start. False when no heading has it.
    pub fn go_to_anchor(&mut self, cx: &mut Cx, anchor: &str) -> bool {
        let want = slug(anchor);
        let found = {
            let Some(doc) = &self.doc else { return false };
            let text = doc.document.as_text();
            let lines = text.as_lines();
            doc.index.blocks().iter().enumerate().find_map(|(at, block)| {
                let BlockKind::Heading(_) = block.kind else { return None };
                let first = doc.index.block_lines(at).start;
                (heading_anchor(lines.get(first)?) == want).then_some(first)
            })
        };
        let Some(line) = found else { return false };
        self.set_caret(cx, Position { line_index: line, byte_index: 0 }, Some(0.0));
        self.snap = true;
        self.moved(cx);
        true
    }

    // Movement.

    /// The drawn row holding the caret's line, and its parse.
    fn caret_row(&self) -> Option<(&DrawnRow, &RowCache)> {
        let index = &self.doc.as_ref()?.index;
        if index.row_count() == 0 {
            return None;
        }
        let row = index.row_at_line(self.caret.line_index.min(index.line_count().saturating_sub(1)));
        let drawn = self.drawn.iter().find(|drawn| drawn.row == row)?;
        let cache = self.rows.get(&drawn.key)?;
        let (start, end) = cache.span();
        (start.line_index..=end.line_index).contains(&self.caret.line_index).then_some((drawn, cache))
    }

    /// Moves the caret; `extend` keeps the anchor (Shift).
    fn move_to(&mut self, cx: &mut Cx, to: Position, extend: bool) {
        let to = self.clamp(to);
        self.caret = to;
        if !extend {
            self.anchor = to;
        }
        if let Some(doc) = &self.doc {
            doc.document.force_new_group();
        }
        self.last_input = None;
        self.pending.clear();
        self.reveal = true;
        self.moved(cx);
    }

    /// One step left or right through what the rows draw: a character, or
    /// with `word` to a word's start. Steps that land where the source does
    /// not change (a bullet, a hidden marker) step on.
    fn step(&mut self, right: bool, word: bool) -> Option<Position> {
        let (drawn, cache) = self.caret_row()?;
        let md = drawn.item.borrow::<Markdown>()?;
        let text = md.text_flow.tracked_text();
        let here = cache.offset_of(self.caret);
        let mut flat = flat_of(&cache.pieces, here);
        loop {
            let next = match (right, word) {
                (true, false) => next_char(text, flat),
                (true, true) => next_word(text, flat),
                (false, false) => prev_char(text, flat),
                (false, true) => prev_word(text, flat),
            };
            if next == flat {
                break;
            }
            flat = next;
            let src = caret_src(&cache.pieces, flat, &cache.links);
            if src != here {
                return Some(cache.position_of(src));
            }
        }
        // Past the row's edge: the next row's start, or the previous row's end.
        let row = drawn.row;
        drop(md);
        self.row_edge(if right { row + 1 } else { row.checked_sub(1)? }, right)
    }

    /// The first (`start`) or last caret place of row `row`. A row not
    /// drawn yet gets its first or last line, and the caret snaps to a
    /// caret place once it is drawn.
    fn row_edge(&mut self, row: usize, start: bool) -> Option<Position> {
        let index = &self.doc.as_ref()?.index;
        if row >= index.row_count() {
            return None;
        }
        if let Some(drawn) = self.drawn.iter().find(|drawn| drawn.row == row) {
            let cache = self.rows.get(&drawn.key)?;
            let md = drawn.item.borrow::<Markdown>()?;
            let flat = if start { 0 } else { md.text_flow.tracked_text().len() };
            return Some(cache.caret_at(flat));
        }
        let lines = index.row(row).lines;
        self.snap = true;
        let line = if start { lines.start } else { lines.end.saturating_sub(1) };
        Some(Position { line_index: line, byte_index: if start { 0 } else { usize::MAX } })
    }

    /// The line above or below the caret: the caret's rect moved half a line
    /// up or one and a half down, at the x the moves keep, hit-tested.
    fn step_line(&mut self, cx: &Cx, down: bool) -> Option<Position> {
        let rect = self.caret_rect?;
        let x = *self.preferred_x.get_or_insert(rect.pos.x);
        let y = if down { rect.pos.y + rect.size.y * 1.5 } else { rect.pos.y - rect.size.y * 0.5 };
        let found = self.position_at(cx, dvec2(x, y))?;
        if found != self.caret {
            return Some(found);
        }
        let (drawn, _) = self.caret_row()?;
        let row = drawn.row;
        self.row_edge(if down { row + 1 } else { row.checked_sub(1)? }, down)
    }

    /// The start or end of the caret's line as drawn.
    fn line_edge(&self, cx: &Cx, end: bool) -> Option<Position> {
        let rect = self.caret_rect?;
        let (drawn, _) = self.caret_row()?;
        let area = drawn.item.area().rect(cx);
        let x = if end { area.pos.x + area.size.x } else { area.pos.x };
        self.position_at(cx, dvec2(x, rect.pos.y + rect.size.y * 0.5))
    }

    fn document_edge(&self, end: bool) -> Position {
        let Some(doc) = &self.doc else { return Position::default() };
        let text = doc.document.as_text();
        let lines = text.as_lines();
        if !end {
            return Position::default();
        }
        let last = lines.len().saturating_sub(1);
        Position { line_index: last, byte_index: lines.get(last).map_or(0, String::len) }
    }

    fn handle_key(&mut self, cx: &mut Cx, ke: &KeyEvent) {
        let shift = ke.modifiers.shift;
        let primary = ke.modifiers.is_primary();
        // By word: Alt on macOS, Control elsewhere. To the line's or the
        // document's edge: Cmd on macOS (elsewhere Home, End and
        // Control+Home or End).
        let word = ke.modifiers.alt || ke.modifiers.control;
        let edge = ke.modifiers.logo;
        let alt = ke.modifiers.alt;
        let (start, end) = self.ordered_selection();
        match ke.key_code {
            KeyCode::ArrowLeft | KeyCode::ArrowRight => {
                let right = ke.key_code == KeyCode::ArrowRight;
                self.preferred_x = None;
                let to = if edge {
                    self.line_edge(cx, right)
                } else if start != end && !shift && !word {
                    Some(if right { end } else { start })
                } else {
                    self.step(right, word)
                };
                if let Some(to) = to {
                    self.move_to(cx, to, shift);
                }
            }
            KeyCode::ArrowUp | KeyCode::ArrowDown => {
                let down = ke.key_code == KeyCode::ArrowDown;
                let to = if edge { Some(self.document_edge(down)) } else { self.step_line(cx, down) };
                let keep_x = self.preferred_x;
                if let Some(to) = to {
                    self.move_to(cx, to, shift);
                }
                self.preferred_x = if edge { None } else { keep_x };
            }
            KeyCode::Home | KeyCode::End => {
                let end = ke.key_code == KeyCode::End;
                self.preferred_x = None;
                let to = if primary { Some(self.document_edge(end)) } else { self.line_edge(cx, end) };
                if let Some(to) = to {
                    self.move_to(cx, to, shift);
                }
            }
            KeyCode::ReturnKey if primary => {
                let caret = self.caret;
                self.toggle_task(cx, caret);
            }
            KeyCode::ReturnKey => self.enter(cx, shift),
            KeyCode::Tab => self.indent(cx, shift),
            // Inline marks and links.
            KeyCode::KeyB if primary && !shift && !alt => self.toggle_mark(cx, Mark::Strong),
            KeyCode::KeyI if primary && !shift && !alt => self.toggle_mark(cx, Mark::Emphasis),
            KeyCode::KeyX if primary && shift && !alt => self.toggle_mark(cx, Mark::Strikethrough),
            KeyCode::KeyE if primary && !shift && !alt => self.toggle_mark(cx, Mark::Code),
            KeyCode::KeyK if primary && !shift && !alt => match self.selected_picture() {
                Some(picture) => self.edit_picture(cx, &picture),
                None => {
                    let url = self.link_at_caret();
                    cx.widget_action(self.widget_uid(), MarkdownEditorAction::EditLink(url));
                }
            },
            // Block styles: Cmd+Alt+0 to 6 a paragraph or a heading level;
            // Cmd+Shift+7, 8, 9 numbered, bullet and task lists; Cmd+Shift+.
            // a quote; Cmd+Shift+C a code block.
            KeyCode::Key0 if primary && alt && !shift => self.set_block_style(cx, BlockStyle::Paragraph),
            KeyCode::Key1 if primary && alt && !shift => self.set_block_style(cx, BlockStyle::Heading(1)),
            KeyCode::Key2 if primary && alt && !shift => self.set_block_style(cx, BlockStyle::Heading(2)),
            KeyCode::Key3 if primary && alt && !shift => self.set_block_style(cx, BlockStyle::Heading(3)),
            KeyCode::Key4 if primary && alt && !shift => self.set_block_style(cx, BlockStyle::Heading(4)),
            KeyCode::Key5 if primary && alt && !shift => self.set_block_style(cx, BlockStyle::Heading(5)),
            KeyCode::Key6 if primary && alt && !shift => self.set_block_style(cx, BlockStyle::Heading(6)),
            KeyCode::Key7 if primary && shift && !alt => self.set_block_style(cx, BlockStyle::Numbered),
            KeyCode::Key8 if primary && shift && !alt => self.set_block_style(cx, BlockStyle::Bullet),
            KeyCode::Key9 if primary && shift && !alt => self.set_block_style(cx, BlockStyle::Task),
            KeyCode::Period if primary && shift && !alt => self.set_block_style(cx, BlockStyle::Quote),
            KeyCode::KeyC if primary && shift && !alt => self.set_block_style(cx, BlockStyle::Code),
            KeyCode::Backspace | KeyCode::Delete => {
                let forward = ke.key_code == KeyCode::Delete;
                let to = if start != end {
                    None
                } else if edge {
                    self.line_edge(cx, forward)
                } else if word {
                    self.step(forward, true)
                } else {
                    None
                };
                self.delete(cx, forward, to);
            }
            KeyCode::KeyZ if primary => self.undo(cx, shift),
            KeyCode::KeyY if ke.modifiers.control && !ke.modifiers.logo => self.undo(cx, true),
            KeyCode::KeyA if primary && !shift => self.select_all(cx),
            // A paste's text comes as input ahead of its key, and asked for
            // the read; a key alone (a picture on the clipboard, no text)
            // asks for it here.
            KeyCode::KeyV if primary && !alt => {
                if !std::mem::take(&mut self.paste_by_text) {
                    self.paste_reads.push((cx.read_clipboard(), None));
                }
            }
            _ => cx.widget_action(self.widget_uid(), MarkdownEditorAction::UnhandledKey(ke.clone())),
        }
    }
}

/// Text as the editor types it: `\r\n` as `\n`, no control characters
/// but new lines and tabs.
fn typed_text(input: &str) -> String {
    input.replace("\r\n", "\n").chars().filter(|c| *c == '\n' || *c == '\t' || !c.is_control()).collect()
}

impl Widget for MarkdownEditor {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.sync(cx);
        // A press something drawn over the view took before it (a bar of
        // the host's) is that widget's, not the view's.
        let pressed_over = match event {
            Event::MouseDown(e) => !e.handled.get().is_empty(),
            Event::TouchUpdate(e) => e.touches.iter().any(|t| t.state == event::TouchState::Start && !t.handled.get().is_empty()),
            _ => false,
        };
        if let Event::Clipboard(reply) = event {
            if let Some(at) = self.paste_reads.iter().position(|(request, _)| *request == reply.request) {
                let (_, text) = self.paste_reads.remove(at);
                self.paste_by_text = false;
                self.paste(cx, &reply.content, text);
            }
        }
        if !self.saving.is_empty() {
            self.insert_saved(cx);
        }
        self.handle_drag(cx, event);
        if self.moving.is_some() {
            // The chip's picture loads like any image: it takes its decode.
            self.view.widget(cx, ids!(chip_box.chip)).handle_event(cx, event, scope);
        }
        self.view.handle_event(cx, event, scope);
        let area = self.view.area();
        // The rows take the pointer's hover, so the view reads the moves
        // itself: a hand over a task box (a click flips it) and over a link
        // while Cmd/Ctrl is down (a click follows it, underlined).
        if let Event::MouseMove(me) = event {
            if !self.selecting && area.rect(cx).contains(me.abs) {
                let target = self.target_at(cx, me.abs);
                let link = match &target {
                    Some(Target::Link(url)) if me.modifiers.is_primary() => Some(url.clone()),
                    _ => None,
                };
                // A picture can be dragged to another place.
                let hand = link.is_some() || matches!(target, Some(Target::Task(_))) || self.object_at(cx, me.abs).is_some();
                cx.set_cursor(if hand { MouseCursor::Hand } else { MouseCursor::Text });
                self.hover_link(cx, link);
            } else {
                self.hover_link(cx, None);
            }
        }
        // The list under the rows takes the press first (for its scroll bar
        // and wheel); the view shares it.
        let hit = if pressed_over { Hit::Nothing } else { event.hits_with_capture_overload(cx, area, true) };
        match hit {
            Hit::FingerHoverOut(_) => self.hover_link(cx, None),
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                cx.set_key_focus(area);
                self.pending.clear();
                match self.target_at(cx, fe.abs) {
                    Some(Target::Task(at)) => {
                        self.toggle_task(cx, at);
                        return;
                    }
                    Some(Target::Link(url)) if fe.modifiers.is_primary() => {
                        cx.widget_action(self.widget_uid(), MarkdownEditorAction::OpenLink(url));
                        return;
                    }
                    _ => {}
                }
                // A press on a picture selects it whole; a double-click
                // edits its alt text and source.
                let picture = if fe.modifiers.shift { None } else { self.object_at(cx, fe.abs) };
                if picture.is_none() && fe.tap_count >= 2 && !fe.modifiers.shift && self.select_around(cx, fe.abs, fe.tap_count >= 3) {
                    self.selecting = false;
                    return;
                }
                if let Some(picture) = picture {
                    self.anchor = picture.start;
                    self.caret = picture.end;
                    if fe.tap_count >= 2 {
                        self.edit_picture(cx, &picture);
                    } else {
                        self.press_picture = Some((picture, fe.abs));
                    }
                    self.prefer_next_row = false;
                    self.preferred_x = None;
                    self.last_input = None;
                    self.selecting = false;
                    if let Some(doc) = &self.doc {
                        doc.document.force_new_group();
                    }
                    self.moved(cx);
                    return;
                }
                if let Some(position) = self.position_at(cx, fe.abs) {
                    self.caret = position;
                    if !fe.modifiers.shift {
                        self.anchor = position;
                    }
                    self.prefer_next_row = false;
                    self.preferred_x = None;
                    self.last_input = None;
                    self.selecting = true;
                    if let Some(doc) = &self.doc {
                        doc.document.force_new_group();
                    }
                    self.moved(cx);
                }
            }
            Hit::FingerMove(fe) if self.press_picture.is_some() => {
                if self.press_picture.as_ref().is_some_and(|(_, at)| (fe.abs - *at).length() >= DRAG_START) {
                    if let Some((picture, _)) = self.press_picture.take() {
                        self.start_move(cx, picture, fe.abs);
                    }
                }
            }
            Hit::FingerMove(fe) if self.selecting => {
                if let Some(position) = self.position_at(cx, fe.abs) {
                    if position != self.caret {
                        self.caret = position;
                        self.moved(cx);
                    }
                }
            }
            Hit::FingerUp(_) => {
                self.selecting = false;
                self.press_picture = None;
            }
            Hit::TextCopy(ce) => {
                if let Some(write) = self.copied(self.selected_text()) {
                    ce.answer(write);
                }
            }
            Hit::TextCut(ce) => {
                if let Some(write) = self.delete_selection(cx, true).and_then(|(_, fragment)| self.copied(fragment)) {
                    ce.answer(write);
                }
            }
            Hit::TextInput(te) => {
                // Compositions, replaced ranges and full-state syncs belong
                // to the soft keyboards' protocol; their committed text
                // arrives as plain input.
                if te.composition.is_some() || te.full_state_sync.is_some() || te.replace_range.is_some() {
                    return;
                }
                let input = typed_text(&te.input);
                if input.is_empty() {
                    return;
                }
                // A paste waits for its read: the clipboard's Markdown
                // flavour, when it has one, pastes instead of this text.
                // Where its key came first (the web), the key's own read
                // is this paste's too: only this one pastes.
                if te.was_paste {
                    self.paste_by_text = true;
                    self.paste_reads.retain(|(_, text)| text.is_some());
                    self.paste_reads.push((cx.read_clipboard(), Some(input)));
                    return;
                }
                // A dead key's accent, replaced by the accented letter.
                if te.replace_last {
                    if let Some((start, end)) = self.last_input {
                        self.anchor = start;
                        self.caret = end;
                    }
                }
                self.type_text(cx, &input, te.was_paste);
            }
            Hit::KeyDown(ke) => self.handle_key(cx, &ke),
            Hit::KeyFocus(_) => self.view.redraw(cx),
            Hit::KeyFocusLost(_) => {
                cx.hide_text_ime();
                self.view.redraw(cx);
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.sync(cx);
        self.draws += 1;
        self.drawn.clear();
        while let Some(step) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(mut list) = step.as_portal_list().borrow_mut() {
                self.draw_rows(cx, &mut list);
            }
        }
        if std::mem::take(&mut self.rewash) {
            self.view.redraw(cx);
        }
        // A placement holds for the one draw that applied it.
        if let Some(place) = self.place.take() {
            self.place_caret_y = Some(place.y);
        }
        let draws = self.draws;
        self.rows.retain(|_, cache| cache.drawn_in == draws);
        if let Some(doc) = &self.doc {
            self.highlights.sweep(doc.index.blocks().iter().map(|block| block.gen));
        }
        self.snap_caret();
        self.draw_caret(cx);
        self.draw_chip(cx, scope);
        self.reveal_caret(cx);
        if let (Some(target), Some(at)) = (self.place_caret_y.take(), self.caret_y) {
            let delta = at - target;
            if delta.abs() > 0.5 {
                let list = self.view.portal_list(cx, ids!(list));
                if let Some(mut list) = list.borrow_mut() {
                    let (first, scroll) = (list.first_id(), list.first_scroll());
                    list.set_first_id_and_scroll(first, scroll - delta);
                }
                self.view.redraw(cx);
            }
        }
        DrawStep::done()
    }
}

/// Writes `image` beside `file` as `<stem>-image-N.png`, the first N no
/// file has (a file another writer takes first is skipped): its name.
fn write_picture(file: &Path, image: ClipboardImage) -> Result<String, String> {
    use std::io::Write;
    let png = match image {
        ClipboardImage::Png(png) => png,
        ClipboardImage::Rgba { width, height, rgba } => Arc::new(Cx::encode_rgba_as_png(width, height, &rgba)?),
    };
    let folder = file.parent().ok_or("the document has no folder")?;
    let stem = file.file_stem().map_or("image".into(), |stem| stem.to_string_lossy());
    let mut n = 1u64;
    loop {
        let name = format!("{stem}-image-{n}.png");
        match std::fs::OpenOptions::new().write(true).create_new(true).open(folder.join(&name)) {
            Ok(mut out) => {
                out.write_all(&png).map_err(|error| format!("{name}: {error}"))?;
                return Ok(name);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => n += 1,
            Err(error) => return Err(format!("{name}: {error}")),
        }
    }
}

/// The matches in a row's source `start..=end` as marks on its flow, from
/// what the row drew last; None when it has matches but has not drawn yet.
/// A match in markup the view hides shows nothing.
fn found_in_row(matches: &[(Position, Position)], cache: &RowCache, start: Position, end: Position) -> Option<Vec<TextMark>> {
    let first = matches.partition_point(|m| m.0 < start);
    let within = matches[first..].iter().take_while(|m| m.1 <= end);
    if cache.pieces.is_empty() {
        return if within.clone().next().is_some() { None } else { Some(Vec::new()) };
    }
    Some(
        within
            .filter_map(|&(a, b)| {
                let a = flat_of(&cache.pieces, cache.offset_of(a));
                let b = flat_of(&cache.pieces, cache.offset_of(b));
                (a < b).then(|| TextMark::found(a, b))
            })
            .collect(),
    )
}

/// Copies `source` into `folder` under its own name, or the first free
/// `name-N.ext` (a file in the way is never overwritten): the new name.
/// The whole cells a selection inside `cache`'s row covers, its table
/// counted as the row's flow counts them: None unless the selection runs
/// from one cell of a table to another.
fn cell_selection(cache: &RowCache, anchor: Position, caret: Position) -> Option<CellSelection> {
    let block = plan::cell_block(&cache.slice.text, &cache.events, cache.offset_of(anchor), cache.offset_of(caret))?;
    let table = cache
        .events
        .iter()
        .filter(|(event, range)| matches!(event, MdEvent::Start(Tag::Table(_))) && range.end <= block.table)
        .count();
    Some(CellSelection { table, rows: block.rows, columns: block.columns })
}

fn copy_picture(folder: &Path, source: &Path) -> Result<String, String> {
    let name = source.file_name().ok_or("the picture has no file name")?.to_string_lossy().into_owned();
    let stem = source.file_stem().map_or_else(|| name.clone(), |stem| stem.to_string_lossy().into_owned());
    let ext = source.extension().map(|ext| format!(".{}", ext.to_string_lossy())).unwrap_or_default();
    let mut input = std::fs::File::open(source).map_err(|error| format!("{name}: {error}"))?;
    let mut n = 1u64;
    loop {
        let candidate = if n == 1 { name.clone() } else { format!("{stem}-{n}{ext}") };
        match std::fs::OpenOptions::new().write(true).create_new(true).open(folder.join(&candidate)) {
            Ok(mut out) => {
                std::io::copy(&mut input, &mut out).map_err(|error| format!("{candidate}: {error}"))?;
                return Ok(candidate);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => n += 1,
            Err(error) => return Err(format!("{candidate}: {error}")),
        }
    }
}

/// A file the final view draws as a picture, by its extension.
fn is_picture(path: &str) -> bool {
    Path::new(path).extension().and_then(|ext| ext.to_str()).is_some_and(|ext| {
        ["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg"].iter().any(|known| ext.eq_ignore_ascii_case(known))
    })
}

/// A picture's alt text: its file's name without the extension.
fn picture_alt(path: &Path) -> String {
    path.file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pictures_brought_in_take_a_free_name_beside_the_document() {
        let dir = std::env::temp_dir().join(format!("markdown-editor-paste-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("my notes.md");
        let png = Arc::new(vec![0x89, b'P', b'N', b'G', 1, 2, 3]);
        let first = write_picture(&file, ClipboardImage::Png(png.clone())).unwrap();
        assert_eq!(first, "my notes-image-1.png", "named after the document");
        assert_eq!(std::fs::read(dir.join(&first)).unwrap(), *png, "a PNG is saved as it came");
        std::fs::write(dir.join("my notes-image-2.png"), b"taken").unwrap();
        let rgba = ClipboardImage::Rgba { width: 1, height: 1, rgba: Arc::new(vec![255, 0, 0, 255]) };
        let third = write_picture(&file, rgba).unwrap();
        assert_eq!(third, "my notes-image-3.png", "a name in use is skipped, never overwritten");
        assert_eq!(std::fs::read(dir.join("my notes-image-2.png")).unwrap(), b"taken", "the file in the way is kept");
        assert!(std::fs::read(dir.join(&third)).unwrap().starts_with(&[0x89, b'P', b'N', b'G']), "RGBA is encoded as PNG");
        // A picture dropped from another folder is copied in under its own
        // name, and beside a file of that name under the next free one.
        let outside = dir.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("shot.png"), b"shot").unwrap();
        assert_eq!(copy_picture(&dir, &outside.join("shot.png")).unwrap(), "shot.png", "a dropped picture keeps its name");
        assert_eq!(copy_picture(&dir, &outside.join("shot.png")).unwrap(), "shot-2.png", "a name in use is not overwritten");
        assert_eq!(std::fs::read(dir.join("shot-2.png")).unwrap(), b"shot", "the copy holds the picture");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
