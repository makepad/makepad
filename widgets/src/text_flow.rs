use crate::makepad_draw::text::{
    geom::Point as TextPoint,
    layouter::LaidoutText,
    selection::{Cursor, Selection},
};
use crate::{
    animator::*, makepad_derive_widget::*, makepad_draw::shader::draw_text::TextOverflow,
    makepad_draw::*, widget::*, widget_tree::CxWidgetExt,
};
use std::rc::Rc;

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
    pub combine_spaces: SmallVec<[bool; 4]>,
    #[rust]
    pub ignore_newlines: SmallVec<[bool; 4]>,
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
    pub item_counter: u64,
    #[rust]
    pub first_thing_on_a_line: bool,
    /// Number of visual lines drawn so far (across all text runs).
    /// Used for widget-level `max_lines` tracking in rich text.
    #[rust]
    lines_drawn: usize,
    /// Set when `lines_drawn >= max_lines`; further text draws are skipped.
    #[rust]
    content_truncated: bool,

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
    #[live]
    pub inline_code_padding: Inset,
    #[live]
    pub inline_code_margin: Inset,
    #[live(Inset{top:0.5,bottom:0.5,left:0.0,right:0.0})]
    pub heading_margin: Inset,
    #[live(Inset{top:0.5,bottom:0.5,left:0.0,right:0.0})]
    pub paragraph_margin: Inset,

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
                cx.set_cursor(MouseCursor::Default);
            }
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                cx.set_key_focus(self.area);
                if fe.device.is_touch() {
                    cx.hide_clipboard_actions();
                }
                if let Some(idx) = self.selection_tracker.point_to_index(cx, fe.abs) {
                    self.selection_anchor = idx;
                    self.selection_cursor = idx;
                    self.is_selecting = true;
                    self.redraw(cx);
                }
            }
            Hit::FingerMove(fe) if self.is_selecting => {
                if let Some(idx) = self.selection_tracker.point_to_index(cx, fe.abs) {
                    if self.selection_cursor != idx {
                        self.selection_cursor = idx;
                        // Propagate selection to child widgets (e.g., CodeView)
                        self.propagate_selection_to_children();
                        self.redraw(cx);
                    }
                }
            }
            Hit::FingerUp(fe) => {
                self.is_selecting = false;
                if fe.device.is_touch() {
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
        if self.selectable {
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
        self.font_sizes.clear();
        self.y_shift_scales.clear();
        self.font_colors.clear();
        self.area_stack.clear();
        self.combine_spaces.clear();
        self.ignore_newlines.clear();
        self.first_thing_on_a_line = true;
        self.table_num_columns = 0;
        self.in_table_header = false;
        self.table_row_cell_rects.clear();
        self.table_row_is_header = false;
        self.table_is_first_row = false;
        self.cell_text_align_x = 0.0;
    }

    pub fn push_size_rel_scale(&mut self, scale: f64) {
        self.font_sizes
            .push(self.font_sizes.last().unwrap_or(&self.font_size) * (scale as f32));
    }

    pub fn push_size_abs_scale(&mut self, scale: f64) {
        self.font_sizes.push(self.font_size * (scale as f32));
    }

    pub fn end(&mut self, cx: &mut Cx2d) {
        self.draw_text.end_deferred_slug_flush(cx);

        // Draw selection highlight before finishing the turtle
        self.draw_selection_rects(cx);

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
            self.selection_anchor = 0;
            self.selection_cursor = self.selection_tracker.total_len();
            for (widget, _, _) in &self.widget_text_entries {
                widget.selection_select_all();
            }
        }
    }

    /// Clear selection
    pub fn clear_selection(&mut self) {
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
        if self.selectable {
            let text_start = self.selection_tracker.text.len();
            let text_len = text.len();
            self.selection_tracker.push_widget_text(widget.area(), text);
            self.widget_text_entries
                .push((widget, text_start, text_len));
        }
    }

    pub fn begin_code(&mut self, cx: &mut Cx2d) {
        self.draw_block.block_type = FlowBlockType::Code;
        self.draw_block.begin(cx, self.code_walk, self.code_layout);
        self.area_stack.push(self.draw_block.draw_vars.area);
        self.first_thing_on_a_line = true;
    }

    pub fn end_code(&mut self, cx: &mut Cx2d) {
        self.draw_block.draw_vars.area = self.area_stack.pop().unwrap();
        self.draw_block.end(cx);
        if self.selectable {
            self.selection_tracker.push_newline();
        }
    }

    pub fn begin_list_item(&mut self, cx: &mut Cx2d, dot: &str, pad: f64) {
        let fs = *self.font_sizes.last().unwrap_or(&self.font_size);
        let font_based_padding = fs as f64 * pad;

        cx.begin_turtle(
            self.list_item_walk,
            Layout {
                padding: Inset {
                    left: self.list_item_layout.padding.left + font_based_padding,
                    ..self.list_item_layout.padding
                },
                ..self.list_item_layout
            },
        );

        cx.turtle_mut()
            .move_right_down(dvec2(-font_based_padding, 0.0));

        self.draw_text(cx, dot);
        TextFlow::walk_margin(cx, self.list_item_marker_pad);

        // Adjust the left padding to match the actual cursor position after the
        // bullet marker and its trailing pad, so that wrapped lines align with
        // the text after the marker rather than being over-indented.
        let actual_indent = cx.turtle().pos().x - cx.turtle().origin().x;
        cx.turtle_mut().set_padding_left(actual_indent);

        self.area_stack.push(self.draw_block.draw_vars.area);
    }

    pub fn end_list_item(&mut self, cx: &mut Cx2d) {
        cx.end_turtle();
        self.first_thing_on_a_line = true;
        if self.selectable {
            self.selection_tracker.push_newline();
        }
    }

    pub fn new_line_collapsed(&mut self, cx: &mut Cx2d) {
        cx.turtle_new_line();
        self.first_thing_on_a_line = true;
        if self.selectable {
            self.selection_tracker.push_newline();
        }
    }

    /// Starts a new line using the current wrap spacing, so that the vertical
    /// gap matches the line spacing of the most recently drawn text.
    /// This is intended for `<br>` tags in HTML rendering.
    pub fn new_line_with_wrap_spacing(&mut self, cx: &mut Cx2d) {
        let spacing = cx.turtle().wrap_spacing();
        cx.turtle_new_line_with_spacing(spacing);
        self.first_thing_on_a_line = true;
        if self.selectable {
            self.selection_tracker.push_newline();
        }
    }

    pub fn new_line_collapsed_with_spacing(&mut self, cx: &mut Cx2d, spacing: f64) {
        cx.turtle_new_line_with_spacing(spacing);
        self.first_thing_on_a_line = true;
        if self.selectable {
            self.selection_tracker.push_newline();
        }
    }

    pub fn sep(&mut self, cx: &mut Cx2d) {
        self.draw_block.block_type = FlowBlockType::Sep;
        self.draw_block.draw_walk(cx, self.sep_walk);
    }

    pub fn begin_quote(&mut self, cx: &mut Cx2d) {
        self.draw_block.block_type = FlowBlockType::Quote;
        self.draw_block
            .begin(cx, self.quote_walk, self.quote_layout);
        self.area_stack.push(self.draw_block.draw_vars.area);
    }

    pub fn end_quote(&mut self, cx: &mut Cx2d) {
        self.draw_block.draw_vars.area = self.area_stack.pop().unwrap();
        self.draw_block.end(cx);
        if self.selectable {
            self.selection_tracker.push_newline();
        }
    }

    pub fn begin_table(&mut self, cx: &mut Cx2d, num_columns: usize) {
        self.table_num_columns = num_columns;
        self.table_is_first_row = true;
        cx.begin_turtle(self.table_walk, self.table_layout);
    }

    pub fn end_table(&mut self, cx: &mut Cx2d) {
        cx.end_turtle();
        self.table_num_columns = 0;
        self.in_table_header = false;
        if self.selectable {
            self.selection_tracker.push_newline();
        }
    }

    pub fn begin_table_header_row(&mut self, cx: &mut Cx2d) {
        self.in_table_header = true;
        self.table_row_is_header = true;
        self.table_row_cell_rects.clear();
        cx.begin_turtle(self.table_row_walk, self.table_row_layout);
    }

    pub fn begin_table_row(&mut self, cx: &mut Cx2d) {
        self.table_row_is_header = false;
        self.table_row_cell_rects.clear();
        cx.begin_turtle(self.table_row_walk, self.table_row_layout);
    }

    pub fn end_table_row(&mut self, cx: &mut Cx2d) {
        let row_rect = cx.end_turtle();
        self.draw_row_cell_borders(cx, row_rect);
        if self.selectable {
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

            self.draw_block.table_header_bg_color = if self.table_row_is_header {
                saved_bg
            } else {
                transparent
            };
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
        self.first_thing_on_a_line = true;
        self.cell_text_align_x = align_x;
    }

    pub fn end_table_cell(&mut self, cx: &mut Cx2d) {
        let cell_rect = cx.end_turtle();
        self.table_row_cell_rects.push(cell_rect);
        self.cell_text_align_x = 0.0;
    }

    pub fn draw_item_counted(&mut self, cx: &mut Cx2d, template: LiveId) -> LiveId {
        let entry_id = self.new_counted_id();
        let start_pos = if self.selectable {
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
        let start_pos = if self.selectable {
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

            if (text == " " || text == "") && self.first_thing_on_a_line {
                return;
            }
            let text = if self.first_thing_on_a_line {
                text.trim_start().trim_end_matches("\n")
            } else {
                text.trim_end_matches("\n")
            };

            // Select the appropriate text style based on bold/italic/fixed state
            let text_style = if self.fixed.value() > 0 {
                self.text_style_fixed.clone()
            } else if self.bold.value() > 0 {
                if self.italic.value() > 0 {
                    self.text_style_bold_italic.clone()
                } else {
                    self.text_style_bold.clone()
                }
            } else if self.italic.value() > 0 {
                self.text_style_italic.clone()
            } else {
                self.text_style_normal.clone()
            };

            // Apply the text style to the single draw_text instance
            let top_drop = text_style.top_drop;
            self.draw_text.text_style = text_style;
            let font_size = self.font_sizes.last().unwrap_or(&self.font_size);
            let font_color = self.font_colors.last().unwrap_or(&self.font_color);
            self.draw_text.text_style.font_size = *font_size as _;
            self.draw_text.color = *font_color;
            let y_shift_scale = self.y_shift_scales.last().copied().unwrap_or(0.0);
            self.draw_text.temp_y_shift = top_drop + y_shift_scale;
            self.draw_text.layout_align = Align {
                x: self.cell_text_align_x,
                y: 0.0,
            };

            // Widget-level max_lines: compute how many layouter rows this run
            // is allowed. A "continuation" run starts mid-line (turtle x > left
            // edge), so its first row shares the current visual line.
            let is_continuation = if self.max_lines > 0 {
                let turtle_pos = cx.turtle().pos();
                let turtle_rect = cx.turtle().inner_rect();
                (turtle_pos.x - turtle_rect.pos.x) > 0.5
            } else {
                false
            };

            if self.max_lines > 0 {
                let remaining_new_lines = self.max_lines.saturating_sub(self.lines_drawn);
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
            } else {
                self.draw_text.max_lines = 0;
                self.draw_text.text_overflow = TextOverflow::Clip;
            };

            let dt = &mut self.draw_text;

            // Capture LaidoutText for selection when selectable
            if self.selectable {
                let turtle_pos = cx.turtle().pos();
                let turtle_rect = cx.turtle().inner_rect();
                let origin = dvec2(turtle_rect.pos.x, turtle_pos.y);
                let first_row_indent = (turtle_pos.x - turtle_rect.pos.x) as f32;
                let row_height = cx.turtle().next_row_offset() as f32;
                let max_width = if !turtle_rect.size.x.is_nan() {
                    Some(turtle_rect.size.x as f32)
                } else {
                    None
                };
                let wrap = matches!(cx.turtle().layout().flow, Flow::Right { wrap: true, .. });

                let laidout_text = dt.layout(
                    cx,
                    first_row_indent,
                    row_height,
                    max_width,
                    wrap,
                    dt.layout_align,
                    text,
                );

                self.selection_tracker
                    .push_text(laidout_text, origin, dt.font_scale, text);
            }

            let areas_tracker = &mut self.areas_tracker;
            let (run_rows, run_truncated) = if self.inline_code.value() > 0 {
                let db = &mut self.draw_block;
                db.block_type = FlowBlockType::InlineCode;
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
                let wrap_enabled =
                    matches!(cx.turtle().layout().flow, Flow::Right { wrap: true, .. });
                if wrap_enabled && !self.first_thing_on_a_line {
                    if let Some(max_width) = max_width {
                        // The eventual layout will start at cursor + pad_l
                        // (because we walk pad_l below). Predict against
                        // that effective indent.
                        let first_row_indent =
                            (turtle_pos.x - turtle_rect.pos.x) as f32 + pad_l as f32;
                        let row_offset = cx.turtle().next_row_offset() as f32;
                        let layout_align = dt.layout_align;
                        let measured = dt.layout(
                            cx,
                            first_row_indent,
                            row_offset,
                            Some(max_width),
                            true,
                            layout_align,
                            text,
                        );
                        let row0_empty = measured
                            .rows
                            .first()
                            .map(|r| r.glyphs.is_empty())
                            .unwrap_or(false);
                        let has_more_rows = measured.rows.len() > 1;
                        // Also measure on a fresh line (indent=0) to see if
                        // the text would fit on a single row by itself.
                        let fresh = dt.layout(
                            cx,
                            0.0,
                            row_offset,
                            Some(max_width),
                            true,
                            layout_align,
                            text,
                        );
                        let fits_on_fresh_line = fresh.rows.len() == 1;
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
                        let needs_wrap =
                            (row0_empty && has_more_rows) || (has_more_rows && fits_on_fresh_line);
                        if needs_wrap {
                            // Match the spacing the layouter would have
                            // used had it wrapped naturally — otherwise
                            // the forced new line sits tight against the
                            // previous row while naturally-wrapped lines
                            // get the configured `wrap_spacing` gap.
                            let ws = cx.turtle().wrap_spacing();
                            // Restore the un-inflated left padding around
                            // the new-line call. `turtle_new_line_with_spacing`
                            // positions the cursor at `origin.x + padding.left`,
                            // and we already added `pad_l` to padding.left
                            // above. Without this, the cursor lands at
                            // `parent_left + pad_l`, then the `walk_margin(pad_l)`
                            // below adds another `pad_l` — leaving the box's
                            // left edge floating `pad_l` px in from the
                            // parent's content edge instead of flush against it.
                            cx.turtle_mut().set_padding_left(old_padding_left);
                            cx.turtle_new_line_with_spacing(ws);
                            cx.turtle_mut().set_padding_left(old_padding_left + pad_l);
                        }
                    }
                }

                // Walk the box's left padding so the actual text glyphs
                // start `pad_l` past the cursor — this leaves room inside
                // the box for the visible left padding (text never sits
                // flush with the box's rounded left edge).
                let pad_l_rect = TextFlow::walk_margin(cx, pad_l);
                areas_tracker.track_rect(cx, pad_l_rect);

                let code_pad_h = (self.inline_code_padding.top
                    + self.inline_code_padding.bottom
                    + self.inline_code_margin.top
                    + self.inline_code_margin.bottom) as f64;
                let result = dt.draw_walk_resumable_with_background(cx, text, |cx, mut rect, _| {
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
                result
            } else if self.strikethrough.value() > 0 {
                let db = &mut self.draw_block;
                db.line_color = *font_color;
                db.block_type = FlowBlockType::Strikethrough;
                dt.draw_walk_resumable_with(cx, text, |cx, rect, _| {
                    db.draw_abs(cx, rect);
                    areas_tracker.track_rect(cx, rect);
                })
            } else if self.underline.value() > 0 {
                let db = &mut self.draw_block;
                db.line_color = *font_color;
                db.block_type = FlowBlockType::Underline;
                dt.draw_walk_resumable_with(cx, text, |cx, rect, _| {
                    db.draw_abs(cx, rect);
                    areas_tracker.track_rect(cx, rect);
                })
            } else {
                dt.draw_walk_resumable_with(cx, text, |cx, rect, _| {
                    areas_tracker.track_rect(cx, rect);
                })
            };

            // Update widget-level line tracking.
            if self.max_lines > 0 {
                let new_lines = run_rows.saturating_sub(if is_continuation { 1 } else { 0 });
                self.lines_drawn += new_lines;
                // If this run was truncated (ellipsis was appended), stop here.
                if run_truncated {
                    self.content_truncated = true;
                }
            }
        }
        self.first_thing_on_a_line = false;
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
        // Check main area
        let area = self.area();
        if area.is_valid(cx) && area.rect(cx).contains(point) {
            return true;
        }
        // Links span multiple text rects via drawn_areas
        for area in self.drawn_areas.iter() {
            if area.is_valid(cx) && area.rect(cx).contains(point) {
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

        for area in self.drawn_areas.clone().into_iter() {
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
        self.text.as_mut_empty().push_str(v);
        self.redraw(cx);
    }
}
