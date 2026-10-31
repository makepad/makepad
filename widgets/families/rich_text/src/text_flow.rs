use crate::makepad_draw::text::{
    font_family::FontFamilyId,
    geom::Point as TextPoint,
    layouter::{is_hangable_white_space, is_short_of_following_room, LaidoutRow, LaidoutText, SelectionRect},
    line_break::{find_first_line_break, FollowingContext, PrecedingContext},
    selection::{Cursor, Selection},
    shaper::ShapedText,
};
use crate::text_input::{mark_band_rect, DrawTextMark, TextMark, TextMarkKind, TextMarkSet};
use crate::{
    animator::*, html::{FollowingRoomFinder, PendingFollowingRoom, PendingTextAfterRun}, makepad_derive_widget::*,
    makepad_draw::shader::draw_text::TextOverflow, makepad_draw::turtle::{RowAlign, RowAlignRole}, makepad_draw::*,
    widget::*, widget_tree::CxWidgetExt,
};
use std::{cell::Cell, rc::Rc};
use unicode_segmentation::UnicodeSegmentation;

const LPXS_PER_PT: f32 = 96.0 / 72.0;

/// Returns `text` without the `'\n'` and `'\r'` at its end, which we drop from every run.
pub(crate) fn trim_trailing_line_breaks(text: &str) -> &str {
    let mut end = text.len();
    while end > 0 && matches!(text.as_bytes()[end - 1], b'\n' | b'\r') {
        end -= 1;
    }
    &text[..end]
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    let FlowBlockType = set_type_default() do #(FlowBlockType::script_api(vm))

    mod.widgets.DrawFlowBlock = set_type_default() do #(DrawFlowBlock::script_shader(vm)){
        ..mod.draw.DrawQuad

        block_type: instance(FlowBlockType.Quote)
        line_color: #fff
        sep_color: #888
        code_color: #333
        quote_bg_color: #222
        quote_fg_color: #aaa
        selection_color: #FF5C3966
        table_header_bg_color: #FFFFFF22
        table_border_color: #666

        space_1: uniform(4.0)
        space_2: uniform(8.0)

        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            match self.block_type {
                FlowBlockType.Quote => {
                    sdf.box(0. 0. self.rect_size.x self.rect_size.y 2.)
                    sdf.fill(self.quote_bg_color)
                    sdf.box(self.space_1 self.space_1 self.space_1 self.rect_size.y-self.space_2 1.5)
                    sdf.fill(self.quote_fg_color)
                    return sdf.result
                }
                FlowBlockType.Sep => {
                    sdf.box(0. 1. self.rect_size.x-1. self.rect_size.y-2. 2.)
                    sdf.fill(self.sep_color)
                    return sdf.result
                }
                FlowBlockType.Code => {
                    sdf.box(0. 0. self.rect_size.x self.rect_size.y 2.)
                    sdf.fill(self.code_color)
                    return sdf.result
                }
                FlowBlockType.InlineCode => {
                    sdf.box(1. 1. self.rect_size.x-2. self.rect_size.y-2. 2.)
                    sdf.fill(self.code_color)
                    return sdf.result
                }
                FlowBlockType.Underline => {
                    sdf.box(0. self.rect_size.y-2. self.rect_size.x 2.0 0.5)
                    sdf.fill(self.line_color)
                    return sdf.result
                }
                FlowBlockType.Strikethrough => {
                    sdf.box(0. self.rect_size.y * 0.45 self.rect_size.x 2.0 0.5)
                    sdf.fill(self.line_color)
                    return sdf.result
                }
                FlowBlockType.Selection => {
                    return vec4(self.selection_color.rgb * self.selection_color.a, self.selection_color.a)
                }
                FlowBlockType.TableCell => {
                    sdf.rect(0. 0. self.rect_size.x self.rect_size.y)
                    sdf.fill(self.table_header_bg_color)
                    // Draw the right/bottom 1px borders as hard-edged
                    // lines rather than SDF rects, so they stay crisp and
                    // fully opaque on low-DPI screens where a 1px SDF rect
                    // gets AA'd across both edges and fades.
                    //
                    // Match whichever pixel actually sits in the rightmost
                    // column / bottom row of the rasterized rect (pos > size - 1)
                    // rather than a floor-snapped position. Cell dimensions
                    // can be fractional (total table width isn't always a
                    // multiple of the column count), and in that case the
                    // last shaded pixel's local pos exceeds floor(size) by
                    // a fraction — so floor-snapping would either leave a
                    // seam at that pixel (gap) or place the line inside,
                    // leaving a stub past the junction.
                    let pos = self.pos * self.rect_size
                    if pos.x > self.rect_size.x - 1.0
                        || pos.y > self.rect_size.y - 1.0
                    {
                        return self.table_border_color
                    }
                    return sdf.result
                }
            }
            return #f00
        }
    }

    mod.widgets.FlowBlockType = FlowBlockType

    mod.widgets.TextFlowBase = #(TextFlow::register_widget(vm)){
        font_size: 8
        flow: Flow.Right{wrap: true}
        draw_selection +: {
            draw_call_group: @selection
            color: theme.color_u_3
        }
        /** The marked-span squiggle: drawn under the words, not round the flow. */
        draw_mark +: {
            color_error: theme.color_error
            color_warning: theme.color_warning
            color_note: theme.color_info
        }
    }

    mod.widgets.TextFlowLinkBase = #(TextFlowLink::register_widget(vm)){}

    mod.widgets.TextFlowLink = set_type_default() do mod.widgets.TextFlowLinkBase{
        color: #xa
        color_hover: #xf
        color_down: #x3
        margin: Inset{right: 5}

        animator: Animator{
            hover: {
                default: @off
                off: AnimatorState{
                    redraw: true
                    from: {all: Forward {duration: 0.01}}
                    apply: {
                        hovered: 0.0
                        down: 0.0
                    }
                }

                on: AnimatorState{
                    redraw: true
                    from: {
                        all: Forward {duration: 0.1}
                        down: Forward {duration: 0.01}
                    }
                    apply: {
                        hovered: snap(1.0)
                        down: snap(1.0)
                    }
                }

                down: AnimatorState{
                    redraw: true
                    from: {all: Forward {duration: 0.01}}
                    apply: {
                        hovered: snap(1.0)
                        down: snap(1.0)
                    }
                }
            }
        }
    }

    mod.widgets.TextFlow = set_type_default() do mod.widgets.TextFlowBase{
        width: Fill height: Fit
        flow: Flow.Right{wrap: true}
        padding: 0

        font_size: theme.font_size_p
        font_color: theme.color_text

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
        list_item_walk: Walk{
            height: Fit width: Fill
        }

        inline_code_padding: theme.mspace_1
        inline_code_margin: theme.mspace_1

        sep_walk: Walk{
            width: Fill height: 4.
            margin: theme.mspace_v_1
        }

        table_walk: Walk{width: Fill, height: Fit}
        table_layout: Layout{flow: Flow.Down}
        table_row_walk: Walk{width: Fill, height: Fit}
        table_row_layout: Layout{flow: Flow.Right}
        table_cell_layout: Layout{
            flow: Flow.Right{wrap: true}
            padding: Inset{left: 6, right: 6, top: 4, bottom: 4}
        }

        link := mod.widgets.TextFlowLink{}

        draw_block +: {
            line_color: theme.color_text
            sep_color: theme.color_shadow
            quote_bg_color: theme.color_bg_highlight
            quote_fg_color: theme.color_text
            code_color: theme.color_bg_highlight
            selection_color: theme.color_selection_focus
            table_header_bg_color: theme.color_bg_highlight
            table_border_color: theme.color_shadow
            space_1: uniform(theme.space_1)
            space_2: uniform(theme.space_2)
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(u32)]
pub enum FlowBlockType {
    #[pick]
    Quote = 1,
    Sep = 2,
    Code = 3,
    InlineCode = 4,
    Underline = 5,
    Strikethrough = 6,
    Selection = 7,
    TableCell = 8,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawFlowBlock {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    pub line_color: Vec4f,
    #[live]
    pub sep_color: Vec4f,
    #[live]
    pub code_color: Vec4f,
    #[live]
    pub quote_bg_color: Vec4f,
    #[live]
    pub quote_fg_color: Vec4f,
    #[live]
    pub selection_color: Vec4f,
    #[live]
    pub table_header_bg_color: Vec4f,
    #[live]
    pub table_border_color: Vec4f,
    #[live]
    pub block_type: FlowBlockType,
}

#[derive(Default)]
pub struct StackCounter(usize);
impl StackCounter {
    pub fn push(&mut self) {
        self.0 += 1;
    }
    pub fn pop(&mut self) {
        if self.0 > 0 {
            self.0 -= 1;
        }
    }
    pub fn clear(&mut self) {
        self.0 = 0
    }
    pub fn value(&self) -> usize {
        self.0
    }
}

/// Which of TextFlow's five text styles a run is drawn with.
#[derive(Clone, Copy, Debug, PartialEq)]
enum TextStyleSlot {
    Normal,
    Bold,
    Italic,
    BoldItalic,
    Fixed,
}

/// The style a run of text gets from TextFlow's style stacks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RunStyle {
    pub is_bold: bool,
    pub is_italic: bool,
    pub is_fixed: bool,
    pub font_size: f32,
}

impl RunStyle {
    fn get_text_style_slot(&self) -> TextStyleSlot {
        match (self.is_fixed, self.is_bold, self.is_italic) {
            (true, _, _) => TextStyleSlot::Fixed,
            (false, true, true) => TextStyleSlot::BoldItalic,
            (false, true, false) => TextStyleSlot::Bold,
            (false, false, true) => TextStyleSlot::Italic,
            (false, false, false) => TextStyleSlot::Normal,
        }
    }
}

/// The largest index at or below `index` that starts a character.
///
/// Byte offsets that come from outside - a spell checker, a validator, a diff -
/// are not guaranteed to fall between characters, and a position asked for at a
/// byte inside one is meaningless rather than merely wrong.
fn floor_char_boundary(text: &str, index: usize) -> usize {
    let mut index = index.min(text.len());
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

/// A segment in the TextFlow selection stream
pub enum SelectionSegment {
    /// Text segment with layout info for selection
    Text {
        /// The laid out text (cached, Rc'd)
        laidout_text: Rc<LaidoutText>,
        /// Origin in screen coordinates
        origin: DVec2,
        /// Font scale used when drawing
        font_scale: f32,
        /// Start index in accumulated text buffer
        text_start: usize,
    },
    /// Non-text gap (widget, image, icon, etc)
    Gap {
        /// Bounding rect in screen coordinates
        rect: Rect,
        /// Start index in accumulated text buffer
        text_start: usize,
    },
    /// A child widget whose text content participates in selection (e.g. CodeView).
    /// The widget draws its own selection highlights; TextFlow skips it in draw_selection.
    WidgetText {
        /// The widget's draw area (queried at event time for its rect)
        area: Area,
        /// Start index in accumulated text buffer
        text_start: usize,
        /// Length of the widget's text in the accumulated buffer
        text_len: usize,
    },
}

/// Tracks all segments during drawing for selection support
#[derive(Default)]
pub struct SelectionTracker {
    /// All segments in draw order
    pub segments: Vec<SelectionSegment>,
    /// Accumulated text content (for copy operations)
    pub text: String,
}

impl SelectionTracker {
    pub fn clear(&mut self) {
        self.segments.clear();
        self.text.clear();
    }

    pub fn push_text(
        &mut self,
        laidout_text: Rc<LaidoutText>,
        origin: DVec2,
        font_scale: f32,
        text: &str,
    ) {
        let text_start = self.text.len();
        self.text.push_str(text);
        self.segments.push(SelectionSegment::Text {
            laidout_text,
            origin,
            font_scale,
            text_start,
        });
    }

    pub fn push_gap(&mut self, rect: Rect) {
        let text_start = self.text.len();
        // Use object replacement character for gaps
        self.text.push('\u{FFFC}');
        self.segments
            .push(SelectionSegment::Gap { rect, text_start });
    }

    /// Push a child widget's text content as a selectable segment.
    /// The text is stored in the accumulated buffer so it participates in
    /// copy and char-index mapping. The widget draws its own selection highlights.
    pub fn push_widget_text(&mut self, area: Area, text: &str) {
        let text_start = self.text.len();
        let text_len = text.len();
        self.text.push_str(text);
        self.segments.push(SelectionSegment::WidgetText {
            area,
            text_start,
            text_len,
        });
    }

    pub fn push_newline(&mut self) {
        self.text.push('\n');
    }

    pub fn total_len(&self) -> usize {
        self.text.len()
    }

    /// UTF-8 byte range of the Unicode word (or punctuation/space segment)
    /// under the cursor. Use the complete selection stream so styling changes
    /// in the middle of a word do not split the selection.
    fn word_range(&self, index: usize) -> (usize, usize) {
        let index = index.min(self.text.len().saturating_sub(1));
        self.text.split_word_bound_indices()
            .find_map(|(start, word)| {
                let end = start + word.len();
                (index < end).then_some((start, end))
            })
            .unwrap_or((0, 0))
    }

    /// Find character index from screen point.
    /// `cx` is needed to query widget areas for WidgetText segments.
    pub fn point_to_index(&self, cx: &Cx, point: DVec2) -> Option<usize> {
        for segment in &self.segments {
            match segment {
                SelectionSegment::Text {
                    laidout_text,
                    origin,
                    font_scale,
                    text_start,
                } => {
                    // Convert point to layout-local coords
                    let local_point = TextPoint::new(
                        ((point.x - origin.x) / *font_scale as f64) as f32,
                        ((point.y - origin.y) / *font_scale as f64) as f32,
                    );

                    // Check if point is within text bounds
                    let size = laidout_text.size_in_lpxs;
                    if local_point.x >= 0.0
                        && local_point.x <= size.width
                        && local_point.y >= 0.0
                        && local_point.y <= size.height
                    {
                        let cursor = laidout_text.point_in_lpxs_to_cursor(local_point);
                        return Some(text_start + cursor.index);
                    }
                }
                SelectionSegment::Gap { rect, text_start } => {
                    if rect.contains(point) {
                        return Some(*text_start);
                    }
                }
                SelectionSegment::WidgetText {
                    area,
                    text_start,
                    text_len,
                } => {
                    if area.is_valid(cx) {
                        let rect = area.rect(cx);
                        if rect.size.y > 0.0 && rect.contains(point) {
                            // Linear interpolation based on y position
                            let local_y = (point.y - rect.pos.y).max(0.0);
                            let fraction = (local_y / rect.size.y).min(1.0);
                            return Some(text_start + (fraction * *text_len as f64) as usize);
                        }
                    }
                }
            }
        }

        // Find nearest segment if point outside all
        self.nearest_index(cx, point)
    }

    /// Find the nearest character index when point is outside all segments
    fn nearest_index(&self, cx: &Cx, point: DVec2) -> Option<usize> {
        let mut best: Option<(usize, f64)> = None;

        for segment in &self.segments {
            match segment {
                SelectionSegment::Text {
                    laidout_text,
                    origin,
                    font_scale,
                    text_start,
                } => {
                    // Check distance to this text segment's bounding box
                    let size = laidout_text.size_in_lpxs;
                    let rect = Rect {
                        pos: *origin,
                        size: dvec2(
                            (size.width * font_scale) as f64,
                            (size.height * font_scale) as f64,
                        ),
                    };
                    let dist = Self::point_to_rect_distance(point, rect);

                    if best.map_or(true, |(_, d)| dist < d) {
                        // Find cursor position within this segment
                        let local_point = TextPoint::new(
                            ((point.x - origin.x) / *font_scale as f64) as f32,
                            ((point.y - origin.y) / *font_scale as f64) as f32,
                        );
                        let cursor = laidout_text.point_in_lpxs_to_cursor(local_point);
                        best = Some((text_start + cursor.index, dist));
                    }
                }
                SelectionSegment::Gap { rect, text_start } => {
                    let dist = Self::point_to_rect_distance(point, *rect);
                    if best.map_or(true, |(_, d)| dist < d) {
                        best = Some((*text_start, dist));
                    }
                }
                SelectionSegment::WidgetText {
                    area,
                    text_start,
                    text_len,
                } => {
                    if area.is_valid(cx) {
                        let rect = area.rect(cx);
                        let dist = Self::point_to_rect_distance(point, rect);
                        if best.map_or(true, |(_, d)| dist < d) {
                            // Linear interpolation based on y position
                            let local_y = (point.y - rect.pos.y).max(0.0);
                            let fraction = if rect.size.y > 0.0 {
                                (local_y / rect.size.y).min(1.0)
                            } else {
                                0.0
                            };
                            best =
                                Some((text_start + (fraction * *text_len as f64) as usize, dist));
                        }
                    }
                }
            }
        }

        best.map(|(idx, _)| idx)
    }

    fn point_to_rect_distance(point: DVec2, rect: Rect) -> f64 {
        let dx = if point.x < rect.pos.x {
            rect.pos.x - point.x
        } else if point.x > rect.pos.x + rect.size.x {
            point.x - (rect.pos.x + rect.size.x)
        } else {
            0.0
        };
        let dy = if point.y < rect.pos.y {
            rect.pos.y - point.y
        } else if point.y > rect.pos.y + rect.size.y {
            point.y - (rect.pos.y + rect.size.y)
        } else {
            0.0
        };
        (dx * dx + dy * dy).sqrt()
    }

    /// Screen-space bands for one mark: one per row the mark touches, each
    /// hanging from that row's own baseline.
    ///
    /// Not the same shape as [`SelectionTracker::selection_rects`]. A selection
    /// is a box around the words; a mark goes *under* them, so each band starts
    /// at the baseline and is `band_height` tall whatever the row's height is.
    ///
    /// The mark's range is cut down to each text segment in turn - a flow lays
    /// out one run at a time and a mark is given against the whole flow - and
    /// the run's layout then splits its piece again at every wrap, which is why
    /// a mark that crosses a line break comes back as two bands rather than one
    /// box spanning both rows.
    ///
    /// Gaps and child-widget segments are skipped: an inline image has no
    /// baseline to hang a mark from, and a child that draws its own text draws
    /// its own decorations with it.
    pub fn mark_rects(&self, mark: TextMark, band_height: f64) -> Vec<Rect> {
        let mut rects = Vec::new();
        for segment in &self.segments {
            let SelectionSegment::Text {
                laidout_text,
                origin,
                font_scale,
                text_start,
            } = segment
            else {
                continue;
            };
            let Some(range) = mark.clip_to(*text_start, laidout_text.text.len()) else {
                continue;
            };
            // A mark arrives as byte offsets a host computed against its own
            // copy of the text; a spell checker or a diff can land one inside a
            // multi-byte character, and the layout is asked for a position, not
            // a slice, so it would answer nonsense rather than panic.
            let selection = Selection {
                anchor: Cursor {
                    index: floor_char_boundary(&laidout_text.text, range.start),
                    prefer_next_row: false,
                },
                cursor: Cursor {
                    index: floor_char_boundary(&laidout_text.text, range.end),
                    prefer_next_row: false,
                },
            };
            for SelectionRect {
                rect_in_lpxs,
                ascender_in_lpxs,
            } in laidout_text.selection_rects(selection)
            {
                let band = mark_band_rect(
                    *origin,
                    dvec2(rect_in_lpxs.origin.x as f64, rect_in_lpxs.origin.y as f64),
                    rect_in_lpxs.size.width as f64,
                    ascender_in_lpxs as f64,
                    *font_scale as f64,
                    band_height,
                );
                if band.size.x > 0.0 {
                    rects.push(band);
                }
            }
        }
        rects
    }

    /// Get all selection rects for the given character range
    pub fn selection_rects(&self, start: usize, end: usize) -> Vec<Rect> {
        let mut rects = Vec::new();

        for segment in &self.segments {
            match segment {
                SelectionSegment::Text {
                    laidout_text,
                    origin,
                    font_scale,
                    text_start,
                } => {
                    let seg_end = text_start + laidout_text.text.len();

                    // Check overlap
                    if end <= *text_start || start >= seg_end {
                        continue;
                    }

                    // Clamp selection to segment bounds
                    let sel_start = start.saturating_sub(*text_start);
                    let sel_end = (end - text_start).min(laidout_text.text.len());

                    let selection = Selection {
                        anchor: Cursor {
                            index: sel_start,
                            prefer_next_row: false,
                        },
                        cursor: Cursor {
                            index: sel_end,
                            prefer_next_row: false,
                        },
                    };

                    // Add a small padding to selection rects so descenders aren't cut off
                    let padding = 2.0;
                    for sel_rect in laidout_text.selection_rects(selection) {
                        rects.push(Rect {
                            pos: *origin
                                + dvec2(
                                    (sel_rect.rect_in_lpxs.origin.x * font_scale) as f64,
                                    (sel_rect.rect_in_lpxs.origin.y * font_scale) as f64 - padding,
                                ),
                            size: dvec2(
                                (sel_rect.rect_in_lpxs.size.width * font_scale) as f64,
                                (sel_rect.rect_in_lpxs.size.height * font_scale) as f64
                                    + padding * 2.0,
                            ),
                        });
                    }
                }
                SelectionSegment::Gap { rect, text_start } => {
                    // If gap is in selection range, include its rect
                    let seg_end = text_start + 1; // Gap is 1 char
                    if start < seg_end && end > *text_start {
                        rects.push(*rect);
                    }
                }
                SelectionSegment::WidgetText { .. } => {
                    // Widget draws its own selection highlights - skip here
                }
            }
        }

        rects
    }
}

// this widget has a retained and an immediate mode api
#[derive(Script, WidgetRef, WidgetSet, WidgetRegister)]
pub struct TextFlow {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[live]
    pub draw_text: DrawText,
    #[live]
    pub text_style_normal: TextStyle,
    #[live]
    pub text_style_italic: TextStyle,
    #[live]
    pub text_style_bold: TextStyle,
    #[live]
    pub text_style_bold_italic: TextStyle,
    #[live]
    pub text_style_fixed: TextStyle,
    #[live]
    pub draw_block: DrawFlowBlock,

    /// The default font size used for all text if not otherwise specified.
    #[live]
    pub font_size: f32,
    /// The default font color used for all text if not otherwise specified.
    #[live]
    pub font_color: Vec4f,

    /// Maximum number of lines to display. 0 means unlimited (default).
    /// Combined with `text_overflow: Ellipsis`, truncated text shows "…".
    #[live(0usize)]
    pub max_lines: usize,
    /// Controls how text overflow is handled when text exceeds the container.
    #[live]
    pub text_overflow: TextOverflow,
    #[walk]
    walk: Walk,

    #[rust]
    area_stack: SmallVec<[Area; 4]>,
    #[rust]
    pub font_sizes: SmallVec<[f32; 8]>,
    /// Per-run vertical baseline shifts, in multiples of the run's font size.
    /// Positive values shift glyphs down; negative values shift them up.
    /// Used to render `<sub>` / `<sup>` at a raised or lowered baseline.
    #[rust]
    pub y_shift_scales: SmallVec<[f32; 4]>,
    #[rust]
    pub font_colors: SmallVec<[Vec4f; 8]>,
    #[rust]
    pub bold: StackCounter,
    #[rust]
    pub italic: StackCounter,
    #[rust]
    pub fixed: StackCounter,
    #[rust]
    pub underline: StackCounter,
    #[rust]
    pub strikethrough: StackCounter,
    #[rust]
    pub inline_code: StackCounter,
    #[rust]
    pub code_block: StackCounter,

    /// (Ascender, descender) in ems per style slot (normal/bold/italic/
    /// bold_italic/fixed), probed lazily via a tiny layout call. Used to
    /// baseline-align mixed runs: a 0.85x inline-code run must sit ON the
    /// line's baseline, not hang from its top.
    #[rust]
    style_metrics_em: [Option<(f32, f32)>; 5],
    #[rust]
    pub item_counter: u64,
    #[rust]
    pub first_thing_on_a_line: bool,
    /// Number of visual lines drawn so far (across all text runs).
    /// Used for widget-level `max_lines` tracking in rich text.
    #[rust]
    lines_drawn: usize,
    /// Latched once content has been dropped: either a run needed a new visual
    /// line and the `max_lines` budget had none, or the layouter cut a run
    /// short. Gates every later draw. A full budget alone does not latch it —
    /// the last allowed line stays open to runs that continue it.
    #[rust]
    content_truncated: bool,
    /// How many of the blocks we skipped after the content got cut haven't ended yet.
    #[rust]
    skipped_block_depth: usize,
    /// Turtle y position where the most recent text run ended, identifying the
    /// visual line that run last occupied. A later run landing on a different
    /// row means something other than a text run opened that row. NaN until
    /// the first run of a draw pass.
    #[rust]
    last_row_y: f64,
    /// What the line breaking rules see before the next run, if it continues the current line.
    #[rust]
    pub line_break_before: PrecedingContext,
    /// Whether the last run kept room on its row for the next run's first unit.
    #[rust]
    is_start_reserved: bool,
    /// Whether that room includes the margin and padding of the code box the unit starts.
    #[rust]
    is_start_reserved_for_inline_code: bool,
    #[rust]
    line_break_after: FollowingContext,
    #[rust]
    trailing_glue_width_in_lpxs: f32,
    #[rust]
    is_trailing_glue_in_inline_code: bool,
    /// The room that what follows the inline content we hold needs on its row.
    #[rust]
    following_room_in_lpxs: f32,
    /// What Html needs to find what follows a widget's runs, which it does at the first one.
    #[rust]
    pending_text_after_run: Option<PendingTextAfterRun>,
    /// What Html gives us to find the room that what follows our runs needs, for the draw it's doing.
    #[rust]
    following_room_finder: Option<FollowingRoomFinder>,
    /// Where what follows our next runs starts, which we find the room for once one ends on the last line.
    #[rust]
    pending_following_room: Option<PendingFollowingRoom>,
    /// The ellipsis width in each of our five text styles, with the font family and generation we measured it in.
    #[rust]
    ellipsis_widths_in_ems: Cell<[Option<(FontFamilyId, u64, f32)>; 5]>,
    #[rust]
    inline_content_start_pos: DVec2,
    #[rust]
    was_text_drawn_in_inline_content: bool,
    #[rust]
    is_holding_inline_content: bool,
    /// Whether we turned wrapping off for the inline content we hold, which its runs turn back on.
    #[rust]
    is_wrap_off_for_inline_content: bool,
    /// Where the first run that ran past its row's edge in the inline content we hold starts,
    /// less the white space the run before it ends with.
    #[rust]
    overrun_start_x: Option<f64>,
    /// Where the last run ended and how it was laid out, while `max_lines` is set.
    #[rust]
    last_run_end: Option<(DVec2, Rc<LaidoutText>)>,

    #[rust]
    pub areas_tracker: RectAreasTracker,

    #[layout]
    layout: Layout,

    #[live]
    quote_layout: Layout,
    #[live]
    quote_walk: Walk,
    #[live]
    code_layout: Layout,
    #[live]
    code_walk: Walk,
    #[live]
    sep_walk: Walk,
    #[live]
    list_item_layout: Layout,
    #[live]
    list_item_walk: Walk,
    /// The spacing (in pixels) between the list item marker and the content text.
    #[live(5.0)]
    list_item_marker_pad: f64,
    #[live]
    table_walk: Walk,
    #[live]
    table_layout: Layout,
    #[live]
    table_row_walk: Walk,
    #[live]
    table_row_layout: Layout,
    #[live]
    table_cell_layout: Layout,
    #[rust]
    pub table_num_columns: usize,
    /// Horizontal text alignment applied by the layouter within the
    /// currently active table cell. Set by `begin_table_cell`, cleared
    /// by `end_table_cell`. Outside a cell it is always 0.0 (left).
    #[rust]
    pub cell_text_align_x: f64,
    #[rust]
    pub in_table_header: bool,
    #[rust]
    table_row_cell_rects: Vec<Rect>,
    #[rust]
    pub table_row_is_header: bool,
    #[rust]
    table_is_first_row: bool,
    /// The row each table we're in has open, whose cells all start on its first line.
    #[rust]
    open_table_rows: SmallVec<[Option<TableRowLines>; 2]>,
    #[live]
    pub inline_code_padding: Inset,
    #[live]
    pub inline_code_margin: Inset,
    /// Font-size scale for fixed/code runs relative to the surrounding text.
    #[live(0.85)]
    pub fixed_font_size_scale: f64,
    #[live(Inset{top:0.5,bottom:0.5,left:0.0,right:0.0})]
    pub heading_margin: Inset,
    #[live(Inset{top:0.5,bottom:0.5,left:0.0,right:0.0})]
    pub paragraph_margin: Inset,
    /// Collapses block margins like CSS does: neighboring ones share the larger one, and none
    /// are added at the start or end of the flow, a list item, a quote, a code block or a table cell.
    #[live(false)]
    pub collapse_block_margins: bool,
    /// The collapsed block margin that's applied right before the next content is drawn.
    #[rust]
    pending_block_margin: f64,
    /// Whether no content has been drawn yet in the current flow, list item (past its marker), quote, code block or table cell.
    #[rust]
    is_at_block_start: bool,

    #[rust]
    area: Area,
    #[rust]
    draw_state: DrawStateWrap<DrawState>,
    #[rust(Some(Default::default()))]
    items: Option<ComponentMap<LiveId, (WidgetRef, LiveId)>>,
    #[rust]
    templates: ComponentMap<LiveId, ScriptObjectRef>,

    #[live]
    pub draw_selection: DrawColor,

    #[live]
    pub draw_mark: DrawTextMark,

    /// The marked spans this flow is showing, over the text it accumulates as
    /// it draws (the same offsets [`TextFlow::get_full_text`] returns).
    #[rust]
    marks: TextMarkSet,

    /// Enable text selection
    #[live(false)]
    pub selectable: bool,

    /// Selection anchor (start) character index
    #[rust]
    selection_anchor: usize,

    /// Selection cursor (end) character index
    #[rust]
    selection_cursor: usize,

    /// Selection tracker (only populated when selectable)
    #[rust]
    selection_tracker: SelectionTracker,

    /// Child widgets participating in selection, with their text ranges.
    /// Kept separate from the tracker so it only holds Areas, not WidgetRefs.
    #[rust]
    widget_text_entries: Vec<(WidgetRef, usize, usize)>,

    /// Original word range while extending a double-click selection.
    #[rust]
    selection_word_anchor: Option<(usize, usize)>,

    /// Whether currently dragging to select
    #[rust]
    is_selecting: bool,

    // Streaming text animation fields
    #[rust]
    next_frame: NextFrame,
    /// Whether streaming animation is active
    #[rust]
    pub streaming_animation: bool,
    /// Animated char count for fade effect (lags behind actual)
    #[rust]
    animated_chars: f32,
    /// Actual drawn char count
    #[rust]
    actual_chars: f32,
    /// Last frame time for dt calculation
    #[rust]
    last_rate_time: f64,
    /// Number of chars over which to fade (default 50)
    #[live(50.0)]
    pub fade_chars: f32,
    /// Minimum animation speed in chars per second
    #[live(100.0)]
    pub min_fade_speed: f32,
}

impl TextFlow {
    fn apply_template(
        &mut self,
        vm: &mut ScriptVm,
        apply: &Apply,
        scope: &mut Scope,
        id: LiveId,
        template_obj: ScriptObject,
    ) {
        // Root the template object
        let template_ref = vm.bx.heap.new_object_ref(template_obj);
        self.templates.insert(id, template_ref);
        // Apply to existing items with matching template
        let template_value: ScriptValue = template_obj.into();
        for (node, templ_id) in self.items.as_mut().unwrap().values_mut() {
            if *templ_id == id {
                node.script_apply(vm, apply, scope, template_value);
            }
        }
    }

    /// Whether the `max_lines` budget is spent, so any further inline content
    /// would render past the line allowance.
    ///
    /// Text drawn through [`Self::draw_text`] honours this automatically.
    /// Callers that draw child widgets straight into the turtle — link pills,
    /// images, fold buttons — bypass that path and must consult this first.
    pub fn is_content_truncated(&self) -> bool {
        self.content_truncated
    }

    /// Prepares the turtle for inline content we don't lay out ourselves. Pair it with [`Self::end_inline_content`].
    ///
    /// Content starting on the last allowed line gets a clip so we can hide it, and with Ellipsis so does content
    /// that could wrap onto it. On the last line we turn wrapping off too, so it can't move to a row we don't have.
    pub fn begin_inline_content(&mut self, cx: &mut Cx2d) -> Option<InlineContentHold> {
        let is_budget_spent = self.max_lines > 0 && self.lines_drawn >= self.max_lines;
        // A new line past the budget can't hold anything, e.g., right after a list or quote ends.
        if is_budget_spent && self.collapse_block_margins && self.is_at_new_line_start(cx) {
            self.content_truncated = true;
            return None;
        }
        self.flush_block_margin(cx);
        self.inline_content_start_pos = cx.turtle().pos();
        self.was_text_drawn_in_inline_content = false;
        if self.max_lines == 0 {
            return None;
        }
        // The row the content starts on costs a line too when nothing charged it yet, like a list item's marker row.
        let lines_drawn = self.lines_drawn + self.is_on_new_row(cx) as usize;
        // Content that can wrap onto the last line gets the clip too, so we can hide it there.
        let can_reach_last_line = self.text_overflow == TextOverflow::Ellipsis && lines_drawn + 1 >= self.max_lines;
        if lines_drawn < self.max_lines && !can_reach_last_line {
            return None;
        }
        let is_wrap_off =
            lines_drawn >= self.max_lines && matches!(cx.turtle().layout().flow, Flow::Right { wrap: true, .. });
        if is_wrap_off {
            cx.turtle_mut().set_flow_wrap(false);
        }
        self.is_holding_inline_content = true;
        self.is_wrap_off_for_inline_content = is_wrap_off;
        self.overrun_start_x = None;
        // A covering rect, so the clip is a no-op unless the content overruns
        // and end_inline_content shrinks it.
        let clip_index = cx.push_clip_rect_tracked(Rect {
            pos: dvec2(-1e7, -1e7),
            size: dvec2(2e7, 2e7),
        });
        Some(InlineContentHold {
            clip_index,
            start_x: self.inline_content_start_pos.x,
        })
    }

    /// Whether the turtle is at the left edge of its row, so the next thing drawn opens a new visual line.
    fn is_at_row_start(cx: &Cx2d) -> bool {
        cx.turtle().pos().x - cx.turtle().inner_rect().pos.x <= 0.5
    }

    /// Whether the turtle is past the right edge of its row, after content that didn't fit there.
    fn is_past_row_end(cx: &Cx2d) -> bool {
        cx.turtle().pos().x > Self::get_row_end(cx) + 0.5
    }

    /// Returns where the turtle's rows end on the right, which is NaN when they have no width.
    fn get_row_end(cx: &Cx2d) -> f64 {
        let inner = cx.turtle().inner_rect();
        inner.pos.x + inner.size.x
    }

    /// Whether the turtle is on another row than the one the last item we charged ended on.
    fn is_on_new_row(&self, cx: &Cx2d) -> bool {
        self.last_row_y.is_nan() || (cx.turtle().pos().y - self.last_row_y).abs() > 0.5
    }

    /// Whether the turtle is at the start of a row we haven't charged, so the next thing drawn opens a new line.
    fn is_at_new_line_start(&self, cx: &Cx2d) -> bool {
        Self::is_at_row_start(cx) && self.is_on_new_row(cx)
    }

    /// Whether the inline content since [`Self::begin_inline_content`] drew no runs and ends on the last line
    /// `max_lines` allows, so what follows it decides if it ends with the ellipsis.
    pub(crate) fn is_atomic_inline_content_on_last_line(&self, cx: &Cx2d) -> bool {
        !self.was_text_drawn_in_inline_content
            && self.has_ellipsis_at_max_lines()
            && self.lines_drawn + self.has_left_uncharged_row(cx) as usize + self.is_on_new_row(cx) as usize
                == self.max_lines
    }

    /// Whether the inline content since [`Self::begin_inline_content`] drew no runs and wrapped off the row
    /// it started on, which nothing charged yet, like a list item's marker row.
    fn has_left_uncharged_row(&self, cx: &Cx2d) -> bool {
        let start_y = self.inline_content_start_pos.y;
        !self.was_text_drawn_in_inline_content
            && (self.last_row_y.is_nan() || (start_y - self.last_row_y).abs() > 0.5)
            && (cx.turtle().pos().y - start_y).abs() > 0.5
    }

    /// Returns how wide the white space is that the last run ends with, when it ended at `pos`.
    fn get_white_space_width_before(&self, pos: DVec2) -> f64 {
        self.last_run_end.as_ref().filter(|(end, _)| *end == pos).map_or(0.0, |(_, run)| {
            let last_row = run.rows.last().unwrap();
            let white_space_start = last_row.text.trim_end_matches(is_hangable_white_space).len();
            let white_space_width_in_lpxs: f32 = last_row
                .glyphs
                .iter()
                .filter(|glyph| glyph.cluster >= white_space_start)
                .map(|glyph| glyph.advance_in_lpxs())
                .sum();
            (white_space_width_in_lpxs * self.draw_text.font_scale) as f64
        })
    }

    /// The block-start state that `end_inline_widget` puts back if a widget draws nothing.
    pub(crate) fn block_start_state(&self) -> (bool, f64) {
        (self.is_at_block_start, self.pending_block_margin)
    }

    /// Counts a drawn inline widget as content on its line. If it drew nothing, the block
    /// start (and pending margin) that `begin_inline_content` used up are put back.
    pub(crate) fn end_inline_widget(&mut self, cx: &Cx2d, block_start_state: (bool, f64)) {
        if !self.collapse_block_margins {
            return;
        }
        if cx.turtle().pos() != self.inline_content_start_pos {
            self.first_thing_on_a_line = false;
        } else if block_start_state.0 {
            (self.is_at_block_start, self.pending_block_margin) = block_start_state;
        }
    }

    /// Ends inline content begun with [`Self::begin_inline_content`] and charges it to the line budget.
    ///
    /// When held content overruns the last allowed line, or leaves no room there for what follows, we end the
    /// flow with an ellipsis and hide the content past it. `TextOverflow::Clip` hides it without an ellipsis.
    pub fn end_inline_content(&mut self, cx: &mut Cx2d, hold: Option<InlineContentHold>) {
        if !self.was_text_drawn_in_inline_content {
            // The room the run before kept was for runs, so it's gone after content that draws none.
            self.is_start_reserved = false;
            // Content that moved the turtle without drawing text through us, like a pill or an
            // image, is atomic: a line can break on either side of it.
            if cx.turtle().pos() != self.inline_content_start_pos {
                self.line_break_before = PrecedingContext::AFTER_ATOMIC_INLINE;
            }
        }
        if self.max_lines > 0 && self.has_left_uncharged_row(cx) {
            self.lines_drawn += 1;
        }
        let Some(hold) = hold else {
            self.track_inline_content(cx);
            return;
        };
        self.is_holding_inline_content = false;
        let was_wrap_off = std::mem::take(&mut self.is_wrap_off_for_inline_content);
        let following_room = (std::mem::take(&mut self.following_room_in_lpxs) * self.draw_text.font_scale) as f64;
        // The turtle can end past the edge after text that fits, like after a hanging space, so runs tell us.
        let overrun_start_x = if self.was_text_drawn_in_inline_content {
            self.overrun_start_x.take()
        } else {
            Self::is_past_row_end(cx)
                .then(|| hold.start_x - self.get_white_space_width_before(self.inline_content_start_pos))
        };
        let overran = overrun_start_x.is_some();
        self.charge_inline_content(cx, overran);

        let pos = cx.turtle().pos();
        let inner = cx.turtle().inner_rect();
        let right = inner.pos.x + inner.size.x;
        let is_on_one_row = (pos.y - self.inline_content_start_pos.y).abs() <= 0.5;
        // Text that wrapped onto this row already ended with its own ellipsis where it could.
        let can_hide = is_on_one_row || !self.was_text_drawn_in_inline_content;
        let is_cut_after = following_room > 0.0 && pos.x + following_room > right;
        let has_ellipsis = self.text_overflow == TextOverflow::Ellipsis;
        let ellipsis_x = if self.lines_drawn == self.max_lines && can_hide && (overran || is_cut_after) {
            let ellipsis_width = if has_ellipsis {
                (self.measure_ellipsis_in_lpxs(cx, self.get_run_style()) * self.draw_text.font_scale) as f64
            } else {
                0.0
            };
            // The ellipsis goes right after the content if it fits there, and otherwise where the content starts,
            // or where the run that overran starts, less the white space before it that would hang at a wrap.
            let ellipsis_x = if !overran && pos.x + ellipsis_width <= right {
                pos.x
            } else if !is_on_one_row {
                inner.pos.x
            } else {
                overrun_start_x.unwrap_or(hold.start_x)
            };
            // Like text truncation, we hide whole glyphs rather than slicing one, so we hide a widget whole
            // or from the run in it that overran. When even that spot can't fit the ellipsis, we pull it left.
            Some(ellipsis_x.min(right - ellipsis_width).max(inner.pos.x))
        } else {
            None
        };
        match ellipsis_x {
            Some(ellipsis_x) if ellipsis_x < pos.x => {
                cx.update_clip_rect_at(
                    hold.clip_index,
                    Rect {
                        pos: dvec2(-1e7, -1e7),
                        size: dvec2(ellipsis_x + 1e7, 2e7),
                    },
                );
                cx.pop_clip_rect();
            }
            _ => cx.remove_clip_rect_at(hold.clip_index),
        }
        if let Some(ellipsis_x) = ellipsis_x {
            let row_y = pos.y;
            if has_ellipsis {
                // The ellipsis fits on this row, so it can't open a row the budget has no line for.
                cx.turtle_mut().move_to(dvec2(ellipsis_x, row_y));
                // A run in the content may have cut it already, which would skip our ellipsis.
                self.content_truncated = false;
                self.draw_text(cx, "…");
            }
            self.content_truncated = true;
            self.last_row_y = row_y;
        }
        if was_wrap_off {
            cx.turtle_mut().set_flow_wrap(true);
        }
    }

    /// Charges the `max_lines` budget for inline content that TextFlow does not
    /// lay out itself: a link pill, an image, a fold button.
    ///
    /// The turtle relocates such content whole onto a fresh row when it does
    /// not fit, and that row counts against `max_lines` exactly like a row of
    /// text. Call this immediately after drawing the content. Content sharing
    /// the row of the previously charged item costs nothing, so calling it for
    /// every inline item is correct.
    ///
    /// Whether the content fits is only knowable after it is drawn — makepad
    /// has no measure-without-draw path for widgets — so content that spills
    /// past the last allowed line latches truncation to stop everything after
    /// it rather than being withdrawn.
    pub fn track_inline_content(&mut self, cx: &mut Cx2d) {
        if self.max_lines == 0 {
            return;
        }
        // Content held on the last allowed line runs past the right edge when
        // it did not fit. Nothing after it can be placed without opening a row
        // the budget cannot pay for, so the flow ends here.
        self.charge_inline_content(cx, Self::is_past_row_end(cx));
    }

    /// Charges the row the turtle is on to the `max_lines` budget, and cuts the content that follows
    /// when that's past the budget, or when the content `has_overrun` the last line.
    fn charge_inline_content(&mut self, cx: &Cx2d, has_overrun: bool) {
        if self.is_on_new_row(cx) {
            self.lines_drawn += 1;
        }
        self.last_row_y = cx.turtle().pos().y;
        if self.lines_drawn > self.max_lines || (has_overrun && self.lines_drawn == self.max_lines) {
            self.content_truncated = true;
        }
    }

    /// Sets the default font color used for all text.
    ///
    /// Does nothing if the color is unchanged.
    pub fn set_font_color(&mut self, cx: &mut Cx, color: Vec4f) {
        if self.font_color == color {
            return;
        }
        self.font_color = color;
        self.redraw(cx);
    }

    /// Sets the background color of inline code and code blocks.
    ///
    /// Does nothing if the color is unchanged.
    pub fn set_code_color(&mut self, cx: &mut Cx, color: Vec4f) {
        if self.draw_block.code_color == color {
            return;
        }
        self.draw_block.code_color = color;
        self.redraw(cx);
    }

    /// Sets the background color of quote blocks.
    ///
    /// Does nothing if the color is unchanged.
    pub fn set_quote_bg_color(&mut self, cx: &mut Cx, color: Vec4f) {
        if self.draw_block.quote_bg_color == color {
            return;
        }
        self.draw_block.quote_bg_color = color;
        self.redraw(cx);
    }
}

impl ScriptHook for TextFlow {
    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        apply: &Apply,
        scope: &mut Scope,
        value: ScriptValue,
    ) {
        // Only collect during template applies (not eval) to avoid storing temporary objects
        if !apply.is_eval() {
            if let Some(obj) = value.as_object() {
                vm.vec_with(obj, |vm, vec| {
                    for kv in vec {
                        if let Some(id) = kv.key.as_id() {
                            if let Some(template_obj) = kv.value.as_object() {
                                self.apply_template(vm, apply, scope, id, template_obj);
                            }
                        }
                    }
                });
            }
        }
    }
}

#[derive(Default)]
pub struct RectAreasTracker {
    pub areas: SmallVec<[Area; 4]>,
    pos: usize,
    stack: SmallVec<[usize; 2]>,
}

impl RectAreasTracker {
    fn clear_stack(&mut self) {
        self.pos = 0;
        self.areas.clear();
        self.stack.clear();
    }

    pub fn push_tracker(&mut self) {
        self.stack.push(self.pos);
    }

    // this returns the range in the area vec
    pub fn pop_tracker(&mut self) -> (usize, usize) {
        return (self.stack.pop().unwrap(), self.pos);
    }

    pub fn track_rect(&mut self, cx: &mut Cx2d, rect: Rect) {
        if self.stack.len() > 0 {
            if self.pos >= self.areas.len() {
                self.areas.push(Area::Empty);
            }
            cx.add_aligned_rect_area(&mut self.areas[self.pos], rect);
            self.pos += 1;
        }
    }
}

#[derive(Clone)]
pub enum DrawState {
    Begin,
    Drawing,
}

impl WidgetNode for TextFlow {
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }
    fn walk(&mut self, _cx: &mut Cx) -> Walk {
        self.walk
    }

    fn area(&self) -> Area {
        self.area.area()
    }

    fn redraw(&mut self, cx: &mut Cx) {
        self.area.redraw(cx);
    }

    fn children(&self, visit: &mut dyn FnMut(LiveId, WidgetRef)) {
        if let Some(items) = self.items.as_ref() {
            for (id, (widget, _template)) in items.iter() {
                visit(*id, widget.clone());
            }
        }
    }

    fn find_widgets_from_point(&self, cx: &Cx, point: DVec2, found: &mut dyn FnMut(&WidgetRef)) {
        if let Some(items) = self.items.as_ref() {
            for (_id, (widget, _template)) in items.iter() {
                widget.find_widgets_from_point(cx, point, found);
            }
        }
    }

    fn selection_text_len(&self) -> usize {
        self.text_len()
    }
    fn selection_point_to_char_index(&self, cx: &Cx, abs: DVec2) -> Option<usize> {
        self.selection_tracker.point_to_index(cx, abs)
    }
    fn selection_set(&mut self, anchor: usize, cursor: usize) {
        self.set_selection(anchor, cursor)
    }
    fn selection_clear(&mut self) {
        self.clear_selection()
    }
    fn selection_select_all(&mut self) {
        self.select_all()
    }
    fn selection_get_text_for_range(&self, start: usize, end: usize) -> String {
        self.get_text_for_range(start, end)
    }
    fn selection_get_full_text(&self) -> String {
        self.get_full_text()
    }
}

