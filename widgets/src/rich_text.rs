//! RichTextEditor — a document of blocks and marked runs, drawn and typed
//! into.
//!
//! The document model is the widget. A document is a list of blocks; a block
//! is a kind — paragraph, heading, bullet, numbered, quote, code — and a list
//! of runs; a run is a string and the set of marks over it: bold, italic,
//! underline, strikethrough, inline code, and a link with a target. Every
//! edit is a method on that model, so applying a mark over a selection,
//! splitting a block or merging two of them can be read and tested without a
//! window: [`RichDoc`] and everything under it hold no `Cx`, no ids and no
//! draw state, and the tests at the bottom of this file exercise them
//! directly.
//!
//! # Runs split and merge; they are never a list of styled characters
//!
//! Applying bold to the middle of one run leaves three runs, and taking it
//! off the middle of a bold run leaves three again. The other school keeps a
//! style per character and rebuilds the runs when it draws; it is simpler to
//! write and it loses the one thing worth having — the invariant that two
//! adjacent runs never carry the same marks. That invariant is what makes
//! "did the whole selection have this mark" a cheap question, what keeps the
//! model from growing a run per keystroke, and what makes the tests below
//! short enough to read. [`Block::normalize`] restores it after every edit.
//!
//! A link is a mark with a target rather than a sixth flag, so two links
//! side by side stay two links: runs merge only when their marks — target
//! included — are equal.
//!
//! # It draws through the flow and does not lay out text itself
//!
//! Drawing is [`TextFlow`]: the same engine the markdown widget uses, driven
//! a block at a time. Every run is pushed through `draw_text` with the flow's
//! style counters set, and the flow's own text tracker is what turns a click
//! into a character index. The map from that flat index back into the
//! document is built during the draw, one entry per drawn piece, so nothing
//! here needs to know how the flow wraps, indents or numbers anything.
//!
//! The caret is drawn INSIDE the flow: the run it sits in is drawn in two
//! halves with a thin quad walked between them. The flow therefore puts the
//! caret exactly where the next glyph would go, on the right line, with no
//! geometry arithmetic here at all.
//!
//! # What this deliberately does not do
//!
//! * **No IME composition.** Typed text arrives as `Hit::TextInput`; the
//!   composition, replace-range and full-state-sync paths that the phone
//!   keyboards use are ignored rather than half-handled, so this edits from
//!   a hardware keyboard and not from a soft one.
//! * **No caret blink.** The caret is solid while the widget has focus.
//! * **Plain text on the clipboard.** Copy hands out the selection as text
//!   and paste puts text in carrying the caret's marks; the marks themselves
//!   do not travel.
//! * **Six block kinds and five marks.** No images, tables, rules, nested
//!   lists, task lists or footnotes. `set_kind` on a block is the whole of
//!   block editing.
//! * **A link is coloured underlined text, not a child widget.** In an
//!   editable document a click on one puts the caret in it, which is what an
//!   editor must do; a read-only one reports the click instead.
//! * A run that is both struck through and underlined draws only the strike:
//!   the flow draws one decoration per run, and this picks the stronger one
//!   rather than fighting for both.
use crate::{makepad_derive_widget::*, makepad_draw::*, text_flow::TextFlow, widget::*};
use pulldown_cmark::{Event as MdEvent, HeadingLevel, Options, Parser, Tag, TagEnd};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.RichTextEditorBase = #(RichTextEditor::register_widget(vm))

    /** Text with marks and block kinds, edited in place. */
    mod.widgets.RichTextEditor = set_type_default() do mod.widgets.RichTextEditorBase{
        width: Fill
        height: Fit
        flow: Flow.Right{wrap: true}
        padding: theme.mspace_1

        /** off draws the same document and takes no keystrokes */
        editable: true

        font_size: theme.font_size_p
        font_color: theme.color_text

        /** room between two blocks in pixels 0..48 step 1 */
        block_spacing: 12.
        /** the largest heading, as a multiple of the body size 1..3 step 0.05 */
        heading_scale: 1.8
        /** caret thickness in pixels 1..4 step 0.5 */
        caret_width: 1.5
        /** caret height as a multiple of the font size 1..2.5 step 0.05 */
        caret_height: 1.55

        link_color: theme.color_primary

        draw_caret +: {
            // The caret is the same ink as the text: a theme where the two
            // disagree has an invisible caret on one of its backgrounds.
            color: theme.color_text
        }

        draw_selection +: {
            color: theme.color_selection_focus
        }

        draw_text +: {
            color: theme.color_text
            extend_area: true
        }

        text_style_normal: theme.font_regular{
            font_size: theme.font_size_p
        }

        text_style_italic: theme.font_italic{
            font_size: theme.font_size_p
        }

        text_style_bold: theme.font_bold{
            font_size: theme.font_size_p
        }

        text_style_bold_italic: theme.font_bold_italic{
            font_size: theme.font_size_p
        }

        text_style_fixed: theme.font_code{
            font_size: theme.font_size_p
        }

        code_layout: Layout{
            flow: Flow.Right{wrap: true}
            padding: Inset{left: theme.space_3, right: theme.space_3, top: theme.space_2, bottom: theme.space_2}
        }
        code_walk: Walk{width: Fill, height: Fit}

        quote_layout: Layout{
            flow: Flow.Right{wrap: true}
            padding: Inset{left: theme.space_3, right: theme.space_3, top: theme.space_2, bottom: theme.space_2}
        }
        quote_walk: Walk{width: Fill, height: Fit}

        list_item_layout: Layout{
            flow: Flow.Right{wrap: true}
            padding: theme.mspace_1
        }
        list_item_walk: Walk{width: Fill, height: Fit}

        inline_code_padding: theme.mspace_1
        inline_code_margin: theme.mspace_1

        // No `draw_block +:` here. It reaches the flow through `#[deref]` in
        // Rust, but the DSL type-checks against the base this preset derives
        // from, which does not carry it — the merge was refused at runtime,
        // where only the log could see it. Every value it set was the flow's
        // own default anyway.
    }

    /** The same document with the typing turned off: a page that renders it. */
    mod.widgets.RichTextView = set_type_default() do mod.widgets.RichTextEditor{
        editable: false
    }
}

// ---------------------------------------------------------------------------
// The document model. No Cx, no ids, no draw state — see the tests below.
// ---------------------------------------------------------------------------

/// One of the five marks a run either carries or does not. A link is not in
/// here because it carries a target; see [`RichMarks::link`].
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Mark {
    Bold,
    Italic,
    Underline,
    Strike,
    Code,
}

impl Mark {
    /// Every mark in the order a toolbar shows them.
    pub const ALL: [Mark; 5] = [
        Mark::Bold,
        Mark::Italic,
        Mark::Underline,
        Mark::Strike,
        Mark::Code,
    ];

    fn bit(self) -> u8 {
        1 << (self as u8)
    }
}

/// The marks over one run. Two runs merge only when these compare equal, so
/// the link target is part of the identity: two links that happen to sit
/// side by side stay two runs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RichMarks {
    bits: u8,
    link: Option<String>,
}

impl RichMarks {
    pub fn none() -> RichMarks {
        RichMarks::default()
    }

    /// A set built from a list, for callers and tests that want one line.
    pub fn of(marks: &[Mark]) -> RichMarks {
        let mut m = RichMarks::default();
        for mark in marks {
            m.add(*mark);
        }
        m
    }

    pub fn has(&self, mark: Mark) -> bool {
        self.bits & mark.bit() != 0
    }

    pub fn add(&mut self, mark: Mark) {
        self.bits |= mark.bit();
    }

    pub fn remove(&mut self, mark: Mark) {
        self.bits &= !mark.bit();
    }

    pub fn set(&mut self, mark: Mark, on: bool) {
        if on {
            self.add(mark)
        } else {
            self.remove(mark)
        }
    }

    pub fn toggle(&mut self, mark: Mark) {
        let on = self.has(mark);
        self.set(mark, !on);
    }

    pub fn with(mut self, mark: Mark) -> RichMarks {
        self.add(mark);
        self
    }

    pub fn link(&self) -> Option<&str> {
        self.link.as_deref()
    }

    pub fn set_link(&mut self, href: Option<&str>) {
        self.link = href.map(|h| h.to_string());
    }

    pub fn with_link(mut self, href: &str) -> RichMarks {
        self.set_link(Some(href));
        self
    }

    /// The same marks with the link taken off. What typing at the right edge
    /// of a link inherits: text written after a link is not part of it.
    pub fn without_link(&self) -> RichMarks {
        RichMarks {
            bits: self.bits,
            link: None,
        }
    }

    pub fn is_plain(&self) -> bool {
        self.bits == 0 && self.link.is_none()
    }
}

/// A piece of text and the marks over the whole of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RichRun {
    pub text: String,
    pub marks: RichMarks,
}

impl RichRun {
    pub fn new(text: &str, marks: RichMarks) -> RichRun {
        RichRun {
            text: text.to_string(),
            marks,
        }
    }

    pub fn plain(text: &str) -> RichRun {
        RichRun::new(text, RichMarks::none())
    }
}

/// What a block is. A heading carries its level, 1 to 3; deeper headings are
/// clamped, because a page that needs six sizes of title needs an outline
/// rather than a fourth size.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BlockKind {
    Paragraph,
    Heading(u8),
    Bullet,
    Numbered,
    Quote,
    Code,
}

impl BlockKind {
    /// A kind that goes on by itself: pressing Return at the end of one makes
    /// another, and pressing it on an empty one ends the run of them.
    pub fn continues(self) -> bool {
        matches!(self, BlockKind::Bullet | BlockKind::Numbered | BlockKind::Quote)
    }

    pub fn is_list(self) -> bool {
        matches!(self, BlockKind::Bullet | BlockKind::Numbered)
    }

    /// The kinds the two halves of a split take.
    ///
    /// A heading is a title, and the thing written after a title is body
    /// text — so the tail of a split heading is a paragraph. Splitting at the
    /// very head of one is the other gesture entirely: it pushes the heading
    /// down and leaves a paragraph above it.
    fn split_kinds(self, at_start: bool) -> (BlockKind, BlockKind) {
        match self {
            BlockKind::Heading(_) if at_start => (BlockKind::Paragraph, self),
            BlockKind::Heading(_) => (self, BlockKind::Paragraph),
            kind => (kind, kind),
        }
    }
}

/// One block: a kind, and the runs that make up its text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    pub kind: BlockKind,
    pub runs: Vec<RichRun>,
}

impl Block {
    pub fn new(kind: BlockKind) -> Block {
        Block {
            kind,
            runs: Vec::new(),
        }
    }

    pub fn plain(kind: BlockKind, text: &str) -> Block {
        let mut block = Block::new(kind);
        if !text.is_empty() {
            block.runs.push(RichRun::plain(text));
        }
        block
    }

    /// The block's text, the runs laid end to end.
    pub fn text(&self) -> String {
        let mut out = String::with_capacity(self.len());
        for run in &self.runs {
            out.push_str(&run.text);
        }
        out
    }

