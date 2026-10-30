//! `FollowingText`, the text after a run in its line, so Html's runs break like one paragraph.

use std::{cell::RefCell, ops::Deref, rc::Rc};

use unicode_segmentation::UnicodeSegmentation;

use super::{element_text, InlineStyleTag, SUB_SUP_FONT_SIZE_SCALE};
use crate::{
    makepad_draw::text::{
        font_family::FontFamilyId,
        layouter::is_hangable_white_space,
        line_break::{
            find_first_line_break, find_line_breaks, FollowingContext, PrecedingContext, UNITS_READ_AHEAD,
        },
    },
    makepad_draw::*,
    makepad_html::*,
    text_flow::{trim_trailing_line_breaks, RunStyle, TextFlow},
};

/// Returns whether a tag ends the line of text before it: blocks, `<br>`, and details and their summaries.
fn is_block_tag(lc: LiveId) -> bool {
    matches!(
        lc,
        live_id!(h1) | live_id!(h2) | live_id!(h3) | live_id!(h4) | live_id!(h5) | live_id!(h6)
            | live_id!(p) | live_id!(pre) | live_id!(blockquote) | live_id!(br) | live_id!(hr) | live_id!(sep)
            | live_id!(ul) | live_id!(ol) | live_id!(li)
            | live_id!(table) | live_id!(thead) | live_id!(tbody) | live_id!(tr) | live_id!(th) | live_id!(td)
            | live_id!(details) | live_id!(summary)
    )
}

/// Tells `tf` what follows a run of `text` in its line, which goes on at `node_index` in `doc`.
///
/// Runs in the summary of a collapsed `<details>` go on at `collapsed_details_end` instead.
pub(super) fn set_line_break_after_run(
    cx: &mut Cx2d,
    tf: &mut TextFlow,
    doc: &HtmlDoc,
    node_index: usize,
    text: &str,
    collapsed_details_end: Option<usize>,
    text_after_run_cache: &RefCell<TextAfterRunCache>,
) {
    let inputs = TextAfterRunInputs::new(cx, tf);
    let font_family_generation = inputs.font_family_generation;
    let mut text_after_run_cache = text_after_run_cache.borrow_mut();
    let (line_break_after_run, following_room) = match text_after_run_cache.get(node_index) {
        Some(CachedTextAfterRun {
            inputs: cached_inputs,
            line_break_after_run: Some(line_break_after_run),
            following_room,
        }) if *cached_inputs == inputs => (*line_break_after_run, *following_room),
        _ => {
            let inputs = inputs.to_static();
            (find_line_break_after_run(cx, tf, doc, node_index, text, inputs, &mut text_after_run_cache), None)
        }
    };
    line_break_after_run.set_on(tf);
    if tf.has_ellipsis_at_max_lines() {
        let cached_room = following_room.and_then(|room| room.get(collapsed_details_end));
        tf.set_following_room_lazily(PendingFollowingRoom {
            node_index,
            collapsed_details_end,
            font_family_generation,
            runs: PrecedingRuns::TextNode { cached_room },
        });
    }
}

/// Tells `tf` what follows the runs that a custom element's widget draws, in its line.
///
/// Many widgets draw no runs, like pills, so we only look once the widget draws one.
pub(super) fn set_line_break_after_widget(
    cx: &mut Cx2d,
    tf: &mut TextFlow,
    doc: &Rc<HtmlDoc>,
    widget_runs: WidgetRuns,
    collapsed_details_end: Option<usize>,
    text_after_run_cache: &Rc<RefCell<TextAfterRunCache>>,
) {
    if tf.has_ellipsis_at_max_lines() {
        // The widget's runs before its own text are followed by that text.
        tf.set_following_room_lazily(PendingFollowingRoom {
            node_index: widget_runs.open_index + 1,
            collapsed_details_end,
            font_family_generation: cx.fonts.borrow().font_family_generation(),
            runs: PrecedingRuns::Widget(widget_runs),
        });
    }
    let node_index = widget_runs.node_index_after_widget;
    let inputs = TextAfterRunInputs::new(cx, tf);
    let mut cache = text_after_run_cache.borrow_mut();
    match cache.get(node_index) {
        Some(cached) if cached.inputs == inputs => {
            if let Some(line_break_after_run) = cached.line_break_after_run {
                line_break_after_run.set_on(tf);
                return;
            }
        }
        // The widget can push styles of its own before its first run, so we keep the inputs from now.
        _ => cache.insert(node_index, CachedTextAfterRun::new(inputs.to_static(), None)),
    }
    tf.set_line_break_after_lazily(PendingTextAfterRun {
        doc: doc.clone(),
        text_after_run_cache: text_after_run_cache.clone(),
        widget_runs,
    });
}

/// Tells `tf` the room that what follows a widget that drew no runs needs on its row.
///
/// What follows goes on at `node_index` in `doc`, and `collapsed_details_end` works like it does for a run.
pub(super) fn set_following_room_after_widget(
    cx: &mut Cx2d,
    tf: &mut TextFlow,
    doc: &HtmlDoc,
    node_index: usize,
    collapsed_details_end: Option<usize>,
    following_room_cache: &RefCell<FollowingRoomCache>,
) {
    let inputs = TextAfterRunInputs {
        line_break_before_run: PrecedingContext::AFTER_ATOMIC_INLINE,
        ..TextAfterRunInputs::new(cx, tf)
    };
    let cached_room = following_room_cache.borrow().get(node_index).and_then(|cached| cached.get_room(&inputs, collapsed_details_end));
    let following_room_in_lpxs = cached_room.unwrap_or_else(|| {
        let inputs = inputs.to_static();
        let line_break_before = inputs.line_break_before_run;
        let following_room_in_lpxs =
            find_following_room(cx, tf, doc, node_index, line_break_before, &inputs, collapsed_details_end);
        if !TextAfterRunStyles::new(tf).is_font_loading(cx) {
            let room = FollowingRoom { collapsed_details_end, following_room_in_lpxs };
            following_room_cache.borrow_mut().insert(node_index, CachedFollowingRoom { inputs, room });
        }
        following_room_in_lpxs
    });
    tf.set_following_room(following_room_in_lpxs);
}

/// What we need to find what follows a widget's runs in their line, which we do once it draws one.
pub(crate) struct PendingTextAfterRun {
    doc: Rc<HtmlDoc>,
    text_after_run_cache: Rc<RefCell<TextAfterRunCache>>,
    widget_runs: WidgetRuns,
}

impl PendingTextAfterRun {
    /// Finds what follows the widget's runs, and tells `tf`.
    pub(crate) fn resolve(self, cx: &mut Cx2d, tf: &mut TextFlow) {
        let WidgetRuns { open_index, close_index, node_index_after_widget } = self.widget_runs;
        let mut text_after_run_cache = self.text_after_run_cache.borrow_mut();
        // The widget's entry has the inputs from before it drew anything.
        let Some(cached) = text_after_run_cache.get(node_index_after_widget) else {
            return;
        };
        let inputs = cached.inputs.to_static();
        let label = element_text(&self.doc, open_index, close_index);
        find_line_break_after_run(cx, tf, &self.doc, node_index_after_widget, &label, inputs, &mut text_after_run_cache)
            .set_on(tf);
    }
}

/// Where what follows TextFlow's next runs starts, for finding the room it needs on a run's last row.
///
/// Only a run that ends on the last line `max_lines` allows needs it, so we look once one does.
#[derive(Clone, Copy)]
pub(crate) struct PendingFollowingRoom {
    /// Where what follows the next run starts in the document.
    node_index: usize,
    collapsed_details_end: Option<usize>,
    /// The fonts' generation when we set this, so we can tell when a font came since.
    pub(crate) font_family_generation: u64,
    runs: PrecedingRuns,
}

#[derive(Clone, Copy)]
enum PrecedingRuns {
    /// A text node's run, with the room its entry in the text-after-run cache has.
    TextNode { cached_room: Option<f32> },
    Widget(WidgetRuns),
}

/// The element of a widget that draws runs, and the node where its line goes on after it.
#[derive(Clone, Copy)]
pub(super) struct WidgetRuns {
    pub(super) open_index: usize,
    pub(super) close_index: usize,
    pub(super) node_index_after_widget: usize,
}