impl Widget for TextFlow {
    fn is_interactive(&self) -> bool {
        false
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if self.draw_state.begin(cx, DrawState::Begin) {
            self.begin(cx, walk);
            return DrawStep::make_step();
        }
        if let Some(_) = self.draw_state.get() {
            self.end(cx);
            self.draw_state.end();
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        // Handle child item events first
        for (_id, (entry, _)) in self.items.as_mut().unwrap().iter_mut() {
            entry.handle_event(cx, event, scope);
        }

        // Handle streaming animation NextFrame
        if let Some(ev) = self.next_frame.is_event(event) {
            let time = ev.time;
            // Calculate time delta
            let dt = if self.last_rate_time > 0.0 {
                (time - self.last_rate_time).max(0.001)
            } else {
                1.0 / 60.0
            };
            self.last_rate_time = time;

            // Target depends on streaming state:
            // - While streaming: chase actual_chars (so new text fades in)
            // - After streaming: go to actual_chars + fade_chars (complete the fade)
            let target = if self.streaming_animation {
                self.actual_chars
            } else {
                self.actual_chars + self.fade_chars
            };

            // Calculate how far behind we are
            let distance = target - self.animated_chars;

            if distance > 0.0 {
                // Speed scales with distance - further behind = faster catch up
                // Use a proportional approach: catch up ~20% of distance per frame at 60fps
                // Plus a minimum speed to ensure we always make progress
                let catch_up_factor = 0.15; // Fraction of distance to cover per frame (at 60fps baseline)
                let frame_factor = (dt * 60.0) as f32; // Scale for actual frame rate
                let proportional_speed = distance * catch_up_factor * frame_factor;
                let min_speed = self.min_fade_speed * dt as f32;
                let speed = proportional_speed.max(min_speed);

                self.animated_chars += speed;
                // Don't overshoot
                if self.animated_chars > target {
                    self.animated_chars = target;
                }
                // Update shader directly and request redraw for the area
                self.draw_text.set_total_chars(cx, self.animated_chars);
                self.draw_text.redraw_areas(cx);
            }

            // Keep animation alive if streaming or not done fading
            let done = !self.streaming_animation
                && self.animated_chars >= self.actual_chars + self.fade_chars;
            if !done {
                self.next_frame = cx.new_next_frame();
            }
        }

        // Handle selection events when selectable (standalone mode only)
        // When inside a selectable PortalList, PortalList handles events directly
        if !self.selectable {
            return;
        }

        match event.hits(cx, self.area) {
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Text);
            }
            Hit::FingerHoverOut(_) => {
                // A link or other child may just have claimed hover and set
                // its own cursor. Leaving the selectable text must not erase it.
                if event.pointer_claimed_area().is_empty() {
                    cx.set_cursor(MouseCursor::Default);
                }
            }
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                cx.set_key_focus(self.area);
                if fe.device.is_touch() {
                    cx.hide_clipboard_actions();
                }
                if let Some(idx) = self.selection_tracker.point_to_index(cx, fe.abs) {
                    self.selection_word_anchor = if fe.tap_count == 2 {
                        let (start, end) = self.selection_tracker.word_range(idx);
                        self.set_selection(start, end);
                        Some((start, end))
                    } else {
                        self.set_selection(idx, idx);
                        None
                    };
                    self.is_selecting = true;
                    self.redraw(cx);
                }
            }
            Hit::FingerMove(fe) if self.is_selecting => {
                if let Some(idx) = self.selection_tracker.point_to_index(cx, fe.abs) {
                    let (anchor, cursor) = if let Some((start, end)) = self.selection_word_anchor {
                        let (word_start, word_end) = self.selection_tracker.word_range(idx);
                        if idx < start {
                            (end, word_start)
                        } else if idx >= end {
                            (start, word_end)
                        } else {
                            (start, end)
                        }
                    } else {
                        (self.selection_anchor, idx)
                    };
                    if self.selection_anchor != anchor || self.selection_cursor != cursor {
                        self.set_selection(anchor, cursor);
                        self.redraw(cx);
                    }
                }
            }
            Hit::FingerUp(fe) => {
                self.is_selecting = false;
                self.selection_word_anchor = None;
                // A press taken away offers no clipboard actions.
                if fe.device.is_touch() && !fe.cancelled {
                    let has_selection = self.has_selection();
                    if has_selection {
                        let selection_rect = self.selection_clipboard_rect(cx);
                        cx.show_clipboard_actions(true, selection_rect, cx.keyboard_shift);
                    } else {
                        cx.hide_clipboard_actions();
                    }
                }
            }
            Hit::KeyFocusLost(_) => {
                self.clear_selection();
                cx.hide_clipboard_actions();
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
                }
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::KeyA,
                modifiers,
                ..
            }) if modifiers.is_primary() => {
                self.select_all();
                self.redraw(cx);
            }
            _ => {}
        }
    }
}

