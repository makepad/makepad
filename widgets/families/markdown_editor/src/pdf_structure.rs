//! The structure tree of a Markdown document's PDF. A page's first draw walks the events of
//! each row it draws: the block elements are made in document order (once, though a row
//! drawn on two pages is walked on both), and each leaf element notes where its lines drew.
//! On the writer's worker each text line of the page goes to the leaf it lies in.
//!
//! An element holds either child elements or text, never both: text that sits directly in a
//! list item, a quote or a note goes into a paragraph of its own, and a block that holds a
//! formula holds its text in spans beside the formula elements. A paragraph that is one
//! display formula is that formula.

use makepad_markdown_edit::pulldown_cmark::{Event as MdEvent, Tag, TagEnd};
use makepad_pdf_write::{
    ArtifactKind, ListNumbering, PdfWriter, Rect as PdfRect, RunTag, StructAttrs, StructId, StructKind, WritingMode,
};
use makepad_widgets_core::Rect;
use std::{collections::HashMap, ops::Range};

/// An element before the writer has it.
struct ElementSpec {
    parent: Option<usize>,
    kind: StructKind,
    attrs: StructAttrs,
}

/// What a page's first draw found: the elements made on it and where its leaves' lines are.
#[derive(Default)]
pub(crate) struct PageStructure {
    /// The index of the first element made on this page.
    first: usize,
    elements: Vec<ElementSpec>,
    /// Each line rect of each leaf, in page points, with the row (its index in `rows`) it is in.
    leaves: Vec<(usize, usize, PdfRect)>,
    /// Where a list item's marker draws: left of its body's first line.
    labels: Vec<(usize, usize, PdfRect)>,
    /// The bands of the rows drawn on the page.
    rows: Vec<(f32, f32)>,
}

impl PageStructure {
    /// A row drawn from `top` to `bottom` (page points).
    pub(crate) fn add_row(&mut self, top: f64, bottom: f64) {
        self.rows.push((top as f32, bottom as f32));
    }

    /// Hands the page's new elements to the writer (`ids` holds the writer's id of every element
    /// so far) and answers where the page's lines go.
    pub(crate) fn resolve(self, writer: &mut PdfWriter, ids: &mut Vec<StructId>) -> PageTags {
        assert_eq!(self.first, ids.len(), "a page's elements follow those of the pages before it");
        for spec in self.elements {
            let parent = spec.parent.map(|parent| ids[parent]);
            ids.push(writer.add_struct(parent, spec.kind, spec.attrs));
        }
        PageTags {
            leaves: self.leaves.into_iter().map(|(element, row, rect)| (ids[element], row, rect)).collect(),
            labels: self.labels.into_iter().map(|(element, row, rect)| (ids[element], row, rect)).collect(),
            rows: self.rows,
        }
    }
}

/// Where a page's text lines go, once the writer has its elements.
pub(crate) struct PageTags {
    leaves: Vec<(StructId, usize, PdfRect)>,
    labels: Vec<(StructId, usize, PdfRect)>,
    rows: Vec<(f32, f32)>,
}