/// Lets TextFlow find the room after its runs while Html draws `doc`.
pub(crate) struct FollowingRoomFinder {
    doc: Rc<HtmlDoc>,
    text_after_run_cache: Rc<RefCell<TextAfterRunCache>>,
    following_room_cache: Rc<RefCell<FollowingRoomCache>>,
}

impl FollowingRoomFinder {
    pub(super) fn new(
        doc: &Rc<HtmlDoc>,
        text_after_run_cache: &Rc<RefCell<TextAfterRunCache>>,
        following_room_cache: &Rc<RefCell<FollowingRoomCache>>,
    ) -> Self {
        Self {
            doc: doc.clone(),
            text_after_run_cache: text_after_run_cache.clone(),
            following_room_cache: following_room_cache.clone(),
        }
    }

    /// Moves `pending` on to the next run, a run of `text`, which is what follows once it's the widget's own text.
    pub(crate) fn begin_run(&self, pending: &mut PendingFollowingRoom, text: &str) {
        let PrecedingRuns::Widget(widget_runs) = pending.runs else {
            return;
        };
        if pending.node_index != widget_runs.node_index_after_widget
            && element_text(&self.doc, widget_runs.open_index, widget_runs.close_index).ends_with(text)
        {
            pending.node_index = widget_runs.node_index_after_widget;
        }
    }

    /// Returns the room that what follows `pending`'s run needs, when we found it on an earlier draw.
    pub(crate) fn get_cached_room(&self, cx: &Cx2d, tf: &TextFlow, pending: &PendingFollowingRoom) -> Option<f32> {
        if let PrecedingRuns::TextNode { cached_room } = pending.runs {
            // The cache checked the run's inputs before it drew, and only a font asked for since can change them.
            return cached_room.filter(|_| cx.fonts.borrow().font_family_generation() == pending.font_family_generation);
        }
        // Most runs never end on the last line, so they have no room to check the inputs of.
        let following_room_cache = self.following_room_cache.borrow();
        let cached = following_room_cache.get(pending.node_index)?;
        let text_after_run_cache = self.text_after_run_cache.borrow();
        cached.get_room(&get_room_inputs(cx, tf, pending, &text_after_run_cache)?, pending.collapsed_details_end)
    }

    /// Finds the room that what follows `pending`'s run, a run of `text`, needs, and keeps it for later draws.
    pub(crate) fn find_room(&self, cx: &mut Cx2d, tf: &TextFlow, pending: &PendingFollowingRoom, text: &str) -> f32 {
        let Some(inputs) = get_room_inputs(cx, tf, pending, &self.text_after_run_cache.borrow()).map(|inputs| inputs.to_static())
        else {
            return 0.0;
        };
        let mut line_break_before = inputs.line_break_before_run;
        line_break_before.append(trim_trailing_line_breaks(text));
        let collapsed_details_end = pending.collapsed_details_end;
        let following_room_in_lpxs =
            find_following_room(cx, tf, &self.doc, pending.node_index, line_break_before, &inputs, collapsed_details_end);
        if !TextAfterRunStyles::new(tf).is_font_loading(cx) {
            let room = FollowingRoom { collapsed_details_end, following_room_in_lpxs };
            match pending.runs {
                // A text node's run keeps its room with what follows it, while that's for the same inputs.
                PrecedingRuns::TextNode { .. } => {
                    let mut text_after_run_cache = self.text_after_run_cache.borrow_mut();
                    let cached = text_after_run_cache.get_mut(pending.node_index);
                    if let Some(cached) = cached.filter(|cached| cached.inputs == inputs) {
                        cached.following_room = Some(room);
                    }
                }
                PrecedingRuns::Widget(_) => {
                    let cached = CachedFollowingRoom { inputs, room };
                    self.following_room_cache.borrow_mut().insert(pending.node_index, cached);
                }
            }
        }
        following_room_in_lpxs
    }
}

/// Returns what the room after `pending`'s run depends on.
///
/// A widget's runs start from the stacks in its entry in `text_after_run_cache`, from before it drew anything.
fn get_room_inputs<'a>(
    cx: &Cx2d,
    tf: &'a TextFlow,
    pending: &PendingFollowingRoom,
    text_after_run_cache: &'a TextAfterRunCache,
) -> Option<TextAfterRunInputs<'a>> {
    let inputs = TextAfterRunInputs::new(cx, tf);
    let PrecedingRuns::Widget(widget_runs) = pending.runs else {
        return Some(inputs);
    };
    let stacks = text_after_run_cache.get(widget_runs.node_index_after_widget)?.inputs.stacks.reborrow();
    Some(TextAfterRunInputs { stacks, ..inputs })
}

/// Finds what follows a run of `text` in its line, which goes on at `node_index` in `doc`.
///
/// We keep what we found in the cache, along with what we found for the runs that end inside its glue.
fn find_line_break_after_run(
    cx: &mut Cx2d,
    tf: &mut TextFlow,
    doc: &HtmlDoc,
    node_index: usize,
    text: &str,
    inputs: TextAfterRunInputs<'static>,
    text_after_run_cache: &mut TextAfterRunCache,
) -> LineBreakAfterRun {
    let mut line_break_before = inputs.line_break_before_run;
    line_break_before.append(trim_trailing_line_breaks(text));
    let text_after = FollowingText::new(
        doc,
        node_index,
        inputs.stacks.reborrow(),
        inputs.is_in_table,
        |template| tf.has_template(template),
    )
    .find_text_after_run(line_break_before);
    let glue = &text_after.glue;
    // Most runs have nothing that has to stay with them, so nothing to measure with any font.
    if glue.pieces.is_empty() && text_after.runs_in_glue.is_empty() {
        let line_break_after_run = LineBreakAfterRun {
            line_break_after: text_after.line_break_after,
            trailing_glue_width_in_lpxs: 0.0,
            is_trailing_glue_in_inline_code: false,
        };
        let cached = CachedTextAfterRun::new(inputs, Some(line_break_after_run));
        text_after_run_cache.insert(node_index, cached);
        return line_break_after_run;
    }
    let styles = TextAfterRunStyles::new(tf);
    let glue_piece_widths = measure_glue_pieces(cx, tf, doc, glue);
    let (glue_width_in_lpxs, glue_end_padding_in_lpxs) =
        add_up_glue_unit(glue, &glue_piece_widths, &styles.code_box, 0);
    let row_end_width_in_lpxs = if let Some(style) = glue.soft_hyphen_style {
        // A row that ends at the soft hyphen gets a hyphen there, and otherwise the next unit
        // follows it, so the narrower of the two is what always has to fit.
        let unit = &text_after.unit_after_soft_hyphen;
        let unit_piece_widths = measure_glue_pieces(cx, tf, doc, unit);
        let (unit_width_in_lpxs, unit_end_padding_in_lpxs) =
            add_up_glue_unit(unit, &unit_piece_widths, &styles.code_box, 0);
        let unit_hyphen_width_in_lpxs =
            unit.soft_hyphen_style.map_or(0.0, |unit_style| tf.measure_hyphen_in_lpxs(cx, unit_style));
        // A unit that's only white space hangs, so the row's text still ends at the soft hyphen.
        let unit_end_padding_in_lpxs = if unit.pieces.is_empty() {
            glue_end_padding_in_lpxs
        } else {
            unit_end_padding_in_lpxs
        };
        (tf.measure_hyphen_in_lpxs(cx, style) + glue_end_padding_in_lpxs)
            .min(unit_width_in_lpxs + unit_hyphen_width_in_lpxs + unit_end_padding_in_lpxs)
    } else {
        glue_end_padding_in_lpxs
    };
    let line_break_after_run = LineBreakAfterRun {
        line_break_after: text_after.line_break_after,
        trailing_glue_width_in_lpxs: glue_width_in_lpxs + row_end_width_in_lpxs,
        is_trailing_glue_in_inline_code: glue.pieces.first().is_some_and(|piece| piece.is_inline_code),
    };
    // A font that's still on its way would change what we measured, so we measure again until it's here.
    if !styles.is_font_loading(cx) {
        // The runs that end inside our glue have the rest of it as their own glue, so we keep that
        // for them too. We read the text again for the stacks each one starts with and where it ends.
        if !text_after.runs_in_glue.is_empty() {
            let mut following_text = FollowingText::new(
                doc,
                node_index,
                inputs.stacks.reborrow(),
                inputs.is_in_table,
                |template| tf.has_template(template),
            );
            for run in &text_after.runs_in_glue {
                following_text.get_piece(run.first_piece_index);
                let stacks = following_text.stacks.to_static();
                following_text.get_piece(run.glue_start_index - 1);
                // The rest of the glue can be only white space, which hangs.
                let has_glue = run.glue_start_index < glue.pieces.len();
                let (glue_width_in_lpxs, _) =
                    add_up_glue_unit(glue, &glue_piece_widths, &styles.code_box, run.glue_start_index);
                let line_break_after_run = LineBreakAfterRun {
                    line_break_after: run.line_break_after,
                    trailing_glue_width_in_lpxs: if has_glue { glue_width_in_lpxs + row_end_width_in_lpxs } else { 0.0 },
                    is_trailing_glue_in_inline_code: has_glue && glue.pieces[run.glue_start_index].is_inline_code,
                };
                let inputs = TextAfterRunInputs { line_break_before_run: run.line_break_before_run, stacks, ..inputs };
                let cached = CachedTextAfterRun::new(inputs, Some(line_break_after_run));
                text_after_run_cache.insert(following_text.node.index, cached);
            }
        }
        let cached = CachedTextAfterRun::new(inputs, Some(line_break_after_run));
        text_after_run_cache.insert(node_index, cached);
    }
    line_break_after_run
}