impl TextFlow {
    pub fn begin(&mut self, cx: &mut Cx2d, walk: Walk) {
        cx.begin_turtle(walk, self.layout);
        self.draw_state.set(DrawState::Drawing);
        self.draw_block.append_to_draw_call(cx);
        self.draw_text.begin_deferred_slug_flush();
        self.clear_stacks();
        self.lines_drawn = 0;
        self.content_truncated = false;
        self.skipped_block_depth = 0;
        self.last_row_y = f64::NAN;
        self.last_run_end = None;
        self.is_holding_inline_content = false;
        self.is_wrap_off_for_inline_content = false;
        self.overrun_start_x = None;
        self.following_room_finder = None;
        if self.tracks_text() {
            self.selection_tracker.clear();
            self.widget_text_entries.clear();
        }
        // Always reset char_index so it doesn't accumulate across redraws.
        // Without this, char_index grows unboundedly and eventually exceeds
        // the shader default total_chars (1 000 000), causing get_color to
        // compute alpha=0 for every glyph — making text invisible.
        self.draw_text.reset_char_index();
    }

    /// Check if animation is completely idle (not streaming and fade complete)
    pub fn is_animation_idle(&self) -> bool {
        // Animation was never started — both counters still at their initial zero
        // values and streaming flag is off.  Without this early-out a fresh
        // TextFlow would be treated as "mid-animation" because 0.0 < 0.0 + fade_chars,
        // causing set_total_chars(0.0) which overrides the shader default (1 000 000)
        // and makes all text invisible.
        if !self.streaming_animation && self.animated_chars == 0.0 && self.actual_chars == 0.0 {
            return true;
        }
        !self.streaming_animation && self.animated_chars >= self.actual_chars + self.fade_chars
    }

    pub fn clear_stacks(&mut self) {
        self.item_counter = 0;
        self.areas_tracker.clear_stack();
        self.bold.clear();
        self.italic.clear();
        self.fixed.clear();
        self.underline.clear();
        self.strikethrough.clear();
        self.inline_code.clear();
        self.code_block.clear();
        self.font_sizes.clear();
        self.y_shift_scales.clear();
        self.font_colors.clear();
        self.area_stack.clear();
        self.mark_as_line_start();
        self.clear_line_break_after();
        self.table_num_columns = 0;
        self.in_table_header = false;
        self.table_row_cell_rects.clear();
        self.table_row_is_header = false;
        self.table_is_first_row = false;
        self.open_table_rows.clear();
        self.cell_text_align_x = 0.0;
        self.pending_block_margin = 0.0;
        self.is_at_block_start = true;
    }

    /// Marks whatever is drawn next as the first thing on its line.
    pub(crate) fn mark_as_line_start(&mut self) {
        self.first_thing_on_a_line = true;
        self.reset_line_breaking();
    }

    /// Makes line breaking start over at whatever is drawn next, as if it began a paragraph.
    pub(crate) fn reset_line_breaking(&mut self) {
        self.line_break_before = PrecedingContext::PARAGRAPH_START;
        self.is_start_reserved = false;
    }

    /// Sets what follows the runs drawn from now on in their line, until [`Self::clear_line_break_after`].
    ///
    /// That lets those runs break like the rest of their paragraph.
    /// Pass `is_trailing_glue_in_inline_code` when the glue's width includes the code box it starts in.
    pub fn set_line_break_after(
        &mut self,
        line_break_after: FollowingContext,
        trailing_glue_width_in_lpxs: f32,
        is_trailing_glue_in_inline_code: bool,
    ) {
        self.line_break_after = line_break_after;
        self.trailing_glue_width_in_lpxs = trailing_glue_width_in_lpxs;
        self.is_trailing_glue_in_inline_code = is_trailing_glue_in_inline_code;
    }

    /// Like [`Self::set_line_break_after`], but `pending` finds what follows only when we lay out a run.
    pub(crate) fn set_line_break_after_lazily(&mut self, pending: PendingTextAfterRun) {
        self.pending_text_after_run = Some(pending);
    }

    /// Sets the room that what follows the inline content we hold needs on its row.
    ///
    /// On the last line `max_lines` allows, it ends with the ellipsis when it leaves less room than that.
    pub(crate) fn set_following_room(&mut self, following_room_in_lpxs: f32) {
        self.following_room_in_lpxs = following_room_in_lpxs;
    }

    /// Sets what finds the room that what follows our runs needs, until the next draw begins.
    pub(crate) fn set_following_room_finder(&mut self, finder: FollowingRoomFinder) {
        self.following_room_finder = Some(finder);
    }

    /// Like [`Self::set_following_room`], but for our next runs, and we find the room only for a run
    /// that ends on the last line `max_lines` allows.
    pub(crate) fn set_following_room_lazily(&mut self, pending: PendingFollowingRoom) {
        self.pending_following_room = Some(pending);
    }

    /// Whether the last line `max_lines` allows ends with an ellipsis when content gets cut.
    pub(crate) fn has_ellipsis_at_max_lines(&self) -> bool {
        self.max_lines > 0 && self.text_overflow == TextOverflow::Ellipsis
    }

    /// Goes back to laying out runs as if nothing followed them.
    pub fn clear_line_break_after(&mut self) {
        self.pending_text_after_run = None;
        self.pending_following_room = None;
        self.set_line_break_after(FollowingContext::END_OF_TEXT, 0.0, false);
    }

    pub fn push_size_rel_scale(&mut self, scale: f64) {
        self.font_sizes
            .push(self.font_sizes.last().unwrap_or(&self.font_size) * (scale as f32));
    }

    fn get_text_style(&self, slot: TextStyleSlot) -> &TextStyle {
        match slot {
            TextStyleSlot::Normal => &self.text_style_normal,
            TextStyleSlot::Bold => &self.text_style_bold,
            TextStyleSlot::Italic => &self.text_style_italic,
            TextStyleSlot::BoldItalic => &self.text_style_bold_italic,
            TextStyleSlot::Fixed => &self.text_style_fixed,
        }
    }

    /// Returns the text style that runs in `slot` are drawn with, and our DrawText.
    ///
    /// We borrow both at once so a probe can swap the style into the DrawText instead of cloning it.
    fn get_text_style_and_draw_text(&mut self, slot: TextStyleSlot) -> (&mut TextStyle, &mut DrawText) {
        let text_style = match slot {
            TextStyleSlot::Normal => &mut self.text_style_normal,
            TextStyleSlot::Bold => &mut self.text_style_bold,
            TextStyleSlot::Italic => &mut self.text_style_italic,
            TextStyleSlot::BoldItalic => &mut self.text_style_bold_italic,
            TextStyleSlot::Fixed => &mut self.text_style_fixed,
        };
        (text_style, &mut self.draw_text)
    }

    /// (Ascender, descender) in ems (descender positive) of one of the five
    /// run styles, probed once via a tiny layout call and cached.
    fn style_metrics_em(&mut self, cx: &mut Cx2d, slot: TextStyleSlot) -> (f32, f32) {
        if let Some(v) = self.style_metrics_em[slot as usize] {
            return v;
        }
        const PROBE_PT: f32 = 100.0;
        let (asc_lpxs, desc_lpxs) = self
            .layout_probe(cx, slot, PROBE_PT, "Ag")
            .rows
            .first()
            .map(|row| (row.ascender_in_lpxs, -row.descender_in_lpxs))
            .unwrap_or((PROBE_PT * LPXS_PER_PT * 0.9, PROBE_PT * LPXS_PER_PT * 0.3));
        let em = (
            asc_lpxs / (PROBE_PT * LPXS_PER_PT),
            desc_lpxs / (PROBE_PT * LPXS_PER_PT),
        );
        self.style_metrics_em[slot as usize] = Some(em);
        em
    }

    /// The font size (in points) of the current line. A code or sub/superscript run
    /// pushes its own scaled size; the entry below it (or the base size) is the line's.
    pub fn line_font_size(&self) -> f32 {
        let scaled_run = self.fixed.value() > 0
            || self.y_shift_scales.last().is_some_and(|scale| *scale != 0.0);
        if scaled_run {
            self.font_sizes.iter().rev().nth(1).copied().unwrap_or(self.font_size)
        } else {
            self.font_sizes.last().copied().unwrap_or(self.font_size)
        }
    }

    /// Returns the height and baseline of a line of text in the current line's style.
    fn get_line_height_and_baseline_in_lpxs(&mut self, cx: &mut Cx2d) -> (f32, f32) {
        let line_size = self.line_font_size();
        let (line_asc, line_desc) = self.style_metrics_em(cx, TextStyleSlot::Normal);
        ((line_asc + line_desc) * line_size * LPXS_PER_PT, line_asc * line_size * LPXS_PER_PT)
    }

    /// Makes the current line at least as tall as a line of text, even with no text on it.
    fn make_line_as_tall_as_text(&mut self, cx: &mut Cx2d) {
        let (height_in_lpxs, baseline_in_lpxs) = self.get_line_height_and_baseline_in_lpxs(cx);
        let font_scale = self.draw_text.font_scale as f64;
        let height = height_in_lpxs as f64 * font_scale;
        let align_list_start = cx.align_list_len();
        let pos = cx.turtle().pos();
        let turtle = cx.turtle_mut();
        turtle.allocate_height(height);
        turtle.set_wrap_spacing((baseline_in_lpxs * (self.text_style_normal.line_spacing - 1.0)) as f64);
        cx.emit_turtle_walk_with_role(
            Rect { pos, size: dvec2(0.0, height) },
            align_list_start,
            Metrics::default(),
            Some(height),
            Some(baseline_in_lpxs as f64 * font_scale),
            RowAlignRole::Shiftable,
        );
    }

    /// Lays out a probe in the current line's own style (the normal style at the
    /// line's size), so an inline widget can read the metrics it has to line up with.
    pub fn line_probe(&mut self, cx: &mut Cx2d) -> Rc<LaidoutText> {
        let line_size = self.line_font_size();
        self.layout_probe(cx, TextStyleSlot::Normal, line_size, "Ag")
    }

    /// Returns the style our style stacks give the next run.
    fn get_run_style(&self) -> RunStyle {
        RunStyle {
            is_bold: self.bold.value() > 0,
            is_italic: self.italic.value() > 0,
            is_fixed: self.fixed.value() > 0,
            font_size: *self.font_sizes.last().unwrap_or(&self.font_size),
        }
    }

    /// Measures `text` the way a run in `style` draws it, without wrapping.
    pub(crate) fn measure_text_in_lpxs(&mut self, cx: &mut Cx2d, text: &str, style: RunStyle) -> f32 {
        self.draw_text.max_lines = 0;
        self.draw_text.text_overflow = TextOverflow::Clip;
        self.layout_probe(cx, style.get_text_style_slot(), style.font_size, text).size_in_lpxs.width
    }

    /// Shapes `text` the way a run in `style` draws it, and returns it with the run's font size in lpxs.
    pub(crate) fn shape_text(&self, cx: &mut Cx2d, text: &str, style: RunStyle) -> (Rc<ShapedText>, f32) {
        let shaped_text = self.get_text_style(style.get_text_style_slot()).get_or_shape(cx, text);
        (shaped_text, style.font_size * LPXS_PER_PT)
    }

