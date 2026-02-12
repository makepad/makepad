use crate::{makepad_derive_widget::*, makepad_draw::*, portal_list::*, view::*, widget::*};
use makepad_pdf_parse::{
    content::parse_content_stream,
    document::PdfDocument,
    font::{char_width, decode_text},
    PdfOp, PdfPage, TextArrayItem,
};
use std::rc::Rc;

// ---- Text segment tracking for selection ----

/// A drawn text fragment with its screen-space position and char range.
struct TextSegment {
    /// Screen-space rect of this text fragment
    rect: Rect,
    /// The decoded text
    text: String,
    /// Byte offset into the page's flat text buffer
    char_offset: usize,
}

/// Tracks all text segments on a page for selection support.
#[derive(Default)]
struct PageTextTracker {
    /// All text segments in draw order
    segments: Vec<TextSegment>,
    /// Accumulated text content of the entire page
    text: String,
}

impl PageTextTracker {
    fn clear(&mut self) {
        self.segments.clear();
        self.text.clear();
    }

    fn push(&mut self, rect: Rect, text: &str) {
        let char_offset = self.text.len();
        self.text.push_str(text);
        self.segments.push(TextSegment {
            rect,
            text: text.to_string(),
            char_offset,
        });
    }

    fn total_len(&self) -> usize {
        self.text.len()
    }

    /// Find char index from absolute screen point.
    fn point_to_index(&self, abs: DVec2) -> Option<usize> {
        // First try exact hit within a segment
        for seg in &self.segments {
            if seg.rect.contains(abs) {
                // Interpolate x position within the segment
                let local_x = (abs.x - seg.rect.pos.x) / seg.rect.size.x;
                let char_count = seg.text.chars().count();
                let char_idx = ((local_x * char_count as f64).round() as usize).min(char_count);
                // Convert char index to byte offset
                let byte_idx: usize = seg.text.chars().take(char_idx).map(|c| c.len_utf8()).sum();
                return Some(seg.char_offset + byte_idx);
            }
        }
        // Fall back to nearest segment
        self.nearest_index(abs)
    }

    fn nearest_index(&self, abs: DVec2) -> Option<usize> {
        let mut best: Option<(usize, f64)> = None;
        for seg in &self.segments {
            let dist = point_to_rect_distance(abs, seg.rect);
            if best.map_or(true, |(_, d)| dist < d) {
                let local_x = ((abs.x - seg.rect.pos.x) / seg.rect.size.x).clamp(0.0, 1.0);
                let char_count = seg.text.chars().count();
                let char_idx = ((local_x * char_count as f64).round() as usize).min(char_count);
                let byte_idx: usize = seg.text.chars().take(char_idx).map(|c| c.len_utf8()).sum();
                best = Some((seg.char_offset + byte_idx, dist));
            }
        }
        best.map(|(idx, _)| idx)
    }

    /// Get selection highlight rects for the given byte range.
    fn selection_rects(&self, start: usize, end: usize) -> Vec<Rect> {
        let mut rects = Vec::new();
        for seg in &self.segments {
            let seg_end = seg.char_offset + seg.text.len();
            // Check overlap
            if end <= seg.char_offset || start >= seg_end {
                continue;
            }
            // Compute partial rect
            let seg_start_clamped = start.saturating_sub(seg.char_offset);
            let seg_end_clamped = (end - seg.char_offset).min(seg.text.len());

            let total_bytes = seg.text.len();
            if total_bytes == 0 {
                continue;
            }
            let x_start_frac = seg_start_clamped as f64 / total_bytes as f64;
            let x_end_frac = seg_end_clamped as f64 / total_bytes as f64;

            rects.push(Rect {
                pos: dvec2(
                    seg.rect.pos.x + seg.rect.size.x * x_start_frac,
                    seg.rect.pos.y,
                ),
                size: dvec2(
                    seg.rect.size.x * (x_end_frac - x_start_frac),
                    seg.rect.size.y,
                ),
            });
        }
        rects
    }

    fn get_text_for_range(&self, start: usize, end: usize) -> String {
        self.text.get(start..end).unwrap_or("").to_string()
    }