impl PageTags {
    /// The tag of a text line whose first glyph's middle is at (`x`, `y`). Outside every row it
    /// is the page's header or footer. In a row it goes to the leaf of that row whose line holds
    /// it, an item's label beside its body, or else the row's nearest leaf (text a row draws
    /// beside its lines, such as a footnote's number). A leaf's lines past the page's edge (a
    /// paragraph that goes on over the next page) never take the footer's text.
    pub(crate) fn tag(&self, x: f32, y: f32) -> RunTag {
        let Some(row) = self.rows.iter().position(|&(top, bottom)| y >= top && y < bottom) else {
            let above = self.rows.first().is_some_and(|&(top, _)| y < top);
            return RunTag::Artifact(if above { ArtifactKind::Header } else { ArtifactKind::Footer });
        };
        let inside = |rect: &PdfRect| x >= rect.x && x < rect.x + rect.w && y >= rect.y && y < rect.y + rect.h;
        let in_row = |list: &[(StructId, usize, PdfRect)]| {
            list.iter().find(|(_, at, rect)| *at == row && inside(rect)).map(|(id, _, _)| *id)
        };
        if let Some(id) = in_row(&self.leaves).or_else(|| in_row(&self.labels)) {
            return RunTag::Element(id);
        }
        let distance = |rect: &PdfRect| {
            let dy = (rect.y - y).max(y - (rect.y + rect.h)).max(0.0);
            let dx = (rect.x - x).max(x - (rect.x + rect.w)).max(0.0);
            dy * 1000.0 + dx
        };
        let nearest = |row: Option<usize>| {
            self.leaves
                .iter()
                .filter(|(_, at, _)| row.is_none_or(|row| *at == row))
                .min_by(|a, b| distance(&a.2).total_cmp(&distance(&b.2)))
                .map(|(id, _, _)| RunTag::Element(*id))
        };
        nearest(Some(row)).or_else(|| nearest(None)).unwrap_or(RunTag::None)
    }
}

/// An element's identity across the walks of its rows: a list or a code block shared by the
/// rows of its items or chunks, an element made for an event of a row, or one made beside it.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Key {
    Root,
    Block(usize),
    Event(usize, usize),
    Part(usize, usize, u8),
}

/// Makes the document's elements as its pages are first drawn.
#[derive(Default)]
pub(crate) struct StructureBuilder {
    count: usize,
    keys: HashMap<Key, usize>,
}

/// A container open while a row is walked.
struct Open {
    element: usize,
    kind: OpenKind,
}

enum OpenKind {
    Plain,
    /// Inline text in it goes into a paragraph of its own (a quote, a note, a definition).
    Flow,
    /// A list item: its label, its body once something is in it, and whether its marker's
    /// place is known.
    Item { label: usize, body: Option<usize>, placed: bool },
    /// A table: its body once a body row starts.
    Table { body: Option<usize> },
    /// A table's header row: its cells are header cells.
    Head,
}

/// The leaf being walked: its element, or the one made at its end with its place (a picture,
/// a formula, or a span of a block's text beside its formulas), and its source range.
struct Leaf {
    element: Option<usize>,
    pending: Option<Pending>,
    range: Range<usize>,
    /// The block whose text goes into spans beside its formulas, once it holds one.
    holder: Option<usize>,
}

/// An element made when its leaf ends: a figure or a formula gets its place as its box, and a
/// span is made only where its text drew.
struct Pending {
    key: Key,
    parent: usize,
    kind: StructKind,
    attrs: StructAttrs,
}

/// One row's walk.
pub(crate) struct RowWalk<'a> {
    builder: &'a mut StructureBuilder,
    page: &'a mut PageStructure,
    page_index: usize,
    row: usize,
    block: usize,
    left: f64,
    rects: &'a dyn Fn(usize, usize) -> Vec<Rect>,
    stack: Vec<Open>,
    leaf: Option<Leaf>,
}

impl StructureBuilder {
    /// The structure of page `index`, which starts empty.
    pub(crate) fn begin_page(&self) -> PageStructure {
        PageStructure { first: self.count, ..PageStructure::default() }
    }

    /// The elements made so far.
    pub(crate) fn element_count(&self) -> usize {
        self.count
    }