    /// Measures the ellipsis that a run in `style` ends with when it gets cut.
    pub(crate) fn measure_ellipsis_in_lpxs(&self, cx: &mut Cx2d, style: RunStyle) -> f32 {
        let slot = style.get_text_style_slot();
        let text_style = self.get_text_style(slot);
        let font_family_id = text_style.font_family_id();
        // We read this before measuring, since measuring can load a fallback font and change it.
        let font_family_generation = cx.fonts.borrow().font_family_generation();
        let mut ellipsis_widths_in_ems = self.ellipsis_widths_in_ems.get();
        let width_in_ems = match ellipsis_widths_in_ems[slot as usize] {
            Some((measured_font_family_id, measured_generation, width_in_ems))
                if (measured_font_family_id, measured_generation) == (font_family_id, font_family_generation) =>
            {
                width_in_ems
            }
            _ => {
                let width_in_ems = text_style.get_or_shape(cx, "…").width_in_ems;
                // A font that's still on its way would change the width, so we measure again until it's here.
                if !cx.fonts.borrow().is_font_family_loading(font_family_id) {
                    ellipsis_widths_in_ems[slot as usize] = Some((font_family_id, font_family_generation, width_in_ems));
                    self.ellipsis_widths_in_ems.set(ellipsis_widths_in_ems);
                }
                width_in_ems
            }
        };
        width_in_ems * (style.font_size * LPXS_PER_PT)
    }

    /// Lays out `text` without wrapping, in the style of `slot` at `font_size`.
    fn layout_probe(
        &mut self,
        cx: &mut Cx2d,
        slot: TextStyleSlot,
        font_size: f32,
        text: &str,
    ) -> Rc<LaidoutText> {
        let (text_style, draw_text) = self.get_text_style_and_draw_text(slot);
        std::mem::swap(&mut draw_text.text_style, text_style);
        let orig_font_size = std::mem::replace(&mut draw_text.text_style.font_size, font_size);
        let probe = draw_text.layout(cx, 0.0, 0.0, None, false, Align::default(), text);
        draw_text.text_style.font_size = orig_font_size;
        std::mem::swap(&mut draw_text.text_style, text_style);
        probe
    }

    /// Measures the hyphen a run in `style` draws where a row wraps after a soft hyphen.
    pub(crate) fn measure_hyphen_in_lpxs(&mut self, cx: &mut Cx2d, style: RunStyle) -> f32 {
        let hyphen = self.get_text_style(style.get_text_style_slot()).get_hyphen(cx);
        self.measure_text_in_lpxs(cx, hyphen, style)
    }

    pub fn push_size_abs_scale(&mut self, scale: f64) {
        self.font_sizes.push(self.font_size * (scale as f32));
    }

    pub fn end(&mut self, cx: &mut Cx2d) {
        self.draw_text.end_deferred_slug_flush(cx);

        // Draw selection highlight before finishing the turtle, then the
        // marks over it: a mark under a selected word must stay visible.
        self.draw_selection_rects(cx);
        self.draw_mark_rects(cx);

        cx.end_turtle_with_area(&mut self.area);
        self.items.as_mut().unwrap().retain_visible();

        // Update streaming animation state after drawing (also while fading out)
        let is_idle = self.is_animation_idle();
        if self.streaming_animation || !is_idle {
            self.actual_chars = self.draw_text.char_index;
            self.draw_text.set_total_chars(cx, self.animated_chars);
            self.next_frame = cx.new_next_frame();
        }
    }

    /// Start streaming text animation with fade-in effect on new characters.
    /// Call this before drawing when streaming new content.
    pub fn start_streaming_animation(&mut self) {
        self.streaming_animation = true;
    }

    /// Reset streaming animation state (for reused widgets).
    /// Call this when starting to stream new content.
    pub fn reset_streaming_animation(&mut self) {
        self.streaming_animation = true;
        self.animated_chars = 0.0;
        self.actual_chars = 0.0;
        self.last_rate_time = 0.0;
    }

    /// Stop streaming animation. The fade will complete naturally.
    pub fn stop_streaming_animation(&mut self) {
        self.streaming_animation = false;
    }

    /// Check if streaming animation is still running (including fade-out)
    pub fn is_streaming_animation_done(&self) -> bool {
        self.is_animation_idle()
    }

    /// Reset all streaming animations (text fade).
    pub fn reset_all_streaming_animations(&mut self) {
        self.reset_streaming_animation();
    }

    /// Whether this draw pass has to record its laid-out text.
    ///
    /// Selection needs it and so do marks: both turn a byte range into rects
    /// after the glyphs are placed, and neither can do that from the drawn
    /// glyphs alone. A flow with neither pays for none of it.
    fn tracks_text(&self) -> bool {
        self.selectable || !self.marks.is_empty()
    }

    /// Mark a stretch of this flow's text as wrong, doubtful or noteworthy,
    /// keeping the marks already set.
    ///
    /// `start` and `end` are byte offsets into the text the flow accumulates as
    /// it draws - what [`TextFlow::get_full_text`] returns - and may arrive
    /// either way round. The mark draws under the words themselves, so a host
    /// can say *which* word it means rather than colouring the whole block.
    ///
    /// Setting a mark makes the flow record its layout as it draws, the same
    /// work `selectable` asks for; a flow with no marks pays nothing.
    pub fn add_mark(&mut self, cx: &mut Cx, start: usize, end: usize, kind: TextMarkKind) {
        self.marks.push(TextMark::new(start, end, kind));
        self.redraw(cx);
    }

    /// Replace every mark on this flow.
    pub fn set_marks(&mut self, cx: &mut Cx, marks: impl IntoIterator<Item = TextMark>) {
        self.marks.set(marks);
        self.redraw(cx);
    }

    /// Drop every mark on this flow.
    pub fn clear_marks(&mut self, cx: &mut Cx) {
        if self.marks.clear_on_edit() {
            self.redraw(cx);
        }
    }

    /// The marks this flow is showing, in the order they were given.
    pub fn marks(&self) -> &[TextMark] {
        self.marks.as_slice()
    }

    /// Draw a mark under the words of every marked range.
    ///
    /// Overlapping marks all draw, in the order they were given, so a word
    /// flagged twice carries both squiggles rather than silently losing one.
    fn draw_mark_rects(&mut self, cx: &mut Cx2d) {
        if self.marks.is_empty() {
            return;
        }
        let band_height = self.draw_mark.band_height();
        for mark in self.marks.as_slice() {
            self.draw_mark.mark_kind = mark.kind;
            for rect in self.selection_tracker.mark_rects(*mark, band_height) {
                self.draw_mark.draw_abs(cx, rect);
            }
        }
    }

    /// Draw selection highlight rectangles
    fn draw_selection_rects(&mut self, cx: &mut Cx2d) {
        if !self.selectable {
            return;
        }

        let start = self.selection_anchor.min(self.selection_cursor);
        let end = self.selection_anchor.max(self.selection_cursor);

        if start == end {
            return;
        }

        for rect in self.selection_tracker.selection_rects(start, end) {
            self.draw_selection.draw_abs(cx, rect);
        }
    }

    /// Get the currently selected text
    pub fn selected_text(&self) -> String {
        if !self.selectable {
            return String::new();
        }
        let start = self.selection_anchor.min(self.selection_cursor);
        let end = self.selection_anchor.max(self.selection_cursor);
        if start == end {
            return String::new();
        }
        // Filter out object replacement characters from gaps
        self.selection_tracker
            .text
            .get(start..end)
            .unwrap_or("")
            .chars()
            .filter(|c| *c != '\u{FFFC}')
            .collect()
    }

    /// Select all text in this TextFlow
    pub fn select_all(&mut self) {
        if self.selectable {
            self.selection_word_anchor = None;
            self.selection_anchor = 0;
            self.selection_cursor = self.selection_tracker.total_len();
            for (widget, _, _) in &self.widget_text_entries {
                widget.selection_select_all();
            }
        }
    }

    /// Clear selection
    pub fn clear_selection(&mut self) {
        self.selection_word_anchor = None;
        self.selection_anchor = 0;
        self.selection_cursor = 0;
        self.is_selecting = false;
        for (widget, _, _) in &self.widget_text_entries {
            widget.selection_clear();
        }
    }

    /// Check if there is a selection
    pub fn has_selection(&self) -> bool {
        self.selectable && self.selection_anchor != self.selection_cursor
    }

    /// Selection anchor rect for clipboard/action popups.
    fn selection_clipboard_rect(&self, cx: &Cx) -> Rect {
        let start = self.selection_anchor.min(self.selection_cursor);
        let end = self.selection_anchor.max(self.selection_cursor);
        let rects = self.selection_tracker.selection_rects(start, end);

        let mut out: Option<Rect> = None;
        for rect in rects {
            out = Some(if let Some(acc) = out {
                let x0 = acc.pos.x.min(rect.pos.x);
                let y0 = acc.pos.y.min(rect.pos.y);
                let x1 = (acc.pos.x + acc.size.x).max(rect.pos.x + rect.size.x);
                let y1 = (acc.pos.y + acc.size.y).max(rect.pos.y + rect.size.y);
                Rect {
                    pos: dvec2(x0, y0),
                    size: dvec2((x1 - x0).max(1.0), (y1 - y0).max(1.0)),
                }
            } else {
                rect
            });
        }

        out.unwrap_or_else(|| self.area.rect(cx))
    }

    /// Set selection range (for external control, e.g., cross-TextFlow selection).
    /// Propagates sub-ranges to child WidgetText widgets so they draw their own selection.
    pub fn set_selection(&mut self, anchor: usize, cursor: usize) {
        if self.selectable {
            self.selection_anchor = anchor;
            self.selection_cursor = cursor;
            self.propagate_selection_to_children();
        }
    }

    /// Propagate the current selection range to child widgets (e.g., CodeView).
    /// Called both from external `set_selection` and internal mouse selection handling.
    fn propagate_selection_to_children(&self) {
        let start = self.selection_anchor.min(self.selection_cursor);
        let end = self.selection_anchor.max(self.selection_cursor);

        for (widget, text_start, text_len) in &self.widget_text_entries {
            let seg_end = text_start + text_len;
            if start < seg_end && end > *text_start {
                let sub_start = start.saturating_sub(*text_start);
                let sub_end = (end - text_start).min(*text_len);
                widget.selection_set(sub_start, sub_end);
            } else {
                widget.selection_clear();
            }
        }
    }

    /// Get the full text content (for cross-boundary copy)
    pub fn get_full_text(&self) -> String {
        // Filter out object replacement characters from gaps
        self.selection_tracker
            .text
            .chars()
            .filter(|c| *c != '\u{FFFC}')
            .collect()
    }

    /// Get text for a specific character range (for cross-boundary copy)
    pub fn get_text_for_range(&self, start: usize, end: usize) -> String {
        self.selection_tracker
            .text
            .get(start..end)
            .unwrap_or("")
            .chars()
            .filter(|c| *c != '\u{FFFC}')
            .collect()
    }

    /// Get the total text length
    pub fn text_len(&self) -> usize {
        self.selection_tracker.total_len()
    }

    /// Register a child widget's text content in the selection tracker.
    /// Call after drawing a child widget whose text should participate
    /// in cross-child selection (e.g. CodeView code blocks).
    /// The widget's Area is stored in the tracker for hit testing;
    /// the WidgetRef is stored separately for selection propagation.
    pub fn push_widget_text_for_selection(&mut self, widget: WidgetRef, text: &str) {
        if self.tracks_text() {
            let text_start = self.selection_tracker.text.len();
            let text_len = text.len();
            self.selection_tracker.push_widget_text(widget.area(), text);
            self.widget_text_entries
                .push((widget, text_start, text_len));
        }
    }

    pub fn begin_code(&mut self, cx: &mut Cx2d) {
        if self.skip_block_if_cut() {
            return;
        }
        self.draw_block.block_type = FlowBlockType::Code;
        let walk = self.begin_block_box(cx, self.code_walk);
        self.draw_block.begin(cx, walk, self.code_layout);
        self.area_stack.push(self.draw_block.draw_vars.area);
        self.code_block.push();
        self.mark_as_line_start();
        self.is_at_block_start = true;
    }

    pub fn end_code(&mut self, cx: &mut Cx2d) {
        if self.skip_end_of_skipped_block() {
            return;
        }
        // Tolerate unbalanced HTML: a stray close tag with no matching `begin_code`
        // (untrusted input, e.g. a chat message containing `</pre>`) must not panic
        // here or unbalance the turtle stack by ending a block that never began.
        let Some(area) = self.area_stack.pop() else {
            return;
        };
        self.code_block.pop();
        self.draw_block.draw_vars.area = area;
        self.draw_block.end(cx);
        self.end_block_box(cx, self.code_walk);
        if self.tracks_text() {
            self.selection_tracker.push_newline();
        }
    }

    /// Applies the pending block margin right before content is drawn,
    /// or drops it if nothing's been drawn yet in the current container.
    pub fn flush_block_margin(&mut self, cx: &mut Cx2d) {
        if !self.collapse_block_margins {
            return;
        }
        let margin = std::mem::take(&mut self.pending_block_margin);
        if self.is_at_block_start {
            self.is_at_block_start = false;
        } else if margin > 0.0 {
            cx.turtle_new_line_with_spacing(margin);
        }
    }

    /// Collapses a block's top margin into the pending one, then applies or drops it.
    fn begin_block_margin(&mut self, cx: &mut Cx2d, top: f64) {
        self.pending_block_margin = self.pending_block_margin.max(top);
        self.flush_block_margin(cx);
    }

    /// Ends a block: whatever comes next starts below it, after its bottom margin.
    fn end_block_margin(&mut self, cx: &mut Cx2d, bottom: f64) {
        cx.turtle_new_line();
        self.mark_as_line_start();
        self.is_at_block_start = false;
        self.pending_block_margin = self.pending_block_margin.max(bottom);
    }

    /// Returns the walk to begin a block box with. When collapsing,
    /// its vertical margins are handled as block margins instead.
    fn begin_block_box(&mut self, cx: &mut Cx2d, walk: Walk) -> Walk {
        if !self.collapse_block_margins {
            return walk;
        }
        // A box only lines up with a list item's marker on a baseline-aligned row,
        // so otherwise it starts on the row below the marker.
        let is_after_marker = self.is_at_block_start && !self.first_thing_on_a_line;
        if is_after_marker && !matches!(cx.turtle().flow(), Flow::Right { row_align: RowAlign::Baseline, .. }) {
            cx.turtle_new_line();
            self.mark_as_line_start();
            if self.tracks_text() {
                self.selection_tracker.push_newline();
            }
        }
        self.begin_block_margin(cx, walk.margin.top);
        Walk {
            margin: Inset { top: 0.0, bottom: 0.0, ..walk.margin },
            ..walk
        }
    }

    /// Ends a block box begun with `begin_block_box`, dropping any trailing margin inside it.
    fn end_block_box(&mut self, cx: &mut Cx2d, walk: Walk) {
        if !self.collapse_block_margins {
            return;
        }
        self.pending_block_margin = 0.0;
        self.end_block_margin(cx, walk.margin.bottom);
    }

    /// Skips a block that starts after the content got cut, so it draws nothing, and returns whether we skipped it.
    fn skip_block_if_cut(&mut self) -> bool {
        if self.content_truncated {
            self.skipped_block_depth += 1;
        }
        self.content_truncated
    }

    /// Returns whether the block that ends now is one we skipped, so we skip its end too.
    fn skip_end_of_skipped_block(&mut self) -> bool {
        if self.skipped_block_depth == 0 {
            return false;
        }
        self.skipped_block_depth -= 1;
        true
    }

    pub fn begin_list_item(&mut self, cx: &mut Cx2d, dot: &str, pad: f64) {
        if self.skip_block_if_cut() {
            return;
        }
        let fs = *self.font_sizes.last().unwrap_or(&self.font_size);
        let font_based_padding = fs as f64 * pad;

        let mut walk = self.list_item_walk;
        let mut padding = Inset {
            left: self.list_item_layout.padding.left + font_based_padding,
            ..self.list_item_layout.padding
        };
        if self.collapse_block_margins {
            // A list item doesn't draw a box, so its vertical padding acts as margin too.
            self.begin_block_margin(cx, walk.margin.top + padding.top);
            walk.margin.top = 0.0;
            walk.margin.bottom = 0.0;
            padding.top = 0.0;
            padding.bottom = 0.0;
        }
        cx.begin_turtle(walk, Layout { padding, ..self.list_item_layout });

        cx.turtle_mut()
            .move_right_down(dvec2(-font_based_padding, 0.0));

        // The marker and the item's first content run share one visual line,
        // but the padding adjustment below puts the content run back at the
        // line's left edge, where it reads as starting a line of its own. Let
        // the content run be the one that charges the line to the `max_lines`
        // budget, so a one-line list item costs one line rather than two.
        // A marker that exhausts the budget outright still truncates: only the
        // count is rolled back, never the truncation itself.
        let lines_before_marker = self.lines_drawn;
        self.draw_text(cx, dot);
        if !self.content_truncated {
            // Widgets only charge a row they see as new, so the row the marker charged has to look new to them too.
            if self.lines_drawn > lines_before_marker {
                self.last_row_y = f64::NAN;
            }
            self.lines_drawn = lines_before_marker;
        }
        TextFlow::walk_margin(cx, self.list_item_marker_pad);

        // Adjust the left padding to match the actual cursor position after the
        // bullet marker and its trailing pad, so that wrapped lines align with
        // the text after the marker rather than being over-indented.
        let actual_indent = cx.turtle().pos().x - cx.turtle().origin().x;
        cx.turtle_mut().set_padding_left(actual_indent);
        // Note: deliberately NOT pushed onto `area_stack` — `end_list_item` never pops,
        // so a push here would leak an entry per list item and let a stray close tag
        // (unbalanced HTML) pop the wrong block's area in `end_code`/`end_quote`.
        self.is_at_block_start = true;
        // The item's content breaks as a line of its own, so the marker isn't before it.
        self.reset_line_breaking();
    }

    pub fn end_list_item(&mut self, cx: &mut Cx2d) {
        if self.skip_end_of_skipped_block() {
            return;
        }
        cx.end_turtle();
        self.mark_as_line_start();
        if self.collapse_block_margins {
            // No box here either, so the last child's bottom margin collapses with the item's own.
            self.end_block_margin(cx, self.list_item_walk.margin.bottom + self.list_item_layout.padding.bottom);
        }
        if self.tracks_text() {
            self.selection_tracker.push_newline();
        }
    }

    pub fn new_line_collapsed(&mut self, cx: &mut Cx2d) {
        // Nothing's been drawn in this container yet, so there's no line to end.
        // Right after a list item's marker, that keeps the block on the marker's row.
        if self.collapse_block_margins && self.is_at_block_start {
            return;
        }
        cx.turtle_new_line();
        self.mark_as_line_start();
        if self.tracks_text() {
            self.selection_tracker.push_newline();
        }
    }

    /// Starts a new line using the current wrap spacing, so that the vertical
    /// gap matches the line spacing of the most recently drawn text.
    /// This is intended for `<br>` tags in HTML rendering.
    pub fn new_line_with_wrap_spacing(&mut self, cx: &mut Cx2d) {
        if self.content_truncated {
            return;
        }
        // Like in a browser, a line break on an empty line leaves a blank line as tall as text.
        let is_blank_line = self.first_thing_on_a_line && Self::is_at_row_start(cx);
        // A blank line past the `max_lines` budget cuts what follows, just like a run would.
        if is_blank_line && self.max_lines > 0 && self.lines_drawn >= self.max_lines && self.is_on_new_row(cx) {
            self.content_truncated = true;
            return;
        }
        self.flush_block_margin(cx);
        if is_blank_line {
            self.make_line_as_tall_as_text(cx);
            if self.max_lines > 0 {
                self.charge_inline_content(cx, false);
            }
        }
        let spacing = cx.turtle().wrap_spacing();
        cx.turtle_new_line_with_spacing(spacing);
        self.mark_as_line_start();
        if self.tracks_text() {
            self.selection_tracker.push_newline();
        }
    }

    pub fn new_line_collapsed_with_spacing(&mut self, cx: &mut Cx2d, spacing: f64) {
        if self.collapse_block_margins {
            self.new_line_collapsed(cx);
            self.pending_block_margin = self.pending_block_margin.max(spacing);
            return;
        }
        cx.turtle_new_line_with_spacing(spacing);
        self.mark_as_line_start();
        if self.tracks_text() {
            self.selection_tracker.push_newline();
        }
    }

    pub fn sep(&mut self, cx: &mut Cx2d) {
        if self.content_truncated {
            return;
        }
        self.draw_block.block_type = FlowBlockType::Sep;
        let walk = self.begin_block_box(cx, self.sep_walk);
        self.draw_block.draw_walk(cx, walk);
        self.end_block_box(cx, self.sep_walk);
    }

    pub fn begin_quote(&mut self, cx: &mut Cx2d) {
        if self.skip_block_if_cut() {
            return;
        }
        self.draw_block.block_type = FlowBlockType::Quote;
        let walk = self.begin_block_box(cx, self.quote_walk);
        self.draw_block
            .begin(cx, walk, self.quote_layout);
        self.area_stack.push(self.draw_block.draw_vars.area);
        self.mark_as_line_start();
        self.is_at_block_start = true;
    }

    pub fn end_quote(&mut self, cx: &mut Cx2d) {
        if self.skip_end_of_skipped_block() {
            return;
        }
        // Tolerate unbalanced HTML (see `end_code`): never panic on a stray close tag.
        let Some(area) = self.area_stack.pop() else {
            return;
        };
        self.draw_block.draw_vars.area = area;
        self.draw_block.end(cx);
        self.end_block_box(cx, self.quote_walk);
        if self.tracks_text() {
            self.selection_tracker.push_newline();
        }
    }

    pub fn begin_table(&mut self, cx: &mut Cx2d, num_columns: usize) {
        if self.skip_block_if_cut() {
            return;
        }
        self.table_num_columns = num_columns;
        self.table_is_first_row = true;
        if self.max_lines > 0 {
            self.open_table_rows.push(None);
        }
        let walk = self.begin_block_box(cx, self.table_walk);
        cx.begin_turtle(walk, self.table_layout);
    }