    /// In bytes, like every other offset in this model.
    pub fn len(&self) -> usize {
        self.runs.iter().map(|run| run.text.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The nearest character boundary at or before `byte`. Positions arrive
    /// here from hit tests and key presses and both can land inside a
    /// multi-byte character; slicing there would panic mid-draw.
    pub fn floor(&self, byte: usize) -> usize {
        let text = self.text();
        floor_boundary(&text, byte)
    }

    /// The index a run starting at `byte` would have, splitting a run in two
    /// if `byte` falls inside one.
    fn split_run_at(&mut self, byte: usize) -> usize {
        let mut acc = 0;
        for i in 0..self.runs.len() {
            let len = self.runs[i].text.len();
            if byte == acc {
                return i;
            }
            if byte < acc + len {
                let tail = self.runs[i].text.split_off(byte - acc);
                let marks = self.runs[i].marks.clone();
                self.runs.insert(i + 1, RichRun { text: tail, marks });
                return i + 1;
            }
            acc += len;
        }
        self.runs.len()
    }

    /// Restore the model's two invariants: no empty run, no two adjacent runs
    /// with the same marks — and no line break outside a code block, since a
    /// paragraph that swallowed one would draw its two halves as one line
    /// anyway and then disagree with its own byte offsets.
    pub fn normalize(&mut self) {
        if self.kind != BlockKind::Code {
            for run in &mut self.runs {
                if run.text.contains('\n') {
                    run.text = run.text.replace('\n', " ");
                }
            }
        }
        self.runs.retain(|run| !run.text.is_empty());
        let mut i = 1;
        while i < self.runs.len() {
            if self.runs[i].marks == self.runs[i - 1].marks {
                let text = std::mem::take(&mut self.runs[i].text);
                self.runs[i - 1].text.push_str(&text);
                self.runs.remove(i);
            } else {
                i += 1;
            }
        }
    }

    /// Change the marks of everything between two offsets, splitting the runs
    /// at both ends first and merging whatever comes out identical.
    pub fn apply(&mut self, from: usize, to: usize, f: &dyn Fn(&mut RichMarks)) {
        let from = self.floor(from);
        let to = self.floor(to);
        if from >= to {
            return;
        }
        let i = self.split_run_at(from);
        let j = self.split_run_at(to);
        for run in &mut self.runs[i..j] {
            f(&mut run.marks);
        }
        self.normalize();
    }

    pub fn insert(&mut self, byte: usize, text: &str, marks: &RichMarks) {
        if text.is_empty() {
            return;
        }
        let byte = self.floor(byte);
        let i = self.split_run_at(byte);
        self.runs.insert(
            i,
            RichRun {
                text: text.to_string(),
                marks: marks.clone(),
            },
        );
        self.normalize();
    }

    pub fn remove(&mut self, from: usize, to: usize) {
        let from = self.floor(from);
        let to = self.floor(to);
        if from >= to {
            return;
        }
        let i = self.split_run_at(from);
        let j = self.split_run_at(to);
        self.runs.drain(i..j);
        self.normalize();
    }

    /// Take everything from `byte` on out of the block.
    fn split_runs_off(&mut self, byte: usize) -> Vec<RichRun> {
        let byte = self.floor(byte);
        let i = self.split_run_at(byte);
        self.runs.split_off(i)
    }

    /// The run the byte sits in, leaning right at a boundary: what a click
    /// at that point landed on.
    pub fn run_at(&self, byte: usize) -> Option<&RichRun> {
        let mut acc = 0;
        for run in &self.runs {
            let end = acc + run.text.len();
            if byte < end {
                return Some(run);
            }
            acc = end;
        }
        self.runs.last()
    }
}

/// Where in the document: which block, and how many bytes into it. Ordered
/// the way the document reads.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Pos {
    pub block: usize,
    pub byte: usize,
}

impl Pos {
    pub const ZERO: Pos = Pos { block: 0, byte: 0 };

    pub fn new(block: usize, byte: usize) -> Pos {
        Pos { block, byte }
    }
}

/// A stretch of document, always in reading order.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Span {
    pub from: Pos,
    pub to: Pos,
}

impl Span {
    /// Two ends in either order.
    pub fn new(a: Pos, b: Pos) -> Span {
        if a <= b {
            Span { from: a, to: b }
        } else {
            Span { from: b, to: a }
        }
    }

    /// The empty span at a point — a caret.
    pub fn at(pos: Pos) -> Span {
        Span { from: pos, to: pos }
    }

    pub fn is_empty(&self) -> bool {
        self.from == self.to
    }
}

/// The document: a list of blocks, never empty.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RichDoc {
    pub blocks: Vec<Block>,
}

impl Default for RichDoc {
    fn default() -> RichDoc {
        RichDoc::new()
    }
}

impl RichDoc {
    /// One empty paragraph. A document with no blocks at all has no place to
    /// put the caret, so there is always one.
    pub fn new() -> RichDoc {
        RichDoc {
            blocks: vec![Block::new(BlockKind::Paragraph)],
        }
    }

    /// Nothing has been written in it yet.
    pub fn is_empty(&self) -> bool {
        self.blocks.iter().all(|block| block.is_empty())
    }

    fn settle(&mut self) {
        if self.blocks.is_empty() {
            self.blocks.push(Block::new(BlockKind::Paragraph));
        }
    }

    pub fn clamp(&self, pos: Pos) -> Pos {
        if self.blocks.is_empty() {
            return Pos::ZERO;
        }
        let block = pos.block.min(self.blocks.len() - 1);
        Pos {
            block,
            byte: self.blocks[block].floor(pos.byte),
        }
    }

    pub fn clamp_span(&self, span: Span) -> Span {
        Span::new(self.clamp(span.from), self.clamp(span.to))
    }

    /// The whole document, for a select-all.
    pub fn all(&self) -> Span {
        let last = self.blocks.len().saturating_sub(1);
        Span {
            from: Pos::ZERO,
            to: Pos::new(last, self.blocks.get(last).map(|b| b.len()).unwrap_or(0)),
        }
    }

    pub fn end(&self) -> Pos {
        self.all().to
    }

    /// The plain text of the whole document, one line per block.
    pub fn text(&self) -> String {
        self.text_in(self.all())
    }

    /// The plain text of a stretch of it.
    pub fn text_in(&self, span: Span) -> String {
        let span = self.clamp_span(span);
        if span.is_empty() {
            return String::new();
        }
        let mut out = String::new();
        for bi in span.from.block..=span.to.block {
            if bi > span.from.block {
                out.push('\n');
            }
            let text = self.blocks[bi].text();
            let lo = if bi == span.from.block { span.from.byte } else { 0 };
            let hi = if bi == span.to.block { span.to.byte } else { text.len() };
            out.push_str(&text[lo.min(text.len())..hi.min(text.len())]);
        }
        out
    }

    /// The run a point landed in — the one a link click reads.
    pub fn run_at(&self, pos: Pos) -> Option<&RichRun> {
        self.blocks.get(pos.block)?.run_at(pos.byte)
    }

    /// The marks the next character typed here should carry.
    ///
    /// Inside a run, that run's. At a boundary, the run to the LEFT decides,
    /// because that is the text the typing continues — except for the link,
    /// which is dropped: writing on after a link is not part of the link, and
    /// an editor that grew the link every time somebody typed past it would
    /// have to be fought.
    pub fn marks_at(&self, pos: Pos) -> RichMarks {
        let Some(block) = self.blocks.get(pos.block) else {
            return RichMarks::none();
        };
        if block.runs.is_empty() {
            return RichMarks::none();
        }
        let byte = block.floor(pos.byte);
        if byte == 0 {
            return block.runs[0].marks.without_link();
        }
        let mut acc = 0;
        for run in &block.runs {
            let end = acc + run.text.len();
            if byte < end {
                return run.marks.clone();
            }
            if byte == end {
                return run.marks.without_link();
            }
            acc = end;
        }
        block.runs[block.runs.len() - 1].marks.without_link()
    }

    /// Walk the marks of every piece of text the span covers.
    fn for_each_piece(&self, span: Span, f: &mut dyn FnMut(&RichMarks)) {
        let span = self.clamp_span(span);
        for bi in span.from.block..=span.to.block {
            let Some(block) = self.blocks.get(bi) else {
                break;
            };
            let lo = if bi == span.from.block { span.from.byte } else { 0 };
            let hi = if bi == span.to.block {
                span.to.byte
            } else {
                block.len()
            };
            let mut acc = 0;
            for run in &block.runs {
                let end = acc + run.text.len();
                if acc.max(lo) < end.min(hi) {
                    f(&run.marks);
                }
                acc = end;
            }
        }
    }

    /// Does everything in the span carry this mark? The question a toolbar
    /// button asks to decide whether it is pressed, and the one a toggle asks
    /// to decide which way to go. An empty span asks the caret instead.
    pub fn has_mark(&self, span: Span, mark: Mark) -> bool {
        if span.is_empty() {
            return self.marks_at(span.from).has(mark);
        }
        let mut any = false;
        let mut all = true;
        self.for_each_piece(span, &mut |marks: &RichMarks| {
            any = true;
            if !marks.has(mark) {
                all = false;
            }
        });
        any && all
    }

    /// The link every piece of the span shares, if they share one.
    pub fn link_in(&self, span: Span) -> Option<String> {
        if span.is_empty() {
            return self.run_at(span.from).and_then(|run| run.marks.link()).map(|h| h.to_string());
        }
        let mut first = true;
        let mut common: Option<String> = None;
        self.for_each_piece(span, &mut |marks: &RichMarks| {
            let here = marks.link().map(|h| h.to_string());
            if first {
                common = here;
                first = false;
            } else if common != here {
                common = None;
            }
        });
        common
    }

    fn apply(&mut self, span: Span, f: &dyn Fn(&mut RichMarks)) {
        let span = self.clamp_span(span);
        if span.is_empty() {
            return;
        }
        for bi in span.from.block..=span.to.block {
            let Some(block) = self.blocks.get_mut(bi) else {
                break;
            };
            let lo = if bi == span.from.block { span.from.byte } else { 0 };
            let hi = if bi == span.to.block {
                span.to.byte
            } else {
                block.len()
            };
            block.apply(lo, hi, f);
        }
    }

    pub fn set_mark(&mut self, span: Span, mark: Mark, on: bool) {
        self.apply(span, &|marks: &mut RichMarks| marks.set(mark, on));
    }

    /// Put the mark on unless the whole span already has it. Returns what the
    /// span carries afterwards.
    pub fn toggle_mark(&mut self, span: Span, mark: Mark) -> bool {
        let on = !self.has_mark(span, mark);
        self.set_mark(span, mark, on);
        on
    }

    /// Make the span a link, or take the link off it.
    pub fn set_link(&mut self, span: Span, href: Option<&str>) {
        self.apply(span, &|marks: &mut RichMarks| marks.set_link(href));
    }

    /// Change the kind of every block the span touches.
    pub fn set_kind(&mut self, span: Span, kind: BlockKind) {
        let span = self.clamp_span(span);
        // Backwards: turning a code block into paragraphs makes several
        // blocks out of one, which would move every block still to come.
        for bi in (span.from.block..=span.to.block.min(self.blocks.len().saturating_sub(1))).rev() {
            self.set_block_kind(bi, kind);
        }
    }

    fn set_block_kind(&mut self, bi: usize, kind: BlockKind) {
        let Some(was) = self.blocks.get(bi).map(|block| block.kind) else {
            return;
        };
        if was == kind {
            return;
        }
        if was == BlockKind::Code && kind != BlockKind::Code {
            // The lines of a code block are lines. They become blocks of
            // their own rather than one paragraph with the breaks pressed
            // out of it.
            let old = self.blocks.remove(bi);
            let lines = split_runs_on_newlines(old.runs);
            for (n, runs) in lines.into_iter().enumerate() {
                let mut block = Block { kind, runs };
                block.normalize();
                self.blocks.insert(bi + n, block);
            }
            self.settle();
            return;
        }
        let block = &mut self.blocks[bi];
        block.kind = kind;
        block.normalize();
    }