    /// Get cursor rect (thin vertical line) at the given byte index.
    fn cursor_rect(&self, index: usize) -> Option<Rect> {
        for seg in &self.segments {
            let seg_end = seg.char_offset + seg.text.len();
            if index >= seg.char_offset && index <= seg_end {
                let local_byte = index - seg.char_offset;
                let total_bytes = seg.text.len();
                let frac = if total_bytes > 0 {
                    local_byte as f64 / total_bytes as f64
                } else {
                    0.0
                };
                let x = seg.rect.pos.x + seg.rect.size.x * frac;
                return Some(Rect {
                    pos: dvec2(x - 1.0, seg.rect.pos.y),
                    size: dvec2(2.0, seg.rect.size.y),
                });
            }
        }
        None
    }
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

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.PdfPageViewBase = #(PdfPageView::register_widget(vm))
    mod.widgets.PdfPageView = set_type_default() do mod.widgets.PdfPageViewBase {
        width: Fill
        height: 792
        draw_bg +: {
            color: #fff
        }
        draw_selection +: {
            color: uniform(#x3399FF66)
            pixel: fn() {
                return vec4(self.color.rgb * self.color.a, self.color.a)
            }
        }
        draw_cursor +: {
            color: uniform(#000)
            pixel: fn() {
                return vec4(self.color.rgb * self.color.a, self.color.a)
            }
        }
        draw_text +: {
            text_style: theme.font_regular
        }
        draw_text_bold +: {
            text_style: theme.font_bold
        }
        draw_text_code +: {
            text_style: theme.font_code
        }
    }

    mod.widgets.PdfViewBase = #(PdfView::register_widget(vm))
    mod.widgets.PdfView = set_type_default() do mod.widgets.PdfViewBase {
        width: Fill
        height: Fill
        flow: Down
        list := PortalList {
            width: Fill
            height: Fill
            flow: Down
            drag_scrolling: true
            selectable: true
            Page := View {
                width: Fill
                height: Fit
                margin: Inset{ bottom: 8. }
                new_batch: true
                page_view := mod.widgets.PdfPageView {}
            }
        }
    }
}

// ---- Shared page data ----

pub struct CachedPage {
    ops: Vec<PdfOp>,
    page: PdfPage,
}

// ---- PdfPageView: renders a single PDF page (TextFlow-like step pattern) ----

#[derive(Clone)]
struct GfxState {
    ctm: [f64; 6],
    fill_color: [f32; 4],
    stroke_color: [f32; 4],
    line_width: f64,
    font_name: String,
    font_size: f64,
    text_matrix: [f64; 6],
    text_line_matrix: [f64; 6],
    char_spacing: f64,
    word_spacing: f64,
    text_leading: f64,
    text_rise: f64,
    horiz_scaling: f64,
    fill_alpha: f32,
    stroke_alpha: f32,
}

impl Default for GfxState {
    fn default() -> Self {
        Self {
            ctm: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            fill_color: [0.0, 0.0, 0.0, 1.0],
            stroke_color: [0.0, 0.0, 0.0, 1.0],
            line_width: 1.0,
            font_name: String::new(),
            font_size: 12.0,
            text_matrix: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            text_line_matrix: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            char_spacing: 0.0,
            word_spacing: 0.0,
            text_leading: 0.0,
            text_rise: 0.0,
            horiz_scaling: 100.0,
            fill_alpha: 1.0,
            stroke_alpha: 1.0,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum PageDrawState {
    Begin,
    Drawing,
}

#[derive(Script, ScriptHook, WidgetRef, WidgetSet, WidgetRegister)]
pub struct PdfPageView {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[live]
    draw_bg: DrawQuad,
    #[live]
    draw_selection: DrawQuad,
    #[live]
    draw_cursor: DrawQuad,
    #[live]
    draw_vector: DrawVector,
    #[live]
    draw_text: DrawText,
    #[live]
    draw_text_bold: DrawText,
    #[live]
    draw_text_code: DrawText,
    #[rust]
    area: Area,
    #[rust]
    draw_state: DrawStateWrap<PageDrawState>,
    #[rust]
    text_tracker: PageTextTracker,
    #[rust]
    selection_anchor: usize,
    #[rust]
    selection_cursor: usize,
}

impl WidgetNode for PdfPageView {
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }
    fn walk(&mut self, _cx: &mut Cx) -> Walk {
        self.walk
    }
    fn area(&self) -> Area {
        self.area
    }
    fn redraw(&mut self, cx: &mut Cx) {
        self.area.redraw(cx);
    }
    fn selection_text_len(&self) -> usize {
        self.text_tracker.total_len()
    }
    fn selection_point_to_char_index(&self, _cx: &Cx, abs: DVec2) -> Option<usize> {
        self.text_tracker.point_to_index(abs)
    }
    fn selection_set(&mut self, anchor: usize, cursor: usize) {
        self.selection_anchor = anchor;
        self.selection_cursor = cursor;
    }
    fn selection_clear(&mut self) {
        self.selection_anchor = 0;
        self.selection_cursor = 0;
    }
    fn selection_select_all(&mut self) {
        self.selection_anchor = 0;
        self.selection_cursor = self.text_tracker.total_len();
    }
    fn selection_get_text_for_range(&self, start: usize, end: usize) -> String {
        self.text_tracker.get_text_for_range(start, end)
    }
    fn selection_get_full_text(&self) -> String {
        self.text_tracker.text.clone()
    }
}

impl Widget for PdfPageView {
    fn is_interactive(&self) -> bool {
        false
    }

    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if self.draw_state.begin(cx, PageDrawState::Begin) {
            cx.begin_turtle(walk, self.layout);
            // Draw white background
            let rect = cx.turtle().rect();
            self.draw_bg.draw_abs(cx, rect);
            self.draw_state.set(PageDrawState::Drawing);
            return DrawStep::make_step();
        }
        if let Some(PageDrawState::Drawing) = self.draw_state.get() {
            cx.end_turtle_with_area(&mut self.area);
            self.draw_state.end();
        }
        DrawStep::done()
    }
}

impl PdfPageView {
    /// Render a page's content. Called by PdfView between begin step and end.
    pub(crate) fn render_page(&mut self, cx: &mut Cx2d, cached: &CachedPage, zoom: f64) {
        let rect = cx.turtle().rect();
        let origin_x = rect.pos.x as f32;
        let origin_y = rect.pos.y as f32;
        let z = zoom as f32;
        let page_height = cached.page.height() as f32;

        // Pass 1: all vector geometry
        self.render_vectors(
            cx,
            &cached.ops,
            &cached.page,
            origin_x,
            origin_y,
            z,
            page_height,
        );

        // Rebuild text tracker for selection support
        self.text_tracker.clear();
        self.collect_text_segments(
            &cached.ops,
            &cached.page,
            origin_x,
            origin_y,
            z,
            page_height,
        );

        // Pass 2: selection highlights (drawn before text so text appears on top)
        self.draw_selection_highlights(cx);

        // Pass 3: all text
        self.render_text(
            cx,
            &cached.ops,
            &cached.page,
            origin_x,
            origin_y,
            z,
            page_height,
        );
    }

    fn render_vectors(
        &mut self,
        cx: &mut Cx2d,
        ops: &[PdfOp],
        page: &PdfPage,
        ox: f32,
        oy: f32,
        zoom: f32,
        ph: f32,
    ) {
        let mut st = GfxState::default();
        let mut stack: Vec<GfxState> = Vec::new();
        let mut has_path = false;
        let mut any = false;

        let scr = |s: &GfxState, px: f64, py: f64| -> (f32, f32) {
            let tx = s.ctm[0] * px + s.ctm[2] * py + s.ctm[4];
            let ty = s.ctm[1] * px + s.ctm[3] * py + s.ctm[5];
            (ox + tx as f32 * zoom, oy + (ph - ty as f32) * zoom)
        };

        self.draw_vector.begin();

        for op in ops {
            match op {
                PdfOp::SaveState => stack.push(st.clone()),
                PdfOp::RestoreState => {
                    if let Some(s) = stack.pop() {
                        st = s;
                    }
                }
                PdfOp::ConcatMatrix(m) => st.ctm = mul_mat(&st.ctm, m),
                PdfOp::SetLineWidth(w) => st.line_width = *w,
                PdfOp::SetExtGState(name) => {
                    if let Some(gs) = page.ext_gstate.get(name) {
                        if let Some(a) = gs.ca {
                            st.stroke_alpha = a as f32;
                        }
                        if let Some(a) = gs.ca_lower {
                            st.fill_alpha = a as f32;
                        }
                    }
                }
                PdfOp::SetFillGray(g) => {
                    let v = *g as f32;
                    st.fill_color = [v, v, v, st.fill_alpha];
                }
                PdfOp::SetStrokeGray(g) => {
                    let v = *g as f32;
                    st.stroke_color = [v, v, v, st.stroke_alpha];
                }
                PdfOp::SetFillRgb(r, g, b) => {
                    st.fill_color = [*r as f32, *g as f32, *b as f32, st.fill_alpha]
                }
                PdfOp::SetStrokeRgb(r, g, b) => {
                    st.stroke_color = [*r as f32, *g as f32, *b as f32, st.stroke_alpha]
                }
                PdfOp::SetFillCmyk(c, m, y, k) => {
                    let (r, g, b) = cmyk_to_rgb(*c, *m, *y, *k);
                    st.fill_color = [r, g, b, st.fill_alpha];
                }
                PdfOp::SetStrokeCmyk(c, m, y, k) => {
                    let (r, g, b) = cmyk_to_rgb(*c, *m, *y, *k);
                    st.stroke_color = [r, g, b, st.stroke_alpha];
                }
                PdfOp::SetFillColor(v) => st.fill_color = color_from_vals(v, st.fill_alpha),
                PdfOp::SetStrokeColor(v) => st.stroke_color = color_from_vals(v, st.stroke_alpha),

                PdfOp::MoveTo(x, y) => {
                    let (sx, sy) = scr(&st, *x, *y);
                    self.draw_vector.move_to(sx, sy);
                    has_path = true;
                }
                PdfOp::LineTo(x, y) => {
                    let (sx, sy) = scr(&st, *x, *y);
                    self.draw_vector.line_to(sx, sy);
                }
                PdfOp::CurveTo(x1, y1, x2, y2, x3, y3) => {
                    let (a, b) = scr(&st, *x1, *y1);
                    let (c, d) = scr(&st, *x2, *y2);
                    let (e, f) = scr(&st, *x3, *y3);
                    self.draw_vector.bezier_to(a, b, c, d, e, f);
                }
                PdfOp::CurveToV(x2, y2, x3, y3) => {
                    let (a, b) = scr(&st, *x2, *y2);
                    let (c, d) = scr(&st, *x3, *y3);
                    self.draw_vector.quad_to(a, b, c, d);
                }
                PdfOp::CurveToY(x1, y1, x3, y3) => {
                    let (a, b) = scr(&st, *x1, *y1);
                    let (c, d) = scr(&st, *x3, *y3);
                    self.draw_vector.bezier_to(a, b, c, d, c, d);
                }
                PdfOp::ClosePath => self.draw_vector.close(),
                PdfOp::Rectangle(x, y, w, h) => {
                    let (sx, sy) = scr(&st, *x, *y + *h);
                    let sw = (*w as f32) * zoom * st.ctm[0].abs() as f32;
                    let sh = (*h as f32) * zoom * st.ctm[3].abs() as f32;
                    self.draw_vector.rect(sx, sy, sw, sh);
                    has_path = true;
                }

                PdfOp::Fill | PdfOp::FillEvenOdd => {
                    if has_path {
                        let c = st.fill_color;
                        self.draw_vector.set_color(c[0], c[1], c[2], c[3]);
                        self.draw_vector.fill();
                        any = true;
                        has_path = false;
                    }
                }
                PdfOp::Stroke => {
                    if has_path {
                        let c = st.stroke_color;
                        self.draw_vector.set_color(c[0], c[1], c[2], c[3]);
                        self.draw_vector
                            .stroke((st.line_width as f32 * zoom).max(0.5));
                        any = true;
                        has_path = false;
                    }
                }
                PdfOp::CloseStroke => {
                    if has_path {
                        self.draw_vector.close();
                        let c = st.stroke_color;
                        self.draw_vector.set_color(c[0], c[1], c[2], c[3]);
                        self.draw_vector
                            .stroke((st.line_width as f32 * zoom).max(0.5));
                        any = true;
                        has_path = false;
                    }
                }
                PdfOp::FillStroke
                | PdfOp::FillStrokeEvenOdd
                | PdfOp::CloseFillStroke
                | PdfOp::CloseFillStrokeEvenOdd => {
                    if has_path {
                        let c = st.fill_color;
                        self.draw_vector.set_color(c[0], c[1], c[2], c[3]);
                        self.draw_vector.fill();
                        any = true;
                        has_path = false;
                    }
                }
                PdfOp::EndPath | PdfOp::Clip | PdfOp::ClipEvenOdd => {
                    self.draw_vector.clear();
                    has_path = false;
                }

                // Track text state for CTM consistency
                PdfOp::BeginText => {
                    st.text_matrix = [1., 0., 0., 1., 0., 0.];
                    st.text_line_matrix = [1., 0., 0., 1., 0., 0.];
                }
                PdfOp::EndText => {}
                PdfOp::SetFont(n, s) => {
                    st.font_name = n.clone();
                    st.font_size = *s;
                }
                PdfOp::MoveText(tx, ty) => {
                    st.text_line_matrix[4] += tx;
                    st.text_line_matrix[5] += ty;
                    st.text_matrix = st.text_line_matrix;
                }
                PdfOp::MoveTextSetLeading(tx, ty) => {
                    st.text_leading = -ty;
                    st.text_line_matrix[4] += tx;
                    st.text_line_matrix[5] += ty;
                    st.text_matrix = st.text_line_matrix;
                }
                PdfOp::SetTextMatrix(m) => {
                    st.text_matrix = *m;
                    st.text_line_matrix = *m;
                }
                PdfOp::NextLine => {
                    st.text_line_matrix[5] -= st.text_leading;
                    st.text_matrix = st.text_line_matrix;
                }
                PdfOp::SetCharSpacing(v) => st.char_spacing = *v,
                PdfOp::SetWordSpacing(v) => st.word_spacing = *v,
                PdfOp::SetTextLeading(v) => st.text_leading = *v,
                PdfOp::SetTextRise(v) => st.text_rise = *v,
                PdfOp::SetHorizScaling(v) => st.horiz_scaling = *v,
                PdfOp::ShowText(b) => {
                    st.text_matrix[4] += text_advance(&st, page, b);
                }
                PdfOp::ShowTextArray(items) => {
                    for it in items {
                        match it {
                            TextArrayItem::Text(b) => {
                                st.text_matrix[4] += text_advance(&st, page, b);
                            }
                            TextArrayItem::Adjustment(a) => {
                                st.text_matrix[4] -= a / 1000.0 * st.font_size;
                            }
                        }
                    }
                }
                PdfOp::ShowTextNextLine(b) => {
                    st.text_line_matrix[5] -= st.text_leading;
                    st.text_matrix = st.text_line_matrix;
                    st.text_matrix[4] += text_advance(&st, page, b);
                }
                _ => {}
            }
        }
        if any {
            self.draw_vector.end(cx);
        }
    }

    fn render_text(
        &mut self,
        cx: &mut Cx2d,
        ops: &[PdfOp],
        page: &PdfPage,
        ox: f32,
        oy: f32,
        zoom: f32,
        ph: f32,
    ) {
        let mut st = GfxState::default();
        let mut stack: Vec<GfxState> = Vec::new();

        for op in ops {
            match op {
                PdfOp::SaveState => stack.push(st.clone()),
                PdfOp::RestoreState => {
                    if let Some(s) = stack.pop() {
                        st = s;
                    }
                }
                PdfOp::ConcatMatrix(m) => st.ctm = mul_mat(&st.ctm, m),
                PdfOp::SetExtGState(name) => {
                    if let Some(gs) = page.ext_gstate.get(name) {
                        if let Some(a) = gs.ca {
                            st.stroke_alpha = a as f32;
                        }
                        if let Some(a) = gs.ca_lower {
                            st.fill_alpha = a as f32;
                        }
                    }
                }
                PdfOp::SetFillGray(g) => {
                    let v = *g as f32;
                    st.fill_color = [v, v, v, st.fill_alpha];
                }
                PdfOp::SetStrokeGray(g) => {
                    let v = *g as f32;
                    st.stroke_color = [v, v, v, st.stroke_alpha];
                }
                PdfOp::SetFillRgb(r, g, b) => {
                    st.fill_color = [*r as f32, *g as f32, *b as f32, st.fill_alpha]
                }
                PdfOp::SetStrokeRgb(r, g, b) => {
                    st.stroke_color = [*r as f32, *g as f32, *b as f32, st.stroke_alpha]
                }
                PdfOp::SetFillCmyk(c, m, y, k) => {
                    let (r, g, b) = cmyk_to_rgb(*c, *m, *y, *k);
                    st.fill_color = [r, g, b, st.fill_alpha];
                }
                PdfOp::SetStrokeCmyk(c, m, y, k) => {
                    let (r, g, b) = cmyk_to_rgb(*c, *m, *y, *k);
                    st.stroke_color = [r, g, b, st.stroke_alpha];
                }
                PdfOp::SetFillColor(v) => st.fill_color = color_from_vals(v, st.fill_alpha),
                PdfOp::SetStrokeColor(v) => st.stroke_color = color_from_vals(v, st.stroke_alpha),

                PdfOp::BeginText => {
                    st.text_matrix = [1., 0., 0., 1., 0., 0.];
                    st.text_line_matrix = [1., 0., 0., 1., 0., 0.];
                }
                PdfOp::EndText => {}
                PdfOp::SetFont(n, s) => {
                    st.font_name = n.clone();
                    st.font_size = *s;
                }
                PdfOp::MoveText(tx, ty) => {
                    st.text_line_matrix[4] += tx;
                    st.text_line_matrix[5] += ty;
                    st.text_matrix = st.text_line_matrix;
                }
                PdfOp::MoveTextSetLeading(tx, ty) => {
                    st.text_leading = -ty;
                    st.text_line_matrix[4] += tx;
                    st.text_line_matrix[5] += ty;
                    st.text_matrix = st.text_line_matrix;
                }
                PdfOp::SetTextMatrix(m) => {
                    st.text_matrix = *m;
                    st.text_line_matrix = *m;
                }
                PdfOp::NextLine => {
                    st.text_line_matrix[5] -= st.text_leading;
                    st.text_matrix = st.text_line_matrix;
                }
                PdfOp::SetCharSpacing(v) => st.char_spacing = *v,
                PdfOp::SetWordSpacing(v) => st.word_spacing = *v,
                PdfOp::SetTextLeading(v) => st.text_leading = *v,
                PdfOp::SetTextRise(v) => st.text_rise = *v,
                PdfOp::SetHorizScaling(v) => st.horiz_scaling = *v,

                PdfOp::ShowText(bytes) => {
                    self.draw_one_text(cx, &st, page, bytes, ox, oy, zoom, ph);
                    st.text_matrix[4] += text_advance(&st, page, bytes);
                }
                PdfOp::ShowTextArray(items) => {
                    for it in items {
                        match it {
                            TextArrayItem::Text(bytes) => {
                                self.draw_one_text(cx, &st, page, bytes, ox, oy, zoom, ph);
                                st.text_matrix[4] += text_advance(&st, page, bytes);
                            }
                            TextArrayItem::Adjustment(a) => {
                                st.text_matrix[4] -= a / 1000.0 * st.font_size;
                            }
                        }
                    }
                }
                PdfOp::ShowTextNextLine(bytes) => {
                    st.text_line_matrix[5] -= st.text_leading;
                    st.text_matrix = st.text_line_matrix;
                    self.draw_one_text(cx, &st, page, bytes, ox, oy, zoom, ph);
                    st.text_matrix[4] += text_advance(&st, page, bytes);
                }
                _ => {}
            }
        }
    }

    fn pick_draw_text(&mut self, font_name: &str) -> &mut DrawText {
        let name = font_name.to_lowercase();
        if name.contains("courier") || name.contains("mono") || name.contains("code") {
            &mut self.draw_text_code
        } else if name.contains("bold") || name.contains("black") || name.contains("heavy") {
            &mut self.draw_text_bold
        } else {
            &mut self.draw_text
        }
    }

    fn draw_one_text(
        &mut self,
        cx: &mut Cx2d,
        st: &GfxState,
        page: &PdfPage,
        bytes: &[u8],
        ox: f32,
        oy: f32,
        zoom: f32,
        ph: f32,
    ) {
        let font = page.fonts.get(&st.font_name);
        let text = if let Some(f) = font {
            decode_text(f, bytes)
        } else {
            String::from_utf8_lossy(bytes).to_string()
        };
        if text.is_empty() {
            return;
        }

        let (tm, ctm) = (&st.text_matrix, &st.ctm);
        let tx = ctm[0] * tm[4] + ctm[2] * tm[5] + ctm[4];
        let ty = ctm[1] * tm[4] + ctm[3] * tm[5] + ctm[5];
        let sx = ox + tx as f32 * zoom;
        let sy = oy + (ph - ty as f32) * zoom;
        let fs = (st.font_size * tm[3].abs() * ctm[3].abs()) as f32 * zoom;
        if fs < 0.5 || fs > 500.0 {
            return;
        }

        let c = st.fill_color;
        let color = Vec4f {
            x: c[0],
            y: c[1],
            z: c[2],
            w: c[3],
        };
        let dt = self.pick_draw_text(&st.font_name);
        dt.color = color;
        dt.text_style.font_size = fs * 0.75;
        let text_x = sx as f64;
        let text_y = sy as f64 - fs as f64 * 0.8;
        dt.draw_abs(
            cx,
            DVec2 {
                x: text_x,
                y: text_y,
            },
            &text,
        );
    }

    /// Compute the screen-space position and size for a text fragment (without drawing).
    fn text_screen_rect(
        st: &GfxState,
        page: &PdfPage,
        bytes: &[u8],
        text: &str,
        ox: f32,
        oy: f32,
        zoom: f32,
        ph: f32,
    ) -> Option<Rect> {
        if text.is_empty() {
            return None;
        }
        let (tm, ctm) = (&st.text_matrix, &st.ctm);
        let tx = ctm[0] * tm[4] + ctm[2] * tm[5] + ctm[4];
        let ty = ctm[1] * tm[4] + ctm[3] * tm[5] + ctm[5];
        let sx = ox + tx as f32 * zoom;
        let sy = oy + (ph - ty as f32) * zoom;
        let fs = (st.font_size * tm[3].abs() * ctm[3].abs()) as f32 * zoom;
        if fs < 0.5 || fs > 500.0 {
            return None;
        }
        // Compute text width from advance
        let advance = text_advance(st, page, bytes) as f32 * zoom * ctm[0].abs() as f32;
        let text_y = sy as f64 - fs as f64 * 0.8;
        Some(Rect {
            pos: dvec2(sx as f64, text_y),
            size: dvec2(advance as f64, fs as f64),
        })
    }

    /// Walk through ops and record text segments for selection (no drawing).
    fn collect_text_segments(
        &mut self,
        ops: &[PdfOp],
        page: &PdfPage,
        ox: f32,
        oy: f32,
        zoom: f32,
        ph: f32,
    ) {
        let mut st = GfxState::default();
        let mut stack: Vec<GfxState> = Vec::new();

        for op in ops {
            match op {
                PdfOp::SaveState => stack.push(st.clone()),
                PdfOp::RestoreState => {
                    if let Some(s) = stack.pop() {
                        st = s;
                    }
                }
                PdfOp::ConcatMatrix(m) => st.ctm = mul_mat(&st.ctm, m),
                PdfOp::SetExtGState(name) => {
                    if let Some(gs) = page.ext_gstate.get(name) {
                        if let Some(a) = gs.ca {
                            st.stroke_alpha = a as f32;
                        }
                        if let Some(a) = gs.ca_lower {
                            st.fill_alpha = a as f32;
                        }
                    }
                }
                PdfOp::BeginText => {
                    st.text_matrix = [1., 0., 0., 1., 0., 0.];
                    st.text_line_matrix = [1., 0., 0., 1., 0., 0.];
                }
                PdfOp::EndText => {}
                PdfOp::SetFont(n, s) => {
                    st.font_name = n.clone();
                    st.font_size = *s;
                }
                PdfOp::MoveText(tx, ty) => {
                    st.text_line_matrix[4] += tx;
                    st.text_line_matrix[5] += ty;
                    st.text_matrix = st.text_line_matrix;
                }
                PdfOp::MoveTextSetLeading(tx, ty) => {
                    st.text_leading = -ty;
                    st.text_line_matrix[4] += tx;
                    st.text_line_matrix[5] += ty;
                    st.text_matrix = st.text_line_matrix;
                }
                PdfOp::SetTextMatrix(m) => {
                    st.text_matrix = *m;
                    st.text_line_matrix = *m;
                }
                PdfOp::NextLine => {
                    st.text_line_matrix[5] -= st.text_leading;
                    st.text_matrix = st.text_line_matrix;
                }
                PdfOp::SetCharSpacing(v) => st.char_spacing = *v,
                PdfOp::SetWordSpacing(v) => st.word_spacing = *v,
                PdfOp::SetTextLeading(v) => st.text_leading = *v,
                PdfOp::SetTextRise(v) => st.text_rise = *v,
                PdfOp::SetHorizScaling(v) => st.horiz_scaling = *v,

                PdfOp::ShowText(bytes) => {
                    self.collect_one_segment(&st, page, bytes, ox, oy, zoom, ph);
                    st.text_matrix[4] += text_advance(&st, page, bytes);
                }
                PdfOp::ShowTextArray(items) => {
                    for it in items {
                        match it {
                            TextArrayItem::Text(bytes) => {
                                self.collect_one_segment(&st, page, bytes, ox, oy, zoom, ph);
                                st.text_matrix[4] += text_advance(&st, page, bytes);
                            }
                            TextArrayItem::Adjustment(a) => {
                                st.text_matrix[4] -= a / 1000.0 * st.font_size;
                            }
                        }
                    }
                }
                PdfOp::ShowTextNextLine(bytes) => {
                    st.text_line_matrix[5] -= st.text_leading;
                    st.text_matrix = st.text_line_matrix;
                    self.collect_one_segment(&st, page, bytes, ox, oy, zoom, ph);
                    st.text_matrix[4] += text_advance(&st, page, bytes);
                }
                _ => {}
            }
        }
    }

    fn collect_one_segment(
        &mut self,
        st: &GfxState,
        page: &PdfPage,
        bytes: &[u8],
        ox: f32,
        oy: f32,
        zoom: f32,
        ph: f32,
    ) {
        let font = page.fonts.get(&st.font_name);
        let text = if let Some(f) = font {
            decode_text(f, bytes)
        } else {
            String::from_utf8_lossy(bytes).to_string()
        };
        if let Some(rect) = Self::text_screen_rect(st, page, bytes, &text, ox, oy, zoom, ph) {
            self.text_tracker.push(rect, &text);
        }
    }

    fn draw_selection_highlights(&mut self, cx: &mut Cx2d) {
        let start = self.selection_anchor.min(self.selection_cursor);
        let end = self.selection_anchor.max(self.selection_cursor);
        if start == end {
            // Draw cursor caret at the anchor position
            if self.selection_anchor > 0 || self.selection_cursor > 0 {
                if let Some(rect) = self.text_tracker.cursor_rect(self.selection_cursor) {
                    self.draw_cursor.draw_abs(cx, rect);
                }
            }
            return;
        }
        for rect in self.text_tracker.selection_rects(start, end) {
            self.draw_selection.draw_abs(cx, rect);
        }
        // Draw cursor at the cursor end of the selection
        if let Some(rect) = self.text_tracker.cursor_rect(self.selection_cursor) {
            self.draw_cursor.draw_abs(cx, rect);
        }
    }
}

// ---- PdfView: container with PortalList ----

#[derive(Script, ScriptHook, Widget)]
pub struct PdfView {
    #[deref]
    view: View,
    #[live(1.0)]
    zoom: f64,
    #[rust]
    page_cache: Vec<Rc<CachedPage>>,
    #[rust]
    page_count: usize,
    #[rust]
    pdf_data: Option<Vec<u8>>,
}

impl Widget for PdfView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        while let Some(step) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(mut list) = step.as_portal_list().borrow_mut() {
                self.draw_pages(cx, &mut *list);
            }
        }
        DrawStep::done()
    }
}

