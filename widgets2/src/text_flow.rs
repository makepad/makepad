use crate::makepad_draw::text::{
    geom::Point as TextPoint,
    layouter::LaidoutText,
    selection::{Cursor, Selection},
};
use crate::{animator::*, makepad_derive_widget::*, makepad_draw::*, widget::*};
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

        space_1: uniform(4.0)
        space_2: uniform(8.0)
    }

    mod.widgets.FlowBlockType = FlowBlockType

    mod.widgets.TextFlowBase = #(TextFlow::register_widget(vm)){
        font_size: 8
        flow: Flow.Right{wrap: true}
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

        $link: mod.widgets.TextFlowLink{}

        draw_block +: {
            line_color: theme.color_text
            sep_color: theme.color_shadow
            quote_bg_color: theme.color_bg_highlight
            quote_fg_color: theme.color_text
            code_color: theme.color_bg_highlight
            selection_color: theme.color_selection_focus
            space_1: uniform(theme.space_1)
            space_2: uniform(theme.space_2)
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
                }
                return #f00
            }
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

    pub fn push_newline(&mut self) {
        self.text.push('\n');
    }

    pub fn total_len(&self) -> usize {
        self.text.len()
    }

    /// Find character index from screen point
    pub fn point_to_index(&self, point: DVec2) -> Option<usize> {
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
            }
        }

        // Find nearest segment if point outside all
        self.nearest_index(point)
    }

    /// Find the nearest character index when point is outside all segments
    fn nearest_index(&self, point: DVec2) -> Option<usize> {
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
            }
        }

        rects
    }
}

// this widget has a retained and an immediate mode api
#[derive(Script, Widget)]
pub struct TextFlow {
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
    #[walk]
    walk: Walk,

    #[rust]
    area_stack: SmallVec<[Area; 4]>,
    #[rust]
    pub font_sizes: SmallVec<[f32; 8]>,
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
    #[live]
    pub inline_code_padding: Inset,
    #[live]
    pub inline_code_margin: Inset,
    #[live(Inset{top:0.5,bottom:0.5,left:0.0,right:0.0})]
    pub heading_margin: Inset,
    #[live(Inset{top:0.5,bottom:0.5,left:0.0,right:0.0})]
    pub paragraph_margin: Inset,

    #[redraw]
    #[rust]
    area: Area,
    #[rust]
    draw_state: DrawStateWrap<DrawState>,
    #[rust(Some(Default::default()))]
    items: Option<ComponentMap<LiveId, (WidgetRef, LiveId)>>,
    #[rust]
    templates: ComponentMap<LiveId, ScriptObjectRef>,

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

impl Widget for TextFlow {
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
        for (id, (entry, _)) in self.items.as_mut().unwrap().iter_mut() {
            scope.with_id(*id, |scope| {
                entry.handle_event(cx, event, scope);
            });
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
                self.draw_text.draw_vars.area.redraw(cx);
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
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                cx.set_key_focus(self.area);
                if let Some(idx) = self.selection_tracker.point_to_index(fe.abs) {
                    self.selection_anchor = idx;
                    self.selection_cursor = idx;
                    self.is_selecting = true;
                    self.redraw(cx);
                }
            }
            Hit::FingerMove(fe) if self.is_selecting => {
                if let Some(idx) = self.selection_tracker.point_to_index(fe.abs) {
                    if self.selection_cursor != idx {
                        self.selection_cursor = idx;
                        self.redraw(cx);
                    }
                }
            }
            Hit::FingerUp(_) => {
                self.is_selecting = false;
            }
            Hit::KeyFocusLost(_) => {
                self.clear_selection();
                self.redraw(cx);
            }
            Hit::TextCopy(event) => {
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
        self.clear_stacks();
        if self.selectable {
            self.selection_tracker.clear();
        }
        // Reset char_index for streaming animation (also while fading out)
        if self.streaming_animation || !self.is_animation_idle() {
            self.draw_text.reset_char_index();
        }
    }

    /// Check if animation is completely idle (not streaming and fade complete)
    pub fn is_animation_idle(&self) -> bool {
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
        self.font_colors.clear();
        self.area_stack.clear();
        self.combine_spaces.clear();
        self.ignore_newlines.clear();
        self.first_thing_on_a_line = true;
    }

    pub fn push_size_rel_scale(&mut self, scale: f64) {
        self.font_sizes
            .push(self.font_sizes.last().unwrap_or(&self.font_size) * (scale as f32));
    }

    pub fn push_size_abs_scale(&mut self, scale: f64) {
        self.font_sizes.push(self.font_size * (scale as f32));
    }