    /// Take the span out and return where the caret lands.
    pub fn delete(&mut self, span: Span) -> Pos {
        let span = self.clamp_span(span);
        if span.is_empty() {
            return span.from;
        }
        if span.from.block == span.to.block {
            self.blocks[span.from.block].remove(span.from.byte, span.to.byte);
            return span.from;
        }
        let tail = self.blocks[span.to.block].split_runs_off(span.to.byte);
        self.blocks.drain(span.from.block + 1..=span.to.block);
        let head = &mut self.blocks[span.from.block];
        head.remove(span.from.byte, head.len());
        head.runs.extend(tail);
        head.normalize();
        self.settle();
        span.from
    }

    /// Replace the span with text carrying `marks`, and say where the caret
    /// ends up. A line break in the text splits the block, except inside a
    /// code block, which keeps its breaks.
    pub fn replace(&mut self, span: Span, text: &str, marks: &RichMarks) -> Pos {
        let mut at = self.delete(span);
        if text.is_empty() {
            return at;
        }
        let code = self.blocks[at.block].kind == BlockKind::Code;
        for (n, line) in text.split('\n').enumerate() {
            if n > 0 {
                if code {
                    self.blocks[at.block].insert(at.byte, "\n", marks);
                    at.byte += 1;
                } else {
                    at = self.split(at);
                }
            }
            if !line.is_empty() {
                self.blocks[at.block].insert(at.byte, line, marks);
                at.byte += line.len();
            }
        }
        at
    }

    /// Return: break the block in two. Inside a code block it is a line
    /// break; on an empty list item or quote it ends the run of them, which
    /// is how anyone gets out of a list without reaching for the mouse.
    pub fn split(&mut self, pos: Pos) -> Pos {
        let pos = self.clamp(pos);
        let kind = self.blocks[pos.block].kind;
        if kind == BlockKind::Code {
            let marks = self.marks_at(pos);
            self.blocks[pos.block].insert(pos.byte, "\n", &marks);
            return Pos::new(pos.block, pos.byte + 1);
        }
        if kind.continues() && self.blocks[pos.block].is_empty() {
            self.blocks[pos.block].kind = BlockKind::Paragraph;
            return Pos::new(pos.block, 0);
        }
        let (head_kind, tail_kind) = kind.split_kinds(pos.byte == 0);
        let tail = self.blocks[pos.block].split_runs_off(pos.byte);
        self.blocks[pos.block].kind = head_kind;
        self.blocks.insert(
            pos.block + 1,
            Block {
                kind: tail_kind,
                runs: tail,
            },
        );
        Pos::new(pos.block + 1, 0)
    }

    /// Backspace. At the head of a block it takes the block's kind off first
    /// — a list item becomes a paragraph before it becomes part of the block
    /// above it — so one key undoes a formatting mistake without eating text.
    pub fn backspace(&mut self, pos: Pos) -> Pos {
        let pos = self.clamp(pos);
        if pos.byte > 0 {
            let text = self.blocks[pos.block].text();
            let from = prev_char(&text, pos.byte);
            self.blocks[pos.block].remove(from, pos.byte);
            return Pos::new(pos.block, from);
        }
        if self.blocks[pos.block].kind != BlockKind::Paragraph {
            self.set_block_kind(pos.block, BlockKind::Paragraph);
            return pos;
        }
        if pos.block == 0 {
            return pos;
        }
        let prev = pos.block - 1;
        let at = self.blocks[prev].len();
        let block = self.blocks.remove(pos.block);
        self.blocks[prev].runs.extend(block.runs);
        self.blocks[prev].normalize();
        Pos::new(prev, at)
    }

    /// Delete forward: the character after the caret, or the block after it.
    pub fn delete_forward(&mut self, pos: Pos) -> Pos {
        let pos = self.clamp(pos);
        let len = self.blocks[pos.block].len();
        if pos.byte < len {
            let text = self.blocks[pos.block].text();
            let to = next_char(&text, pos.byte);
            self.blocks[pos.block].remove(pos.byte, to);
            return pos;
        }
        if pos.block + 1 >= self.blocks.len() {
            return pos;
        }
        let next = self.blocks.remove(pos.block + 1);
        self.blocks[pos.block].runs.extend(next.runs);
        self.blocks[pos.block].normalize();
        pos
    }

    /// One character (or one word) back, over the block boundary if need be.
    pub fn left(&self, pos: Pos, word: bool) -> Pos {
        let pos = self.clamp(pos);
        if pos.byte == 0 {
            if pos.block == 0 {
                return pos;
            }
            let prev = pos.block - 1;
            return Pos::new(prev, self.blocks[prev].len());
        }
        let text = self.blocks[pos.block].text();
        let byte = if word {
            prev_word(&text, pos.byte)
        } else {
            prev_char(&text, pos.byte)
        };
        Pos::new(pos.block, byte)
    }

    pub fn right(&self, pos: Pos, word: bool) -> Pos {
        let pos = self.clamp(pos);
        let len = self.blocks[pos.block].len();
        if pos.byte >= len {
            if pos.block + 1 >= self.blocks.len() {
                return pos;
            }
            return Pos::new(pos.block + 1, 0);
        }
        let text = self.blocks[pos.block].text();
        let byte = if word {
            next_word(&text, pos.byte)
        } else {
            next_char(&text, pos.byte)
        };
        Pos::new(pos.block, byte)
    }

    pub fn block_start(&self, pos: Pos) -> Pos {
        Pos::new(self.clamp(pos).block, 0)
    }

    pub fn block_end(&self, pos: Pos) -> Pos {
        let pos = self.clamp(pos);
        Pos::new(pos.block, self.blocks[pos.block].len())
    }
}

/// The nearest character boundary at or before `byte`.
fn floor_boundary(text: &str, byte: usize) -> usize {
    let mut byte = byte.min(text.len());
    while byte > 0 && !text.is_char_boundary(byte) {
        byte -= 1;
    }
    byte
}

fn prev_char(text: &str, byte: usize) -> usize {
    floor_boundary(text, floor_boundary(text, byte).saturating_sub(1))
}

fn next_char(text: &str, byte: usize) -> usize {
    let mut byte = floor_boundary(text, byte) + 1;
    while byte < text.len() && !text.is_char_boundary(byte) {
        byte += 1;
    }
    byte.min(text.len())
}

/// Back over any run of spaces, then back over the word before them.
fn prev_word(text: &str, byte: usize) -> usize {
    let mut at = floor_boundary(text, byte);
    while at > 0 && text[..at].chars().next_back().is_some_and(|c| c.is_whitespace()) {
        at = prev_char(text, at);
    }
    while at > 0 && text[..at].chars().next_back().is_some_and(|c| !c.is_whitespace()) {
        at = prev_char(text, at);
    }
    at
}

fn next_word(text: &str, byte: usize) -> usize {
    let mut at = floor_boundary(text, byte);
    while at < text.len() && text[at..].chars().next().is_some_and(|c| !c.is_whitespace()) {
        at = next_char(text, at);
    }
    while at < text.len() && text[at..].chars().next().is_some_and(|c| c.is_whitespace()) {
        at = next_char(text, at);
    }
    at
}

/// One list of runs per line, marks kept.
fn split_runs_on_newlines(runs: Vec<RichRun>) -> Vec<Vec<RichRun>> {
    let mut out: Vec<Vec<RichRun>> = vec![Vec::new()];
    for run in runs {
        for (n, line) in run.text.split('\n').enumerate() {
            if n > 0 {
                out.push(Vec::new());
            }
            if !line.is_empty() {
                out.last_mut().unwrap().push(RichRun::new(line, run.marks.clone()));
            }
        }
    }
    out
}

/// What each block is numbered when it is drawn: 1, 2, 3 down a run of
/// numbered items, and 0 for everything else. Anything that is not a numbered
/// item ends the run, so a paragraph between two lists starts the second one
/// at 1 again.
pub fn list_numbers(blocks: &[Block]) -> Vec<usize> {
    let mut out = Vec::with_capacity(blocks.len());
    let mut n = 0;
    for block in blocks {
        if block.kind == BlockKind::Numbered {
            n += 1;
            out.push(n);
        } else {
            n = 0;
            out.push(0);
        }
    }
    out
}

/// The room left above a block: none between two items of one list, so a list
/// reads as one thing, and `spacing` everywhere else.
pub fn gap_above(prev: BlockKind, next: BlockKind, spacing: f64) -> f64 {
    if prev.is_list() && prev == next {
        0.0
    } else {
        spacing
    }
}

fn heading_scale(level: u8, base: f64) -> f64 {
    match level {
        1 => base,
        2 => base * 0.75,
        _ => base * 0.58,
    }
}

// ---------------------------------------------------------------------------
// Markup. The document's serialization, and how a DSL `text:` becomes one.
// ---------------------------------------------------------------------------

impl RichDoc {
    /// Read a document out of a small, common markup: `**bold**`,
    /// `*italic*`, `<u>underline</u>`, `~~struck~~`, `` `code` ``,
    /// `[text](target)`, `#` headings, `-` and `1.` items, `>` quotes and
    /// fenced code blocks. Anything else the parser recognises and this
    /// model has no room for — images, tables, rules — is dropped.
    pub fn from_markup(src: &str) -> RichDoc {
        let mut doc = RichDoc { blocks: Vec::new() };
        let mut open: Option<Block> = None;
        let mut lists: Vec<BlockKind> = Vec::new();
        let mut quote = 0usize;
        let mut item = 0usize;
        let mut bold = 0usize;
        let mut italic = 0usize;
        let mut under = 0usize;
        let mut strike = 0usize;
        let mut link: Option<String> = None;
        let mut code_text = String::new();
        let mut in_code = false;

        for event in Parser::new_ext(src, Options::ENABLE_STRIKETHROUGH) {
            let kind = default_kind(&lists, quote);
            match event {
                MdEvent::Start(Tag::Paragraph) => {
                    if open.is_none() {
                        open = Some(Block::new(kind));
                    }
                }
                MdEvent::End(TagEnd::Paragraph) => {
                    // A paragraph inside a list item is the item; the item
                    // closes it.
                    if item == 0 {
                        flush(&mut doc, &mut open);
                    }
                }
                MdEvent::Start(Tag::Heading { level, .. }) => {
                    flush(&mut doc, &mut open);
                    let level = match level {
                        HeadingLevel::H1 => 1,
                        HeadingLevel::H2 => 2,
                        _ => 3,
                    };
                    open = Some(Block::new(BlockKind::Heading(level)));
                }
                MdEvent::End(TagEnd::Heading(_)) => flush(&mut doc, &mut open),
                MdEvent::Start(Tag::BlockQuote(_)) => quote += 1,
                MdEvent::End(TagEnd::BlockQuote(_)) => quote = quote.saturating_sub(1),
                MdEvent::Start(Tag::List(first)) => lists.push(if first.is_some() {
                    BlockKind::Numbered
                } else {
                    BlockKind::Bullet
                }),
                MdEvent::End(TagEnd::List(_)) => {
                    lists.pop();
                }
                MdEvent::Start(Tag::Item) => {
                    flush(&mut doc, &mut open);
                    item += 1;
                    open = Some(Block::new(kind));
                }
                MdEvent::End(TagEnd::Item) => {
                    flush(&mut doc, &mut open);
                    item = item.saturating_sub(1);
                }
                MdEvent::Start(Tag::CodeBlock(_)) => {
                    flush(&mut doc, &mut open);
                    in_code = true;
                    code_text.clear();
                }
                MdEvent::End(TagEnd::CodeBlock) => {
                    in_code = false;
                    let text = code_text.trim_end_matches('\n').to_string();
                    doc.blocks.push(Block::plain(BlockKind::Code, &text));
                }
                MdEvent::Start(Tag::Emphasis) => italic += 1,
                MdEvent::End(TagEnd::Emphasis) => italic = italic.saturating_sub(1),
                MdEvent::Start(Tag::Strong) => bold += 1,
                MdEvent::End(TagEnd::Strong) => bold = bold.saturating_sub(1),
                MdEvent::Start(Tag::Strikethrough) => strike += 1,
                MdEvent::End(TagEnd::Strikethrough) => strike = strike.saturating_sub(1),
                MdEvent::Start(Tag::Link { dest_url, .. }) => link = Some(dest_url.to_string()),
                MdEvent::End(TagEnd::Link) => link = None,
                // The one piece of html this reads: markup has no underline
                // of its own, and an editor that can apply one has to be able
                // to write it down.
                MdEvent::InlineHtml(html) | MdEvent::Html(html) => {
                    match html.trim().to_ascii_lowercase().as_str() {
                        "<u>" => under += 1,
                        "</u>" => under = under.saturating_sub(1),
                        _ => {}
                    }
                }
                MdEvent::Text(text) => {
                    if in_code {
                        code_text.push_str(&text);
                    } else {
                        let marks = compose(bold, italic, under, strike, false, &link);
                        push_run(&mut open, kind, &text, marks);
                    }
                }
                MdEvent::Code(text) => {
                    let marks = compose(bold, italic, under, strike, true, &link);
                    push_run(&mut open, kind, &text, marks);
                }
                MdEvent::SoftBreak | MdEvent::HardBreak => {
                    if in_code {
                        code_text.push('\n');
                    } else {
                        let marks = compose(bold, italic, under, strike, false, &link);
                        push_run(&mut open, kind, " ", marks);
                    }
                }
                _ => {}
            }
        }
        flush(&mut doc, &mut open);
        doc.settle();
        doc
    }