/// Measures each piece of a unit of glue in the style it's drawn with.
fn measure_glue_pieces(cx: &mut Cx2d, tf: &mut TextFlow, doc: &HtmlDoc, unit: &GlueUnit) -> SmallVec<[f32; 4]> {
    let mut widths = SmallVec::new();
    for piece in &unit.pieces {
        widths.push(tf.measure_text_in_lpxs(cx, piece.get_text(doc), piece.style));
    }
    widths
}

/// Adds up the room a unit of glue takes on its row from piece `start_index` on, inline code boxes included.
///
/// Also returns the room a row that ends with the unit keeps free after it, for a code box's padding.
fn add_up_glue_unit(unit: &GlueUnit, piece_widths: &[f32], code_box: &CodeBoxInsets, start_index: usize) -> (f32, f32) {
    let mut width_in_lpxs = 0.0;
    for (index, piece) in unit.pieces.iter().enumerate().skip(start_index) {
        // Every run of inline code gets a box of its own.
        if piece.is_inline_code && piece.is_run_start {
            width_in_lpxs += code_box.start_in_lpxs;
        }
        width_in_lpxs += piece_widths[index];
        if piece.is_inline_code && unit.pieces.get(index + 1).is_some_and(|next| next.is_run_start) {
            width_in_lpxs += code_box.end_in_lpxs;
        }
    }
    let end_padding_in_lpxs = if unit.pieces.last().is_some_and(|piece| piece.is_inline_code) {
        code_box.padding_end_in_lpxs
    } else {
        0.0
    };
    (width_in_lpxs, end_padding_in_lpxs)
}

/// The style stacks of a TextFlow, as the tags after a run push and pop them.
#[derive(Clone, PartialEq)]
struct StyleStacks<'a> {
    bold: usize,
    italic: usize,
    fixed: usize,
    inline_code: usize,
    is_in_code_block: bool,
    font_size: f32,
    font_sizes: FontSizes<'a>,
    fixed_font_size_scale: f64,
}

impl StyleStacks<'_> {
    /// Returns a copy of the stacks that keeps its own font sizes, like the cache needs.
    fn to_static(&self) -> StyleStacks<'static> {
        let font_sizes = if self.font_sizes.is_empty() {
            FontSizes::Borrowed(&[])
        } else {
            FontSizes::Owned(SmallVec::from_slice(&self.font_sizes))
        };
        StyleStacks { font_sizes, ..*self }
    }

    /// Returns a copy of the stacks that borrows our font sizes.
    fn reborrow(&self) -> StyleStacks<'_> {
        StyleStacks { font_sizes: FontSizes::Borrowed(&self.font_sizes), ..*self }
    }

    fn get_run_style(&self) -> RunStyle {
        RunStyle {
            is_bold: self.bold > 0,
            is_italic: self.italic > 0,
            is_fixed: self.fixed > 0,
            font_size: *self.font_sizes.last().unwrap_or(&self.font_size),
        }
    }

    /// Does what Html's tag handlers do to TextFlow's stacks for an inline style tag.
    fn apply_style_tag(&mut self, tag: InlineStyleTag, is_open: bool) {
        let push_or_pop = |count: &mut usize| {
            *count = if is_open { *count + 1 } else { count.saturating_sub(1) };
        };
        let font_size_scale = match tag {
            InlineStyleTag::Bold => {
                push_or_pop(&mut self.bold);
                None
            }
            InlineStyleTag::Italic => {
                push_or_pop(&mut self.italic);
                None
            }
            InlineStyleTag::Code => {
                push_or_pop(&mut self.fixed);
                if !self.is_in_code_block {
                    push_or_pop(&mut self.inline_code);
                }
                Some(self.fixed_font_size_scale)
            }
            InlineStyleTag::Subscript | InlineStyleTag::Superscript => Some(SUB_SUP_FONT_SIZE_SCALE),
            InlineStyleTag::Underline | InlineStyleTag::Strikethrough => None,
        };
        if let Some(scale) = font_size_scale {
            if is_open {
                let font_size = self.font_sizes.last().unwrap_or(&self.font_size) * scale as f32;
                self.font_sizes.to_mut().push(font_size);
            } else {
                self.font_sizes.to_mut().pop();
            }
        }
    }
}

/// Font sizes that we either borrow from TextFlow or keep a copy of.
///
/// Both compare by their values, so a lookup with borrowed sizes matches the copy in the cache.
#[derive(Clone)]
enum FontSizes<'a> {
    Borrowed(&'a [f32]),
    Owned(SmallVec<[f32; 8]>),
}

impl FontSizes<'_> {
    /// Returns our own copy of the sizes, making it first if we borrowed them.
    fn to_mut(&mut self) -> &mut SmallVec<[f32; 8]> {
        if let Self::Borrowed(font_sizes) = *self {
            *self = Self::Owned(SmallVec::from_slice(font_sizes));
        }
        match self {
            Self::Owned(font_sizes) => font_sizes,
            Self::Borrowed(_) => unreachable!(),
        }
    }
}

impl Deref for FontSizes<'_> {
    type Target = [f32];

    fn deref(&self) -> &[f32] {
        match self {
            Self::Borrowed(font_sizes) => font_sizes,
            Self::Owned(font_sizes) => font_sizes,
        }
    }
}

impl PartialEq for FontSizes<'_> {
    fn eq(&self, other: &Self) -> bool {
        **self == **other
    }
}

/// A piece of the text after a run, which Html draws in one style.
#[derive(Clone, Copy, Debug, PartialEq)]
struct TextPiece {
    /// Where its text starts and ends in the document's decoded text.
    start_byte: usize,
    end_byte: usize,
    style: RunStyle,
    is_inline_code: bool,
    /// Whether the piece starts a run, rather than going on with the one before it.
    is_run_start: bool,
}

impl TextPiece {
    fn get_text<'a>(&self, doc: &'a HtmlDoc) -> &'a str {
        &doc.decoded[self.start_byte..self.end_byte]
    }
}

/// Where a unit starts among the pieces of the text after a run.
#[derive(Clone, Copy, Default)]
struct PiecePosition {
    piece_index: usize,
    byte_index: usize,
}

/// Text that has to stay on one row, from where it starts up to the next break opportunity.
#[derive(Default)]
struct GlueUnit {
    pieces: SmallVec<[TextPiece; 2]>,
    /// The style of the soft hyphen it ends with, when a row that ends there draws a hyphen.
    soft_hyphen_style: Option<RunStyle>,
}

/// What follows a run in its line.
struct TextAfterRun {
    line_break_after: FollowingContext,
    /// The text that has to stay with the run, when its end isn't a break opportunity.
    glue: GlueUnit,
    /// The unit after the glue, when the glue ends right after a soft hyphen.
    unit_after_soft_hyphen: GlueUnit,
    runs_in_glue: Vec<RunInGlue>,
}