    pub fn end(&mut self, cx: &mut Cx2d) {
        // Draw selection highlight before finishing the turtle
        self.draw_selection(cx);
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

    /// Draw selection highlight rectangles
    fn draw_selection(&mut self, cx: &mut Cx2d) {
        if !self.selectable {
            return;
        }

        let start = self.selection_anchor.min(self.selection_cursor);
        let end = self.selection_anchor.max(self.selection_cursor);

        if start == end {
            return;
        }

        self.draw_block.block_type = FlowBlockType::Selection;

        for rect in self.selection_tracker.selection_rects(start, end) {
            self.draw_block.draw_abs(cx, rect);
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
        }
    }

    /// Clear selection
    pub fn clear_selection(&mut self) {
        self.selection_anchor = 0;
        self.selection_cursor = 0;
        self.is_selecting = false;
    }

    /// Check if there is a selection
    pub fn has_selection(&self) -> bool {
        self.selectable && self.selection_anchor != self.selection_cursor
    }

    /// Set selection range (for external control, e.g., cross-TextFlow selection)
    pub fn set_selection(&mut self, anchor: usize, cursor: usize) {
        if self.selectable {
            self.selection_anchor = anchor;
            self.selection_cursor = cursor;
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

    /// Convert absolute position to character index
    pub fn point_to_char_index(&self, abs: DVec2) -> Option<usize> {
        self.selection_tracker.point_to_index(abs)
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
        self.draw_text(cx, " ");

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
            return Some(entry.0.clone());
        }
        None
    }

    /// Check if a point hits any interactive item widget in this TextFlow.
    /// This includes links, buttons, and any other inline components.
    /// Used by PortalList to decide whether to handle selection or pass through to items.
    pub fn point_hits_item(&self, cx: &Cx, abs: DVec2) -> bool {
        for (_id, (widget, _template)) in self.items.as_ref().unwrap().iter() {
            // Check if the point is within this item widget's area
            let area = widget.area();
            if !area.is_empty() && area.rect(cx).contains(abs) {
                return true;
            }
            // Also check TextFlowLink's drawn_areas (since links span multiple text rects)
            if let Some(link) = widget.as_text_flow_link().borrow() {
                for area in link.drawn_areas.iter() {
                    if area.rect(cx).contains(abs) {
                        return true;
                    }
                }
            }
        }
        false
    }

    pub fn draw_text(&mut self, cx: &mut Cx2d, text: &str) {
        if let Some(DrawState::Drawing) = self.draw_state.get() {
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
            self.draw_text.text_style = text_style;
            let font_size = self.font_sizes.last().unwrap_or(&self.font_size);
            let font_color = self.font_colors.last().unwrap_or(&self.font_color);
            self.draw_text.text_style.font_size = *font_size as _;
            self.draw_text.color = *font_color;

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
                let wrap = cx.turtle().layout().flow == Flow::right_wrap();

                let laidout_text = dt.layout(
                    cx,
                    first_row_indent,
                    row_height,
                    max_width,
                    wrap,
                    Align::default(),
                    text,
                );

                self.selection_tracker
                    .push_text(laidout_text, origin, dt.font_scale, text);
            }

            let areas_tracker = &mut self.areas_tracker;
            if self.inline_code.value() > 0 {
                let db = &mut self.draw_block;
                db.block_type = FlowBlockType::InlineCode;
                if !self.first_thing_on_a_line {
                    let rect = TextFlow::walk_margin(cx, self.inline_code_margin.left);
                    areas_tracker.track_rect(cx, rect);
                }
                dt.draw_walk_resumable_with(cx, text, |cx, mut rect, _| {
                    rect.pos -= self.inline_code_padding.left_top();
                    rect.size += self.inline_code_padding.size();
                    db.draw_abs(cx, rect);
                    areas_tracker.track_rect(cx, rect);
                });
                let rect = TextFlow::walk_margin(cx, self.inline_code_margin.right);
                areas_tracker.track_rect(cx, rect);
            } else if self.strikethrough.value() > 0 {
                let db = &mut self.draw_block;
                db.line_color = *font_color;
                db.block_type = FlowBlockType::Strikethrough;
                dt.draw_walk_resumable_with(cx, text, |cx, rect, _| {
                    db.draw_abs(cx, rect);
                    areas_tracker.track_rect(cx, rect);
                });
            } else if self.underline.value() > 0 {
                let db = &mut self.draw_block;
                db.line_color = *font_color;
                db.block_type = FlowBlockType::Underline;
                dt.draw_walk_resumable_with(cx, text, |cx, rect, _| {
                    db.draw_abs(cx, rect);
                    areas_tracker.track_rect(cx, rect);
                });
            } else {
                dt.draw_walk_resumable_with(cx, text, |cx, rect, _| {
                    areas_tracker.track_rect(cx, rect);
                });
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

#[derive(Script, ScriptHook, Widget, Animator)]
pub struct TextFlowLink {
    #[source]
    source: ScriptObjectRef,
    #[apply_default]
    animator: Animator,

    #[redraw]
    #[area]
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

    #[action_data]
    #[rust]
    action_data: WidgetActionData,
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
                            &scope.path,
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
                                &scope.path,
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