    /// Write the document back out in the same markup, so a host can keep it
    /// in a string, a file or a field.
    pub fn to_markup(&self) -> String {
        let mut out = String::new();
        for (i, block) in self.blocks.iter().enumerate() {
            if i > 0 {
                let tight = self.blocks[i - 1].kind.is_list() && self.blocks[i - 1].kind == block.kind;
                out.push_str(if tight { "\n" } else { "\n\n" });
            }
            match block.kind {
                BlockKind::Paragraph => {}
                BlockKind::Heading(level) => {
                    for _ in 0..level.max(1) {
                        out.push('#');
                    }
                    out.push(' ');
                }
                BlockKind::Bullet => out.push_str("- "),
                BlockKind::Numbered => out.push_str("1. "),
                BlockKind::Quote => out.push_str("> "),
                BlockKind::Code => out.push_str("```\n"),
            }
            // Text that begins with a dash is read back as a list item, so
            // one backslash goes in front of it to say it is not one. It
            // only goes in front of a plain run: a marked one already starts
            // with its own punctuation, and a backslash there would eat the
            // mark instead. A number and a dot is the hole this cannot
            // close — a backslash before a digit is not an escape.
            if block.kind != BlockKind::Code {
                if let Some(run) = block.runs.first() {
                    if run.marks.is_plain()
                        && matches!(run.text.as_bytes().first(), Some(b'-') | Some(b'+'))
                    {
                        out.push('\\');
                    }
                }
            }
            for run in &block.runs {
                if block.kind == BlockKind::Code {
                    out.push_str(&run.text);
                } else {
                    write_run(&mut out, run);
                }
            }
            if block.kind == BlockKind::Code {
                out.push_str("\n```");
            }
        }
        out
    }
}

fn default_kind(lists: &[BlockKind], quote: usize) -> BlockKind {
    if let Some(kind) = lists.last() {
        *kind
    } else if quote > 0 {
        BlockKind::Quote
    } else {
        BlockKind::Paragraph
    }
}

fn compose(
    bold: usize,
    italic: usize,
    under: usize,
    strike: usize,
    code: bool,
    link: &Option<String>,
) -> RichMarks {
    let mut marks = RichMarks::none();
    marks.set(Mark::Bold, bold > 0);
    marks.set(Mark::Italic, italic > 0);
    marks.set(Mark::Underline, under > 0);
    marks.set(Mark::Strike, strike > 0);
    marks.set(Mark::Code, code);
    marks.set_link(link.as_deref());
    marks
}

fn push_run(open: &mut Option<Block>, kind: BlockKind, text: &str, marks: RichMarks) {
    if text.is_empty() {
        return;
    }
    let block = open.get_or_insert_with(|| Block::new(kind));
    block.runs.push(RichRun::new(text, marks));
}

fn flush(doc: &mut RichDoc, open: &mut Option<Block>) {
    if let Some(mut block) = open.take() {
        block.normalize();
        doc.blocks.push(block);
    }
}

fn write_run(out: &mut String, run: &RichRun) {
    let marks = &run.marks;
    if marks.link().is_some() {
        out.push('[');
    }
    if marks.has(Mark::Bold) {
        out.push_str("**");
    }
    if marks.has(Mark::Italic) {
        out.push('*');
    }
    if marks.has(Mark::Underline) {
        out.push_str("<u>");
    }
    if marks.has(Mark::Strike) {
        out.push_str("~~");
    }
    if marks.has(Mark::Code) {
        out.push('`');
        out.push_str(&run.text);
        out.push('`');
    } else {
        escape_into(out, &run.text);
    }
    if marks.has(Mark::Strike) {
        out.push_str("~~");
    }
    if marks.has(Mark::Underline) {
        out.push_str("</u>");
    }
    if marks.has(Mark::Italic) {
        out.push('*');
    }
    if marks.has(Mark::Bold) {
        out.push_str("**");
    }
    if let Some(href) = marks.link() {
        out.push_str("](");
        out.push_str(href);
        out.push(')');
    }
}

/// Anything the parser would read as markup is written down as itself, so a
/// paragraph about a star still has a star in it after a round trip.
fn escape_into(out: &mut String, text: &str) {
    for c in text.chars() {
        if matches!(c, '\\' | '*' | '_' | '`' | '[' | ']' | '~' | '<' | '>' | '#') {
            out.push('\\');
        }
        out.push(c);
    }
}

// ---------------------------------------------------------------------------
// The flat map: what the flow drew, and where it came from.
// ---------------------------------------------------------------------------

/// One piece of text as it was handed to the flow. The flow keeps its own
/// flat buffer of everything it drew — bullets, numbers and the newlines
/// between blocks included — and a hit test answers in indices into that
/// buffer, so the way back into the document is recorded here as it is drawn
/// rather than guessed at afterwards.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Piece {
    block: usize,
    /// Where this piece starts in its block.
    byte: usize,
    /// How many bytes of the block it covers.
    len: usize,
    /// Where the flow put it in its own buffer.
    flat: usize,
    /// How much of it reached that buffer: the flow trims the whitespace at
    /// the head of a line, so this can be shorter than `len`.
    flat_len: usize,
}

impl Piece {
    /// The bytes the flow trimmed off the front.
    fn trim(&self) -> usize {
        self.len - self.flat_len.min(self.len)
    }
}

/// Where a flat index from the flow lands in the document.
fn pos_at_flat(map: &[Piece], flat: usize) -> Pos {
    let mut best = Pos::ZERO;
    for piece in map {
        if flat < piece.flat {
            break;
        }
        if flat <= piece.flat + piece.flat_len {
            // Never past the piece's own bytes: a bullet or a number is a
            // piece of no length at the head of its block, and a click on
            // one belongs at the head of that block rather than inside the
            // marker.
            let off = (flat - piece.flat).min(piece.len - piece.trim());
            return Pos::new(piece.block, piece.byte + piece.trim() + off);
        }
        best = Pos::new(piece.block, piece.byte + piece.len);
    }
    best
}

/// Where a document position sits in the flow's flat buffer.
fn flat_at_pos(map: &[Piece], pos: Pos) -> usize {
    let mut best = 0;
    for piece in map {
        // A marker carries no bytes of the document, so a position is never
        // inside one: the highlight starts at the text, not at the bullet.
        if piece.len == 0 {
            best = piece.flat + piece.flat_len;
            continue;
        }
        if piece.block < pos.block {
            best = piece.flat + piece.flat_len;
            continue;
        }
        if piece.block > pos.block {
            return best;
        }
        if pos.byte < piece.byte {
            return piece.flat;
        }
        if pos.byte <= piece.byte + piece.len {
            let off = pos.byte.saturating_sub(piece.byte + piece.trim());
            return piece.flat + off.min(piece.flat_len);
        }
        best = piece.flat + piece.flat_len;
    }
    best
}

// ---------------------------------------------------------------------------
// The widget.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub enum RichTextEditorAction {
    /// The document changed: read it with `markup()` or `doc()`.
    Changed,
    /// The caret moved or the selection changed, so a toolbar showing what
    /// the caret carries has something to redraw.
    SelectionChanged,
    /// A link was clicked in a document that cannot be typed into.
    LinkClicked(String),
    #[default]
    None,
}

/// What the last edit was, for grouping keystrokes into one undo.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum EditKind {
    Typing,
    Other,
}

#[derive(Clone)]
struct Snapshot {
    doc: RichDoc,
    anchor: Pos,
    focus: Pos,
}

/// The numbers the block drawing needs, gathered so the draw can hand one
/// thing around instead of six.
#[derive(Copy, Clone)]
struct DrawStyle {
    link_color: Vec4f,
    caret_width: f64,
    caret_height: f64,
    heading_scale: f64,
}

/// Which of the flow's style stacks a run pushed, so exactly those are popped.
#[derive(Copy, Clone, Default)]
struct Pushed {
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
    code: bool,
    color: bool,
}

const UNDO_DEPTH: usize = 200;

#[derive(Script, ScriptHook, Widget)]
pub struct RichTextEditor {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    text_flow: TextFlow,
    #[live]
    draw_caret: DrawColor,

    /// The document in markup. Set it to load one; it is rewritten after
    /// every edit, so it is always what the document says.
    #[live]
    text: String,

    #[live(true)]
    editable: bool,
    #[live(12.0)]
    block_spacing: f64,
    #[live(1.8)]
    heading_scale: f64,
    #[live(1.5)]
    caret_width: f64,
    #[live(1.55)]
    caret_height: f64,
    #[live]
    link_color: Vec4f,

    #[rust]
    doc: RichDoc,
    /// The markup the document was last built from. Compared with `text` at
    /// draw time, so a live edit or a host writing the property reloads the
    /// document and an edit made here does not.
    #[rust]
    parsed_from: String,

    /// Where the selection was dropped and where it is being dragged to. The
    /// caret is the focus; a collapsed selection is a caret.
    #[rust]
    anchor: Pos,
    #[rust]
    focus: Pos,
    /// Marks a toggle put on with nothing selected: the next thing typed
    /// carries them. Cleared by anything that moves the caret.
    #[rust]
    pending: Option<RichMarks>,
    #[rust]
    selecting: bool,

    /// Where the flow drew the caret last, so an up or down key can ask the
    /// flow what is on the line above or below that point rather than
    /// working out the lines here.
    #[rust]
    caret_rect: Option<Rect>,
    #[rust]
    map: Vec<Piece>,

    #[rust]
    undo_stack: Vec<Snapshot>,
    #[rust]
    redo_stack: Vec<Snapshot>,
    #[rust(EditKind::Other)]
    last_edit: EditKind,
}

impl RichTextEditor {
    /// The document as it stands.
    pub fn doc(&self) -> &RichDoc {
        &self.doc
    }

    pub fn set_doc(&mut self, cx: &mut Cx, doc: RichDoc) {
        self.doc = doc;
        self.anchor = Pos::ZERO;
        self.focus = Pos::ZERO;
        self.pending = None;
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.after_change(cx);
    }