/// A later run that ends inside the glue, whose own glue is the rest of it.
struct RunInGlue {
    /// Where its first piece is among the pieces after the run before the glue.
    first_piece_index: usize,
    line_break_before_run: PrecedingContext,
    line_break_after: FollowingContext,
    /// Where the rest of the glue starts among its pieces.
    glue_start_index: usize,
}

/// What the text after a run depends on, besides the document, where the run is in it, and `TextAfterRunStyles`.
#[derive(PartialEq)]
struct TextAfterRunInputs<'a> {
    line_break_before_run: PrecedingContext,
    stacks: StyleStacks<'a>,
    is_in_table: bool,
    font_family_generation: u64,
}

impl<'a> TextAfterRunInputs<'a> {
    /// Returns the inputs for the next run that `tf` draws.
    fn new(cx: &Cx2d, tf: &'a TextFlow) -> Self {
        Self {
            line_break_before_run: tf.line_break_before,
            stacks: StyleStacks {
                bold: tf.bold.value(),
                italic: tf.italic.value(),
                fixed: tf.fixed.value(),
                inline_code: tf.inline_code.value(),
                is_in_code_block: tf.code_block.value() > 0,
                font_size: tf.font_size,
                font_sizes: FontSizes::Borrowed(&tf.font_sizes),
                fixed_font_size_scale: tf.fixed_font_size_scale,
            },
            is_in_table: tf.table_num_columns > 0,
            // We read this before measuring, since measuring can load a fallback font and change it.
            font_family_generation: cx.fonts.borrow().font_family_generation(),
        }
    }

    /// Returns a copy of the inputs that keeps its own font sizes, like the cache needs.
    fn to_static(&self) -> TextAfterRunInputs<'static> {
        TextAfterRunInputs { stacks: self.stacks.to_static(), ..*self }
    }
}

/// The styles that the text after every run depends on, which only change between Html's draws.
#[derive(PartialEq)]
pub(super) struct TextAfterRunStyles {
    code_box: CodeBoxInsets,
    /// The font families of TextFlow's five text styles.
    font_family_ids: [FontFamilyId; 5],
}

impl TextAfterRunStyles {
    pub(super) fn new(tf: &TextFlow) -> Self {
        let font_scale = tf.draw_text.font_scale.max(0.0001) as f64;
        let code_padding_end_in_lpxs = (tf.inline_code_padding.right / font_scale) as f32;
        Self {
            code_box: CodeBoxInsets {
                start_in_lpxs: ((tf.inline_code_margin.left + tf.inline_code_padding.left) / font_scale) as f32,
                padding_end_in_lpxs: code_padding_end_in_lpxs,
                end_in_lpxs: code_padding_end_in_lpxs + (tf.inline_code_margin.right / font_scale) as f32,
            },
            font_family_ids: [
                tf.text_style_normal.font_family_id(),
                tf.text_style_italic.font_family_id(),
                tf.text_style_bold.font_family_id(),
                tf.text_style_bold_italic.font_family_id(),
                tf.text_style_fixed.font_family_id(),
            ],
        }
    }

    /// Whether one of our fonts is still on its way, which would change what we measure.
    fn is_font_loading(&self, cx: &Cx2d) -> bool {
        let fonts = cx.fonts.borrow();
        self.font_family_ids.iter().any(|&id| fonts.is_font_family_loading(id))
    }
}

/// The room a code box takes around its text, in lpxs.
#[derive(Clone, Copy, PartialEq)]
struct CodeBoxInsets {
    start_in_lpxs: f32,
    padding_end_in_lpxs: f32,
    end_in_lpxs: f32,
}

/// What TextFlow needs to know about the text after a run.
#[derive(Clone, Copy)]
struct LineBreakAfterRun {
    line_break_after: FollowingContext,
    trailing_glue_width_in_lpxs: f32,
    is_trailing_glue_in_inline_code: bool,
}

impl LineBreakAfterRun {
    fn set_on(self, tf: &mut TextFlow) {
        tf.set_line_break_after(self.line_break_after, self.trailing_glue_width_in_lpxs, self.is_trailing_glue_in_inline_code);
    }
}

/// Finds the room that what follows a run needs on the run's last row, which goes on at `node_index` in `doc`.
///
/// That's the rest of the line, or only what the next thing on it needs to end with the ellipsis itself.
/// When the run is in the summary of a collapsed `<details>`, Html goes on at `collapsed_details_end`.
fn find_following_room(
    cx: &mut Cx2d,
    tf: &TextFlow,
    doc: &HtmlDoc,
    node_index: usize,
    mut line_break_before: PrecedingContext,
    inputs: &TextAfterRunInputs<'_>,
    collapsed_details_end: Option<usize>,
) -> f32 {
    let has_template = |template| tf.has_template(template);
    let mut following_text =
        FollowingText::new(doc, node_index, inputs.stacks.reborrow(), inputs.is_in_table, has_template);
    let code_box = TextAfterRunStyles::new(tf).code_box;
    // The room the rest of the line takes so far, and up to the end of its last visible grapheme.
    let mut width_in_lpxs = 0.0;
    let mut visible_width_in_lpxs = 0.0;
    // The room the next thing needs for its first grapheme and the ellipsis, and for its first unit.
    let mut grapheme_room_in_lpxs = None;
    let mut unit_room_in_lpxs = None;
    let mut is_after_inline_code = false;
    let mut piece_index = 0;
    loop {
        while let Some(piece) = following_text.get_piece(piece_index) {
            piece_index += 1;
            // Html draws a run without the line breaks it ends with.
            let is_run_end = following_text.get_piece(piece_index).is_none_or(|next| next.is_run_start);
            let text = piece.get_text(doc);
            let text = if is_run_end { trim_trailing_line_breaks(text) } else { text };
            if text.is_empty() {
                continue;
            }
            let line_break_after = following_text.get_line_break_after(piece_index);
            if piece.is_run_start {
                if is_after_inline_code {
                    width_in_lpxs += code_box.end_in_lpxs;
                }
                if piece.is_inline_code {
                    width_in_lpxs += code_box.start_in_lpxs;
                }
            }
            is_after_inline_code = piece.is_inline_code;
            // An ellipsis in a code box has the box's padding after it.
            let padding_end_in_lpxs = if piece.is_inline_code { code_box.padding_end_in_lpxs } else { 0.0 };
            let mut unit_start = 0;
            let unit_ends = find_line_breaks(text, line_break_before, line_break_after)
                .filter(|line_break| line_break.byte_index > 0)
                .map(|line_break| (line_break.byte_index, Some(line_break)))
                .chain([(text.len(), None)]);
            for (unit_end, line_break) in unit_ends {
                if unit_end <= unit_start {
                    continue;
                }
                let newline_len = line_break.map_or(0, |line_break| line_break.newline_len);
                let unit = &text[unit_start..unit_end - newline_len];
                // We shape units like the layouter does, so it finds them shaped when it lays out the run.
                let (shaped_unit, font_size_in_lpxs) = tf.shape_text(cx, unit, piece.style);
                let glyphs = &shaped_unit.glyphs;
                let width_through = |cluster: usize| -> f32 {
                    glyphs.iter().filter(|glyph| glyph.cluster <= cluster).map(|glyph| glyph.advance_in_ems).sum::<f32>()
                        * font_size_in_lpxs
                };
                if grapheme_room_in_lpxs.is_none() {
                    // Which clusters have a glyph that takes room, so we don't search the glyphs for each grapheme.
                    let mut takes_room = SmallVec::<[bool; 64]>::from_elem(false, unit.len());
                    for glyph in glyphs.iter().filter(|glyph| glyph.advance_in_ems > 0.0) {
                        if let Some(cluster_takes_room) = takes_room.get_mut(glyph.cluster) {
                            *cluster_takes_room = true;
                        }
                    }
                    let first_grapheme = unit.grapheme_indices(true).find(|(grapheme_start, _)| {
                        takes_room[*grapheme_start] && !unit[*grapheme_start..].starts_with(char::is_whitespace)
                    });
                    if let Some((grapheme_start, grapheme)) = first_grapheme {
                        grapheme_room_in_lpxs = Some(
                            width_in_lpxs
                                + width_through(grapheme_start + grapheme.len() - 1)
                                + tf.measure_ellipsis_in_lpxs(cx, piece.style)
                                + padding_end_in_lpxs,
                        );
                    }
                }
                // White space at the end of a row hangs, and so does what takes no room after it.
                let last_cluster = glyphs
                    .iter()
                    .filter(|glyph| glyph.advance_in_ems > 0.0 && !unit[glyph.cluster..].starts_with(is_hangable_white_space))
                    .map(|glyph| glyph.cluster)
                    .max();
                if let Some(last_cluster) = last_cluster {
                    visible_width_in_lpxs = width_in_lpxs + width_through(last_cluster) + padding_end_in_lpxs;
                }
                width_in_lpxs += width_through(unit.len());
                unit_start = unit_end;
                if line_break.is_some() && grapheme_room_in_lpxs.is_some() && unit_room_in_lpxs.is_none() {
                    unit_room_in_lpxs = Some(visible_width_in_lpxs);
                }
                let is_forced_break = line_break.is_some_and(|line_break| line_break.is_forced);
                match (grapheme_room_in_lpxs, unit_room_in_lpxs) {
                    // The next thing ends with the ellipsis itself when a hard line break cuts the rest
                    // of the line, or when the rest is wider than what that needs.
                    (Some(grapheme_room), Some(unit_room))
                        if is_forced_break || visible_width_in_lpxs > grapheme_room.max(unit_room) =>
                    {
                        return grapheme_room.max(unit_room);
                    }
                    // Nothing shows before the hard line break, and what's after it gets cut.
                    _ if is_forced_break => return f32::INFINITY,
                    _ => {}
                }
            }
            line_break_before.append(text);
        }
        if following_text.line_end != Some(FollowingContext::BEFORE_ATOMIC_INLINE) {
            // An open `<details>` draws its body on its summary's row, in a new paragraph.
            let line_end_index = following_text.node.index;
            let is_summary_end = !following_text.node.done()
                && matches!(&doc.nodes[line_end_index - 1], HtmlNode::CloseTag { lc, .. } if *lc == live_id!(summary));
            if !is_summary_end || collapsed_details_end.is_some() {
                break;
            }
            let mut stacks = following_text.stacks;
            stacks.bold = stacks.bold.saturating_sub(1);
            following_text = FollowingText::new(doc, line_end_index, stacks, inputs.is_in_table, has_template);
            line_break_before = PrecedingContext::PARAGRAPH_START;
            piece_index = 0;
            continue;
        }
        // We can't measure a widget, so it needs room for the ellipsis where it starts, in case it doesn't fit.
        let grapheme_room_in_lpxs = grapheme_room_in_lpxs.unwrap_or_else(|| {
            width_in_lpxs + tf.measure_ellipsis_in_lpxs(cx, following_text.stacks.get_run_style())
        });
        return grapheme_room_in_lpxs.max(unit_room_in_lpxs.unwrap_or(visible_width_in_lpxs));
    }
    // The line ends here, so what follows it gets cut unless it fits.
    let line_end_index = following_text.node.index;
    let is_followed_by_more_lines =
        !following_text.node.done() && has_lines_after(doc, line_end_index, collapsed_details_end, has_template);
    let rest_width_in_lpxs = if is_followed_by_more_lines { f32::INFINITY } else { visible_width_in_lpxs };
    let next_room_in_lpxs = grapheme_room_in_lpxs.map_or(f32::INFINITY, |grapheme_room| {
        grapheme_room.max(unit_room_in_lpxs.unwrap_or(visible_width_in_lpxs))
    });
    rest_width_in_lpxs.min(next_room_in_lpxs)
}