    pub fn end_table(&mut self, cx: &mut Cx2d) {
        if self.skip_end_of_skipped_block() {
            return;
        }
        if self.max_lines > 0 {
            self.open_table_rows.pop();
        }
        cx.end_turtle();
        self.table_num_columns = 0;
        self.in_table_header = false;
        self.end_block_box(cx, self.table_walk);
        if self.tracks_text() {
            self.selection_tracker.push_newline();
        }
    }

    pub fn begin_table_header_row(&mut self, cx: &mut Cx2d) {
        self.in_table_header = true;
        self.begin_any_table_row(cx, true);
    }

    pub fn begin_table_row(&mut self, cx: &mut Cx2d) {
        self.begin_any_table_row(cx, false);
    }

    fn begin_any_table_row(&mut self, cx: &mut Cx2d, is_header: bool) {
        if self.skip_block_if_cut() {
            return;
        }
        self.table_row_is_header = is_header;
        self.table_row_cell_rects.clear();
        if let Some(row) = self.open_table_rows.last_mut() {
            *row = Some(TableRowLines { start_lines_drawn: self.lines_drawn, max_lines_drawn: self.lines_drawn });
        }
        if !is_header {
            cx.begin_turtle(self.table_row_walk, self.table_row_layout);
            return;
        }
        // The header's ground goes down BEFORE its cells, not after them.
        // Drawn after, it covered its own text wherever the theme's highlight
        // is opaque -- which is every style sheet the library ships. Only the
        // three base themes escaped it, by making that highlight translucent,
        // which is why the header read fine in them and was blank under every
        // style. `begin` puts the quad in the background draw group and `end`
        // sizes it to the finished row, so it cannot cover what sits on it.
        // Its border is suppressed: the per-cell pass below still draws the
        // rules, on top, where they belong.
        let border = self.draw_block.table_border_color;
        self.draw_block.block_type = FlowBlockType::TableCell;
        self.draw_block.table_border_color = Vec4f::default();
        self.draw_block.begin(cx, self.table_row_walk, self.table_row_layout);
        self.draw_block.table_border_color = border;
    }

    pub fn end_table_row(&mut self, cx: &mut Cx2d) {
        if self.skip_end_of_skipped_block() {
            return;
        }
        if let Some(row) = self.open_table_rows.last_mut().and_then(Option::take) {
            self.lines_drawn = row.max_lines_drawn;
        }
        let row_rect = cx.turtle().rect();
        if self.table_row_is_header {
            self.draw_block.end(cx);
        } else {
            cx.end_turtle();
        }
        self.draw_row_cell_borders(cx, row_rect);
        if self.tracks_text() {
            self.selection_tracker.push_newline();
        }
    }

    /// Draw cell borders/backgrounds after the row has been laid out,
    /// so all cells use the row's height for uniform borders.
    fn draw_row_cell_borders(&mut self, cx: &mut Cx2d, row_rect: Rect) {
        let row_height = row_rect.size.y;
        let is_first_row = self.table_is_first_row;
        let cell_count = self.table_row_cell_rects.len();
        let saved_bg = self.draw_block.table_header_bg_color;
        let transparent = Vec4f::default();
        self.draw_block.block_type = FlowBlockType::TableCell;

        for i in 0..cell_count {
            let cell_rect = self.table_row_cell_rects[i];

            // Always transparent here: a header's ground was already laid
            // down underneath its text by `begin_table_header_row`, and a
            // body row never had one. This pass is for the rules alone.
            self.draw_block.table_header_bg_color = transparent;
            self.draw_block.draw_abs(
                cx,
                Rect {
                    pos: cell_rect.pos,
                    size: dvec2(cell_rect.size.x, row_height),
                },
            );

            if is_first_row {
                self.draw_block.table_header_bg_color = transparent;
                self.draw_block.draw_abs(
                    cx,
                    Rect {
                        pos: cell_rect.pos,
                        size: dvec2(cell_rect.size.x, 1.0),
                    },
                );
            }

            if i == 0 {
                self.draw_block.table_header_bg_color = transparent;
                self.draw_block.draw_abs(
                    cx,
                    Rect {
                        pos: cell_rect.pos,
                        size: dvec2(1.0, row_height),
                    },
                );
            }
        }
        self.draw_block.table_header_bg_color = saved_bg;
        self.table_is_first_row = false;
    }

    /// Begin a table cell with horizontal alignment of its contents.
    ///
    /// `align_x` follows `Layout::align.x` semantics: 0.0 = left, 0.5 = center,
    /// 1.0 = right. For wrapped multi-row content, the whole content block is
    /// shifted by the same amount (not aligned per-row).
    pub fn begin_table_cell(&mut self, cx: &mut Cx2d, align_x: f64) {
        // Every cell of a row we draw draws its box, so the row keeps its shape after a cut.
        let is_in_drawn_row = self.skipped_block_depth == 0 && matches!(self.open_table_rows.last(), Some(Some(_)));
        if !is_in_drawn_row && self.skip_block_if_cut() {
            return;
        }
        let cell_width = if self.table_num_columns > 0 {
            cx.turtle().inner_width() / self.table_num_columns as f64
        } else {
            100.0
        };
        let walk = Walk {
            width: Size::Fixed(cell_width),
            height: Size::Fit {
                min: None,
                max: None,
            },
            ..Walk::default()
        };
        let mut layout = self.table_cell_layout;
        layout.align.x = align_x;
        cx.begin_turtle(walk, layout);
        if let Some(Some(row)) = self.open_table_rows.last() {
            self.lines_drawn = row.start_lines_drawn;
            self.last_row_y = f64::NAN;
        }
        self.mark_as_line_start();
        self.cell_text_align_x = align_x;
        self.is_at_block_start = true;
    }

    pub fn end_table_cell(&mut self, cx: &mut Cx2d) {
        if self.skip_end_of_skipped_block() {
            return;
        }
        let cell_rect = cx.end_turtle();
        self.table_row_cell_rects.push(cell_rect);
        if let Some(Some(row)) = self.open_table_rows.last_mut() {
            row.max_lines_drawn = row.max_lines_drawn.max(self.lines_drawn);
        }
        self.cell_text_align_x = 0.0;
        // Drop any trailing block margin inside the cell; the cell itself counts as content.
        self.pending_block_margin = 0.0;
        self.is_at_block_start = false;
    }

    pub fn draw_item_counted(&mut self, cx: &mut Cx2d, template: LiveId) -> LiveId {
        let entry_id = self.new_counted_id();
        let start_pos = if self.tracks_text() {
            Some(cx.turtle().pos())
        } else {
            None
        };

        self.item_with(cx, entry_id, template, |cx, item, tf| {
            item.draw_all(cx, &mut Scope::with_data(tf));
        });

        // Track gap for selection when selectable
        if let Some(start) = start_pos {
            let end_pos = cx.turtle().pos();
            let row_height = cx.turtle().row_height().max(10.0); // Ensure minimum height
            let rect = Rect {
                pos: start,
                size: dvec2((end_pos.x - start.x).max(1.0), row_height),
            };
            self.selection_tracker.push_gap(rect);
        }

        entry_id
    }

    pub fn new_counted_id(&mut self) -> LiveId {
        self.item_counter += 1;
        LiveId(self.item_counter)
    }

    pub fn draw_item(&mut self, cx: &mut Cx2d, entry_id: LiveId, template: LiveId) {
        self.item_with(cx, entry_id, template, |cx, item, tf| {
            item.draw_all(cx, &mut Scope::with_data(tf));
        });
    }

    pub fn draw_item_counted_ref(&mut self, cx: &mut Cx2d, template: LiveId) -> WidgetRef {
        let entry_id = self.new_counted_id();
        let start_pos = if self.tracks_text() {
            Some(cx.turtle().pos())
        } else {
            None
        };

        let result = self.item_with(cx, entry_id, template, |cx, item, tf| {
            item.draw_all(cx, &mut Scope::with_data(tf));
            item.clone()
        });

        // Track gap for selection when selectable
        if let Some(start) = start_pos {
            let end_pos = cx.turtle().pos();
            let row_height = cx.turtle().row_height().max(10.0);
            let rect = Rect {
                pos: start,
                size: dvec2((end_pos.x - start.x).max(1.0), row_height),
            };
            self.selection_tracker.push_gap(rect);
        }

        result
    }

    pub fn draw_item_ref(
        &mut self,
        cx: &mut Cx2d,
        entry_id: LiveId,
        template: LiveId,
    ) -> WidgetRef {
        self.item_with(cx, entry_id, template, |cx, item, tf| {
            item.draw_all(cx, &mut Scope::with_data(tf));
            item.clone()
        })
    }

    pub fn item_with<F, R: Default>(
        &mut self,
        cx: &mut Cx2d,
        entry_id: LiveId,
        template: LiveId,
        f: F,
    ) -> R
    where
        F: FnOnce(&mut Cx2d, &WidgetRef, &mut TextFlow) -> R,
    {
        let mut items = self.items.take().unwrap();
        let r = if let Some(template_ref) = self.templates.get(&template) {
            let template_value: ScriptValue = template_ref.as_object().into();
            let entry = items.get_or_insert(cx, entry_id, |cx| {
                let widget = cx.with_vm(|vm| WidgetRef::script_from_value(vm, template_value));
                (widget, template)
            });
            // If the template changed (e.g. streaming markdown switched code block type),
            // recreate the widget from the new template.
            if entry.1 != template {
                let widget = cx.with_vm(|vm| WidgetRef::script_from_value(vm, template_value));
                *entry = (widget, template);
            }
            cx.widget_tree_mark_dirty(self.uid);
            f(cx, &entry.0, self)
        } else {
            R::default()
        };
        self.items = Some(items);
        r
    }

    pub fn item(&mut self, cx: &mut Cx, entry_id: LiveId, template: LiveId) -> WidgetRef {
        if let Some(template_ref) = self.templates.get(&template) {
            let template_value: ScriptValue = template_ref.as_object().into();
            let entry = self
                .items
                .as_mut()
                .unwrap()
                .get_or_insert(cx, entry_id, |cx| {
                    let widget = cx.with_vm(|vm| WidgetRef::script_from_value(vm, template_value));
                    (widget, template)
                });
            if entry.1 != template {
                let widget = cx.with_vm(|vm| WidgetRef::script_from_value(vm, template_value));
                *entry = (widget, template);
            }
            cx.widget_tree_mark_dirty(self.uid);
            return entry.0.clone();
        }
        WidgetRef::empty()
    }

    pub fn item_counted(&mut self, cx: &mut Cx, template: LiveId) -> WidgetRef {
        let entry_id = self.new_counted_id();
        if let Some(template_ref) = self.templates.get(&template) {
            let template_value: ScriptValue = template_ref.as_object().into();
            let entry = self
                .items
                .as_mut()
                .unwrap()
                .get_or_insert(cx, entry_id, |cx| {
                    let widget = cx.with_vm(|vm| WidgetRef::script_from_value(vm, template_value));
                    (widget, template)
                });
            if entry.1 != template {
                let widget = cx.with_vm(|vm| WidgetRef::script_from_value(vm, template_value));
                *entry = (widget, template);
            }
            cx.widget_tree_mark_dirty(self.uid);
            return entry.0.clone();
        }
        WidgetRef::empty()
    }

    pub fn existing_item(&mut self, entry_id: LiveId) -> WidgetRef {
        if let Some(item) = self.items.as_mut().unwrap().get(&entry_id) {
            item.0.clone()
        } else {
            WidgetRef::empty()
        }
    }

    pub fn clear_items(&mut self) {
        self.items.as_mut().unwrap().clear();
    }

    /// Returns whether we can draw items from the template with this id.
    pub(crate) fn has_template(&self, template: LiveId) -> bool {
        self.templates.contains_key(&template)
    }

    pub fn item_with_scope(
        &mut self,
        cx: &mut Cx,
        scope: &mut Scope,
        entry_id: LiveId,
        template: LiveId,
    ) -> Option<WidgetRef> {
        if let Some(template_ref) = self.templates.get(&template) {
            let template_value: ScriptValue = template_ref.as_object().into();
            let entry = self
                .items
                .as_mut()
                .unwrap()
                .get_or_insert(cx, entry_id, |cx| {
                    let widget = cx.with_vm(|vm| {
                        WidgetRef::script_from_value_scoped(vm, scope, template_value)
                    });
                    (widget, template)
                });
            // If the template changed, recreate the widget from the new
            // template, as item_with does.
            if entry.1 != template {
                let widget = cx.with_vm(|vm| {
                    WidgetRef::script_from_value_scoped(vm, scope, template_value)
                });
                *entry = (widget, template);
            }
            cx.widget_tree_mark_dirty(self.uid);
            return Some(entry.0.clone());
        }
        None
    }