    /// Walks row `row` (of block `block`) as drawn on page `page_index`: `rects` gives where a
    /// source range of its events drew, in page points, and `left` is the content's left edge.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn walk_row(
        &mut self,
        page: &mut PageStructure,
        page_index: usize,
        row: usize,
        block: usize,
        left: f64,
        events: &[(MdEvent<'static>, Range<usize>)],
        rects: &dyn Fn(usize, usize) -> Vec<Rect>,
    ) {
        let mut walk = RowWalk { builder: self, page, page_index, row, block, left, rects, stack: Vec::new(), leaf: None };
        for (ordinal, (event, range)) in events.iter().enumerate() {
            walk.event(events, ordinal, event, range);
        }
        walk.end_leaf();
    }

    fn element(&mut self, page: &mut PageStructure, key: Key, parent: Option<usize>, kind: StructKind, attrs: StructAttrs) -> usize {
        if let Some(&element) = self.keys.get(&key) {
            return element;
        }
        let element = self.count;
        self.count += 1;
        self.keys.insert(key, element);
        debug_assert_eq!(page.first + page.elements.len(), element, "elements are made on the page that first draws them");
        page.elements.push(ElementSpec { parent, kind, attrs });
        element
    }
}

impl RowWalk<'_> {
    fn event(&mut self, events: &[(MdEvent<'static>, Range<usize>)], ordinal: usize, event: &MdEvent<'static>, range: &Range<usize>) {
        match event {
            MdEvent::Start(tag) => self.start(events, ordinal, tag, range),
            MdEvent::End(tag) => self.end(tag),
            MdEvent::Rule => self.end_leaf(),
            MdEvent::InlineMath(tex) => self.formula(ordinal, range, tex, false),
            MdEvent::DisplayMath(tex) => self.formula(ordinal, range, tex, true),
            // Inline content: text, code spans, breaks, footnote marks, task boxes.
            _ => self.inline(ordinal, range),
        }
    }

    fn start(&mut self, events: &[(MdEvent<'static>, Range<usize>)], ordinal: usize, tag: &Tag<'static>, range: &Range<usize>) {
        let leaf = |kind: StructKind| (kind, StructAttrs::default());
        match tag {
            Tag::Paragraph => {
                if let Some(alt) = picture_only(events, ordinal) {
                    // A paragraph that is one picture is a figure, made at its end with its place.
                    self.end_leaf();
                    let key = Key::Event(self.row, ordinal);
                    let parent = self.block_parent(ordinal);
                    let attrs = StructAttrs { alt: (!alt.is_empty()).then_some(alt), ..StructAttrs::default() };
                    let pending = Pending { key, parent, kind: StructKind::Figure, attrs };
                    self.leaf = Some(Leaf { element: None, pending: Some(pending), range: range.clone(), holder: None });
                } else if let Some(tex) = formula_only(events, ordinal) {
                    // A paragraph that is one display formula is that formula.
                    self.end_leaf();
                    let key = Key::Event(self.row, ordinal);
                    let parent = self.block_parent(ordinal);
                    let attrs = StructAttrs { alt: Some(tex), block: true, ..StructAttrs::default() };
                    let pending = Pending { key, parent, kind: StructKind::Formula, attrs };
                    self.leaf = Some(Leaf { element: None, pending: Some(pending), range: range.clone(), holder: None });
                } else {
                    self.open_leaf(ordinal, range, leaf(StructKind::P));
                }
            }
            Tag::Heading { level, .. } => self.open_leaf(ordinal, range, leaf(StructKind::heading(*level as u8))),
            Tag::CodeBlock(_) => {
                self.open_leaf(ordinal, range, (StructKind::Code, StructAttrs { block: true, ..StructAttrs::default() }))
            }
            Tag::HtmlBlock | Tag::MetadataBlock(_) | Tag::DefinitionListTitle => {
                let kind = if matches!(tag, Tag::DefinitionListTitle) { StructKind::P } else { StructKind::Div };
                self.open_leaf(ordinal, range, leaf(kind))
            }
            Tag::TableCell => {
                let head = matches!(self.stack.last(), Some(Open { kind: OpenKind::Head, .. }));
                let kind = if head { StructKind::TH } else { StructKind::TD };
                self.open_leaf(ordinal, range, (kind, StructAttrs { scope_column: head, ..StructAttrs::default() }));
            }
            Tag::BlockQuote(_) => self.open(ordinal, StructKind::BlockQuote, StructAttrs::default(), OpenKind::Flow),
            Tag::FootnoteDefinition(_) => self.open(ordinal, StructKind::Note, StructAttrs::default(), OpenKind::Flow),
            Tag::DefinitionList => self.open(ordinal, StructKind::Div, StructAttrs::default(), OpenKind::Plain),
            Tag::DefinitionListDefinition => self.open(ordinal, StructKind::Div, StructAttrs::default(), OpenKind::Flow),
            Tag::List(first) => {
                let numbering = if first.is_some() { ListNumbering::Decimal } else { ListNumbering::Disc };
                self.open(ordinal, StructKind::L, StructAttrs { list_numbering: Some(numbering), ..StructAttrs::default() }, OpenKind::Plain);
            }
            Tag::Item => {
                self.open(ordinal, StructKind::LI, StructAttrs::default(), OpenKind::Plain);
                let item = self.stack.last().map(|open| open.element);
                let label = self.builder.element(self.page, Key::Part(self.row, ordinal, 0), item, StructKind::Lbl, StructAttrs::default());
                if let Some(open) = self.stack.last_mut() {
                    open.kind = OpenKind::Item { label, body: None, placed: false };
                }
            }
            Tag::Table(_) => self.open(ordinal, StructKind::Table, StructAttrs::default(), OpenKind::Table { body: None }),
            Tag::TableHead => {
                self.open(ordinal, StructKind::THead, StructAttrs::default(), OpenKind::Plain);
                let head = self.stack.last().map(|open| open.element);
                let tr = self.builder.element(self.page, Key::Part(self.row, ordinal, 0), head, StructKind::TR, StructAttrs::default());
                self.stack.push(Open { element: tr, kind: OpenKind::Head });
            }
            Tag::TableRow => {
                self.end_leaf();
                let body = self.table_body(ordinal);
                let tr = self.builder.element(self.page, Key::Event(self.row, ordinal), body, StructKind::TR, StructAttrs::default());
                self.stack.push(Open { element: tr, kind: OpenKind::Plain });
            }
            // Inline tags: links, emphasis, strong, strike-through, a picture among text.
            _ => self.inline(ordinal, range),
        }
    }

    fn end(&mut self, tag: &TagEnd) {
        match tag {
            TagEnd::Paragraph
            | TagEnd::Heading(_)
            | TagEnd::CodeBlock
            | TagEnd::HtmlBlock
            | TagEnd::MetadataBlock(_)
            | TagEnd::DefinitionListTitle
            | TagEnd::TableCell => self.end_leaf(),
            TagEnd::TableHead => {
                self.end_leaf();
                self.stack.pop();
                self.stack.pop();
            }
            TagEnd::BlockQuote(_)
            | TagEnd::FootnoteDefinition
            | TagEnd::DefinitionList
            | TagEnd::DefinitionListDefinition
            | TagEnd::List(_)
            | TagEnd::Item
            | TagEnd::Table
            | TagEnd::TableRow => {
                self.end_leaf();
                self.stack.pop();
            }
            _ => {}
        }
    }

    /// Text in the open leaf, or in a paragraph of its own where it sits in a container.
    fn inline(&mut self, ordinal: usize, range: &Range<usize>) {
        if let Some(leaf) = &mut self.leaf {
            leaf.range.start = leaf.range.start.min(range.start);
            leaf.range.end = leaf.range.end.max(range.end);
            return;
        }
        let parent = self.block_parent(ordinal);
        let element = self.builder.element(self.page, Key::Part(self.row, ordinal, 2), Some(parent), StructKind::P, StructAttrs::default());
        self.leaf = Some(Leaf { element: Some(element), pending: None, range: range.clone(), holder: None });
    }

    /// A formula, with its TeX source (without the lines around it) as its /Alt: its own element in the block it is in. The
    /// block's text before it becomes a span, and its text after it goes into the next.
    fn formula(&mut self, ordinal: usize, range: &Range<usize>, tex: &str, display: bool) {
        if self.leaf.is_none() {
            // Text right in a container (a tight list's item) goes into a paragraph of its own.
            let parent = self.block_parent(ordinal);
            let element = self.builder.element(self.page, Key::Part(self.row, ordinal, 2), Some(parent), StructKind::P, StructAttrs::default());
            self.leaf = Some(Leaf { element: Some(element), pending: None, range: range.start..range.start, holder: None });
        }
        let Some(leaf) = &mut self.leaf else { return };
        let holder = match (leaf.holder, leaf.element, &leaf.pending) {
            (Some(holder), _, _) => holder,
            (None, Some(block), _) => {
                // The block's text so far is its first span.
                leaf.element = None;
                leaf.pending = Some(Pending { key: Key::Part(self.row, ordinal, 3), parent: block, kind: StructKind::Span, attrs: StructAttrs::default() });
                leaf.holder = Some(block);
                block
            }
            // The paragraph that is this formula, or a formula in a picture's description.
            (None, None, _) => {
                leaf.range.start = leaf.range.start.min(range.start);
                leaf.range.end = leaf.range.end.max(range.end);
                return;
            }
        };
        leaf.range.end = leaf.range.end.min(range.start);
        self.end_leaf();
        let attrs = StructAttrs { alt: Some(tex.trim().to_string()), block: display, ..StructAttrs::default() };
        let pending = Pending { key: Key::Event(self.row, ordinal), parent: holder, kind: StructKind::Formula, attrs };
        self.leaf = Some(Leaf { element: None, pending: Some(pending), range: range.clone(), holder: Some(holder) });
        self.end_leaf();
        let pending = Pending { key: Key::Part(self.row, ordinal, 4), parent: holder, kind: StructKind::Span, attrs: StructAttrs::default() };
        self.leaf = Some(Leaf { element: None, pending: Some(pending), range: range.end..range.end, holder: Some(holder) });
    }

    /// The key of an element made for event `ordinal`: a list or code block at the row's top is
    /// shared by the rows of its block.
    fn key(&self, ordinal: usize) -> Key {
        if self.stack.is_empty() { Key::Block(self.block) } else { Key::Event(self.row, ordinal) }
    }

    fn open(&mut self, ordinal: usize, kind: StructKind, attrs: StructAttrs, open: OpenKind) {
        self.end_leaf();
        let key = match kind {
            StructKind::L if self.stack.is_empty() => Key::Block(self.block),
            _ => Key::Event(self.row, ordinal),
        };
        let parent = self.block_parent(ordinal);
        let element = self.builder.element(self.page, key, Some(parent), kind, attrs);
        self.stack.push(Open { element, kind: open });
    }

    fn open_leaf(&mut self, ordinal: usize, range: &Range<usize>, (kind, attrs): (StructKind, StructAttrs)) {
        self.end_leaf();
        let key = match kind {
            StructKind::Code => self.key(ordinal),
            _ => Key::Event(self.row, ordinal),
        };
        let parent = self.block_parent(ordinal);
        let element = self.builder.element(self.page, key, Some(parent), kind, attrs);
        self.leaf = Some(Leaf { element: Some(element), pending: None, range: range.clone(), holder: None });
    }

    /// Where a block goes: the open container (a list item's body), or the document.
    fn block_parent(&mut self, ordinal: usize) -> usize {
        // The view lays out every paragraph left to right (its base direction); its leaves inherit
        // the writing mode (Layout attributes inherit, 14.8.5.4).
        let attrs = StructAttrs { writing_mode: Some(WritingMode::LrTb), ..StructAttrs::default() };
        let root = self.builder.element(self.page, Key::Root, None, StructKind::Document, attrs);
        let Some(open) = self.stack.last_mut() else { return root };
        match &mut open.kind {
            OpenKind::Item { body: Some(body), .. } => *body,
            OpenKind::Item { body, .. } => {
                let item = open.element;
                let made = self.builder.element(self.page, Key::Part(self.row, ordinal, 1), Some(item), StructKind::LBody, StructAttrs::default());
                *body = Some(made);
                made
            }
            _ => open.element,
        }
    }

    /// The open table's body, made when its first body row starts.
    fn table_body(&mut self, ordinal: usize) -> Option<usize> {
        let open = self.stack.iter_mut().rev().find(|open| matches!(open.kind, OpenKind::Table { .. }))?;
        let table = open.element;
        let OpenKind::Table { body } = &mut open.kind else { return None };
        if body.is_none() {
            *body = Some(self.builder.element(self.page, Key::Part(self.row, ordinal, 1), Some(table), StructKind::TBody, StructAttrs::default()));
        }
        *body
    }

    /// Ends the open leaf: notes where its lines drew, makes a figure or a formula with its
    /// place (a span only where its text drew), and gives the innermost list item without a
    /// marker place its marker's band.
    fn end_leaf(&mut self) {
        let Some(leaf) = self.leaf.take() else { return };
        // The row being walked is the page's last band (it is added before the row draws).
        let row = self.page.rows.len().saturating_sub(1);
        let rects: Vec<PdfRect> = (self.rects)(leaf.range.start, leaf.range.end)
            .into_iter()
            .filter(|rect| rect.size.x > 0.0 && rect.size.y > 0.0)
            .map(|rect| PdfRect { x: rect.pos.x as f32, y: rect.pos.y as f32, w: rect.size.x as f32, h: rect.size.y as f32 })
            .collect();
        let element = match (leaf.element, leaf.pending) {
            (Some(element), _) => element,
            (None, Some(Pending { kind: StructKind::Span, .. })) if rects.is_empty() => return,
            (None, Some(Pending { key, parent, kind, mut attrs })) => {
                if kind != StructKind::Span {
                    let bbox = rects.iter().copied().reduce(|a, b| {
                        let (x0, y0) = (a.x.min(b.x), a.y.min(b.y));
                        let (x1, y1) = ((a.x + a.w).max(b.x + b.w), (a.y + a.h).max(b.y + b.h));
                        PdfRect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 }
                    });
                    attrs.bbox = bbox.map(|rect| (self.page_index, rect));
                }
                self.builder.element(self.page, key, Some(parent), kind, attrs)
            }
            (None, None) => return,
        };
        if let (Some(first), Some(Open { kind: OpenKind::Item { label, placed, .. }, .. })) =
            (rects.first(), self.stack.iter_mut().rev().find(|open| matches!(open.kind, OpenKind::Item { .. })))
        {
            if !*placed {
                *placed = true;
                let x = self.left as f32;
                self.page.labels.push((*label, row, PdfRect { x, y: first.y, w: (first.x - x).max(0.0), h: first.h }));
            }
        }
        self.page.leaves.extend(rects.into_iter().map(|rect| (element, row, rect)));
    }
}

/// The TeX source of a paragraph whose only content is one display formula.
fn formula_only(events: &[(MdEvent<'static>, Range<usize>)], paragraph: usize) -> Option<String> {
    let MdEvent::DisplayMath(tex) = &events.get(paragraph + 1)?.0 else { return None };
    matches!(events.get(paragraph + 2)?.0, MdEvent::End(TagEnd::Paragraph)).then(|| tex.trim().to_string())
}

/// The alt text of a paragraph whose only content is one picture.
fn picture_only(events: &[(MdEvent<'static>, Range<usize>)], paragraph: usize) -> Option<String> {
    let MdEvent::Start(Tag::Image { .. }) = &events.get(paragraph + 1)?.0 else { return None };
    let mut alt = String::new();
    let mut at = paragraph + 2;
    loop {
        match &events.get(at)?.0 {
            MdEvent::End(TagEnd::Image) => break,
            MdEvent::Text(text) | MdEvent::Code(text) => alt.push_str(text),
            _ => {}
        }
        at += 1;
    }
    matches!(events.get(at + 1)?.0, MdEvent::End(TagEnd::Paragraph)).then_some(alt)
}