    pub fn markup(&self) -> String {
        self.doc.to_markup()
    }

    pub fn set_markup(&mut self, cx: &mut Cx, markup: &str) {
        self.set_doc(cx, RichDoc::from_markup(markup));
    }

    /// The selection, in reading order.
    pub fn selection(&self) -> Span {
        Span::new(self.doc.clamp(self.anchor), self.doc.clamp(self.focus))
    }

    pub fn selected_text(&self) -> String {
        self.doc.text_in(self.selection())
    }

    /// What the caret carries: the marks of the selection, or the ones a
    /// toggle put on for the next keystroke.
    pub fn marks_at_caret(&self) -> RichMarks {
        if let Some(pending) = &self.pending {
            return pending.clone();
        }
        let span = self.selection();
        let mut marks = self.doc.marks_at(span.from);
        if !span.is_empty() {
            for mark in Mark::ALL {
                marks.set(mark, self.doc.has_mark(span, mark));
            }
            marks.set_link(self.doc.link_in(span).as_deref());
        }
        marks
    }

    pub fn kind_at_caret(&self) -> BlockKind {
        self.doc
            .blocks
            .get(self.selection().from.block)
            .map(|block| block.kind)
            .unwrap_or(BlockKind::Paragraph)
    }

    /// Put a mark on the selection, or take it off. With nothing selected it
    /// arms the next keystroke instead.
    pub fn toggle_mark(&mut self, cx: &mut Cx, mark: Mark) {
        if !self.editable {
            return;
        }
        let span = self.selection();
        if span.is_empty() {
            let mut marks = self.marks_at_caret();
            marks.toggle(mark);
            self.pending = Some(marks);
            self.redraw(cx);
            cx.widget_action(self.widget_uid(), RichTextEditorAction::SelectionChanged);
            return;
        }
        self.push_undo(EditKind::Other);
        self.doc.toggle_mark(span, mark);
        self.after_change(cx);
    }

    /// Make the selection a link to `href`, or take the link off it.
    pub fn set_link(&mut self, cx: &mut Cx, href: Option<&str>) {
        if !self.editable || self.selection().is_empty() {
            return;
        }
        self.push_undo(EditKind::Other);
        let span = self.selection();
        self.doc.set_link(span, href);
        self.after_change(cx);
    }

    /// Change the kind of every block the selection touches.
    pub fn set_kind(&mut self, cx: &mut Cx, kind: BlockKind) {
        if !self.editable {
            return;
        }
        self.push_undo(EditKind::Other);
        let span = self.selection();
        self.doc.set_kind(span, kind);
        self.focus = self.doc.clamp(self.focus);
        self.anchor = self.doc.clamp(self.anchor);
        self.after_change(cx);
    }

    pub fn select_all(&mut self, cx: &mut Cx) {
        let all = self.doc.all();
        self.anchor = all.from;
        self.focus = all.to;
        self.pending = None;
        self.redraw(cx);
    }

    fn after_change(&mut self, cx: &mut Cx) {
        // Keep the property truthful, and keep the draw from reading it back
        // as a document to load.
        self.text = self.doc.to_markup();
        self.parsed_from = self.text.clone();
        self.redraw(cx);
        cx.widget_action(self.widget_uid(), RichTextEditorAction::Changed);
    }

    fn push_undo(&mut self, kind: EditKind) {
        self.redo_stack.clear();
        // A run of keystrokes is one undo; anything else starts a new one.
        if kind == EditKind::Typing && self.last_edit == EditKind::Typing && !self.undo_stack.is_empty() {
            return;
        }
        self.last_edit = kind;
        self.undo_stack.push(Snapshot {
            doc: self.doc.clone(),
            anchor: self.anchor,
            focus: self.focus,
        });
        if self.undo_stack.len() > UNDO_DEPTH {
            self.undo_stack.remove(0);
        }
    }

    fn undo(&mut self, cx: &mut Cx) {
        let Some(prev) = self.undo_stack.pop() else {
            return;
        };
        self.redo_stack.push(Snapshot {
            doc: self.doc.clone(),
            anchor: self.anchor,
            focus: self.focus,
        });
        self.doc = prev.doc;
        self.anchor = prev.anchor;
        self.focus = prev.focus;
        self.last_edit = EditKind::Other;
        self.pending = None;
        self.after_change(cx);
    }

    fn redo(&mut self, cx: &mut Cx) {
        let Some(next) = self.redo_stack.pop() else {
            return;
        };
        self.undo_stack.push(Snapshot {
            doc: self.doc.clone(),
            anchor: self.anchor,
            focus: self.focus,
        });
        self.doc = next.doc;
        self.anchor = next.anchor;
        self.focus = next.focus;
        self.last_edit = EditKind::Other;
        self.pending = None;
        self.after_change(cx);
    }

    /// Run one edit over the current selection and put the caret where it
    /// left off.
    fn edit(&mut self, cx: &mut Cx, kind: EditKind, f: impl FnOnce(&mut RichDoc, Span) -> Pos) {
        if !self.editable {
            return;
        }
        self.push_undo(kind);
        let span = self.selection();
        let pos = f(&mut self.doc, span);
        self.doc.settle();
        let pos = self.doc.clamp(pos);
        self.anchor = pos;
        self.focus = pos;
        self.pending = None;
        self.after_change(cx);
    }

    fn move_to(&mut self, cx: &mut Cx, pos: Pos, extend: bool) {
        let pos = self.doc.clamp(pos);
        if self.focus == pos && (extend || self.anchor == pos) {
            return;
        }
        self.focus = pos;
        if !extend {
            self.anchor = pos;
        }
        self.pending = None;
        self.last_edit = EditKind::Other;
        self.redraw(cx);
        cx.widget_action(self.widget_uid(), RichTextEditorAction::SelectionChanged);
    }

    /// The line above or below the caret, asked of the flow: the caret's own
    /// rect from the last draw, moved half a line up or one and a half down,
    /// handed back to the flow's hit test. Without a caret rect — the widget
    /// has not drawn one yet — it falls back to the block above or below.
    fn move_line(&mut self, cx: &mut Cx, down: bool, extend: bool) {
        let focus = self.doc.clamp(self.focus);
        if let Some(rect) = self.caret_rect {
            let dy = if down {
                rect.size.y * 1.5
            } else {
                -rect.size.y * 0.5
            };
            let point = dvec2(rect.pos.x + 1.0, rect.pos.y + dy);
            if let Some(flat) = self.text_flow.selection_point_to_char_index(cx, point) {
                let pos = pos_at_flat(&self.map, flat);
                if pos != focus {
                    self.move_to(cx, pos, extend);
                    return;
                }
            }
        }
        let block = if down {
            focus.block + 1
        } else {
            focus.block.wrapping_sub(1)
        };
        if block >= self.doc.blocks.len() {
            let pos = if down {
                self.doc.end()
            } else {
                Pos::ZERO
            };
            self.move_to(cx, pos, extend);
            return;
        }
        self.move_to(cx, Pos::new(block, focus.byte), extend);
    }

    /// Where a point on screen is in the document.
    fn pos_at(&self, cx: &Cx, abs: DVec2) -> Option<Pos> {
        let flat = self.text_flow.selection_point_to_char_index(cx, abs)?;
        Some(self.doc.clamp(pos_at_flat(&self.map, flat)))
    }

    fn link_at(&self, cx: &Cx, abs: DVec2) -> Option<String> {
        let pos = self.pos_at(cx, abs)?;
        self.doc
            .run_at(pos)
            .and_then(|run| run.marks.link())
            .map(|href| href.to_string())
    }

    fn marks_for_input(&self) -> RichMarks {
        if let Some(pending) = &self.pending {
            return pending.clone();
        }
        self.doc.marks_at(self.selection().from)
    }

    fn handle_key(&mut self, cx: &mut Cx, ke: KeyEvent) {
        let shift = ke.modifiers.shift;
        let primary = ke.modifiers.is_primary();
        // Word-wise movement is alt on one platform and control on the
        // others, and neither means anything else on an arrow key.
        let word = ke.modifiers.alt || ke.modifiers.control;
        match ke.key_code {
            KeyCode::ArrowLeft => {
                let pos = self.doc.left(self.focus, word);
                self.move_to(cx, pos, shift);
            }
            KeyCode::ArrowRight => {
                let pos = self.doc.right(self.focus, word);
                self.move_to(cx, pos, shift);
            }
            KeyCode::ArrowUp => self.move_line(cx, false, shift),
            KeyCode::ArrowDown => self.move_line(cx, true, shift),
            KeyCode::Home => {
                let pos = if primary {
                    Pos::ZERO
                } else {
                    self.doc.block_start(self.focus)
                };
                self.move_to(cx, pos, shift);
            }
            KeyCode::End => {
                let pos = if primary {
                    self.doc.end()
                } else {
                    self.doc.block_end(self.focus)
                };
                self.move_to(cx, pos, shift);
            }
            KeyCode::Escape => {
                cx.set_key_focus(Area::Empty);
            }
            KeyCode::KeyA if primary => {
                self.select_all(cx);
                cx.widget_action(self.widget_uid(), RichTextEditorAction::SelectionChanged);
            }
            KeyCode::KeyZ if primary && shift => self.redo(cx),
            KeyCode::KeyZ if primary => self.undo(cx),
            KeyCode::KeyB if primary => self.toggle_mark(cx, Mark::Bold),
            KeyCode::KeyI if primary => self.toggle_mark(cx, Mark::Italic),
            KeyCode::KeyU if primary => self.toggle_mark(cx, Mark::Underline),
            KeyCode::KeyE if primary => self.toggle_mark(cx, Mark::Code),
            KeyCode::KeyX if primary && shift => self.toggle_mark(cx, Mark::Strike),
            KeyCode::Backspace if self.editable => {
                self.edit(cx, EditKind::Other, |doc, span| {
                    if span.is_empty() {
                        doc.backspace(span.from)
                    } else {
                        doc.delete(span)
                    }
                });
            }
            KeyCode::Delete if self.editable => {
                self.edit(cx, EditKind::Other, |doc, span| {
                    if span.is_empty() {
                        doc.delete_forward(span.from)
                    } else {
                        doc.delete(span)
                    }
                });
            }
            KeyCode::ReturnKey | KeyCode::NumpadEnter if self.editable => {
                self.edit(cx, EditKind::Other, |doc, span| {
                    let at = doc.delete(span);
                    doc.split(at)
                });
            }
            _ => (),
        }
    }