impl PdfView {
    pub fn load_pdf_data(&mut self, cx: &mut Cx, data: Vec<u8>) {
        self.page_cache.clear();
        self.page_count = 0;

        match PdfDocument::parse(&data) {
            Ok(mut doc) => {
                self.page_count = doc.page_count();
                for i in 0..self.page_count {
                    match doc.page(i) {
                        Ok(page) => {
                            let ops = parse_content_stream(&page.content_data).unwrap_or_default();
                            self.page_cache.push(Rc::new(CachedPage { ops, page }));
                        }
                        Err(_) => {
                            self.page_cache.push(Rc::new(CachedPage {
                                ops: Vec::new(),
                                page: PdfPage::default(),
                            }));
                        }
                    }
                }
                self.pdf_data = Some(data);
            }
            Err(e) => {
                log!("PDF parse error: {}", e.msg);
            }
        }
        self.view.redraw(cx);
    }

    fn draw_pages(&mut self, cx: &mut Cx2d, list: &mut PortalList) {
        list.set_item_range(cx, 0, self.page_count);
        let zoom = self.zoom;

        while let Some(item_id) = list.next_visible_item(cx) {
            let page_h = if item_id < self.page_cache.len() {
                let h = self.page_cache[item_id].page.height();
                if h > 0.0 {
                    h * zoom
                } else {
                    792.0 * zoom
                }
            } else {
                792.0 * zoom
            };

            let mut item = list.item(cx, item_id, id!(Page));

            // Set the page_view height to match the PDF page
            let height = page_h;
            script_apply_eval!(cx, item, {
                page_view: { height: #(height) }
            });

            // Draw the item — the View contains a PdfPageView.
            // PdfPageView.draw_walk returns make_step(), then we get it here via step(),
            // set page data on it, render, and continue.
            while let Some(step) = item.draw(cx, &mut Scope::empty()).step() {
                if let Some(mut pv) = step.borrow_mut::<PdfPageView>() {
                    if item_id < self.page_cache.len() {
                        pv.render_page(cx, &self.page_cache[item_id], zoom);
                    }
                }
            }
        }
    }
}

impl PdfViewRef {
    pub fn load_pdf(&self, cx: &mut Cx, data: Vec<u8>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.load_pdf_data(cx, data);
        }
    }
}