/// Returns whether Html draws more lines after the line that ends right before `line_end_index` in `doc`.
///
/// A cell's line ends beside the next cell, and Html goes on at `collapsed_details_end` after its summary.
fn has_lines_after(
    doc: &HtmlDoc,
    line_end_index: usize,
    collapsed_details_end: Option<usize>,
    has_template: impl Fn(LiveId) -> bool,
) -> bool {
    let is_cell = |lc: LiveId| lc == live_id!(td) || lc == live_id!(th);
    match &doc.nodes[line_end_index - 1] {
        HtmlNode::OpenTag { lc, .. } if is_cell(*lc) => false,
        HtmlNode::CloseTag { lc, .. } if is_cell(*lc) => {
            let next_index = (line_end_index..doc.nodes.len()).find(|&index| match &doc.nodes[index] {
                HtmlNode::Attribute { .. } => false,
                HtmlNode::Text { all_ws, .. } => !all_ws,
                _ => true,
            });
            match next_index.map(|index| (index, &doc.nodes[index])) {
                Some((_, HtmlNode::OpenTag { lc, .. })) if is_cell(*lc) => false,
                Some((index, HtmlNode::CloseTag { lc, .. })) if *lc == live_id!(tr) => {
                    has_content_from(doc, index + 1, has_template)
                }
                _ => has_content_from(doc, line_end_index, has_template),
            }
        }
        HtmlNode::CloseTag { lc, .. } if *lc == live_id!(summary) => {
            has_content_from(doc, collapsed_details_end.unwrap_or(line_end_index), has_template)
        }
        // The tag that ends the line can show something itself, like a rule.
        _ => has_content_from(doc, line_end_index - 1, has_template),
    }
}

/// Returns whether Html draws anything from `node_index` on in `doc`.
fn has_content_from(doc: &HtmlDoc, node_index: usize, has_template: impl Fn(LiveId) -> bool) -> bool {
    let walker = doc.new_walker_with_index(node_index);
    let mut index = node_index;
    while index < doc.nodes.len() {
        match &doc.nodes[index] {
            HtmlNode::Text { start, end, all_ws } if start != end && !all_ws => return true,
            // Rules, list markers, and quote, code and table boxes show even without text.
            HtmlNode::OpenTag { lc, .. }
                if matches!(
                    *lc,
                    live_id!(hr) | live_id!(sep) | live_id!(li) | live_id!(blockquote) | live_id!(pre)
                        | live_id!(td) | live_id!(th)
                ) =>
            {
                return true;
            }
            HtmlNode::OpenTag { lc, nc } if !is_block_tag(*lc) && InlineStyleTag::from_tag(*lc).is_none() => {
                // A custom element's widget may draw anything, and an element without one draws nothing.
                if has_template(*nc) {
                    return true;
                }
                index = walker.at(index).end_index().unwrap_or(index + 1);
                continue;
            }
            _ => {}
        }
        index += 1;
    }
    false
}

/// What TextFlow needs to know about the text after a run, along with what we found it for.
pub(super) struct CachedTextAfterRun {
    inputs: TextAfterRunInputs<'static>,
    /// `None` for a widget that hasn't drawn a run yet.
    line_break_after_run: Option<LineBreakAfterRun>,
    /// The room that what follows a text node's run needs, once the run ended on the last line `max_lines` allows.
    following_room: Option<FollowingRoom>,
}

impl CachedTextAfterRun {
    fn new(inputs: TextAfterRunInputs<'static>, line_break_after_run: Option<LineBreakAfterRun>) -> Self {
        Self { inputs, line_break_after_run, following_room: None }
    }
}

/// The room that what follows a widget or its runs needs, along with what we found it for.
pub(super) struct CachedFollowingRoom {
    inputs: TextAfterRunInputs<'static>,
    room: FollowingRoom,
}

impl CachedFollowingRoom {
    /// Returns the room we found, when we found it for the same `inputs` and `collapsed_details_end`.
    fn get_room(&self, inputs: &TextAfterRunInputs<'_>, collapsed_details_end: Option<usize>) -> Option<f32> {
        self.room.get(collapsed_details_end).filter(|_| self.inputs == *inputs)
    }
}

/// The room that what follows a run or a widget needs on its row, and where Html goes on after the collapsed
/// `<details>` it's in.
#[derive(Clone, Copy)]
struct FollowingRoom {
    collapsed_details_end: Option<usize>,
    following_room_in_lpxs: f32,
}

impl FollowingRoom {
    /// Returns the room, when we found it for a run or widget that goes on at the same `collapsed_details_end`.
    fn get(&self, collapsed_details_end: Option<usize>) -> Option<f32> {
        (self.collapsed_details_end == collapsed_details_end).then_some(self.following_room_in_lpxs)
    }
}