    /// Draw one block, and the caret if it sits in this one.
    #[allow(clippy::too_many_arguments)]
    fn draw_block(
        cx: &mut Cx2d,
        tf: &mut TextFlow,
        draw_caret: &mut DrawColor,
        map: &mut Vec<Piece>,
        caret_rect: &mut Option<Rect>,
        block: &Block,
        bi: usize,
        number: usize,
        caret: Option<usize>,
        style: &DrawStyle,
    ) {
        match block.kind {
            BlockKind::Paragraph => {
                Self::draw_runs(cx, tf, draw_caret, map, caret_rect, block, bi, caret, style);
            }
            BlockKind::Heading(level) => {
                tf.push_size_abs_scale(heading_scale(level, style.heading_scale));
                tf.bold.push();
                Self::draw_runs(cx, tf, draw_caret, map, caret_rect, block, bi, caret, style);
                tf.bold.pop();
                tf.font_sizes.pop();
            }
            BlockKind::Bullet | BlockKind::Numbered => {
                let marker = if block.kind == BlockKind::Numbered {
                    format!("{}.", number.max(1))
                } else {
                    "\u{2022}".to_string()
                };
                // The marker goes through the flow's own `draw_text`, so it
                // takes room in the flat buffer that belongs to no run. It is
                // written down as a piece of no length at the head of the
                // block, or a click on the bullet would answer with the end
                // of whatever came before it.
                let flat = tf.text_len();
                tf.begin_list_item(cx, &marker, 2.5);
                map.push(Piece {
                    block: bi,
                    byte: 0,
                    len: 0,
                    flat,
                    flat_len: tf.text_len().saturating_sub(flat),
                });
                Self::draw_runs(cx, tf, draw_caret, map, caret_rect, block, bi, caret, style);
                tf.end_list_item(cx);
            }
            BlockKind::Quote => {
                tf.begin_quote(cx);
                Self::draw_runs(cx, tf, draw_caret, map, caret_rect, block, bi, caret, style);
                tf.end_quote(cx);
            }
            BlockKind::Code => {
                tf.push_size_rel_scale(tf.fixed_font_size_scale);
                tf.fixed.push();
                tf.begin_code(cx);
                Self::draw_runs(cx, tf, draw_caret, map, caret_rect, block, bi, caret, style);
                tf.end_code(cx);
                tf.fixed.pop();
                tf.font_sizes.pop();
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_runs(
        cx: &mut Cx2d,
        tf: &mut TextFlow,
        draw_caret: &mut DrawColor,
        map: &mut Vec<Piece>,
        caret_rect: &mut Option<Rect>,
        block: &Block,
        bi: usize,
        caret: Option<usize>,
        style: &DrawStyle,
    ) {
        let in_code_block = block.kind == BlockKind::Code;
        let mut byte = 0usize;
        for run in &block.runs {
            let pushed = push_marks(tf, &run.marks, style.link_color, in_code_block);
            let len = run.text.len();
            match caret {
                // The caret sits in this run, so the run is drawn in two
                // halves with the caret walked between them: the flow puts
                // it exactly where the next glyph would go.
                Some(at) if at >= byte && at < byte + len => {
                    let cut = floor_boundary(&run.text, at - byte);
                    Self::draw_piece(cx, tf, map, bi, byte, &run.text[..cut]);
                    Self::draw_caret(cx, tf, draw_caret, caret_rect, style);
                    Self::draw_piece(cx, tf, map, bi, byte + cut, &run.text[cut..]);
                }
                _ => Self::draw_piece(cx, tf, map, bi, byte, &run.text),
            }
            pop_marks(tf, pushed);
            byte += len;
        }
        // The end of the block, and the whole of an empty one.
        if caret == Some(byte) {
            Self::draw_caret(cx, tf, draw_caret, caret_rect, style);
        }
    }

    /// Hand one piece of a run to the flow and write down where it went.
    fn draw_piece(
        cx: &mut Cx2d,
        tf: &mut TextFlow,
        map: &mut Vec<Piece>,
        block: usize,
        byte: usize,
        text: &str,
    ) {
        if text.is_empty() {
            return;
        }
        let flat = tf.text_len();
        tf.draw_text(cx, text);
        let flat_len = tf.text_len().saturating_sub(flat);
        map.push(Piece {
            block,
            byte,
            len: text.len(),
            flat,
            flat_len,
        });
    }

    fn draw_caret(
        cx: &mut Cx2d,
        tf: &mut TextFlow,
        draw_caret: &mut DrawColor,
        caret_rect: &mut Option<Rect>,
        style: &DrawStyle,
    ) {
        let size = *tf.font_sizes.last().unwrap_or(&tf.font_size) as f64;
        let height = size * style.caret_height;
        let rect = draw_caret.draw_walk(cx, Walk::fixed(style.caret_width, height));
        *caret_rect = Some(rect);
    }
}

fn push_marks(tf: &mut TextFlow, marks: &RichMarks, link_color: Vec4f, in_code_block: bool) -> Pushed {
    let mut pushed = Pushed::default();
    if marks.has(Mark::Bold) {
        tf.bold.push();
        pushed.bold = true;
    }
    if marks.has(Mark::Italic) {
        tf.italic.push();
        pushed.italic = true;
    }
    // The flow draws one decoration per run. A run that is both struck
    // through and underlined keeps the strike, which is the one that says
    // something about the text rather than about the pointer.
    if marks.has(Mark::Strike) {
        tf.strikethrough.push();
        pushed.strike = true;
    } else if marks.has(Mark::Underline) || marks.link().is_some() {
        tf.underline.push();
        pushed.underline = true;
    }
    if marks.has(Mark::Code) && !in_code_block {
        tf.push_size_rel_scale(tf.fixed_font_size_scale);
        tf.fixed.push();
        tf.inline_code.push();
        pushed.code = true;
    }
    if marks.link().is_some() {
        tf.font_colors.push(link_color);
        pushed.color = true;
    }
    pushed
}

fn pop_marks(tf: &mut TextFlow, pushed: Pushed) {
    if pushed.bold {
        tf.bold.pop();
    }
    if pushed.italic {
        tf.italic.pop();
    }
    if pushed.strike {
        tf.strikethrough.pop();
    }
    if pushed.underline {
        tf.underline.pop();
    }
    if pushed.code {
        tf.inline_code.pop();
        tf.fixed.pop();
        tf.font_sizes.pop();
    }
    if pushed.color {
        tf.font_colors.pop();
    }
}

impl Widget for RichTextEditor {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if self.parsed_from != self.text {
            self.doc = RichDoc::from_markup(&self.text);
            self.parsed_from = self.text.clone();
            self.anchor = self.doc.clamp(self.anchor);
            self.focus = self.doc.clamp(self.focus);
        }
        let area = self.text_flow.area();
        // An area with no rect yet compares equal to "nothing has focus",
        // so a first draw would flash a caret in every editor on the page.
        let show_caret = self.editable && !area.is_empty() && cx.has_key_focus(area);
        let span = self.selection();
        let focus = self.doc.clamp(self.focus);
        let style = DrawStyle {
            link_color: self.link_color,
            caret_width: self.caret_width,
            caret_height: self.caret_height,
            heading_scale: self.heading_scale,
        };
        let spacing = self.block_spacing;

        self.map.clear();
        self.caret_rect = None;

        let Self {
            text_flow: tf,
            draw_caret,
            doc,
            map,
            caret_rect,
            ..
        } = self;

        // The flow's text tracker is what turns a click into a position in
        // the document, so this widget cannot work without it; it is not a
        // choice the caller gets to make.
        tf.selectable = true;
        tf.begin(cx, walk);

        let numbers = list_numbers(&doc.blocks);
        for (bi, block) in doc.blocks.iter().enumerate() {
            if bi > 0 {
                let gap = gap_above(doc.blocks[bi - 1].kind, block.kind, spacing);
                tf.new_line_collapsed_with_spacing(cx, gap);
            }
            let caret = (show_caret && focus.block == bi).then_some(focus.byte);
            Self::draw_block(
                cx,
                tf,
                draw_caret,
                map,
                caret_rect,
                block,
                bi,
                numbers[bi],
                caret,
                &style,
            );
        }

        // The flow draws the highlight itself, out of its own flat indices,
        // once everything it is going to draw has been drawn.
        tf.set_selection(flat_at_pos(&map[..], span.from), flat_at_pos(&map[..], span.to));
        tf.end(cx);

        if self.editable {
            cx.add_nav_stop(self.text_flow.area(), NavRole::TextInput, Inset::default());
        }
        if show_caret {
            if let Some(rect) = self.caret_rect {
                let area = self.text_flow.area();
                let origin = area.rect(cx).pos;
                cx.show_text_ime(area, rect.pos - origin + dvec2(0.0, rect.size.y));
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        let uid = self.widget_uid();
        let area = self.text_flow.area();
        match event.hits(cx, area) {
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Text);
            }
            Hit::FingerHoverOut(_) => {
                cx.set_cursor(MouseCursor::Default);
            }
            Hit::FingerHoverOver(fe) => {
                // A link in a page that cannot be typed into is a link.
                let over_link = !self.editable && self.link_at(cx, fe.abs).is_some();
                cx.set_cursor(if over_link {
                    MouseCursor::Hand
                } else {
                    MouseCursor::Text
                });
            }
            Hit::FingerDown(fe) if fe.device.is_primary_hit() => {
                cx.set_key_focus(area);
                if let Some(pos) = self.pos_at(cx, fe.abs) {
                    let extend = fe.modifiers.shift;
                    self.move_to(cx, pos, extend);
                    self.selecting = true;
                }
                self.redraw(cx);
            }
            Hit::FingerMove(fe) if self.selecting => {
                if let Some(pos) = self.pos_at(cx, fe.abs) {
                    self.move_to(cx, pos, true);
                }
            }
            Hit::FingerUp(fe) => {
                self.selecting = false;
                if !self.editable && fe.was_tap() {
                    if let Some(href) = self.link_at(cx, fe.abs) {
                        cx.widget_action(uid, RichTextEditorAction::LinkClicked(href));
                    }
                }
            }
            Hit::KeyFocus(_) => {
                self.redraw(cx);
            }
            Hit::KeyFocusLost(_) => {
                cx.hide_text_ime();
                self.redraw(cx);
            }
            Hit::TextCopy(event) => {
                let text = self.selected_text();
                if !text.is_empty() {
                    *event.response.borrow_mut() = Some(text);
                }
            }
            Hit::TextCut(event) => {
                let text = self.selected_text();
                if !text.is_empty() {
                    *event.response.borrow_mut() = Some(text);
                    self.edit(cx, EditKind::Other, |doc, span| doc.delete(span));
                }
            }
            Hit::TextInput(te) => {
                if !self.editable {
                    return;
                }
                // The composition, replace-range and full-state paths are the
                // soft keyboards' (see the module docs). Taking the text out
                // of them without the rest of the protocol would double
                // every composed character.
                if te.replace_last
                    || te.composition.is_some()
                    || te.full_state_sync.is_some()
                    || te.replace_range.is_some()
                {
                    return;
                }
                let input: String = te
                    .input
                    .chars()
                    .filter(|c| *c == '\n' || !c.is_control())
                    .collect();
                if input.is_empty() {
                    return;
                }
                let marks = self.marks_for_input();
                let kind = if te.was_paste {
                    EditKind::Other
                } else {
                    EditKind::Typing
                };
                self.edit(cx, kind, move |doc, span| doc.replace(span, &input, &marks));
            }
            Hit::KeyDown(ke) => self.handle_key(cx, ke),
            _ => (),
        }
    }

    fn text(&self) -> String {
        self.doc.to_markup()
    }

    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        if self.text != v {
            self.set_markup(cx, v);
        }
    }
}

impl RichTextEditorRef {
    /// The document in markup.
    pub fn markup(&self) -> String {
        self.borrow().map(|inner| inner.markup()).unwrap_or_default()
    }

    pub fn set_markup(&self, cx: &mut Cx, markup: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_markup(cx, markup);
        }
    }

    /// A copy of the document, for a host that wants to walk it.
    pub fn doc(&self) -> RichDoc {
        self.borrow().map(|inner| inner.doc().clone()).unwrap_or_default()
    }