    pub fn draw_text(&mut self, cx: &mut Cx2d, text: &str) {
        if let Some(DrawState::Drawing) = self.draw_state.get() {
            // If we've already exceeded max_lines, skip all further text.
            if self.content_truncated {
                return;
            }

            let is_at_line_start = self.first_thing_on_a_line || (self.collapse_block_margins && self.is_at_block_start);
            let text = trim_trailing_line_breaks(if !is_at_line_start {
                text
            } else if self.collapse_block_margins {
                // Only html whitespace collapses, so `&nbsp;` and full-width spaces stay as content.
                text.trim_start_matches(crate::makepad_html::is_html_whitespace)
            } else {
                text.trim_start()
            });
            // Whitespace that starts a line or follows a list item's marker isn't content,
            // and neither is a run that was empty or only a line break.
            if text.is_empty() {
                // An empty run past the line's start still makes a line of widgets as tall as text.
                if !is_at_line_start {
                    self.make_line_as_tall_as_text(cx);
                }
                return;
            }
            // A run that would start a new line past the budget is dropped before its margin applies.
            if self.collapse_block_margins && self.max_lines > 0 && self.lines_drawn >= self.max_lines && self.is_at_new_line_start(cx) {
                self.content_truncated = true;
                return;
            }
            if let Some(pending) = self.pending_text_after_run.take() {
                pending.resolve(cx, self);
            }
            self.flush_block_margin(cx);

            // Select the appropriate text style based on bold/italic/fixed state
            let style = self.get_run_style();
            let style_slot = style.get_text_style_slot();
            let text_style = self.get_text_style(style_slot).clone();

            let run_size = style.font_size;
            let y_shift_scale = self.y_shift_scales.last().copied().unwrap_or(0.0);
            let line_size = self.line_font_size();

            // Baseline-align mixed runs: a fixed/code run scaled to 0.85x of
            // the surrounding text must sit ON the line's baseline — without
            // this shift it hangs from the row top and inline code floats
            // above the prose. Shift = (line ascender − run ascender), in
            // multiples of the run's own font size (temp_y_shift units).
            // Probe BEFORE the run style is applied (the probe clobbers it).
            let baseline_shift = if style_slot == TextStyleSlot::Fixed && run_size > 0.0 {
                let (line_asc, _) = self.style_metrics_em(cx, TextStyleSlot::Normal);
                let (run_asc, _) = self.style_metrics_em(cx, TextStyleSlot::Fixed);
                (line_asc * line_size - run_asc * run_size) / run_size
            } else {
                0.0
            };

            // RowAlign::Center centers each walk by its own height, which would undo
            // baseline_shift for shorter runs; hand every run the line's height (and its
            // baseline, for RowAlign::Baseline) so they all get the same shift.
            let (line_height_in_lpxs, line_baseline_in_lpxs) = self.get_line_height_and_baseline_in_lpxs(cx);
            self.draw_text.align_row_height = Some(line_height_in_lpxs);
            self.draw_text.align_row_baseline = Some(line_baseline_in_lpxs);
            // Our runs' rows wrap at the flow's pitch, not the font's line gap. Read once here
            // so the selection capture, the wrap probes and the draw share one layout.
            self.draw_text.flow_wrap_spacing_in_lpxs = Some(
                (cx.turtle().wrap_spacing() / self.draw_text.font_scale.max(0.0001) as f64) as f32,
            );

            // Apply the text style to the single draw_text instance
            let top_drop = text_style.top_drop;
            self.draw_text.text_style = text_style;
            let font_size = self.font_sizes.last().unwrap_or(&self.font_size);
            let font_color = *self.font_colors.last().unwrap_or(&self.font_color);
            self.draw_text.text_style.font_size = *font_size as _;
            self.draw_text.color = font_color;
            self.draw_text.temp_y_shift = top_drop + y_shift_scale + baseline_shift;
            self.draw_text.layout_align = Align {
                x: self.cell_text_align_x,
                y: 0.0,
            };

            // Widget-level max_lines: compute how many layouter rows this run is allowed. A "continuation"
            // run starts mid-line or on a row something already charged, so its first row shares that line.
            let mut is_continuation = self.max_lines > 0 && !self.is_at_new_line_start(cx);

            // A continuation run joins a visual line that something else
            // already opened. When the opener was not a text run — an inline
            // widget such as a link pill or an image, a list bullet, a margin
            // walk — nothing has charged that line to the budget yet, so
            // charge it here. Rows are identified by the turtle's y position:
            // a continuation landing on the row the previous run ended on is
            // already paid for, any other row is not.
            if is_continuation && self.is_on_new_row(cx) {
                self.lines_drawn += 1;
            }

            // Visual lines the widget may still start. Meaningless when
            // `max_lines` is 0 (unlimited), so every use is guarded by it.
            let remaining_new_lines = self.max_lines.saturating_sub(self.lines_drawn);

            if self.max_lines > 0 {
                if remaining_new_lines == 0 && !is_continuation {
                    // No visual lines left and this run would start a new one.
                    self.content_truncated = true;
                    return;
                }
                // Continuation runs get +1 because their first row doesn't
                // consume a new visual line (it shares the current one).
                let run_max_rows = remaining_new_lines + if is_continuation { 1 } else { 0 };
                self.draw_text.max_lines = run_max_rows;
                self.draw_text.text_overflow = self.text_overflow;
                // The content we hold on the last line keeps our wrapping off, but a run cuts itself at the edge.
                if self.is_wrap_off_for_inline_content {
                    cx.turtle_mut().set_flow_wrap(true);
                }
            } else {
                self.draw_text.max_lines = 0;
                self.draw_text.text_overflow = TextOverflow::Clip;
            };

            // A code box's right margin sits between its text and the text glued to it.
            self.draw_text.trailing_glue_width_in_lpxs = self.add_code_margin_end(self.trailing_glue_width_in_lpxs);
            let dt = &mut self.draw_text;
            dt.line_break_before = self.line_break_before;
            dt.line_break_after = self.line_break_after;
            // Room kept for text outside a code box has none for the box's margin and padding.
            dt.is_start_reserved = self.is_start_reserved
                && (self.inline_code.value() == 0 || self.is_start_reserved_for_inline_code);

            let areas_tracker = &mut self.areas_tracker;
            // The selection gets the draw's own layout, so this is where that layout starts.
            let mut layout_origin = dvec2(cx.turtle().inner_rect().pos.x, cx.turtle().pos().y);
            // While we hold inline content, where the run starts and the edge it's laid out against.
            let mut held_run_bounds = self.is_holding_inline_content.then(|| (cx.turtle().pos(), Self::get_row_end(cx)));
            let laidout_text = if self.inline_code.value() > 0 {
                let db = &mut self.draw_block;
                db.block_type = FlowBlockType::InlineCode;
                let row_y = cx.turtle().pos().y;
                let wrap_enabled =
                    matches!(cx.turtle().layout().flow, Flow::Right { wrap: true, .. });
                // A forced wrap starts a new visual line, so it is only
                // permissible while the widget-level `max_lines` budget still
                // has a line to give. Without this guard the wrap smuggles in
                // an extra line that the budget never accounted for.
                let may_force_wrap = self.max_lines == 0 || remaining_new_lines > 0;
                // The box's left margin is invisible, so at the end of a row it hangs
                // like white space instead of wrapping, and only the box itself moves on.
                cx.turtle_mut().set_flow_wrap(false);
                if !self.first_thing_on_a_line {
                    let rect = TextFlow::walk_margin(cx, self.inline_code_margin.left);
                    areas_tracker.track_rect(cx, rect);
                }

                // Reserve space on both edges for the inline code box's
                // padding by temporarily inflating the turtle's left and
                // right padding. Without these, wrapped continuation rows
                // start flush at the parent's left edge and the layouter
                // wraps text right at the line's rightmost pixel, so the
                // box's padding on the wrap-side spills past the parent's
                // draw bounds and the inner rounded corners get clipped.
                let pad_l = self.inline_code_padding.left;
                let pad_r = self.inline_code_padding.right;
                let old_padding_left = cx.turtle().padding().left;
                let old_padding_right = cx.turtle().padding().right;
                cx.turtle_mut().set_padding_left(old_padding_left + pad_l);
                cx.turtle_mut().set_padding_right(old_padding_right + pad_r);

                // If, even with the reduced wrap width, the `<code>` text
                // still wouldn't admit any glyphs at the current cursor
                // position (the layouter would emit an empty continuation
                // row before the actual content), wrap to a new line first.
                // Otherwise that empty row renders as a stranded mini-box
                // at the end of the previous line.
                //
                // Mid-text splits (`tofu` on one row, `apply` on the next)
                // are still allowed — this only kicks in when row 0 would
                // be glyph-empty.
                let turtle_pos = cx.turtle().pos();
                let turtle_rect = cx.turtle().inner_rect();
                let max_width = if !turtle_rect.size.x.is_nan() {
                    Some(turtle_rect.size.x as f32)
                } else {
                    None
                };
                let mut needs_wrap = false;
                if wrap_enabled && !self.first_thing_on_a_line && may_force_wrap {
                    if let Some(max_width) = max_width {
                        // The eventual layout will start at cursor + pad_l
                        // (because we walk pad_l below). Predict against
                        // that effective indent.
                        let first_row_indent =
                            (turtle_pos.x - turtle_rect.pos.x) as f32 + pad_l as f32;
                        // Must match the value the draw pass hands to the
                        // layouter so the probe shares its layout-cache entry
                        // and predicts the same row structure.
                        let row_offset = dt.resumable_first_row_min_spacing(cx);
                        let layout_align = dt.layout_align;
                        // These two layouts only measure how the run would
                        // break; they must see the run's natural row count, so
                        // the `max_lines` clamp is lifted for the probe. With
                        // the clamp applied, a run limited to one row always
                        // reports "fits on one line" and never wraps.
                        let clamped_max_lines = dt.max_lines;
                        let clamped_overflow = dt.text_overflow;
                        dt.max_lines = 0;
                        dt.text_overflow = TextOverflow::Clip;
                        let measured = dt.layout(
                            cx,
                            first_row_indent,
                            row_offset,
                            Some(max_width),
                            true,
                            layout_align,
                            text,
                        );
                        // A first row with only white space strands an empty box too.
                        let is_blank = |row: &LaidoutRow| row.text.chars().all(is_hangable_white_space);
                        let row0_empty = measured.rows.first().is_some_and(is_blank);
                        let has_more_rows = measured.rows.len() > 1;
                        // Also measure on a fresh line (indent=0) to see if wrapping first gives
                        // row 0 some text, or, with a break opportunity before it, fits it on one row.
                        let fresh = if has_more_rows
                            && (row0_empty
                                || find_first_line_break(text, dt.line_break_before, dt.line_break_after)
                                    .is_some_and(|line_break| line_break.byte_index == 0))
                        {
                            let orig_line_break_before = std::mem::replace(
                                &mut dt.line_break_before,
                                PrecedingContext::PARAGRAPH_START,
                            );
                            let was_start_reserved = std::mem::replace(&mut dt.is_start_reserved, false);
                            let fresh = dt.layout(
                                cx,
                                0.0,
                                row_offset,
                                Some(max_width),
                                true,
                                layout_align,
                                text,
                            );
                            dt.line_break_before = orig_line_break_before;
                            dt.is_start_reserved = was_start_reserved;
                            Some(fresh)
                        } else {
                            None
                        };
                        let fits_on_fresh_line = !row0_empty && fresh.as_ref().is_some_and(|fresh| fresh.rows.len() == 1);
                        let has_text_on_fresh_line = fresh.is_some_and(|fresh| !is_blank(&fresh.rows[0]));
                        dt.max_lines = clamped_max_lines;
                        dt.text_overflow = clamped_overflow;
                        // Wrap to a new line if EITHER:
                        //   (a) row 0 would be empty (continuation didn't
                        //       admit any glyphs) — leaving a stranded
                        //       mini-box on the previous row, OR
                        //   (b) the text would split across multiple rows
                        //       here but would fit on a single fresh line.
                        //       Splitting here forces draw_walk_resumable_with
                        //       into its per-row glyph batching path, which
                        //       puts row 1+ glyphs into a different draw_item
                        //       than row 0 (because the box draws between
                        //       them) — and that race-conditions in
                        //       parent-heavy contexts (PortalList recycling,
                        //       many concurrent draws) so wrapped row glyphs
                        //       can fail to paint at all.
                        needs_wrap = (row0_empty && has_text_on_fresh_line) || fits_on_fresh_line;
                    }
                }

                // Without our extra left padding, a wrap here starts the box flush with the
                // parent's edge. The padding walk only wraps while `max_lines` has a line to give.
                cx.turtle_mut().set_padding_left(old_padding_left);
                cx.turtle_mut().set_flow_wrap(wrap_enabled && may_force_wrap);
                if needs_wrap {
                    // Match the spacing the layouter would have
                    // used had it wrapped naturally — otherwise
                    // the forced new line sits tight against the
                    // previous row while naturally-wrapped lines
                    // get the configured `wrap_spacing` gap.
                    let ws = cx.turtle().wrap_spacing();
                    cx.turtle_new_line_with_spacing(ws);
                }

                // Walk the box's left padding so the actual text glyphs
                // start `pad_l` past the cursor — this leaves room inside
                // the box for the visible left padding (text never sits
                // flush with the box's rounded left edge).
                let pad_l_rect = TextFlow::walk_margin(cx, pad_l);
                areas_tracker.track_rect(cx, pad_l_rect);
                cx.turtle_mut().set_padding_left(old_padding_left + pad_l);
                cx.turtle_mut().set_flow_wrap(wrap_enabled);

                // After a wrap before the text, ours or a walk's, the run starts a line.
                if cx.turtle().pos().y != row_y {
                    dt.line_break_before = PrecedingContext::PARAGRAPH_START;
                    dt.is_start_reserved = false;
                    // Both the budget handed to the layouter and the post-draw line
                    // accounting come from `is_continuation`, so it has to follow the wrap.
                    is_continuation = false;
                    if self.max_lines > 0 {
                        dt.max_lines = remaining_new_lines;
                    }
                }
                layout_origin = dvec2(cx.turtle().inner_rect().pos.x, cx.turtle().pos().y);

                let code_pad_h = (self.inline_code_padding.top
                    + self.inline_code_padding.bottom
                    + self.inline_code_margin.top
                    + self.inline_code_margin.bottom) as f64;
                if let Some((_, row_end)) = &mut held_run_bounds {
                    *row_end = Self::get_row_end(cx);
                }
                let laidout_text = self.pending_following_room.and_then(|pending| self.lay_out_run(cx, text, pending));
                let (dt, db, areas_tracker) = (&mut self.draw_text, &mut self.draw_block, &mut self.areas_tracker);
                let result = dt.draw_laidout_resumable_with_background(cx, text, laidout_text, |cx, mut rect, _| {
                    rect.pos -= self.inline_code_padding.left_top();
                    rect.size += self.inline_code_padding.size();
                    db.draw_abs(cx, rect);
                    areas_tracker.track_rect(cx, rect);
                });

                // Restore the turtle's padding before walking the right
                // side; we want the trailing pad/margin walks to advance
                // the cursor normally without being constrained by the
                // inflated padding we used for layout.
                cx.turtle_mut().set_padding_left(old_padding_left);
                cx.turtle_mut().set_padding_right(old_padding_right);
                // The box's right side stays on its text's row, so at the row's end
                // it hangs like trailing white space instead of wrapping.
                cx.turtle_mut().set_flow_wrap(false);

                // Walk the box's right padding so the cursor advances past
                // the visible right edge of the box.
                let pad_r_rect = TextFlow::walk_margin(cx, pad_r);
                areas_tracker.track_rect(cx, pad_r_rect);

                // The inline_code padding/margin extends the visual rect
                // beyond what draw_walk_resumable_with allocated in the
                // turtle. Grow used_height so the next row starts below the
                // padded area instead of overlapping it.
                cx.turtle_mut().allocate_height(code_pad_h);
                let rect = TextFlow::walk_margin(cx, self.inline_code_margin.right);
                areas_tracker.track_rect(cx, rect);
                cx.turtle_mut().set_flow_wrap(wrap_enabled);
                result
            } else {
                let laidout_text = self.pending_following_room.and_then(|pending| self.lay_out_run(cx, text, pending));
                let (dt, db, areas_tracker) = (&mut self.draw_text, &mut self.draw_block, &mut self.areas_tracker);
                if self.strikethrough.value() > 0 {
                    db.line_color = font_color;
                    db.block_type = FlowBlockType::Strikethrough;
                    dt.draw_laidout_resumable_with(cx, text, laidout_text, |cx, rect, _| {
                        db.draw_abs(cx, rect);
                        areas_tracker.track_rect(cx, rect);
                    })
                } else if self.underline.value() > 0 {
                    db.line_color = font_color;
                    db.block_type = FlowBlockType::Underline;
                    dt.draw_laidout_resumable_with(cx, text, laidout_text, |cx, rect, _| {
                        db.draw_abs(cx, rect);
                        areas_tracker.track_rect(cx, rect);
                    })
                } else {
                    dt.draw_laidout_resumable_with(cx, text, laidout_text, |cx, rect, _| {
                        areas_tracker.track_rect(cx, rect);
                    })
                }
            };
            if let Some((run_start, row_end)) = held_run_bounds.filter(|_| self.overrun_start_x.is_none()) {
                let last_row = laidout_text.rows.last().unwrap();
                let end_in_lpxs = last_row.origin_in_lpxs.x + last_row.width_without_hanging_in_lpxs();
                let run_end = layout_origin.x + (end_in_lpxs * self.draw_text.font_scale) as f64;
                // Clip cuts a run without an ellipsis, so a box with nothing that fits in it would show alone.
                let is_cut_to_nothing = laidout_text.is_truncated
                    && last_row.glyphs.iter().all(|glyph| last_row.text[glyph.cluster..].starts_with(char::is_whitespace));
                if run_end > row_end + 0.5 || is_cut_to_nothing {
                    self.overrun_start_x = Some(run_start.x - self.get_white_space_width_before(run_start));
                }
            }

            if self.tracks_text() {
                self.selection_tracker
                    .push_text(laidout_text.clone(), layout_origin, self.draw_text.font_scale, text);
            }
            self.line_break_before = laidout_text.line_break_before_next_run;
            self.is_start_reserved = laidout_text.is_trailing_glue_reserved;
            self.is_start_reserved_for_inline_code = self.is_trailing_glue_in_inline_code;
            self.was_text_drawn_in_inline_content = true;
            // Probes between runs, like `line_probe`, keep laying out with the defaults.
            let dt = &mut self.draw_text;
            dt.line_break_before = PrecedingContext::PARAGRAPH_START;
            dt.line_break_after = FollowingContext::END_OF_TEXT;
            dt.trailing_glue_width_in_lpxs = 0.0;
            dt.is_start_reserved = false;

            // Update widget-level line tracking.
            if self.max_lines > 0 {
                if self.is_wrap_off_for_inline_content {
                    cx.turtle_mut().set_flow_wrap(false);
                }
                let new_lines = laidout_text.rows.len().saturating_sub(if is_continuation { 1 } else { 0 });
                self.lines_drawn += new_lines;
                // If this run was truncated (ellipsis was appended), stop here.
                if laidout_text.is_truncated {
                    self.content_truncated = true;
                }
                // What follows a run in a code box comes after the box's padding, not after the run's white space.
                self.last_run_end = (self.inline_code.value() == 0).then(|| (cx.turtle().pos(), laidout_text.clone()));
            }

            // Remember the row this run ended on, so the next run can tell
            // whether it is joining this same visual line or one that inline
            // content opened in between.
            self.last_row_y = cx.turtle().pos().y;
        }
        self.first_thing_on_a_line = false;
    }

    /// Lays out a run of `text` where the turtle is, since what follows it at `pending` may need room on its last row.
    fn lay_out_run(&mut self, cx: &mut Cx2d, text: &str, mut pending: PendingFollowingRoom) -> Option<Rc<LaidoutText>> {
        let finder = self.following_room_finder.as_ref()?;
        finder.begin_run(&mut pending, text);
        self.pending_following_room = Some(pending);
        let max_lines = self.draw_text.max_lines;
        let mut laidout_text = self.draw_text.layout_resumable(cx, text);
        // Only a run that ends on the last line `max_lines` allows can leave too little room after it.
        if laidout_text.rows.len() != max_lines || laidout_text.is_truncated {
            return Some(laidout_text);
        }
        let cached_room = finder.get_cached_room(cx, self, &pending);
        let following_room_in_lpxs = cached_room.unwrap_or_else(|| finder.find_room(cx, self, &pending, text));
        let following_room_in_lpxs = self.add_code_margin_end(following_room_in_lpxs);
        let max_width_in_lpxs = DrawText::get_max_width_in_lpxs(cx.turtle().inner_rect().size.x);
        let is_cut = is_short_of_following_room(&laidout_text.rows, max_lines, max_width_in_lpxs, following_room_in_lpxs);
        let is_font_changed =
            cached_room.is_none() && cx.fonts.borrow().font_family_generation() != pending.font_family_generation;
        // The room only changes the layout when it cuts the run, or after a font came that changes how the run looks.
        if following_room_in_lpxs > 0.0 && (is_cut || is_font_changed) {
            self.draw_text.following_room_in_lpxs = following_room_in_lpxs;
            laidout_text = self.draw_text.layout_resumable(cx, text);
            self.draw_text.following_room_in_lpxs = 0.0;
        }
        Some(laidout_text)
    }

    /// Adds the right margin of the code box we're in to `width_in_lpxs`, the room a run needs after it.
    fn add_code_margin_end(&self, width_in_lpxs: f32) -> f32 {
        if self.inline_code.value() > 0 && width_in_lpxs > 0.0 {
            width_in_lpxs + (self.inline_code_margin.right / self.draw_text.font_scale.max(0.0001) as f64) as f32
        } else {
            width_in_lpxs
        }
    }

    pub fn walk_margin(cx: &mut Cx2d, margin: f64) -> Rect {
        cx.walk_turtle(Walk {
            width: Size::Fixed(margin),
            height: Size::Fixed(0.0),
            ..Default::default()
        })
    }

    pub fn draw_link(
        &mut self,
        cx: &mut Cx2d,
        template: LiveId,
        data: impl ActionTrait + PartialEq,
        label: &str,
    ) {
        let entry_id = self.new_counted_id();
        self.item_with(cx, entry_id, template, |cx, item, tf| {
            item.set_text(cx, label);
            item.set_action_data(data);
            item.draw_all(cx, &mut Scope::with_data(tf));
        })
    }
}

/// What [`TextFlow::end_inline_content`] needs to hide inline content held on or near the last allowed line.
pub struct InlineContentHold {
    /// The align list index of the tracked clip around the content's draw.
    clip_index: usize,
    /// The turtle x where the content's walk begins, so we can hide the content from there.
    start_x: f64,
}

/// How many lines were drawn before a table row, and up to the end of its tallest cell so far.
#[derive(Clone, Copy)]
struct TableRowLines {
    start_lines_drawn: usize,
    max_lines_drawn: usize,
}

/// Actions emitted by TextFlow for cross-boundary selection in PortalList
#[derive(Debug, Clone, Default)]
pub enum TextFlowAction {
    #[default]
    None,
}

#[derive(Debug, Clone, Default)]
pub enum TextFlowLinkAction {
    Clicked {
        key_modifiers: KeyModifiers,
    },
    #[default]
    None,
}

#[derive(Script, ScriptHook, WidgetRef, WidgetSet, WidgetRegister, Animator)]
pub struct TextFlowLink {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[apply_default]
    animator: Animator,

    #[rust]
    area: Area,

    #[live(true)]
    click_on_down: bool,
    #[rust]
    pub drawn_areas: SmallVec<[Area; 2]>,
    #[live(true)]
    grab_key_focus: bool,
    #[live]
    margin: Inset,
    #[live]
    hovered: f32,
    #[live]
    down: f32,

    /// The default font color for the link when not hovered on or down.
    #[live]
    color: Option<Vec4f>,
    /// The font color used when the link is hovered on.
    #[live]
    color_hover: Option<Vec4f>,
    /// The font color used when the link is down.
    #[live]
    color_down: Option<Vec4f>,

    #[live]
    pub text: ArcStringMut,

    #[rust]
    action_data: WidgetActionData,
}

impl WidgetNode for TextFlowLink {
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }
    fn walk(&mut self, _cx: &mut Cx) -> Walk {
        Walk::default()
    }

    fn area(&self) -> Area {
        self.area.area()
    }

    fn redraw(&mut self, cx: &mut Cx) {
        self.area.redraw(cx);
    }

    fn set_action_data(&mut self, data: std::sync::Arc<dyn ActionTrait>) {
        self.action_data.set_box(data)
    }

    fn action_data(&self) -> Option<std::sync::Arc<dyn ActionTrait>> {
        self.action_data.clone_data()
    }

    fn point_hits_area(&self, cx: &Cx, point: DVec2) -> bool {
        // Clipped rects throughout, matching `Event::hits`: a link scrolled out
        // of its container is drawn off-viewport and must not answer point
        // queries there (see `Widget::point_hits_area`).
        let area = self.area();
        if area.is_valid(cx) && area.clipped_rect(cx).contains(point) {
            return true;
        }
        // Links span multiple text rects via drawn_areas
        for area in self.drawn_areas.iter() {
            if area.is_valid(cx) && area.clipped_rect(cx).contains(point) {
                return true;
            }
        }
        false
    }
}

impl Widget for TextFlowLink {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if self.animator_handle_event(cx, event).must_redraw() {
            if let Some(tf) = scope.data.get_mut::<TextFlow>() {
                tf.redraw(cx);
            } else {
                self.drawn_areas.iter().for_each(|area| area.redraw(cx));
            }
        }

        for i in 0..self.drawn_areas.len() {
            let area = self.drawn_areas[i];
            match event.hits(cx, area) {
                Hit::FingerDown(fe) if fe.is_primary_hit() => {
                    if self.grab_key_focus {
                        cx.set_key_focus(self.area());
                    }
                    self.animator_play(cx, ids!(hover.down));
                    if self.click_on_down {
                        cx.widget_action_with_data(
                            &self.action_data,
                            self.widget_uid(),
                            TextFlowLinkAction::Clicked {
                                key_modifiers: fe.modifiers,
                            },
                        );
                    }
                }
                Hit::FingerHoverIn(_) => {
                    cx.set_cursor(MouseCursor::Hand);
                    self.animator_play(cx, ids!(hover.on));
                }
                Hit::FingerHoverOut(_) => {
                    self.animator_play(cx, ids!(hover.off));
                }
                Hit::FingerUp(fe) if fe.is_primary_hit() => {
                    if fe.is_over {
                        if !self.click_on_down {
                            cx.widget_action_with_data(
                                &self.action_data,
                                self.widget_uid(),
                                TextFlowLinkAction::Clicked {
                                    key_modifiers: fe.modifiers,
                                },
                            );
                        }

                        if fe.device.has_hovers() {
                            self.animator_play(cx, ids!(hover.on));
                        } else {
                            self.animator_play(cx, ids!(hover.off));
                        }
                    } else {
                        self.animator_play(cx, ids!(hover.off));
                    }
                }
                _ => (),
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, _walk: Walk) -> DrawStep {
        let Some(tf) = scope.data.get_mut::<TextFlow>() else {
            return DrawStep::done();
        };

        // Here: the text flow has already began drawing, so we just need to draw the text.
        tf.underline.push();
        tf.areas_tracker.push_tracker();
        let mut pushed_color = false;
        if self.hovered > 0.0 {
            if let Some(color) = self.color_hover {
                tf.font_colors.push(color);
                pushed_color = true;
            }
        } else if self.down > 0.0 {
            if let Some(color) = self.color_down {
                tf.font_colors.push(color);
                pushed_color = true;
            }
        } else {
            if let Some(color) = self.color {
                tf.font_colors.push(color);
                pushed_color = true;
            }
        }
        TextFlow::walk_margin(cx, self.margin.left);
        tf.draw_text(cx, self.text.as_ref());
        TextFlow::walk_margin(cx, self.margin.right);

        if pushed_color {
            tf.font_colors.pop();
        }
        tf.underline.pop();

        let (start, end) = tf.areas_tracker.pop_tracker();

        if self.drawn_areas.len() == end - start {
            for i in 0..end - start {
                self.drawn_areas[i] =
                    cx.update_area_refs(self.drawn_areas[i], tf.areas_tracker.areas[i + start]);
            }
        } else {
            self.drawn_areas = SmallVec::from(&tf.areas_tracker.areas[start..end]);
        }

        DrawStep::done()
    }

    fn text(&self) -> String {
        self.text.as_ref().to_string()
    }

    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        if self.text.as_ref() == v { return }
        self.text.as_mut_empty().push_str(v);
        self.redraw(cx);
    }
}

#[cfg(test)]
mod table_structure_tests {
    use super::*;
    use crate::script_eval;