/// What we found after each run, by the index of the node where the text after the run starts.
pub(super) type TextAfterRunCache = NodeCache<CachedTextAfterRun>;

/// The room that what follows each widget and its runs needs on the row they end on.
/// The key is the index of the node where what follows starts,
/// and the value is what we found there for the widget or runs before it.
pub(super) type FollowingRoomCache = NodeCache<CachedFollowingRoom>;

/// What we found at the nodes of a document, so we can use it again on later draws.
/// The key is the index of a node,
/// and the value is what we found at that node.
pub(super) struct NodeCache<T> {
    /// The index of each node's entry in `entries` plus one, or 0 when it has none.
    entry_indices: Vec<u32>,
    entries: Vec<T>,
}

impl<T> Default for NodeCache<T> {
    fn default() -> Self {
        Self { entry_indices: Vec::new(), entries: Vec::new() }
    }
}

impl<T> NodeCache<T> {
    pub(super) fn clear(&mut self) {
        self.entry_indices.clear();
        self.entries.clear();
    }

    fn get(&self, node_index: usize) -> Option<&T> {
        let entry_index = self.entry_indices.get(node_index)?.checked_sub(1)?;
        Some(&self.entries[entry_index as usize])
    }

    fn get_mut(&mut self, node_index: usize) -> Option<&mut T> {
        let entry_index = self.entry_indices.get(node_index)?.checked_sub(1)?;
        Some(&mut self.entries[entry_index as usize])
    }

    fn insert(&mut self, node_index: usize, cached: T) {
        if self.entry_indices.len() <= node_index {
            self.entry_indices.resize(node_index + 1, 0);
        }
        match self.entry_indices[node_index] {
            0 => {
                self.entries.push(cached);
                self.entry_indices[node_index] = self.entries.len() as u32;
            }
            entry_index => self.entries[entry_index as usize - 1] = cached,
        }
    }
}

/// The text nodes left in a custom element, whose widget draws them as one run.
struct CustomElementText {
    next_index: usize,
    end_index: usize,
    style: RunStyle,
    is_inline_code: bool,
    is_at_run_start: bool,
}

/// The text that follows a run in its line, which we read piece by piece as Html draws it.
///
/// We go through inline style tags and custom elements, whose widgets draw the text in them,
/// and stop at blocks, `<br>`, details and summaries, widgets without text, and the end.
struct FollowingText<'a, F> {
    doc: &'a HtmlDoc,
    node: HtmlWalker<'a>,
    stacks: StyleStacks<'a>,
    is_in_table: bool,
    has_template: F,
    custom_element_text: Option<CustomElementText>,
    pieces: SmallVec<[TextPiece; 4]>,
    /// What the line breaking rules see before and after each piece of the last unit we read.
    ///
    /// After its last piece, that's `END_OF_TEXT` when the unit's end didn't depend on it.
    unit_contexts: SmallVec<[(PrecedingContext, FollowingContext); 2]>,
    /// What follows the last piece once we've read to where the line's text stops.
    line_end: Option<FollowingContext>,
}

impl<'a, F: Fn(LiveId) -> bool> FollowingText<'a, F> {
    /// Starts reading the text at `node_index` in `doc`.
    fn new(doc: &'a HtmlDoc, node_index: usize, stacks: StyleStacks<'a>, is_in_table: bool, has_template: F) -> Self {
        Self {
            doc,
            node: doc.new_walker_with_index(node_index),
            stacks,
            is_in_table,
            has_template,
            custom_element_text: None,
            pieces: SmallVec::new(),
            unit_contexts: SmallVec::new(),
            line_end: None,
        }
    }

    /// Finds what follows the run, given what the line breaking rules see at its end.
    fn find_text_after_run(mut self, line_break_before: PrecedingContext) -> TextAfterRun {
        let mut text_after = TextAfterRun {
            line_break_after: self.get_line_break_after(0),
            glue: GlueUnit::default(),
            unit_after_soft_hyphen: GlueUnit::default(),
            runs_in_glue: Vec::new(),
        };
        // Most runs end at a break opportunity, so nothing has to stay with them. The contexts on
        // both sides of the run's end tell us, like they would around an empty text there.
        if self.pieces.is_empty()
            || find_first_line_break("", line_break_before, text_after.line_break_after).is_some()
        {
            return text_after;
        }
        let (glue, unit_after_soft_hyphen_start) = self.read_unit(PiecePosition::default(), line_break_before);
        let mut run_start_index = 0;
        for piece_index in 1..self.unit_contexts.len() {
            if !self.pieces[piece_index].is_run_start {
                continue;
            }
            // Html appends a run's text without its trailing line breaks, so the rest of the glue
            // isn't what that run would read.
            let previous_text = self.pieces[piece_index - 1].get_text(self.doc);
            if trim_trailing_line_breaks(previous_text).len() == previous_text.len() {
                text_after.runs_in_glue.push(RunInGlue {
                    first_piece_index: run_start_index,
                    line_break_before_run: self.unit_contexts[run_start_index].0,
                    line_break_after: self.unit_contexts[piece_index - 1].1,
                    glue_start_index: piece_index,
                });
            }
            run_start_index = piece_index;
        }
        if let Some((position, line_break_before)) = unit_after_soft_hyphen_start {
            text_after.unit_after_soft_hyphen = self.read_unit(position, line_break_before).0;
        }
        text_after.glue = glue;
        text_after
    }

    /// Reads the unit at `position`.
    ///
    /// When it ends at a soft hyphen, also returns where the next unit starts and what the line
    /// breaking rules see there.
    fn read_unit(
        &mut self,
        mut position: PiecePosition,
        mut line_break_before: PrecedingContext,
    ) -> (GlueUnit, Option<(PiecePosition, PrecedingContext)>) {
        let mut unit = GlueUnit::default();
        let mut next_unit_start = None;
        self.unit_contexts.clear();
        while let Some(piece) = self.get_piece(position.piece_index) {
            let text = &piece.get_text(self.doc)[position.byte_index..];
            let next_piece = PiecePosition { piece_index: position.piece_index + 1, byte_index: 0 };
            // A unit can't end where it starts. When the piece has more chars than the rules read ahead,
            // they can often find the unit's end in it alone, and then we skip reading the next piece.
            let mut line_break_after = FollowingContext::END_OF_TEXT;
            let mut line_break = None;
            if text.chars().nth(UNITS_READ_AHEAD).is_some() {
                let mut line_breaks = find_line_breaks(text, line_break_before, line_break_after);
                line_break = line_breaks
                    .find(|line_break| line_break.byte_index > 0)
                    .filter(|_| line_breaks.is_last_break_from_text_alone());
            }
            if line_break.is_none() {
                line_break_after = self.get_line_break_after(next_piece.piece_index);
                line_break = find_line_breaks(text, line_break_before, line_break_after)
                    .find(|line_break| line_break.byte_index > 0);
            }
            self.unit_contexts.push((line_break_before, line_break_after));
            let unit_text = line_break.map_or(text, |line_break| {
                &text[..line_break.byte_index - line_break.newline_len]
            });
            let start_byte = piece.start_byte + position.byte_index;
            unit.pieces.push(TextPiece {
                start_byte,
                end_byte: start_byte + unit_text.len(),
                is_run_start: piece.is_run_start && position.byte_index == 0,
                ..piece
            });
            let Some(line_break) = line_break else {
                line_break_before.append(text);
                position = next_piece;
                continue;
            };
            // A run never draws a hyphen at its very end, but a custom element's
            // later text nodes go on with its run.
            let ends_at_soft_hyphen = !line_break.is_forced
                && unit_text.ends_with('\u{AD}')
                && (line_break.byte_index < text.len()
                    || self.get_piece(next_piece.piece_index).is_some_and(|next| !next.is_run_start));
            if ends_at_soft_hyphen {
                unit.soft_hyphen_style = Some(piece.style);
                line_break_before.append(&text[..line_break.byte_index]);
                let next_unit_position = if line_break.byte_index == text.len() {
                    next_piece
                } else {
                    PiecePosition { byte_index: position.byte_index + line_break.byte_index, ..position }
                };
                next_unit_start = Some((next_unit_position, line_break_before));
            }
            break;
        }
        // White space at the end of the unit hangs, so it takes no room.
        while let Some(piece) = unit.pieces.last_mut() {
            piece.end_byte =
                piece.start_byte + piece.get_text(self.doc).trim_end_matches(is_hangable_white_space).len();
            if piece.end_byte > piece.start_byte {
                break;
            }
            unit.pieces.pop();
        }
        (unit, next_unit_start)
    }