    pub fn set_doc(&self, cx: &mut Cx, doc: RichDoc) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_doc(cx, doc);
        }
    }

    pub fn selection(&self) -> Span {
        self.borrow().map(|inner| inner.selection()).unwrap_or_default()
    }

    pub fn selected_text(&self) -> String {
        self.borrow().map(|inner| inner.selected_text()).unwrap_or_default()
    }

    /// What a toolbar shows as pressed: the marks the selection carries, or
    /// the ones armed for the next keystroke.
    pub fn marks_at_caret(&self) -> RichMarks {
        self.borrow().map(|inner| inner.marks_at_caret()).unwrap_or_default()
    }

    pub fn kind_at_caret(&self) -> BlockKind {
        self.borrow()
            .map(|inner| inner.kind_at_caret())
            .unwrap_or(BlockKind::Paragraph)
    }

    pub fn toggle_mark(&self, cx: &mut Cx, mark: Mark) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.toggle_mark(cx, mark);
        }
    }

    pub fn set_link(&self, cx: &mut Cx, href: Option<&str>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_link(cx, href);
        }
    }

    pub fn set_kind(&self, cx: &mut Cx, kind: BlockKind) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_kind(cx, kind);
        }
    }

    /// The document changed.
    pub fn changed(&self, actions: &Actions) -> bool {
        actions
            .find_widget_action(self.widget_uid())
            .map(|item| matches!(item.cast(), RichTextEditorAction::Changed))
            .unwrap_or(false)
    }

    /// The caret moved, so a toolbar has something to redraw.
    pub fn selection_changed(&self, actions: &Actions) -> bool {
        actions
            .find_widget_action(self.widget_uid())
            .map(|item| matches!(item.cast(), RichTextEditorAction::SelectionChanged))
            .unwrap_or(false)
    }

    /// A link was clicked in a read-only document.
    pub fn link_clicked(&self, actions: &Actions) -> Option<String> {
        let item = actions.find_widget_action(self.widget_uid())?;
        match item.cast() {
            RichTextEditorAction::LinkClicked(href) => Some(href),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc_of(text: &str) -> RichDoc {
        RichDoc::from_markup(text)
    }

    fn runs(doc: &RichDoc, block: usize) -> Vec<(String, RichMarks)> {
        doc.blocks[block]
            .runs
            .iter()
            .map(|run| (run.text.clone(), run.marks.clone()))
            .collect()
    }

    fn one(text: &str) -> RichDoc {
        RichDoc {
            blocks: vec![Block::plain(BlockKind::Paragraph, text)],
        }
    }

    fn span(from: usize, to: usize) -> Span {
        Span::new(Pos::new(0, from), Pos::new(0, to))
    }

    #[test]
    fn a_mark_over_part_of_a_run_leaves_three() {
        let mut doc = one("hello world");
        doc.set_mark(span(6, 11), Mark::Bold, true);
        let r = runs(&doc, 0);
        assert_eq!(r.len(), 2, "the head and the marked tail");
        assert_eq!(r[0], ("hello ".to_string(), RichMarks::none()));
        assert_eq!(r[1], ("world".to_string(), RichMarks::of(&[Mark::Bold])));

        let mut doc = one("hello world");
        doc.set_mark(span(2, 5), Mark::Italic, true);
        let r = runs(&doc, 0);
        assert_eq!(r.len(), 3, "a mark in the middle cuts the run in three");
        assert_eq!(r[0].0, "he");
        assert_eq!(r[1], ("llo".to_string(), RichMarks::of(&[Mark::Italic])));
        assert_eq!(r[2].0, " world");
    }

    #[test]
    fn taking_a_mark_off_the_middle_of_a_marked_run_leaves_three() {
        let mut doc = one("hello world");
        doc.set_mark(span(0, 11), Mark::Bold, true);
        doc.set_mark(span(5, 6), Mark::Bold, false);
        let r = runs(&doc, 0);
        assert_eq!(r.len(), 3);
        assert!(r[0].1.has(Mark::Bold));
        assert_eq!(r[1], (" ".to_string(), RichMarks::none()), "the hole in the middle");
        assert!(r[2].1.has(Mark::Bold));
    }

    #[test]
    fn a_mark_over_several_runs_covers_all_of_them() {
        let mut doc = one("one two three");
        doc.set_mark(span(0, 3), Mark::Bold, true);
        doc.set_mark(span(8, 13), Mark::Italic, true);
        assert_eq!(runs(&doc, 0).len(), 3);
        doc.set_mark(span(0, 13), Mark::Underline, true);
        let r = runs(&doc, 0);
        assert_eq!(r.len(), 3, "the underline crossed them, it did not flatten them");
        assert!(r.iter().all(|(_, marks)| marks.has(Mark::Underline)));
        assert!(r[0].1.has(Mark::Bold));
        assert!(r[2].1.has(Mark::Italic));
    }

    #[test]
    fn a_mark_over_the_whole_block_leaves_one_run() {
        let mut doc = one("one two");
        doc.set_mark(span(0, 3), Mark::Bold, true);
        doc.set_mark(span(0, 7), Mark::Bold, true);
        let r = runs(&doc, 0);
        assert_eq!(r.len(), 1, "everything ended up the same, so it is one run again");
        assert_eq!(r[0].0, "one two");
    }

    #[test]
    fn runs_that_end_up_identical_merge() {
        let mut doc = one("abcdef");
        doc.set_mark(span(2, 4), Mark::Bold, true);
        assert_eq!(runs(&doc, 0).len(), 3);
        // Taking it off again has to leave the block as it started, or the
        // model grows a run for every edit ever made to it.
        doc.set_mark(span(2, 4), Mark::Bold, false);
        assert_eq!(runs(&doc, 0), vec![("abcdef".to_string(), RichMarks::none())]);
    }

    #[test]
    fn an_empty_selection_changes_nothing() {
        let mut doc = one("abcdef");
        let before = doc.clone();
        doc.set_mark(span(3, 3), Mark::Bold, true);
        assert_eq!(doc, before);
        doc.set_link(span(3, 3), Some("http://example.invalid"));
        assert_eq!(doc, before, "and neither does an empty link");
    }

    #[test]
    fn two_links_side_by_side_stay_two_runs() {
        let mut doc = one("hereand there");
        doc.set_link(span(0, 4), Some("one"));
        doc.set_link(span(4, 7), Some("two"));
        let r = runs(&doc, 0);
        assert_eq!(r.len(), 3);
        assert_eq!(r[0].1.link(), Some("one"));
        assert_eq!(r[1].1.link(), Some("two"), "a different target is a different run");
        // The same target either side of the seam is one link again.
        doc.set_link(span(0, 7), Some("one"));
        assert_eq!(runs(&doc, 0).len(), 2);
    }

    #[test]
    fn a_mark_over_several_blocks_stops_at_the_ends_of_the_span() {
        let mut doc = doc_of("one\n\ntwo\n\nthree");
        assert_eq!(doc.blocks.len(), 3);
        doc.set_mark(
            Span::new(Pos::new(0, 1), Pos::new(2, 2)),
            Mark::Bold,
            true,
        );
        assert_eq!(runs(&doc, 0)[0], ("o".to_string(), RichMarks::none()));
        assert!(runs(&doc, 0)[1].1.has(Mark::Bold));
        assert!(runs(&doc, 1)[0].1.has(Mark::Bold), "the whole middle block");
        assert_eq!(runs(&doc, 1).len(), 1);
        assert!(runs(&doc, 2)[0].1.has(Mark::Bold));
        assert_eq!(runs(&doc, 2)[1], ("ree".to_string(), RichMarks::none()));
    }

    #[test]
    fn a_toggle_asks_whether_everything_already_has_it() {
        let mut doc = one("one two");
        doc.set_mark(span(0, 3), Mark::Bold, true);
        assert!(!doc.has_mark(span(0, 7), Mark::Bold), "half of it is not all of it");
        assert!(doc.toggle_mark(span(0, 7), Mark::Bold), "so the toggle put it on");
        assert!(doc.has_mark(span(0, 7), Mark::Bold));
        assert!(!doc.toggle_mark(span(0, 7), Mark::Bold), "and again took it off");
    }

    #[test]
    fn typing_at_the_end_of_a_link_is_not_part_of_the_link() {
        let mut doc = one("here");
        doc.set_link(span(0, 4), Some("target"));
        doc.set_mark(span(0, 4), Mark::Bold, true);
        let marks = doc.marks_at(Pos::new(0, 4));
        assert!(marks.has(Mark::Bold), "the bold carries on");
        assert_eq!(marks.link(), None, "the link does not");
        assert_eq!(
            doc.marks_at(Pos::new(0, 2)).link(),
            Some("target"),
            "inside it, it does"
        );
    }

    #[test]
    fn inserting_carries_the_marks_it_was_given_and_merges() {
        let mut doc = one("ac");
        doc.set_mark(span(0, 2), Mark::Bold, true);
        let marks = doc.marks_at(Pos::new(0, 1));
        let at = doc.replace(Span::at(Pos::new(0, 1)), "b", &marks);
        assert_eq!(at, Pos::new(0, 2));
        assert_eq!(runs(&doc, 0).len(), 1, "the same marks, so one run");
        assert_eq!(doc.text(), "abc");
    }

    #[test]
    fn deleting_across_blocks_joins_what_is_left_of_them() {
        let mut doc = doc_of("one\n\ntwo\n\nthree");
        let at = doc.delete(Span::new(Pos::new(0, 2), Pos::new(2, 2)));
        assert_eq!(at, Pos::new(0, 2));
        assert_eq!(doc.blocks.len(), 1);
        assert_eq!(doc.text(), "onree");
    }

    #[test]
    fn return_splits_a_block_and_the_tail_of_a_heading_is_a_paragraph() {
        let mut doc = doc_of("# Title");
        let at = doc.split(Pos::new(0, 7));
        assert_eq!(at, Pos::new(1, 0));
        assert_eq!(doc.blocks[0].kind, BlockKind::Heading(1));
        assert_eq!(doc.blocks[1].kind, BlockKind::Paragraph, "what follows a title is text");

        // At the head of one it is the other gesture: the heading is pushed
        // down and a paragraph is left above it.
        let mut doc = doc_of("# Title");
        doc.split(Pos::new(0, 0));
        assert_eq!(doc.blocks[0].kind, BlockKind::Paragraph);
        assert_eq!(doc.blocks[1].kind, BlockKind::Heading(1));
        assert_eq!(doc.blocks[1].text(), "Title");
    }

    #[test]
    fn return_on_an_empty_item_ends_the_list() {
        // What pressing Return at the end of "- one" leaves behind: a second,
        // empty item with the caret in it.
        let mut doc = RichDoc {
            blocks: vec![
                Block::plain(BlockKind::Bullet, "one"),
                Block::new(BlockKind::Bullet),
            ],
        };
        assert_eq!(doc.blocks.len(), 2);
        assert_eq!(doc.blocks[1].kind, BlockKind::Bullet);
        let at = doc.split(Pos::new(1, 0));
        assert_eq!(at, Pos::new(1, 0), "no new block, just a different kind");
        assert_eq!(doc.blocks.len(), 2);
        assert_eq!(doc.blocks[1].kind, BlockKind::Paragraph);
    }

    #[test]
    fn return_in_a_code_block_is_a_line_break() {
        let mut doc = doc_of("```\nlet a = 1;\n```");
        assert_eq!(doc.blocks.len(), 1);
        let at = doc.split(Pos::new(0, 10));
        assert_eq!(doc.blocks.len(), 1, "a code block holds its own lines");
        assert_eq!(at, Pos::new(0, 11));
        assert_eq!(doc.blocks[0].text(), "let a = 1;\n");
    }

    #[test]
    fn backspace_at_the_head_of_a_block_takes_the_kind_off_before_the_text() {
        let mut doc = doc_of("one\n\n- two");
        let at = doc.backspace(Pos::new(1, 0));
        assert_eq!(at, Pos::new(1, 0));
        assert_eq!(doc.blocks.len(), 2, "the item is still its own block");
        assert_eq!(doc.blocks[1].kind, BlockKind::Paragraph);
        // Pressed again, now that it is a paragraph, it joins the one above.
        let at = doc.backspace(Pos::new(1, 0));
        assert_eq!(at, Pos::new(0, 3));
        assert_eq!(doc.blocks.len(), 1);
        assert_eq!(doc.text(), "onetwo");
    }

    #[test]
    fn a_paragraph_that_swallows_a_code_block_loses_its_line_breaks() {
        // Nothing outside a code block may hold a line break: the block's
        // byte offsets and what is drawn have to agree.
        let mut doc = doc_of("one\n\n```\na\nb\n```");
        assert_eq!(doc.blocks.len(), 2);
        doc.delete_forward(Pos::new(0, 3));
        assert_eq!(doc.blocks.len(), 1);
        assert!(!doc.blocks[0].text().contains('\n'));
        assert_eq!(doc.text(), "onea b");
    }

    #[test]
    fn a_code_block_turned_into_paragraphs_becomes_one_block_per_line() {
        let mut doc = doc_of("```\na\nb\nc\n```");
        doc.set_kind(Span::at(Pos::new(0, 0)), BlockKind::Paragraph);
        assert_eq!(doc.blocks.len(), 3);
        assert_eq!(doc.blocks[2].text(), "c");
        assert!(doc.blocks.iter().all(|b| b.kind == BlockKind::Paragraph));
    }

    #[test]
    fn a_kind_change_covers_every_block_the_selection_touches() {
        let mut doc = doc_of("one\n\ntwo\n\nthree");
        doc.set_kind(Span::new(Pos::new(0, 1), Pos::new(1, 1)), BlockKind::Bullet);
        assert_eq!(doc.blocks[0].kind, BlockKind::Bullet);
        assert_eq!(doc.blocks[1].kind, BlockKind::Bullet);
        assert_eq!(doc.blocks[2].kind, BlockKind::Paragraph, "and no further");
    }

    #[test]
    fn a_numbered_list_starts_again_after_anything_else() {
        let doc = doc_of("1. a\n1. b\n\ntext\n\n1. c");
        let numbers = list_numbers(&doc.blocks);
        assert_eq!(numbers, vec![1, 2, 0, 1]);
    }

    #[test]
    fn two_items_of_one_list_sit_together_and_everything_else_gets_room() {
        assert_eq!(gap_above(BlockKind::Bullet, BlockKind::Bullet, 12.0), 0.0);
        assert_eq!(gap_above(BlockKind::Bullet, BlockKind::Numbered, 12.0), 12.0);
        assert_eq!(gap_above(BlockKind::Paragraph, BlockKind::Paragraph, 12.0), 12.0);
    }

    #[test]
    fn markup_survives_a_round_trip() {
        let src = "# Title\n\nSome **bold** and *italic* and `code` and [a link](target).\n\n- one\n- two\n\n1. first\n1. second\n\n> quoted\n\n```\nlet a = 1;\nlet b = 2;\n```";
        let doc = RichDoc::from_markup(src);
        let again = RichDoc::from_markup(&doc.to_markup());
        assert_eq!(doc, again, "what it writes it can read");
        assert_eq!(doc.blocks.len(), 8);
        assert_eq!(doc.blocks[0].kind, BlockKind::Heading(1));
        assert_eq!(doc.blocks[2].kind, BlockKind::Bullet);
        assert_eq!(doc.blocks[4].kind, BlockKind::Numbered);
        assert_eq!(doc.blocks[6].kind, BlockKind::Quote);
        assert_eq!(doc.blocks[7].kind, BlockKind::Code);
        assert_eq!(doc.blocks[7].text(), "let a = 1;\nlet b = 2;");
    }

    #[test]
    fn a_paragraph_about_a_star_still_has_a_star_in_it() {
        let doc = one("2 * 3 and _this_ and [that]");
        let again = RichDoc::from_markup(&doc.to_markup());
        assert_eq!(again.text(), "2 * 3 and _this_ and [that]");
    }

    #[test]
    fn a_paragraph_that_starts_with_a_dash_is_not_a_list_when_it_comes_back() {
        let doc = one("- not a list");
        let again = RichDoc::from_markup(&doc.to_markup());
        assert_eq!(again.blocks[0].kind, BlockKind::Paragraph);
        assert_eq!(again.text(), "- not a list");
    }

    #[test]
    fn underline_is_written_down_because_the_markup_has_no_mark_for_it() {
        let mut doc = one("under");
        doc.set_mark(span(0, 5), Mark::Underline, true);
        assert_eq!(doc.to_markup(), "<u>under</u>");
        let again = RichDoc::from_markup(&doc.to_markup());
        assert!(again.blocks[0].runs[0].marks.has(Mark::Underline));
    }

    #[test]
    fn a_link_keeps_its_target_through_the_markup() {
        let mut doc = one("click here");
        doc.set_link(span(6, 10), Some("somewhere"));
        let again = RichDoc::from_markup(&doc.to_markup());
        assert_eq!(again.text(), "click here");
        assert_eq!(again.blocks[0].runs[1].marks.link(), Some("somewhere"));
    }

    #[test]
    fn word_movement_stops_where_the_words_do() {
        let doc = one("one two  three");
        assert_eq!(doc.right(Pos::new(0, 0), true), Pos::new(0, 4));
        assert_eq!(doc.right(Pos::new(0, 4), true), Pos::new(0, 9));
        assert_eq!(doc.left(Pos::new(0, 9), true), Pos::new(0, 4));
        assert_eq!(doc.left(Pos::new(0, 3), true), Pos::new(0, 0));
    }

    #[test]
    fn moving_off_the_end_of_a_block_is_the_next_block() {
        let doc = doc_of("one\n\ntwo");
        assert_eq!(doc.right(Pos::new(0, 3), false), Pos::new(1, 0));
        assert_eq!(doc.left(Pos::new(1, 0), false), Pos::new(0, 3));
        assert_eq!(doc.left(Pos::ZERO, false), Pos::ZERO, "and the start stays put");
        let end = doc.end();
        assert_eq!(doc.right(end, false), end);
    }

    #[test]
    fn a_position_inside_a_character_is_moved_to_its_edge() {
        // Every position here arrives from a hit test or a key press, and
        // both can land inside a multi-byte character; slicing there panics.
        let doc = one("naïve");
        let inside = Pos::new(0, 3);
        assert_eq!(doc.clamp(inside), Pos::new(0, 2));
        let mut doc = doc;
        doc.set_mark(span(0, 3), Mark::Bold, true);
        assert_eq!(runs(&doc, 0)[0].0, "na");
    }

    #[test]
    fn the_text_of_a_span_is_the_text_between_its_ends() {
        let doc = doc_of("one\n\ntwo\n\nthree");
        assert_eq!(doc.text_in(Span::new(Pos::new(0, 1), Pos::new(2, 2))), "ne\ntwo\nth");
        assert_eq!(doc.text_in(Span::at(Pos::new(1, 1))), "");
        assert_eq!(doc.text(), "one\ntwo\nthree");
    }

    #[test]
    fn a_document_always_has_somewhere_to_put_the_caret() {
        let doc = RichDoc::from_markup("");
        assert_eq!(doc.blocks.len(), 1);
        assert!(doc.is_empty());
        let mut doc = doc_of("one");
        doc.delete(doc.all());
        assert_eq!(doc.blocks.len(), 1);
        assert_eq!(doc.clamp(Pos::new(9, 9)), Pos::ZERO);
    }

    // --- the map from what the flow drew back into the document ------------

    fn piece(block: usize, byte: usize, len: usize, flat: usize, flat_len: usize) -> Piece {
        Piece {
            block,
            byte,
            len,
            flat,
            flat_len,
        }
    }

    #[test]
    fn a_flat_index_lands_in_the_piece_that_covers_it() {
        // Two blocks: "one" and "two", with the newline the flow puts
        // between them taking flat index 3.
        let map = [piece(0, 0, 3, 0, 3), piece(1, 0, 3, 4, 3)];
        assert_eq!(pos_at_flat(&map, 0), Pos::new(0, 0));
        assert_eq!(pos_at_flat(&map, 2), Pos::new(0, 2));
        assert_eq!(pos_at_flat(&map, 3), Pos::new(0, 3), "the end of the first");
        assert_eq!(pos_at_flat(&map, 4), Pos::new(1, 0));
        assert_eq!(pos_at_flat(&map, 7), Pos::new(1, 3));
        assert_eq!(pos_at_flat(&map, 99), Pos::new(1, 3), "past everything is the end");
    }

    #[test]
    fn a_position_and_a_flat_index_agree_with_each_other() {
        let map = [piece(0, 0, 3, 0, 3), piece(1, 0, 3, 4, 3)];
        for (block, byte) in [(0, 0), (0, 1), (0, 3), (1, 0), (1, 2), (1, 3)] {
            let pos = Pos::new(block, byte);
            assert_eq!(pos_at_flat(&map, flat_at_pos(&map, pos)), pos);
        }
    }

    #[test]
    fn a_run_the_caret_splits_is_two_pieces_of_one_block() {
        // What the draw records when the caret sits inside "hello": the
        // halves are separate pieces, and the map still reads as one run.
        let map = [piece(0, 0, 2, 0, 2), piece(0, 2, 3, 2, 3)];
        assert_eq!(pos_at_flat(&map, 2), Pos::new(0, 2));
        assert_eq!(flat_at_pos(&map, Pos::new(0, 4)), 4);
    }

    #[test]
    fn whitespace_the_flow_trimmed_does_not_shift_the_map() {
        // The flow drops the spaces at the head of a line, so the piece is
        // shorter in its buffer than in the document; a click on the first
        // glyph has to land after them, not before.
        let map = [piece(0, 0, 5, 0, 3)];
        assert_eq!(pos_at_flat(&map, 0), Pos::new(0, 2), "two spaces were trimmed");
        assert_eq!(pos_at_flat(&map, 3), Pos::new(0, 5));
        assert_eq!(flat_at_pos(&map, Pos::new(0, 2)), 0);
        assert_eq!(flat_at_pos(&map, Pos::new(0, 0)), 0);
    }

    #[test]
    fn a_click_on_a_bullet_is_the_head_of_its_item() {
        // The bullet went through the flow's own drawing, so it takes room
        // in the flat buffer that belongs to no run: it is recorded as a
        // piece of no length at the head of the block.
        let map = [
            piece(0, 0, 3, 0, 3),
            piece(1, 0, 0, 4, 1),
            piece(1, 0, 4, 5, 4),
        ];
        assert_eq!(pos_at_flat(&map, 4), Pos::new(1, 0), "on the bullet");
        assert_eq!(pos_at_flat(&map, 5), Pos::new(1, 0), "on the first letter");
        assert_eq!(pos_at_flat(&map, 7), Pos::new(1, 2));
        assert_eq!(
            flat_at_pos(&map, Pos::new(1, 0)),
            5,
            "and the highlight starts at the text, not at the bullet"
        );
    }

    #[test]
    fn an_empty_map_answers_the_start_rather_than_reaching_into_it() {
        assert_eq!(pos_at_flat(&[], 12), Pos::ZERO);
        assert_eq!(flat_at_pos(&[], Pos::new(3, 4)), 0);
    }

    #[test]
    fn a_heading_is_bigger_than_the_body_and_stops_getting_bigger() {
        assert_eq!(heading_scale(1, 1.8), 1.8);
        assert!(heading_scale(2, 1.8) < heading_scale(1, 1.8));
        assert_eq!(heading_scale(9, 1.8), heading_scale(3, 1.8), "clamped, not scaled away");
    }
}
