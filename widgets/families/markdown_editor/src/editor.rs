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

use crate::rowmap::{caret_src, flat_of, next_char, next_word, prev_char, prev_word};
use makepad_code_editor::{
    history::EditKind,
    selection::{Cursor, Selection, SelectionSet},
    session::SessionId,
    text::{Change, Edit, Position, Text},
    CodeDocument,
};
use makepad_markdown_edit::{
    plan::{self, Plan},
    pulldown_cmark::{CodeBlockKind, CowStr, Event as MdEvent, Tag, TagEnd},
    BlockKind, Index, LineEdit, Row, RowPart, Slice,
};
use makepad_widgets_core::*;
use makepad_widgets_rich_text::{
    markdown::{Markdown, MarkdownHost, MarkdownPiece},
    text_flow::TextFlow,
};
use std::{
    collections::HashMap,
    ops::Range,
    sync::mpsc::{channel, Receiver},
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
    #[default]
    None,
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

struct RowCache {
    slice: Slice,
    events: Vec<(MdEvent<'static>, Range<usize>)>,
    list_start: Option<u64>,
    /// The source ranges of the row's links (the caret leaves a link at its end).
    links: Vec<Range<usize>>,
    /// What the flow drew, from the row's last draw.
    pieces: Vec<MarkdownPiece>,
    /// Where the caret stood in the last draw when its line drew no text (a
    /// blank line, an empty item): the walk made room for it there.
    anchor: Option<(usize, Rect)>,
    /// The draw this row was last drawn in.
    drawn_in: u64,
}

impl RowCache {
    fn build(index: &Index, lines: &[String], row: &Row) -> Self {
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
        let list_start = match (row.part, &block.kind) {
            (RowPart::Item { index, .. }, BlockKind::List { ordered: true, start, .. }) => Some(start + index as u64),
            _ => None,
        };
        let links = events
            .iter()
            .filter(|(event, _)| matches!(event, MdEvent::Start(Tag::Link { .. })))
            .map(|(_, range)| range.clone())
            .collect();
        Self {
            slice,
            events,
            list_start,
            links,
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

#[derive(Script, ScriptHook, Widget)]
pub struct MarkdownEditor {
    #[deref]
    view: View,
    #[live]
    draw_caret: DrawColor,
    #[rust]
    doc: Option<Attached>,
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
}

impl MarkdownEditor {
    /// Shows `document`, from its start. The view joins the document's edit
    /// broadcast until another document replaces it or `detach`.
    pub fn attach(&mut self, cx: &mut Cx, mut document: CodeDocument) {
        self.doc = None;
        self.rows.clear();
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
        self.view.redraw(cx);
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
        while let Some(row_id) = list.next_visible_item(cx) {
            if row_id >= row_count {
                continue;
            }
            let row = index.row(row_id);
            let key = RowKey::of(index, &row);
            let cache = self.rows.entry(key).or_insert_with(|| RowCache::build(index, lines, &row));
            cache.drawn_in = self.draws;
            let item = list.item(cx, row_id, id!(Row));
            let walk = item.walk(cx);
            let Some(mut md) = item.borrow_mut::<Markdown>() else { continue };
            // The selection in the flow's text, from what this row drew last
            // time (a row drawn for the first time shows it from the next draw).
            let (start, end) = cache.span();
            let (a, b) = if sel_start < sel_end && sel_start <= end && sel_end >= start && !cache.pieces.is_empty() {
                let a = if sel_start <= start { 0 } else { flat_of(&cache.pieces, cache.offset_of(sel_start)) };
                let b = if sel_end >= end { usize::MAX } else { flat_of(&cache.pieces, cache.offset_of(sel_end)) };
                (a, b)
            } else {
                (0, 0)
            };
            md.text_flow.set_selection(a, b);
            // A block keeps the spacing above it; the next item of a list or
            // run of a code block continues the one above.
            let follows_block = row_id > 0
                && match row.part {
                    RowPart::Whole => true,
                    RowPart::Item { index, .. } => index == 0,
                    RowPart::Code { chunk, .. } => chunk == 0,
                };
            // A caret on a line that draws no text gets a place made for it.
            let caret_line = self.caret.line_index;
            let anchors = if caret_line >= start.line_index && caret_line <= end.line_index && cache.line_is_blank(caret_line) {
                vec![(cache.offset_of(self.caret), None)]
            } else {
                Vec::new()
            };
            let mut host = MarkdownHost {
                footnote_number: Some(&footnote),
                list_start: cache.list_start,
                follows_block,
                anchors,
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
        let target = match self.caret_y {
            Some(y) if y >= 0.0 && y + height <= view.size.y => None,
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
        let (drawn, _) = best?;
        let cache = self.rows.get(&drawn.key)?;
        let md = drawn.item.borrow::<Markdown>()?;
        let flat = md.text_flow.selection_point_to_char_index(cx, abs).unwrap_or(0);
        Some(cache.caret_at(flat))
    }

    /// The selection as markdown, repaired at its ends (markers open at the
    /// start and still open at the end closed), as the cut rules take it.
    fn selected_text(&self) -> String {
        let (start, end) = self.ordered_selection();
        if start == end {
            return String::new();
        }
        let Some((slice, events)) = self.blocks_around(start.line_index, end.line_index, 0, 0).and_then(|lines| self.parse(lines)) else {
            return String::new();
        };
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
    fn apply_plan(&mut self, cx: &mut Cx, slice: &Slice, plan: &Plan, kind: EditKind) {
        let after = plan.apply(&slice.text);
        let head = &after[..plan.caret.min(after.len())];
        let caret = Position {
            line_index: slice.first_line + head.matches('\n').count(),
            byte_index: head.len() - head.rfind('\n').map_or(0, |at| at + 1),
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
        self.caret = self.clamp(caret);
        self.anchor = self.caret;
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
    /// it started. None without a selection.
    fn delete_selection(&mut self, cx: &mut Cx) -> Option<(EditKind, String)> {
        let (start, end) = self.ordered_selection();
        if start == end {
            return None;
        }
        let lines = self.blocks_around(start.line_index, end.line_index, 0, 0)?;
        let (slice, events) = self.parse(lines)?;
        let range = Self::slice_offset(&slice, start)..Self::slice_offset(&slice, end);
        let (plan, fragment) = plan::cut(&slice.text, &events, range);
        let kind = self.new_group();
        self.apply_plan(cx, &slice, &plan, kind);
        Some((kind, fragment))
    }

    /// Types `typed` at the caret, over the selection. Pasted text keeps the
    /// caret's container: each line after the first takes its prefix.
    fn type_text(&mut self, cx: &mut Cx, typed: &str, was_paste: bool) {
        let kind = match self.delete_selection(cx) {
            Some((group, _)) => group,
            None if was_paste => EditKind::Other,
            None if typed == " " => EditKind::InsertSpace,
            None => EditKind::Insert,
        };
        let Some(lines) = self.blocks_around(self.caret.line_index, self.caret.line_index, 0, 0) else { return };
        let start = self.caret;
        // Only a table cell (`|`), a fence (backticks, tildes), spaces at a
        // line's start and a paste into a container type differently: other
        // text needs no parse.
        let (slice, planned) = if typed.contains(['|', '`', '~', '\n']) || typed.trim().is_empty() {
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
        self.apply_plan(cx, &slice, &planned, kind);
        self.last_input = Some((start, self.caret));
    }

    /// Enter (a new block, or one container level out), or with `hard` a
    /// line break inside the block.
    fn enter(&mut self, cx: &mut Cx, hard: bool) {
        let kind = self.delete_selection(cx).map_or(EditKind::Other, |(group, _)| group);
        let Some(lines) = self.blocks_around(self.caret.line_index, self.caret.line_index, 0, 0) else { return };
        let Some((slice, events)) = self.parse(lines) else { return };
        let at = Self::slice_offset(&slice, self.caret);
        let plan = if hard {
            plan::hard_break(&slice.text, &events, at)
        } else {
            plan::enter(&slice.text, &events, at)
        };
        if let Some(plan) = plan {
            self.apply_plan(cx, &slice, &plan, kind);
        }
    }

    /// Backspace or forward Delete; `to` names how far: a character (None),
    /// or the caret's word or line edge (a position the keys found).
    fn delete(&mut self, cx: &mut Cx, forward: bool, to: Option<Position>) {
        if self.delete_selection(cx).is_some() {
            return;
        }
        if let Some(to) = to {
            if to != self.caret {
                self.anchor = to;
                self.delete_selection(cx);
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
            self.apply_plan(cx, &slice, &plan, EditKind::Delete);
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

    // Movement.

    /// The drawn row holding the caret's line, and its parse.
    fn caret_row(&self) -> Option<(&DrawnRow, &RowCache)> {
        let index = &self.doc.as_ref()?.index;
        if index.row_count() == 0 {
            return None;
        }
        let row = index.row_at_line(self.caret.line_index.min(index.line_count().saturating_sub(1)));
        let drawn = self.drawn.iter().find(|drawn| drawn.row == row)?;
        Some((drawn, self.rows.get(&drawn.key)?))
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
            KeyCode::ReturnKey => self.enter(cx, shift),
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
            _ => cx.widget_action(self.widget_uid(), MarkdownEditorAction::UnhandledKey(ke.clone())),
        }
    }
}

impl Widget for MarkdownEditor {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.sync(cx);
        self.view.handle_event(cx, event, scope);
        let area = self.view.area();
        // The list under the rows takes the press first (for its scroll bar
        // and wheel); the view shares it.
        match event.hits_with_capture_overload(cx, area, true) {
            Hit::FingerHoverIn(_) | Hit::FingerHoverOver(_) => {
                cx.set_cursor(MouseCursor::Text);
            }
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                cx.set_key_focus(area);
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
            Hit::FingerMove(fe) if self.selecting => {
                if let Some(position) = self.position_at(cx, fe.abs) {
                    if position != self.caret {
                        self.caret = position;
                        self.moved(cx);
                    }
                }
            }
            Hit::FingerUp(_) => self.selecting = false,
            Hit::TextCopy(ce) => {
                *ce.response.borrow_mut() = Some(self.selected_text());
            }
            Hit::TextCut(ce) => {
                if let Some((_, fragment)) = self.delete_selection(cx) {
                    *ce.response.borrow_mut() = Some(fragment);
                }
            }
            Hit::TextInput(te) => {
                // Compositions, replaced ranges and full-state syncs belong
                // to the soft keyboards' protocol; their committed text
                // arrives as plain input.
                if te.composition.is_some() || te.full_state_sync.is_some() || te.replace_range.is_some() {
                    return;
                }
                let input: String = te
                    .input
                    .replace("\r\n", "\n")
                    .chars()
                    .filter(|c| *c == '\n' || *c == '\t' || !c.is_control())
                    .collect();
                if input.is_empty() {
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
        // A placement holds for the one draw that applied it.
        if let Some(place) = self.place.take() {
            self.place_caret_y = Some(place.y);
        }
        let draws = self.draws;
        self.rows.retain(|_, cache| cache.drawn_in == draws);
        self.snap_caret();
        self.draw_caret(cx);
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