    /// Returns piece `piece_index`, reading on as far as that.
    fn get_piece(&mut self, piece_index: usize) -> Option<TextPiece> {
        while self.pieces.len() <= piece_index {
            if !self.read_piece() {
                return None;
            }
        }
        Some(self.pieces[piece_index])
    }

    /// Returns what the line breaking rules see in the text from piece `piece_index` on.
    fn get_line_break_after(&mut self, piece_index: usize) -> FollowingContext {
        while self.pieces.len() <= piece_index && self.read_piece() {}
        loop {
            // The rules only read the first few units, so we read on until the pieces have all of those.
            let mut line_break_after = self.line_end.unwrap_or(FollowingContext::END_OF_TEXT);
            let mut unread_unit_count = if self.line_end.is_some() { 0 } else { UNITS_READ_AHEAD };
            for piece in self.pieces.get(piece_index..).unwrap_or_default().iter().rev() {
                let units_from_after = line_break_after.prepend(piece.get_text(self.doc));
                unread_unit_count = (unread_unit_count + units_from_after).saturating_sub(UNITS_READ_AHEAD);
            }
            if unread_unit_count == 0 {
                return line_break_after;
            }
            self.read_piece();
        }
    }

    /// Reads the next piece of text, or returns false where the line's text stops.
    fn read_piece(&mut self) -> bool {
        let nodes = self.node.nodes;
        loop {
            if let Some(element_text) = &mut self.custom_element_text {
                while element_text.next_index < element_text.end_index {
                    let index = element_text.next_index;
                    element_text.next_index += 1;
                    let &HtmlNode::Text { start, end, .. } = &nodes[index] else {
                        continue;
                    };
                    if start == end {
                        continue;
                    }
                    self.pieces.push(TextPiece {
                        start_byte: start,
                        end_byte: end,
                        style: element_text.style,
                        is_inline_code: element_text.is_inline_code,
                        is_run_start: element_text.is_at_run_start,
                    });
                    element_text.is_at_run_start = false;
                    return true;
                }
                // A widget with no text, like an image, is atomic, so a line can break before it.
                if element_text.is_at_run_start {
                    self.line_end = Some(FollowingContext::BEFORE_ATOMIC_INLINE);
                }
                self.custom_element_text = None;
            }
            if self.line_end.is_none() && self.node.done() {
                self.line_end = Some(FollowingContext::END_OF_TEXT);
            }
            if self.line_end.is_some() {
                return false;
            }
            let index = self.node.index;
            self.node.index += 1;
            match &nodes[index] {
                &HtmlNode::Text { start, end, all_ws } => {
                    // Html doesn't draw white space in tables.
                    if start != end && !(self.is_in_table && all_ws) {
                        self.pieces.push(TextPiece {
                            start_byte: start,
                            end_byte: end,
                            style: self.stacks.get_run_style(),
                            is_inline_code: self.stacks.inline_code > 0,
                            is_run_start: true,
                        });
                        return true;
                    }
                }
                HtmlNode::OpenTag { lc, nc } => {
                    if is_block_tag(*lc) {
                        self.line_end = Some(FollowingContext::END_OF_TEXT);
                    } else if let Some(tag) = InlineStyleTag::from_tag(*lc) {
                        self.stacks.apply_style_tag(tag, true);
                    } else {
                        // Anything else is a custom element, whose widget draws the text in it.
                        let element = self.node.at(index);
                        let end_index = element.end_index().unwrap_or(index + 1);
                        if (self.has_template)(*nc) {
                            self.custom_element_text = Some(CustomElementText {
                                next_index: index + 1,
                                end_index: element.close_index().unwrap_or(end_index),
                                style: self.stacks.get_run_style(),
                                is_inline_code: self.stacks.inline_code > 0,
                                is_at_run_start: true,
                            });
                        }
                        self.node.index = end_index;
                    }
                }
                HtmlNode::CloseTag { lc, .. } => {
                    if is_block_tag(*lc) {
                        self.line_end = Some(FollowingContext::END_OF_TEXT);
                    } else if let Some(tag) = InlineStyleTag::from_tag(*lc) {
                        self.stacks.apply_style_tag(tag, false);
                    }
                }
                HtmlNode::Attribute { .. } => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::html::element_text;

    const PLAIN: RunStyle = RunStyle { is_bold: false, is_italic: false, is_fixed: false, font_size: 10.0 };
    const BOLD: RunStyle = RunStyle { is_bold: true, ..PLAIN };

    fn has_test_template(template: LiveId) -> bool {
        template == live_id!(a) || template == live_id!(span)
    }

    /// The style stacks outside of any style tag.
    fn get_plain_stacks() -> StyleStacks<'static> {
        StyleStacks {
            bold: 0,
            italic: 0,
            fixed: 0,
            inline_code: 0,
            is_in_code_block: false,
            font_size: PLAIN.font_size,
            font_sizes: FontSizes::Borrowed(&[]),
            fixed_font_size_scale: 0.5,
        }
    }

    /// A piece with its text spelled out, as the tests write it.
    #[derive(Debug, PartialEq)]
    struct PieceWithText<'a> {
        text: &'a str,
        style: RunStyle,
        is_inline_code: bool,
        is_run_start: bool,
    }

    fn build_piece(text: &str, style: RunStyle) -> PieceWithText<'_> {
        PieceWithText { text, style, is_inline_code: false, is_run_start: true }
    }