    /// `Markdown`, `Html` and `RichTextEditor` reach TextFlow through a Rust
    /// `#[deref]`, not through the prototype chain, so a field their own DSL
    /// block does not name is reset on every reload -- and a theme switch is
    /// a reload -- to its FIELD TYPE's default rather than to what TextFlow's
    /// block says. The `Layout` default flows Right, so a table's rows were
    /// laid side by side: the header took the whole width and every body row
    /// was left zero wide. Tables drew their box, their header, and none of
    /// their content, on every theme switch, on a third of the documentation.
    ///
    /// Each of the three blocks now restates them. This holds them to it.
    #[test]
    fn a_table_still_runs_downward_after_a_reload() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            crate::script_mod(vm);
            let mut md = {
                let value = script_eval!(vm, {use mod.widgets.* Markdown{}});
                crate::markdown::Markdown::script_from_value(vm, value)
            };
            assert_eq!(md.text_flow.table_layout.flow, Flow::Down, "markdown at startup");
            vm.with_reload(crate::script_mod);
            let value = script_eval!(vm, {use mod.widgets.* Markdown{}});
            md.script_apply(vm, &Apply::Reload, &mut Scope::empty(), value);
            assert_eq!(md.text_flow.table_layout.flow, Flow::Down, "markdown after a reload");
            assert!(matches!(md.text_flow.table_row_layout.flow, Flow::Right { .. }), "row flow after a reload");
            assert_eq!(md.text_flow.table_walk.height, Size::fit(), "table height after a reload");
            assert!(md.text_flow.heading_margin.top > 0.0, "heading margin after a reload");
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        fold_button::FoldButton,
        html::Html,
        makepad_draw::{cx_draw::CxDraw, text::line_break::find_line_breaks},
        makepad_platform::script::res::CxScriptResourceData,
    };

    script_mod! {
        use mod.prelude.widgets_internal.*
        use mod.widgets.*

        mod.widgets.HighlightSpan = #(HighlightSpan::register_widget(vm)) {}
    }

    /// Draws its text through TextFlow in a code box, like a span with a background color.
    ///
    /// With a `reason`, it draws that in italics first, like Robrix draws a hidden spoiler.
    #[derive(Script, ScriptHook, Widget)]
    pub struct HighlightSpan {
        #[uid]
        uid: WidgetUid,
        #[source]
        source: ScriptObjectRef,
        #[redraw]
        #[area]
        area: Area,
        #[walk]
        walk: Walk,
        #[layout]
        layout: Layout,
        #[live]
        text: ArcStringMut,
        #[live]
        reason: ArcStringMut,
    }

    impl Widget for HighlightSpan {
        fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}

        fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, _walk: Walk) -> DrawStep {
            let Some(tf) = scope.data.get_mut::<TextFlow>() else {
                return DrawStep::done();
            };
            if !self.reason.as_ref().is_empty() {
                tf.italic.push();
                tf.draw_text(cx, self.reason.as_ref());
                tf.italic.pop();
            }
            tf.inline_code.push();
            tf.draw_text(cx, self.text.as_ref());
            tf.inline_code.pop();
            DrawStep::done()
        }

        fn text(&self) -> String {
            self.text.as_ref().to_string()
        }

        fn set_text(&mut self, _cx: &mut Cx, v: &str) {
            self.text.as_mut_empty().push_str(v);
        }
    }

    /// Draws an Html widget without a window.
    struct HtmlDrawer {
        cx: Cx,
        html: Html,
        pass: DrawPass,
        draw_list: DrawList2d,
    }

    /// A run of text we drew, with the layout the selection got from the draw.
    struct DrawnRun {
        text: String,
        text_start: usize,
        origin: DVec2,
        font_scale: f32,
        laidout_text: Rc<LaidoutText>,
    }

    impl DrawnRun {
        /// Returns the x that's `x_in_lpxs` into row `index`.
        fn get_x(&self, index: usize, x_in_lpxs: f32) -> f64 {
            let row = &self.laidout_text.rows[index];
            self.origin.x + ((row.origin_in_lpxs.x + x_in_lpxs) * self.font_scale) as f64
        }

        /// Returns where the text of row `index` starts and where its visible part ends.
        fn get_row_x_bounds(&self, index: usize) -> (f64, f64) {
            let row = &self.laidout_text.rows[index];
            let start_in_lpxs = row.glyphs.first().map_or(row.width_in_lpxs, |glyph| glyph.origin_in_lpxs.x);
            (self.get_x(index, start_in_lpxs), self.get_x(index, row.width_without_hanging_in_lpxs()))
        }

        /// Returns where the last glyph of row `index` that isn't white space ends.
        fn get_ink_end_x(&self, index: usize) -> f64 {
            let row = &self.laidout_text.rows[index];
            let end_in_lpxs = row
                .glyphs
                .iter()
                .filter(|glyph| !row.text[glyph.cluster..].starts_with(char::is_whitespace))
                .map(|glyph| glyph.origin_in_lpxs.x + glyph.advance_in_lpxs())
                .fold(0.0, f32::max);
            self.get_x(index, end_in_lpxs)
        }
    }

    impl HtmlDrawer {
        fn new() -> Self {
            Self::with_font_set(FontSet::target_default())
        }

        /// Like `new`, but draws with the fonts of `font_set`.
        fn with_font_set(font_set: FontSet) -> Self {
            let mut cx = Cx::new(Box::new(|_, _| {}));
            cx.set_font_set(font_set);
            cx.with_vm(crate::script_mod);
            cx.with_vm(script_mod);
            let html = cx.with_vm(|vm| {
                let value = vm.eval(script! {
                    use mod.prelude.widgets.*
                    Html{
                        padding: 0
                        selectable: true
                        img := View{width: 24 height: 10}
                        pill := View{width: 40 height: 10}
                        wide := View{width: 400 height: 10}
                        mark := mod.widgets.HighlightSpan{}
                        spoiler := mod.widgets.HighlightSpan{reason: " [Spoiler]  "}
                    }
                });
                Html::script_from_value(vm, value)
            });
            let pass = DrawPass::new(&mut cx);
            let draw_list = DrawList2d::new(&mut cx);
            Self { cx, html, pass, draw_list }
        }

        /// Draws `body` `width` wide, and returns its runs of text.
        fn draw(&mut self, body: &str, width: f64) -> Vec<DrawnRun> {
            self.html.set_text(&mut self.cx, body);
            let event = DrawEvent::default();
            let mut draw = CxDraw::new(&mut self.cx, &event);
            let mut cx = Cx2d::new(&mut draw);
            cx.begin_pass(&self.pass, None);
            self.draw_list.begin_always(&mut cx);
            cx.begin_root_turtle(dvec2(width, 1000.0), Layout::flow_down());
            let walk = Walk { width: Size::Fixed(width), height: Size::fit(), ..Walk::default() };
            let _ = self.html.draw_walk(&mut cx, &mut Scope::empty(), walk);
            cx.end_pass_sized_turtle();
            self.draw_list.end(&mut cx);
            cx.end_pass(&self.pass);
            let tracker = &self.html.text_flow.selection_tracker;
            tracker
                .segments
                .iter()
                .filter_map(|segment| match segment {
                    SelectionSegment::Text { laidout_text, origin, font_scale, text_start } => Some(DrawnRun {
                        text: laidout_text.text.as_str().to_string(),
                        text_start: *text_start,
                        origin: *origin,
                        font_scale: *font_scale,
                        laidout_text: laidout_text.clone(),
                    }),
                    _ => None,
                })
                .collect()
        }

        /// Draws `body` `width` wide, and returns its text and where its rows after the first start.
        fn get_row_starts(&mut self, body: &str, width: f64) -> (String, Vec<usize>) {
            let row_starts = self
                .draw(body, width)
                .iter()
                .flat_map(|run| {
                    run.laidout_text.rows[1..]
                        .iter()
                        .map(move |row| run.text_start + row.text.start_in_parent())
                })
                .collect();
            (self.html.text_flow.selection_tracker.text.clone(), row_starts)
        }

        /// Draws `body` `width` wide, and checks that no run's rows end past `right`.
        ///
        /// Only the runs that `is_checked` picks count.
        fn check_rows_end_before(&mut self, body: &str, width: f64, right: f64, is_checked: impl Fn(&str) -> bool) {
            for run in self.draw(body, width).iter().filter(|run| is_checked(&run.text)) {
                for row in 0..run.laidout_text.rows.len() {
                    let end = run.get_row_x_bounds(row).1;
                    assert!(end <= right + 0.01, "{body:?} at {width}: {:?} ends at {end}", run.text);
                }
            }
        }

        /// Draws `body` on one row, and returns the x where `text` ends in the first run that starts with it.
        fn get_x_after(&mut self, body: &str, text: &str) -> f64 {
            let runs = self.draw(body, 1000.0);
            let run = runs.iter().find(|run| run.text.starts_with(text)).unwrap();
            run.get_x(0, run.laidout_text.rows[0].index_to_x_in_lpxs(text.len()))
        }

        /// Draws `body` `width` wide, and returns each run's text with how many glyphs each of its rows shows.
        ///
        /// That includes an ellipsis, and whether we cut what came after.
        fn get_shown_glyph_counts(&mut self, body: &str, width: f64) -> (Vec<(String, Vec<usize>)>, bool) {
            let runs = self.draw(body, width);
            let glyph_counts = runs
                .iter()
                .map(|run| (run.text.clone(), run.laidout_text.rows.iter().map(|row| row.glyphs.len()).collect()))
                .collect();
            (glyph_counts, self.html.text_flow.is_content_truncated())
        }

        /// Opens or closes every `<details>` we drew, like a click on its summary does.
        fn set_details_open(&mut self, is_open: bool) {
            for (widget, _) in self.html.text_flow.items.as_ref().unwrap().values() {
                if let Some(mut fold_button) = widget.borrow_mut::<FoldButton>() {
                    fold_button.set_is_open(&mut self.cx, is_open, Animate::No);
                }
            }
        }
    }

    #[test]
    fn styled_runs_wrap_like_one_text() {
        let mut drawer = HtmlDrawer::new();
        // The tags only underline or strike through their text, so it keeps its width.
        for (body, plain) in [
            (
                "Hello foo<u>bar</u>. Then <u>(</u>quux) and \"<a href='x'>a link</a>\" here, ok.",
                "Hello foobar. Then (quux) and \"a link\" here, ok.",
            ),
            (
                "See (<a href='x'>https://example.com/some/path</a>) or <del>this</del>, maybe<u>!</u>",
                "See (https://example.com/some/path) or this, maybe!",
            ),
            (
                "<u>un</u>believable <u>Note</u>: x, y and <del>z</del>. foo <u>bar</u> baz",
                "unbelievable Note: x, y and z. foo bar baz",
            ),
            (
                "a hyphen<u>\u{AD}ation exam</u>\u{AD}ple with <del>soft\u{AD}hy</del>phens",
                "a hyphen\u{AD}ation exam\u{AD}ple with soft\u{AD}hyphens",
            ),
            (
                "<p>one <u>two</u>.</p><p>(<u>three</u>) four</p>fi<u>ve</u><br>(<del>six</del>)\
                 <ul><li><u>se</u>ven, <u><del>ei</del>ght</u>!</li></ul>",
                "<p>one two.</p><p>(three) four</p>five<br>(six)<ul><li>seven, eight!</li></ul>",
            ),
        ] {
            let mut compared_width_count = 0;
            for width in (40..400).step_by(5).map(f64::from) {
                let (text, plain_row_starts) = drawer.get_row_starts(plain, width);
                // Kerning across runs shifts things a little, so we skip widths where that matters.
                if drawer.get_row_starts(plain, width - 1.0).1 != plain_row_starts
                    || drawer.get_row_starts(plain, width + 1.0).1 != plain_row_starts
                {
                    continue;
                }
                // A unit wider than a row breaks between graphemes, which runs do one by one.
                let line_breaks: Vec<usize> = find_line_breaks(
                    &text,
                    PrecedingContext::PARAGRAPH_START,
                    FollowingContext::END_OF_TEXT,
                )
                .map(|line_break| line_break.byte_index)
                .collect();
                if !plain_row_starts.iter().all(|row_start| line_breaks.contains(row_start)) {
                    continue;
                }
                assert_eq!(drawer.get_row_starts(body, width), (text, plain_row_starts), "{body:?} at {width}");
                compared_width_count += 1;
            }
            assert!(compared_width_count > 0, "{body:?}");
        }
    }

    #[test]
    fn rows_keep_inline_code_padding_and_soft_hyphens_inside() {
        let mut drawer = HtmlDrawer::new();
        let pad_r = drawer.html.text_flow.inline_code_padding.right;
        // The text before each code box has to keep room for all of it, and the
        // text before the link for the hyphen after `yy`.
        for (body, code) in [
            ("some words foo<code>bar</code> qux", "bar"),
            ("some words (<code>bar baz</code>)", "bar baz"),
            ("a b c d e f<code>gh</code>", "gh"),
            ("aaaa bbbb x<a href='x'>yy<b>\u{AD}</b>zzzzzzzz</a>", ""),
        ] {
            for width in (110..300).map(|width| f64::from(width) / 2.0) {
                for run in drawer.draw(body, width) {
                    let right = if run.text == code { width - pad_r } else { width };
                    for row in 0..run.laidout_text.rows.len() {
                        let end = run.get_row_x_bounds(row).1;
                        assert!(end <= right + 0.01, "{body:?} at {width}: {:?} ends at {end}", run.text);
                    }
                }
            }
        }
    }

    #[test]
    fn code_boxes_that_wrap_start_at_the_edge() {
        let mut drawer = HtmlDrawer::new();
        let pad_l = drawer.html.text_flow.inline_code_padding.left;
        // The first box wraps after its own left margin, the second after its left padding.
        for (body, code) in [("see the <code>foo</code> call", "foo"), ("<b>world;3.5<code> \"</code>!$words</b>", " \"")] {
            let mut moved_count = 0;
            for width in (80..400).map(|width| f64::from(width) / 2.0) {
                let runs = drawer.draw(body, width);
                let index = runs.iter().position(|run| run.text == code).unwrap();
                let (before, run) = (&runs[index - 1], &runs[index]);
                if before.laidout_text.rows.len() == 1 && run.origin.y > before.origin.y {
                    assert!((run.get_row_x_bounds(0).0 - pad_l).abs() < 0.01, "{body:?} at {width}");
                    moved_count += 1;
                }
            }
            assert!(moved_count > 0, "{body:?}");
        }
    }

    #[test]
    fn text_after_a_code_box_goes_on_in_its_row() {
        let mut drawer = HtmlDrawer::new();
        // Even when the box ends right at the edge, so the space after it hangs there.
        for body in ["some words foo<code>bar</code> qux", "see the <code>foo</code> call"] {
            for width in (80..400).map(|width| f64::from(width) / 2.0) {
                let runs = drawer.draw(body, width);
                let index = runs.iter().position(|run| run.text.starts_with(' ')).unwrap();
                let (code, after) = (&runs[index - 1], &runs[index]);
                if code.laidout_text.rows.len() == 1 {
                    assert!((after.origin.y - code.origin.y).abs() < 0.01, "{body:?} at {width}");
                }
            }
        }
    }

    #[test]
    fn selection_gets_the_drawn_layout_of_code() {
        let mut drawer = HtmlDrawer::new();
        let body = "some words foo<code>bar</code> qux and <code>more code here</code>, ok";
        for width in (30..120).map(f64::from) {
            for run in drawer.draw(body, width) {
                // We draw a code run on a fresh row rather than leave its first row empty.
                if run.text == "more code here" {
                    assert!(!run.laidout_text.rows[0].glyphs.is_empty(), "at {width}");
                }
            }
        }
    }

    #[test]
    fn empty_runs_draw_nothing() {
        let mut drawer = HtmlDrawer::new();
        let margin_l = drawer.html.text_flow.inline_code_margin.left;
        let pad_l = drawer.html.text_flow.inline_code_padding.left;
        // The parser puts an empty text before the `<b>`, which is inside the code.
        let runs = drawer.draw("see <code><b>x</b>y</code> z", 300.0);
        assert!(runs.iter().all(|run| !run.text.is_empty()));
        let (see, x) = (&runs[0], &runs[1]);
        let pen_x = see.get_x(0, see.laidout_text.rows[0].width_in_lpxs);
        assert!((x.get_row_x_bounds(0).0 - pen_x - (margin_l + pad_l)).abs() < 0.01);
    }

    #[test]
    fn text_before_an_image_breaks_before_it() {
        let mut drawer = HtmlDrawer::new();
        for width in (40..200).map(f64::from) {
            let rows_alone = drawer.get_row_starts("aaaa bbbb", width).1;
            let runs = drawer.draw("aaaa bbbb<img>cccccccccccc dd", width);
            let rows_before_image: Vec<usize> =
                runs[0].laidout_text.rows[1..].iter().map(|row| row.text.start_in_parent()).collect();
            assert_eq!(rows_before_image, rows_alone, "at {width}");
        }
    }

    #[test]
    fn text_after_a_summary_breaks_like_a_new_paragraph() {
        let mut drawer = HtmlDrawer::new();
        // After a number, `,` would go on with it.
        let runs = drawer.draw("<details open><summary>1</summary>,</details>", 300.0);
        let comma = runs.iter().find(|run| run.text == ",").unwrap();
        let mut line_break_before_next_run = PrecedingContext::PARAGRAPH_START;
        line_break_before_next_run.append(",");
        assert_eq!(comma.laidout_text.line_break_before_next_run, line_break_before_next_run);
        let mut after_number = PrecedingContext::PARAGRAPH_START;
        after_number.append("1,");
        assert_ne!(after_number, line_break_before_next_run);
    }

    #[test]
    fn lookahead_follows_a_new_document() {
        let mut drawer = HtmlDrawer::new();
        let mut fresh_drawer = HtmlDrawer::new();
        // The two texts are just as long and have their nodes in the same places.
        let (old_body, new_body) = ("aaaa<b>bbbbbbbbbbbb</b>c", "aaaa<b>b</b>cccccccccccc");
        for width in (40..150).map(f64::from) {
            drawer.get_row_starts(old_body, width);
            assert_eq!(
                drawer.get_row_starts(new_body, width),
                fresh_drawer.get_row_starts(new_body, width),
                "at {width}",
            );
        }
    }

    /// Text whose `(` has to stay with the `中` after it, which is narrower as a missing glyph.
    const CJK_IN_BRACKETS: &str = "aaaa bbbb (<b>\u{4E2D}\u{6587}</b>) cc";

    #[test]
    fn lookahead_measures_again_once_a_fallback_font_arrives() {
        // With the Latin font set, the CJK fonts only load once some text needs them.
        let mut drawer = HtmlDrawer::with_font_set(FontSet::Latin);
        let mut fresh_drawer = HtmlDrawer::with_font_set(FontSet::Latin);
        let width = fresh_drawer.get_x_after(CJK_IN_BRACKETS, "\u{4E2D}") - 0.5;
        // We load the fonts of these styles first, so the CJK font is the only one that arrives next.
        drawer.draw("aaaa <b>bbbb</b> cc", width);
        // It arrives while we measure what follows `(`, so the first draw keeps too little room for it.
        let row_starts_without_font = drawer.get_row_starts(CJK_IN_BRACKETS, width);
        assert_ne!(drawer.get_row_starts(CJK_IN_BRACKETS, width), row_starts_without_font);
        for width in [width].into_iter().chain((60..300).step_by(5).map(f64::from)) {
            fresh_drawer.draw("x", width);
            assert_eq!(
                drawer.get_row_starts(CJK_IN_BRACKETS, width),
                fresh_drawer.get_row_starts(CJK_IN_BRACKETS, width),
                "at {width}",
            );
        }
    }

    #[test]
    fn lookahead_measures_again_once_a_downloaded_font_arrives() {
        let mut drawer = HtmlDrawer::new();
        let mut fresh_drawer = HtmlDrawer::new();
        // Bold text asks for this CJK font after a glyph miss, and its file isn't there.
        for HtmlDrawer { cx, html, .. } in [&mut drawer, &mut fresh_drawer] {
            script_apply_eval!(cx, html, {
                text_style_bold +: {
                    font_family: mod.text.FontFamily{
                        latin := mod.text.FontMember{res: mod.res.crate_resource("makepad_widgets:resources/IBMPlexSans-SemiBold.ttf")}
                        cjk := mod.text.FontMember{res: mod.res.crate_resource("makepad_widgets:resources/downloading/LXGWWenKaiBold.ttf") lazy: 1}
                    }
                }
            });
        }
        // On the web its data comes over HTTP like this, so it shows up between two frames.
        let font_data = Rc::new(std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../../resources/LXGWWenKaiBold.ttf")).unwrap());
        let set_font_data = |cx: &Cx, data: CxScriptResourceData| {
            for resource in cx.script_data.resources.resources.borrow_mut().iter_mut() {
                if resource.abs_path.ends_with("/downloading/LXGWWenKaiBold.ttf") {
                    resource.data = data.clone();
                }
            }
            cx.script_data.resources.bump_generation();
        };
        set_font_data(&fresh_drawer.cx, CxScriptResourceData::Loaded(font_data.clone()));
        let width = fresh_drawer.get_x_after(CJK_IN_BRACKETS, "\u{4E2D}") - 0.5;
        // The first draw asks for the font, and the ones after it lay out without it until it arrives.
        set_font_data(&drawer.cx, CxScriptResourceData::Loading);
        drawer.draw(CJK_IN_BRACKETS, width);
        let row_starts_without_font = drawer.get_row_starts(CJK_IN_BRACKETS, width);
        set_font_data(&drawer.cx, CxScriptResourceData::Loaded(font_data.clone()));
        fresh_drawer.draw("x", width);
        let row_starts = drawer.get_row_starts(CJK_IN_BRACKETS, width);
        assert_ne!(row_starts, row_starts_without_font);
        assert_eq!(row_starts, fresh_drawer.get_row_starts(CJK_IN_BRACKETS, width));
    }

    #[test]
    fn lookahead_measures_again_when_styles_change() {
        let body = "some words foo<b>barbazquux</b>, then (<code>code()</code>) and \"<a href='x'>a link</a>\" here";
        let mut drawer = HtmlDrawer::new();
        let mut fresh_drawer = HtmlDrawer::new();
        // Code boxes with wide left sides, so the room they keep in lpxs changes a lot with the font scale.
        for tf in [&mut drawer.html.text_flow, &mut fresh_drawer.html.text_flow] {
            tf.inline_code_margin.left = 10.0;
            tf.inline_code_padding.left = 10.0;
        }
        let font_size = drawer.html.text_flow.font_size;
        // These change without an apply, which would clear what the lookahead found.
        let set_style = |drawer: &mut HtmlDrawer, (font_size, font_scale): (f32, f32)| {
            drawer.html.text_flow.font_size = font_size;
            drawer.html.text_flow.draw_text.font_scale = font_scale;
        };
        for width in (80..400).step_by(7).map(f64::from) {
            for style in [(font_size * 1.3, 1.0), (font_size, 1.5)] {
                set_style(&mut drawer, (font_size, 1.0));
                drawer.get_row_starts(body, width);
                set_style(&mut drawer, style);
                set_style(&mut fresh_drawer, style);
                fresh_drawer.draw("x", width);
                assert_eq!(drawer.get_row_starts(body, width), fresh_drawer.get_row_starts(body, width), "{style:?} at {width}");
            }
        }
        set_style(&mut drawer, (font_size, 1.0));
        set_style(&mut fresh_drawer, (font_size, 1.0));
        // An apply can change any style, like the font of bold text.
        drawer.get_row_starts(body, 200.0);
        for HtmlDrawer { cx, html, .. } in [&mut drawer, &mut fresh_drawer] {
            script_apply_eval!(cx, html, { text_style_bold +: { font_family: mod.theme.font_italic.font_family } });
        }
        let tf = &drawer.html.text_flow;
        assert_eq!(tf.text_style_bold.font_family_id(), tf.text_style_italic.font_family_id());
        for width in (80..400).step_by(7).map(f64::from) {
            fresh_drawer.draw("x", width);
            assert_eq!(drawer.get_row_starts(body, width), fresh_drawer.get_row_starts(body, width), "at {width}");
        }
    }

    #[test]
    fn lookahead_measures_again_when_code_boxes_or_fonts_change() {
        let body = "aaaa (<code>x</code>) bbbb (<b>cc</b>) dd<code>ee</code>ff gg";
        let mut drawer = HtmlDrawer::new();
        let mut fresh_drawer = HtmlDrawer::new();
        let tf = &drawer.html.text_flow;
        let (code_margin, bold, fixed) = (tf.inline_code_margin, tf.text_style_bold.clone(), tf.text_style_fixed.clone());
        let wide_code_margin = Inset { left: 9.0, right: 6.0, ..code_margin };
        // Each of these changes one thing from the one before. We set them without an apply,
        // which would clear what the lookahead found.
        let styles = [(wide_code_margin, &bold), (code_margin, &bold), (code_margin, &fixed), (code_margin, &bold)];
        for width in (60..300).step_by(3).map(f64::from) {
            for (code_margin, text_style_bold) in styles {
                drawer.get_row_starts(body, width);
                for tf in [&mut drawer.html.text_flow, &mut fresh_drawer.html.text_flow] {
                    tf.inline_code_margin = code_margin;
                    tf.text_style_bold = text_style_bold.clone();
                }
                fresh_drawer.draw("x", width);
                assert_eq!(drawer.get_row_starts(body, width), fresh_drawer.get_row_starts(body, width), "at {width}");
            }
        }
    }

    #[test]
    fn lookahead_follows_width_changes() {
        let mut drawer = HtmlDrawer::new();
        let mut fresh_drawer = HtmlDrawer::new();
        // Code boxes move to a new row at some widths, so what's before the runs after them changes.
        let body = "aaaaaaaaaaaa<code>\u{201C}</code> <b>word</b> x<code>abc</code>(y) and \"<a href='x'>a link</a>\" here";
        for width in (40..300).chain((40..300).rev()).map(f64::from) {
            fresh_drawer.draw("x", width);
            assert_eq!(drawer.get_row_starts(body, width), fresh_drawer.get_row_starts(body, width), "at {width}");
        }
    }

    #[test]
    fn last_line_gets_truncated_after_a_link() {
        let mut drawer = HtmlDrawer::new();
        let (body, plain) = ("aaaa bbbb <a href='x'>cccc</a>dddddd", "aaaa bbbb ccccdddddd");
        let full_width = drawer.draw(plain, 1000.0)[0].get_row_x_bounds(0).1;
        drawer.html.text_flow.max_lines = 1;
        drawer.html.text_flow.text_overflow = TextOverflow::Ellipsis;
        // The link is drawn without wrapping on the last line, so what comes after it has to wrap.
        for width in (60..200).map(f64::from).filter(|width| (width - full_width).abs() > 1.0) {
            drawer.draw(body, width);
            assert_eq!(drawer.html.text_flow.is_content_truncated(), width < full_width, "at {width}");
        }
    }

    /// Returns whether a run ends with the ellipsis, whether it's the layouter's or a widget's.
    fn ends_with_ellipsis(run: &DrawnRun) -> bool {
        let last_row = run.laidout_text.rows.last().unwrap();
        run.text == "\u{2026}" || last_row.glyphs.last().is_some_and(|glyph| glyph.cluster == last_row.text.len())
    }

    #[test]
    fn the_last_line_ends_with_an_ellipsis_inside_its_edge() {
        let mut drawer = HtmlDrawer::new();
        drawer.html.text_flow.text_overflow = TextOverflow::Ellipsis;
        for body in [
            "aaaa bbbb <b>cccc</b> dddd",
            "aaaa bbbb <code>cccc</code> dddd",
            "aaaa bbbb <a href='x'>cccc</a> dddd",
            "aaaa bbbb <mark>cccc</mark> dddd",
            "aaaa <img> bbbb <img> cccc",
        ] {
            for max_lines in [1, 2] {
                drawer.html.text_flow.max_lines = max_lines;
                for width in (30..160).map(f64::from) {
                    drawer.check_rows_end_before(body, width, width, |_| true);
                    let runs = drawer.draw(body, width);
                    let is_truncated = drawer.html.text_flow.is_content_truncated();
                    assert_eq!(runs.last().is_some_and(ends_with_ellipsis), is_truncated, "{body:?} at {width}");
                }
            }
        }
    }

    #[test]
    fn lines_cut_after_a_block_or_break_end_with_an_ellipsis() {
        let mut drawer = HtmlDrawer::new();
        drawer.html.text_flow.max_lines = 1;
        drawer.html.text_flow.text_overflow = TextOverflow::Ellipsis;
        for (body, is_cut) in [
            ("<p>aaaa</p><p>bbbb</p>", true),
            ("aaaa<br>bbbb", true),
            ("<ul><li>aaaa</li><li>bbbb</li></ul>", true),
            ("aaaa<blockquote>bbbb</blockquote>", true),
            // These show up below the line even without text.
            ("aaaa<hr>", true),
            ("aaaa<blockquote></blockquote>", true),
            ("aaaa<table><tr><td></td></tr></table>", true),
            ("aaaa<br>", false),
            ("<p>aaaa</p><p></p>", false),
            ("aaaa<a href='x'></a>", false),
        ] {
            let runs = drawer.draw(body, 300.0);
            assert_eq!(runs.last().is_some_and(ends_with_ellipsis), is_cut, "{body:?}");
            assert_eq!(drawer.html.text_flow.is_content_truncated(), is_cut, "{body:?}");
        }
    }

    #[test]
    fn a_collapsed_summary_is_only_cut_by_what_follows_its_details() {
        let mut drawer = HtmlDrawer::new();
        drawer.html.text_flow.max_lines = 1;
        drawer.html.text_flow.text_overflow = TextOverflow::Ellipsis;
        // The long body would leave no room after the image or the link if it went on in the summary's row.
        let long_body = "cccccccccccccccccccccccccccccccccccccccccccccccccc";
        for (body, is_cut) in [
            (format!("<details><summary>aaaa <img></summary>{long_body}</details>"), false),
            (format!("<details><summary>aaaa <a href='x'>bbbb</a></summary>{long_body}</details>"), false),
            ("<details><summary>aaaa <img></summary>cccc</details>dddd".to_string(), true),
            ("<details><summary>aaaa <a href='x'>bbbb</a></summary>cccc</details>dddd".to_string(), true),
        ] {
            drawer.draw(&body, 300.0);
            assert_eq!(drawer.html.text_flow.is_content_truncated(), is_cut, "{body:?}");
            // Once it's open, its body goes on in the summary's row, and gets cut.
            drawer.set_details_open(true);
            drawer.draw(&body, 300.0);
            assert!(drawer.html.text_flow.is_content_truncated(), "{body:?} when open");
            drawer.set_details_open(false);
            drawer.draw(&body, 300.0);
            assert_eq!(drawer.html.text_flow.is_content_truncated(), is_cut, "{body:?} when closed again");
        }
    }

    #[test]
    fn tables_keep_to_the_line_budget() {
        let mut drawer = HtmlDrawer::new();
        drawer.html.text_flow.text_overflow = TextOverflow::Ellipsis;
        for (body, max_lines, shown) in [
            // A nested table after the cut is skipped whole.
            ("aaaa<table><tr><td><table><tr><td>x</td></tr></table></td><td>y</td></tr></table>", 1, ["aaaa"].as_slice()),
            // Cells without a row stack up, like rows.
            ("<table><td>aaaa</td><td>bbbb</td></table>", 1, ["aaaa"].as_slice()),
            // A table in a cell doesn't make the row forget its taller first cell.
            (
                "<table><tr><td>aaaa<br>bbbb</td><td><table><tr><td>x</td></tr></table></td></tr></table>cccc",
                2,
                ["aaaa", "bbbb", "x"].as_slice(),
            ),
        ] {
            drawer.html.text_flow.max_lines = max_lines;
            let runs = drawer.draw(body, 300.0);
            let texts: Vec<&str> = runs.iter().map(|run| run.text.as_str()).collect();
            assert_eq!(texts, shown, "{body:?}");
            assert!(drawer.html.text_flow.is_content_truncated(), "{body:?}");
        }
    }

    #[test]
    fn lookahead_follows_max_lines_and_text_overflow() {
        let body = "aaaa bbbb <b>cccc</b> dddd <img> eeee <a href='x'>ffff gggg</a> hhhh<br>iiii";
        let mut drawer = HtmlDrawer::new();
        let mut fresh_drawer = HtmlDrawer::new();
        // These change without an apply, like when a preview expands and collapses again.
        let limits = [
            (0, TextOverflow::Ellipsis),
            (1, TextOverflow::Ellipsis),
            (2, TextOverflow::Clip),
            (2, TextOverflow::Ellipsis),
            (0, TextOverflow::Clip),
            (1, TextOverflow::Ellipsis),
        ];
        for width in (60..300).step_by(7).map(f64::from) {
            for (max_lines, text_overflow) in limits {
                for tf in [&mut drawer.html.text_flow, &mut fresh_drawer.html.text_flow] {
                    tf.max_lines = max_lines;
                    tf.text_overflow = text_overflow;
                }
                fresh_drawer.draw("x", width);
                assert_eq!(
                    drawer.get_shown_glyph_counts(body, width),
                    fresh_drawer.get_shown_glyph_counts(body, width),
                    "{max_lines} lines, {text_overflow:?}, at {width}",
                );
            }
        }
    }

    #[test]
    fn finding_the_room_after_a_run_takes_time_linear_in_what_follows() {
        let mut drawer = HtmlDrawer::new();
        drawer.html.text_flow.max_lines = 1;
        // `aaaa` has no room for the long word's first letter and the ellipsis, so only the room after it reads the word.
        let mut get_room_time = |length: usize| {
            (0..3)
                .map(|round| {
                    let body = format!("aaaa <b>{length}{round}{}</b>", "x".repeat(length));
                    // Clip needs no room, so this draw only shapes the word, which the layouter does either way.
                    drawer.html.text_flow.text_overflow = TextOverflow::Clip;
                    drawer.draw(&body, 30.0);
                    drawer.html.text_flow.text_overflow = TextOverflow::Ellipsis;
                    let start_time = Cx::monotonic_now();
                    drawer.draw(&body, 30.0);
                    let room_time = Cx::monotonic_now() - start_time;
                    assert!(drawer.html.text_flow.is_content_truncated());
                    room_time
                })
                .fold(f64::INFINITY, f64::min)
        };
        let (short_time, long_time) = (get_room_time(1000), get_room_time(16000));
        // Sixteen times the text takes about sixteen times as long, where a quadratic search takes about 256 times.
        assert!(long_time < short_time * 48.0, "{short_time} s for 1000 chars, but {long_time} s for 16000");
    }

    #[test]
    fn only_the_last_run_of_a_widget_ends_with_what_follows_it() {
        let mut drawer = HtmlDrawer::new();
        drawer.html.text_flow.max_lines = 1;
        drawer.html.text_flow.text_overflow = TextOverflow::Ellipsis;
        let runs = drawer.draw("the answer is <spoiler>42</spoiler><br>next line", 300.0);
        let texts: Vec<&str> = runs.iter().map(|run| run.text.as_str()).collect();
        assert_eq!(texts, ["the answer is ", " [Spoiler]  ", "42"]);
        assert!(!ends_with_ellipsis(&runs[1]) && ends_with_ellipsis(&runs[2]));
        // A spoiler that doesn't fit after its reason is hidden from there, so the reason stays.
        let mut hidden_count = 0;
        for width in (60..300).map(f64::from) {
            let runs = drawer.draw("the answer is <spoiler>42</spoiler>", width);
            let Some(reason) = runs.iter().find(|run| run.text == " [Spoiler]  ") else {
                continue;
            };
            if let Some(ellipsis) = runs.iter().find(|run| run.text == "\u{2026}") {
                assert!(ellipsis.get_row_x_bounds(0).0 >= reason.get_ink_end_x(0) - 0.5, "at {width}");
                hidden_count += 1;
            }
        }
        assert!(hidden_count > 0);
    }

    #[test]
    fn clip_cuts_widgets_without_an_ellipsis() {
        let mut drawer = HtmlDrawer::new();
        drawer.html.text_flow.text_overflow = TextOverflow::Clip;
        for body in [
            "aaaa bbbb <pill></pill> cccc",
            "aaaa bbbb <a href='x'>cccc dddd</a> eeee",
            "aaaa bbbb <mark>cccc dddd</mark> eeee",
            "aaaa <spoiler>42</spoiler> bbbb",
        ] {
            for max_lines in [1, 2] {
                drawer.html.text_flow.max_lines = max_lines;
                for width in (30..200).map(f64::from) {
                    let runs = drawer.draw(body, width);
                    assert!(!runs.iter().any(ends_with_ellipsis), "{body:?} at {width}");
                }
            }
        }
    }

    #[test]
    fn list_items_that_start_with_a_widget_take_their_lines() {
        let mut drawer = HtmlDrawer::new();
        drawer.html.text_flow.max_lines = 1;
        drawer.html.text_flow.text_overflow = TextOverflow::Ellipsis;
        let runs = drawer.draw("<ul><li><img> aaaa</li><li><img> bbbb</li></ul>", 300.0);
        assert!(!runs.iter().any(|run| run.text.contains("bbbb")));
        assert!(drawer.html.text_flow.is_content_truncated());
        // A widget too wide for the marker's row can only go below it while there are lines left for both.
        for max_lines in [1, 2] {
            drawer.html.text_flow.max_lines = max_lines;
            let runs = drawer.draw("<ul><li><wide></wide> aaaa</li><li>bbbb</li></ul>", 300.0);
            let mut row_tops: Vec<f64> = runs.iter().map(|run| run.origin.y).collect();
            row_tops.dedup_by(|top, other_top| (*top - *other_top).abs() < 1.0);
            assert_eq!(row_tops.len(), max_lines, "{max_lines} lines");
            assert!(!runs.iter().any(|run| run.text.contains("bbbb")), "{max_lines} lines");
        }
    }

    #[test]
    fn what_gets_cut_adds_no_height() {
        let mut drawer = HtmlDrawer::new();
        drawer.html.text_flow.text_overflow = TextOverflow::Ellipsis;
        let mut get_height = |body: &str, max_lines| {
            drawer.html.text_flow.max_lines = max_lines;
            drawer.draw(body, 300.0);
            drawer.html.text_flow.area.rect(&drawer.cx).size.y
        };
        for (body, max_lines, shown) in [
            ("aaaa<br>bbbb<br>cccc<br>dddd", 1, "aaaa"),
            ("aaaa<br>bbbb<br>cccc<br>dddd", 2, "aaaa<br>bbbb"),
            ("aaaa<ul><li>bbbb</li><li>cccc</li><li>dddd</li></ul>", 1, "aaaa"),
            ("<ul><li>aaaa</li><li>bbbb</li><li>cccc</li></ul>", 1, "<ul><li>aaaa</li></ul>"),
        ] {
            let (height, shown_height) = (get_height(body, max_lines), get_height(shown, 0));
            assert!((height - shown_height).abs() < 0.01, "{body:?}: {height} vs {shown_height}");
        }
    }

    #[test]
    fn empty_widgets_keep_room_for_the_ellipsis_before_them() {
        let mut drawer = HtmlDrawer::new();
        drawer.html.text_flow.max_lines = 1;
        drawer.html.text_flow.text_overflow = TextOverflow::Ellipsis;
        // The pill draws with no text in it, so the text before it keeps room for the ellipsis in its place.
        for width in (60..200).map(f64::from) {
            let runs = drawer.draw("aaaa bbbb cccc<pill></pill>", width);
            let Some(ellipsis) = runs.iter().find(|run| run.text == "\u{2026}") else {
                continue;
            };
            for run in runs.iter().filter(|run| run.text != "\u{2026}") {
                assert!(ellipsis.get_row_x_bounds(0).0 >= run.get_ink_end_x(0) - 0.5, "at {width}");
            }
        }
        // And the line it's on comes after the cut.
        drawer.draw("aaaa<br><pill></pill>", 300.0);
        assert!(drawer.html.text_flow.is_content_truncated());
    }

    #[test]
    fn empty_elements_keep_no_room_for_the_text_after_them() {
        let mut drawer = HtmlDrawer::new();
        // Nothing can break around the word joiners, so the empty links stay glued to the text.
        for body in ["xx yy aaaa\u{2060}<a href='x'></a>bbbbbb cc", "xx yy 1\u{2060}<a href='x'></a>000000 cc"] {
            for width in (60..200).map(f64::from) {
                drawer.check_rows_end_before(body, width, width, |_| true);
            }
        }
    }

    #[test]
    fn zero_width_text_takes_no_room_past_the_edge() {
        let mut drawer = HtmlDrawer::new();
        // A code box's margin can end past the edge, and so does the zero width space after it.
        for width in (120..400).map(|width| f64::from(width) / 2.0) {
            drawer.check_rows_end_before("aaaa <code>cccc</code>\u{200B}dd ee", width, width, |_| true);
        }
    }

    #[test]
    fn lines_with_only_widgets_are_as_tall_as_text() {
        let mut drawer = HtmlDrawer::new();
        let mut get_height = |body: &str| {
            drawer.draw(body, 300.0);
            drawer.html.text_flow.area.rect(&drawer.cx).size.y
        };
        // The zero width spaces give these lines the height of a line of text.
        for (body, body_with_text) in [
            ("<img>", "<img>\u{200B}"),
            ("<p><img></p>", "<p><img>\u{200B}</p>"),
            ("aaa<br><img><br>bbb", "aaa<br><img>\u{200B}<br>bbb"),
        ] {
            let (height, height_with_text) = (get_height(body), get_height(body_with_text));
            assert!((height - height_with_text).abs() < 0.01, "{body:?}: {height} vs {height_with_text}");
        }
    }

    #[test]
    fn code_boxes_dont_start_with_only_white_space() {
        let mut drawer = HtmlDrawer::new();
        let pad_l = drawer.html.text_flow.inline_code_padding.left;
        // Nothing can break before these code runs, and their first unit is the white space.
        for (body, code) in [("aaaa bbbb<code> cc</code> dd", " cc"), ("word<code>  spaced</code> after", "  spaced")] {
            let mut moved_count = 0;
            for width in (70..250).map(f64::from) {
                let runs = drawer.draw(body, width);
                let run = runs.iter().find(|run| run.text == code).unwrap();
                assert!(!run.laidout_text.rows[0].text.chars().all(is_hangable_white_space), "{body:?} at {width}");
                if (run.get_row_x_bounds(0).0 - pad_l).abs() < 0.01 {
                    moved_count += 1;
                }
            }
            assert!(moved_count > 0, "{body:?}");
        }
    }

    #[test]
    fn highlighted_text_keeps_its_box_inside() {
        let mut drawer = HtmlDrawer::new();
        let pad_r = drawer.html.text_flow.inline_code_padding.right;
        // Html can't know that a `mark` draws a box, so the text before it keeps room for its text only.
        for (body, highlighted) in [
            ("foo<mark>barbazquux</mark> qux and more words here", ["barbazquux"].as_slice()),
            ("word<mark>joined</mark> word another<mark>(paren)</mark>.", ["joined", "(paren)"].as_slice()),
        ] {
            for width in (100..300).map(f64::from) {
                drawer.check_rows_end_before(body, width, width - pad_r, |text| highlighted.contains(&text));
            }
        }
    }

    #[test]
    fn a_mark_landing_inside_a_character_is_floored_to_its_start() {
        // "cafe" with an acute on the e: that character is two bytes, at 3..5.
        let text = "caf\u{e9}";
        assert_eq!(floor_char_boundary(text, 3), 3);
        assert_eq!(floor_char_boundary(text, 4), 3);
        assert_eq!(floor_char_boundary(text, 5), 5);
    }

    #[test]
    fn a_mark_past_the_end_is_floored_to_the_end() {
        let text = "caf\u{e9}";
        assert_eq!(floor_char_boundary(text, 900), 5);
        assert_eq!(floor_char_boundary("", 4), 0);
    }
}