// ---- Helpers ----

fn text_advance(st: &GfxState, page: &PdfPage, bytes: &[u8]) -> f64 {
    let font = page.fonts.get(&st.font_name);
    let scale = st.font_size / 1000.0 * (st.horiz_scaling / 100.0);
    let mut adv = 0.0;
    if let Some(f) = font {
        let two = matches!(f.encoding, makepad_pdf_parse::page::FontEncoding::Identity)
            || f.subtype == "Type0";
        if two && bytes.len() >= 2 {
            let mut i = 0;
            while i + 1 < bytes.len() {
                let code = ((bytes[i] as u32) << 8) | (bytes[i + 1] as u32);
                adv += char_width(f, code) * scale + st.char_spacing;
                if code == 0x0020 {
                    adv += st.word_spacing;
                }
                i += 2;
            }
        } else {
            for &b in bytes {
                adv += char_width(f, b as u32) * scale + st.char_spacing;
                if b == b' ' {
                    adv += st.word_spacing;
                }
            }
        }
    } else {
        adv = bytes.len() as f64 * 600.0 * scale;
    }
    adv
}

fn mul_mat(a: &[f64; 6], b: &[f64; 6]) -> [f64; 6] {
    [
        a[0] * b[0] + a[1] * b[2],
        a[0] * b[1] + a[1] * b[3],
        a[2] * b[0] + a[3] * b[2],
        a[2] * b[1] + a[3] * b[3],
        a[4] * b[0] + a[5] * b[2] + b[4],
        a[4] * b[1] + a[5] * b[3] + b[5],
    ]
}

fn cmyk_to_rgb(c: f64, m: f64, y: f64, k: f64) -> (f32, f32, f32) {
    (
        ((1.0 - c) * (1.0 - k)) as f32,
        ((1.0 - m) * (1.0 - k)) as f32,
        ((1.0 - y) * (1.0 - k)) as f32,
    )
}

fn color_from_vals(v: &[f64], a: f32) -> [f32; 4] {
    match v.len() {
        0 => [0., 0., 0., a],
        1 => {
            let g = v[0] as f32;
            [g, g, g, a]
        }
        3 => [v[0] as f32, v[1] as f32, v[2] as f32, a],
        4 => {
            let (r, g, b) = cmyk_to_rgb(v[0], v[1], v[2], v[3]);
            [r, g, b, a]
        }
        _ => [0., 0., 0., a],
    }
}