    fn get_pieces<'a>(doc: &'a HtmlDoc, unit: &GlueUnit) -> Vec<PieceWithText<'a>> {
        unit.pieces
            .iter()
            .map(|piece| PieceWithText {
                text: piece.get_text(doc),
                style: piece.style,
                is_inline_code: piece.is_inline_code,
                is_run_start: piece.is_run_start,
            })
            .collect()
    }

    fn build_following_context(text: &str) -> FollowingContext {
        let mut line_break_after = FollowingContext::END_OF_TEXT;
        line_break_after.prepend(text);
        line_break_after
    }

    /// Finds what follows a run of `run` in `doc`, drawn with `stacks`.
    ///
    /// The run is the first text node with that text, or the first custom element with it.
    fn find_text_after(doc: &HtmlDoc, run: &str, stacks: StyleStacks<'_>) -> TextAfterRun {
        let walker = doc.new_walker();
        let index = (0..doc.nodes.len())
            .find_map(|index| {
                let node = walker.at(index);
                if node.text() == Some(run) {
                    return Some(index + 1);
                }
                let end_index = node.end_index()?;
                let is_custom_element = node.open_tag_nc().is_some_and(has_test_template);
                let text = element_text(doc, index, node.close_index().unwrap_or(end_index));
                (is_custom_element && text == run).then_some(end_index)
            })
            .expect("fixture must contain the run");
        let mut line_break_before = PrecedingContext::PARAGRAPH_START;
        line_break_before.append(run);
        FollowingText::new(doc, index, stacks, false, has_test_template).find_text_after_run(line_break_before)
    }

    #[test]
    fn glue_goes_through_style_tags() {
        let doc = parse_html("foo<b>bar</b>.", &mut None, InternLiveId::No);
        let text_after = find_text_after(&doc, "foo", get_plain_stacks());
        assert_eq!(text_after.line_break_after, build_following_context("bar."));
        assert_eq!(get_pieces(&doc, &text_after.glue), [build_piece("bar", BOLD), build_piece(".", PLAIN)]);
        let text_after = find_text_after(&doc, "bar", StyleStacks { bold: 1, ..get_plain_stacks() });
        assert_eq!(text_after.line_break_after, build_following_context("."));
        assert_eq!(get_pieces(&doc, &text_after.glue), [build_piece(".", PLAIN)]);
    }

    #[test]
    fn glue_goes_through_quotes_around_a_link() {
        let doc = parse_html("\"<a>link</a>\"", &mut None, InternLiveId::No);
        let text_after = find_text_after(&doc, "\"", get_plain_stacks());
        assert_eq!(text_after.line_break_after, build_following_context("link\""));
        assert_eq!(get_pieces(&doc, &text_after.glue), [build_piece("link", PLAIN), build_piece("\"", PLAIN)]);
        let text_after = find_text_after(&doc, "link", get_plain_stacks());
        assert_eq!(text_after.line_break_after, build_following_context("\""));
        assert_eq!(get_pieces(&doc, &text_after.glue), [build_piece("\"", PLAIN)]);
    }

    #[test]
    fn glue_keeps_a_bracket_with_the_url_in_it() {
        let doc = parse_html("(<a>https://x.y/z</a>)", &mut None, InternLiveId::No);
        let text_after = find_text_after(&doc, "(", get_plain_stacks());
        assert_eq!(text_after.line_break_after, build_following_context("https://x.y/z)"));
        assert_eq!(get_pieces(&doc, &text_after.glue), [build_piece("https://", PLAIN)]);
        let text_after = find_text_after(&doc, "https://x.y/z", get_plain_stacks());
        assert_eq!(text_after.line_break_after, build_following_context(")"));
        assert_eq!(get_pieces(&doc, &text_after.glue), [build_piece(")", PLAIN)]);
    }

    #[test]
    fn glue_leaves_out_hanging_white_space() {
        let doc = parse_html("<b>Note</b>: x", &mut None, InternLiveId::No);
        let text_after = find_text_after(&doc, "Note", StyleStacks { bold: 1, ..get_plain_stacks() });
        assert_eq!(text_after.line_break_after, build_following_context(": x"));
        assert_eq!(get_pieces(&doc, &text_after.glue), [build_piece(":", PLAIN)]);
    }

    #[test]
    fn glue_goes_in_and_out_of_inline_code() {
        let doc = parse_html("x<code>foo</code>.", &mut None, InternLiveId::No);
        let code = RunStyle { is_fixed: true, font_size: 5.0, ..PLAIN };
        let text_after = find_text_after(&doc, "x", get_plain_stacks());
        assert_eq!(
            get_pieces(&doc, &text_after.glue),
            [PieceWithText { is_inline_code: true, ..build_piece("foo", code) }, build_piece(".", PLAIN)],
        );
        let code_stacks = StyleStacks {
            fixed: 1,
            inline_code: 1,
            font_sizes: FontSizes::Owned(SmallVec::from_slice(&[code.font_size])),
            ..get_plain_stacks()
        };
        let text_after = find_text_after(&doc, "foo", code_stacks);
        assert_eq!(text_after.line_break_after, build_following_context("."));
        assert_eq!(get_pieces(&doc, &text_after.glue), [build_piece(".", PLAIN)]);
    }

    #[test]
    fn glue_runs_to_the_end_of_a_word() {
        let doc = parse_html("<i>un</i>believable", &mut None, InternLiveId::No);
        let text_after = find_text_after(&doc, "un", StyleStacks { italic: 1, ..get_plain_stacks() });
        assert_eq!(text_after.line_break_after, build_following_context("believable"));
        assert_eq!(get_pieces(&doc, &text_after.glue), [build_piece("believable", PLAIN)]);
    }

    #[test]
    fn lines_end_at_breaks_and_blocks() {
        for (html, run) in [
            ("text<br>more", "text"),
            ("<p>foo</p><p>bar</p>", "foo"),
            ("foo<blockquote>bar</blockquote>", "foo"),
            ("<details><summary>foo</summary>bar</details>", "foo"),
        ] {
            let doc = parse_html(html, &mut None, InternLiveId::No);
            let text_after = find_text_after(&doc, run, get_plain_stacks());
            assert_eq!(text_after.line_break_after, FollowingContext::END_OF_TEXT, "{html}");
            assert!(text_after.glue.pieces.is_empty(), "{html}");
        }
    }

    #[test]
    fn custom_elements_draw_their_text_as_one_run() {
        let doc = parse_html("x<span>a<b>b</b></span>y", &mut None, InternLiveId::No);
        let text_after = find_text_after(&doc, "x", get_plain_stacks());
        assert_eq!(text_after.line_break_after, build_following_context("aby"));
        assert_eq!(
            get_pieces(&doc, &text_after.glue),
            [
                build_piece("a", PLAIN),
                PieceWithText { is_run_start: false, ..build_piece("b", PLAIN) },
                build_piece("y", PLAIN),
            ],
        );
    }

    #[test]
    fn elements_without_a_template_draw_no_text() {
        let doc = parse_html("foo<div>hidden</div>bar", &mut None, InternLiveId::No);
        let text_after = find_text_after(&doc, "foo", get_plain_stacks());
        assert_eq!(text_after.line_break_after, build_following_context("bar"));
        assert_eq!(get_pieces(&doc, &text_after.glue), [build_piece("bar", PLAIN)]);
    }

    #[test]
    fn nothing_has_to_stay_with_a_run_before_a_space() {
        let doc = parse_html("foo<a> bar</a>", &mut None, InternLiveId::No);
        let text_after = find_text_after(&doc, "foo", get_plain_stacks());
        assert_eq!(text_after.line_break_after, build_following_context(" bar"));
        assert!(text_after.glue.pieces.is_empty());
    }

    #[test]
    fn glue_that_ends_at_a_soft_hyphen_comes_with_the_next_unit() {
        let doc = parse_html("x<b>ex\u{AD}am\u{AD}ple</b>", &mut None, InternLiveId::No);
        let text_after = find_text_after(&doc, "x", get_plain_stacks());
        assert_eq!(get_pieces(&doc, &text_after.glue), [build_piece("ex\u{AD}", BOLD)]);
        assert_eq!(text_after.glue.soft_hyphen_style, Some(BOLD));
        let unit = &text_after.unit_after_soft_hyphen;
        assert_eq!(get_pieces(&doc, unit), [PieceWithText { is_run_start: false, ..build_piece("am\u{AD}", BOLD) }]);
        assert_eq!(unit.soft_hyphen_style, Some(BOLD));
        // A run that ends with the soft hyphen never draws its hyphen.
        let doc = parse_html("x<b>ex\u{AD}</b>am", &mut None, InternLiveId::No);
        let text_after = find_text_after(&doc, "x", get_plain_stacks());
        assert_eq!(get_pieces(&doc, &text_after.glue), [build_piece("ex\u{AD}", BOLD)]);
        assert_eq!(text_after.glue.soft_hyphen_style, None);
        // But a link's later text nodes go on with its run.
        let doc = parse_html("x<a>ex<b>\u{AD}</b>am</a>", &mut None, InternLiveId::No);
        let text_after = find_text_after(&doc, "x", get_plain_stacks());
        assert_eq!(
            get_pieces(&doc, &text_after.glue),
            [build_piece("ex", PLAIN), PieceWithText { is_run_start: false, ..build_piece("\u{AD}", PLAIN) }],
        );
        assert_eq!(text_after.glue.soft_hyphen_style, Some(PLAIN));
        assert_eq!(
            get_pieces(&doc, &text_after.unit_after_soft_hyphen),
            [PieceWithText { is_run_start: false, ..build_piece("am", PLAIN) }],
        );
    }

    #[test]
    fn widgets_without_text_are_atomic() {
        let doc = parse_html("foo<span></span>bar baz", &mut None, InternLiveId::No);
        let text_after = find_text_after(&doc, "foo", get_plain_stacks());
        assert_eq!(text_after.line_break_after, FollowingContext::BEFORE_ATOMIC_INLINE);
        assert!(text_after.glue.pieces.is_empty());
    }

    #[test]
    fn lookahead_reads_only_as_far_as_the_rules_do() {
        let doc = parse_html("x<b>abcd</b><i>efgh</i>ij", &mut None, InternLiveId::No);
        let mut following_text = FollowingText::new(&doc, 1, get_plain_stacks(), false, has_test_template);
        assert_eq!(following_text.get_line_break_after(0), build_following_context("abcdefghij"));
        assert_eq!(following_text.pieces.len(), 1);
        // Shorter pieces chain into the ones after them.
        let doc = parse_html("x<b>a</b><i>b</i>cd", &mut None, InternLiveId::No);
        let mut following_text = FollowingText::new(&doc, 1, get_plain_stacks(), false, has_test_template);
        assert_eq!(following_text.get_line_break_after(0), build_following_context("abcd"));
        assert_eq!(following_text.pieces.len(), 3);
    }
}
