//! Lane E: `FabSheetView` — the 2D sheets editor.
//!
//! Draws `fab::model::Sheet` items in **paper millimetres** with pan and zoom,
//! a sheet picker along the top, and sheet ↔ model cross-highlighting through
//! `SheetLink`: clicking a drawing element selects it in 3D, and whatever is
//! selected in 3D lights up on the sheet.
//!
//! Two limits, both from the data rather than the drawing (report R5):
//! * real Fab sheets are raster tile pyramids, which `Sheet` cannot express —
//!   until it can, `sheets::fixture` generates plans and an elevation from the
//!   model and the strip says "generated";
//! * `SheetItem::Fill` is drawn solid for axis-aligned rectangles and as an
//!   outline otherwise — there is no polygon rasteriser in the 2D pass, and a
//!   generated sheet only ever fills rectangles.

use crate::api::*;
use crate::sheets::fixture;
use crate::sheets::plan::PlanSettings;
use crate::tools::overlay::DrawToolCard;
use crate::ui::widgets::{FabOverflowTab, FabOverflowTabAction, FabOverflowTabStrip};
use crate::model::{Sheet, SheetItem};
use makepad_widgets::*;

const TAB_H: f64 = 22.0;
const FOOT_H: f64 = 16.0;
/// Points per millimetre at 100 %: a 420 mm page is 420 pt wide.
const MIN_ZOOM: f64 = 0.05;
const MAX_ZOOM: f64 = 12.0;

fn screen_stroke_width(mm: f32, zoom: f64, dpi: f64) -> f64 {
    let zoomed_width = mm as f64 * zoom;
    let paper_weight = zoomed_width / zoom.max(f64::EPSILON);
    let device_px = (paper_weight * 3.0).clamp(1.0, 1.5);
    device_px / dpi.max(1.0)
}

script_mod! {
    use mod.prelude.fab.*
    use mod.widgets.*
    use mod.math.*
    use mod.shader.*
    use mod.draw

    // A flat rectangle. `DrawToolCard` insets by half a pixel and shrinks by
    // one for its border, which erases a poché band thinner than ~1.5 pt —
    // the reason the cut material was not reading as filled.
    mod.draw.DrawSheetFill = mod.std.set_type_default() do #(DrawSheetFill::script_shader(vm)){
        ..mod.draw.DrawQuad
        color: vec4(0.4, 0.4, 0.4, 1.0)
        pixel: fn() {
            return vec4(self.color.xyz * self.color.w, self.color.w)
        }
    }

    // Sheet strokes are specified in logical coordinates by Rust after view
    // zoom and DPI have been removed. The AA ramp must use the same device
    // scale or a one-device-pixel hairline still blooms on HiDPI screens.
    mod.draw.DrawSheetLine = mod.std.set_type_default() do #(DrawSheetLine::script_shader(vm)){
        ..mod.draw.DrawQuad
        color: vec4(1.0, 1.0, 1.0, 1.0)
        line_a: vec2(0.0, 0.0)
        line_b: vec2(1.0, 1.0)
        line_width: 1.0
        dash: 0.0
        pixel: fn() {
            let p = self.pos * self.rect_size
            let ba = self.line_b - self.line_a
            let pa = p - self.line_a
            let h = clamp(dot(pa, ba) / max(dot(ba, ba), 0.0001), 0.0, 1.0)
            let d = length(pa - ba * h)
            let aa = 0.7 / max(self.draw_pass.dpi_factor, 1.0)
            let cover = 1.0 - smoothstep(self.line_width * 0.5 - aa, self.line_width * 0.5 + aa, d)
            let t = h * length(ba)
            let period = max(self.dash, 0.0001)
            let on = 1.0 - step(0.5, self.dash) * (1.0 - step(fract(t / period), 0.58))
            let a = cover * on * self.color.w
            return vec4(self.color.xyz * a, a)
        }
    }

    mod.widgets.FabSheetViewBase = #(FabSheetView::register_widget(vm))
    mod.widgets.FabSheetView = set_type_default() do mod.widgets.FabSheetViewBase{
        width: Fill
        height: Fill
        follow_selection: true
        color_bg: fab.color_editor
        color_strip: fab.color_header
        color_text: fab.color_text
        color_dim: fab.color_text_dim
        color_select: fab.color_vp_select
        color_hover: fab.color_vp_hover
        color_accent: fab.color_accent
        draw_paper: mod.draw.DrawToolCard{
            color: #xf4f2ee
            border_color: #x9a9a9a
            radius: 1.0
        }
        draw_fill: mod.draw.DrawSheetFill{}
        draw_chip: mod.draw.DrawToolCard{
            color: fab.color_button
            border_color: fab.color_border
            radius: fab.radius
        }
        draw_line: mod.draw.DrawSheetLine{}
        draw_text: mod.draw.DrawText{
            text_style: theme.font_regular{
                font_size: fab.font_size_small
            }
            color: fab.color_text
        }
        draw_ink: mod.draw.DrawText{
            text_style: theme.font_regular{
                font_size: fab.font_size_small
            }
            color: #x1a1a1a
        }
        tab_strip: FabOverflowTabStrip{
            height: 22
            color_bg: vec4(0.0, 0.0, 0.0, 0.0)
        }
    }
}

#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawSheetFill {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    pub color: Vec4f,
}

#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawSheetLine {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    color: Vec4f,
    #[live]
    line_a: Vec2f,
    #[live]
    line_b: Vec2f,
    #[live(1.0)]
    line_width: f32,
    #[live(0.0)]
    dash: f32,
}

#[derive(Script, ScriptHook, WidgetRef, WidgetRegister)]
pub struct FabSheetView {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    /// Switch to the sheet that carries the active element when the selection
    /// changes elsewhere.
    #[live(true)]
    follow_selection: bool,
    #[live]
    color_bg: Vec4f,
    #[live]
    color_strip: Vec4f,
    #[live]
    color_text: Vec4f,
    #[live]
    color_dim: Vec4f,
    #[live]
    color_select: Vec4f,
    #[live]
    color_hover: Vec4f,
    #[live]
    color_accent: Vec4f,
    #[live]
    draw_paper: DrawToolCard,
    #[live]
    draw_fill: DrawSheetFill,
    #[live]
    draw_chip: DrawToolCard,
    #[live]
    draw_line: DrawSheetLine,
    #[live]
    draw_text: DrawText,
    #[live]
    draw_ink: DrawText,
    #[live]
    tab_strip: WidgetRef,
    #[rust]
    area: Area,
    /// Sheets for the current scene, rebuilt when `scene_revision` moves.
    #[rust]
    sheets: Vec<Sheet>,
    #[rust]
    cached_revision: Option<u64>,
    #[rust]
    generated: bool,
    /// Points per paper millimetre.
    #[rust]
    zoom: f64,
    /// Screen position of the paper's bottom-left corner.
    #[rust]
    pan: DVec2,
    #[rust]
    fitted_for: Option<(usize, u64)>,
    #[rust]
    links: Vec<(Rect, ElementId)>,
    #[rust]
    hover: Option<ElementId>,
    #[rust]
    drag: Option<DVec2>,
    #[rust]
    down_at: Option<DVec2>,
    #[rust]
    last_active: Option<ElementId>,
    #[rust]
    canvas: Rect,
    /// How the plans are cut. The strip's slider drives `cut_height`.
    #[rust]
    plan_settings: PlanSettings,
    #[rust]
    cached_cut: Option<f32>,
    #[rust]
    slider: Rect,
    #[rust]
    sliding: bool,
}

fn as_rect(points: &[[f32; 2]]) -> Option<([f32; 2], [f32; 2])> {
    if points.len() != 4 && points.len() != 5 {
        return None;
    }
    let p = &points[..4];
    let xs: Vec<f32> = p.iter().map(|q| q[0]).collect();
    let ys: Vec<f32> = p.iter().map(|q| q[1]).collect();
    let (x0, x1) = (
        xs.iter().cloned().fold(f32::MAX, f32::min),
        xs.iter().cloned().fold(f32::MIN, f32::max),
    );
    let (y0, y1) = (
        ys.iter().cloned().fold(f32::MAX, f32::min),
        ys.iter().cloned().fold(f32::MIN, f32::max),
    );
    // Every corner must sit on the bounding box for this to be a rectangle.
    for q in p {
        let on_x = (q[0] - x0).abs() < 1e-4 || (q[0] - x1).abs() < 1e-4;
        let on_y = (q[1] - y0).abs() < 1e-4 || (q[1] - y1).abs() < 1e-4;
        if !on_x || !on_y {
            return None;
        }
    }
    Some(([x0, y0], [x1, y1]))
}

impl FabSheetView {
    fn active_index(&self, state: &AppState) -> usize {
        state
            .ui
            .active_sheet
            .and_then(|id| self.sheets.iter().position(|s| s.id == id))
            .unwrap_or(0)
    }

    fn ensure_sheets(&mut self, state: &AppState) {
        if self.cached_revision == Some(state.scene_revision)
            && self.cached_cut == Some(self.plan_settings.cut_height)
        {
            return;
        }
        self.cached_revision = Some(state.scene_revision);
        self.cached_cut = Some(self.plan_settings.cut_height);
        self.sheets = fixture::sheets_for(&state.scene, &self.plan_settings);
        self.generated = fixture::is_generated(&state.scene);
        self.fitted_for = None;
        self.links.clear();
        self.hover = None;
    }

    /// Paper mm → screen points.
    fn to_screen(&self, mm: [f32; 2]) -> DVec2 {
        dvec2(
            self.pan.x + mm[0] as f64 * self.zoom,
            self.pan.y - mm[1] as f64 * self.zoom,
        )
    }

    fn fit(&mut self, sheet: &Sheet) {
        let r = self.canvas;
        let w = sheet.size_mm[0].max(1.0) as f64;
        let h = sheet.size_mm[1].max(1.0) as f64;
        let z = ((r.size.x - 16.0) / w).min((r.size.y - 16.0) / h).max(MIN_ZOOM);
        self.zoom = z.clamp(MIN_ZOOM, MAX_ZOOM);
        let pw = w * self.zoom;
        let ph = h * self.zoom;
        self.pan = dvec2(
            r.pos.x + (r.size.x - pw) * 0.5,
            r.pos.y + (r.size.y + ph) * 0.5,
        );
    }

    fn zoom_at(&mut self, cursor: DVec2, factor: f64) {
        let next = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        if (next - self.zoom).abs() < 1e-9 {
            return;
        }
        // Keep the paper point under the cursor put.
        let mm_x = (cursor.x - self.pan.x) / self.zoom;
        let mm_y = (self.pan.y - cursor.y) / self.zoom;
        self.zoom = next;
        self.pan = dvec2(cursor.x - mm_x * self.zoom, cursor.y + mm_y * self.zoom);
    }

    fn seg(&mut self, cx: &mut Cx2d, a: DVec2, b: DVec2, color: Vec4f, width: f64, dash: bool) {
        self.seg_dash(cx, a, b, color, width, if dash { 8.0 } else { 0.0 })
    }

    /// `dash_px` is the dash period in points; 0 draws a solid line.
    fn seg_dash(&mut self, cx: &mut Cx2d, a: DVec2, b: DVec2, color: Vec4f, width: f64, dash_px: f64) {
        let pad = width + 3.0;
        let pos = dvec2(a.x.min(b.x) - pad, a.y.min(b.y) - pad);
        let size = dvec2((a.x - b.x).abs() + pad * 2.0, (a.y - b.y).abs() + pad * 2.0);
        if !size.x.is_finite() || !size.y.is_finite() || size.x > 12000.0 || size.y > 12000.0 {
            return;
        }
        self.draw_line.color = color;
        self.draw_line.line_a = vec2((a.x - pos.x) as f32, (a.y - pos.y) as f32);
        self.draw_line.line_b = vec2((b.x - pos.x) as f32, (b.y - pos.y) as f32);
        self.draw_line.line_width = width as f32;
        self.draw_line.dash = dash_px as f32;
        self.draw_line.draw_abs(cx, Rect { pos, size });
    }

    fn poly(&mut self, cx: &mut Cx2d, pts: &[[f32; 2]], closed: bool, color: Vec4f, width: f64, dash_px: f64) {
        if pts.len() < 2 {
            return;
        }
        let s: Vec<DVec2> = pts.iter().map(|p| self.to_screen(*p)).collect();
        for w in s.windows(2) {
            self.seg_dash(cx, w[0], w[1], color, width, dash_px);
        }
        if closed && s.len() > 2 {
            self.seg_dash(cx, s[s.len() - 1], s[0], color, width, dash_px);
        }
        // A polyline of separate quads leaves a notch at every corner once the
        // line is more than a pixel or two wide; a dot on each interior vertex
        // is the cheapest correct round join.
        if width > 1.6 && dash_px == 0.0 {
            let first = if closed { 0 } else { 1 };
            let last = if closed { s.len() } else { s.len() - 1 };
            for i in first..last {
                self.draw_marker_dot(cx, s[i], width * 0.5, color);
            }
        }
    }

    fn draw_marker_dot(&mut self, cx: &mut Cx2d, p: DVec2, r: f64, color: Vec4f) {
        self.draw_line.color = color;
        self.draw_line.line_a = vec2(r as f32, r as f32);
        self.draw_line.line_b = vec2(r as f32, r as f32);
        self.draw_line.line_width = (r * 2.0) as f32;
        self.draw_line.dash = 0.0;
        self.draw_line.draw_abs(
            cx,
            Rect { pos: p - dvec2(r + 2.0, r + 2.0), size: dvec2(r * 2.0 + 4.0, r * 2.0 + 4.0) },
        );
    }

    fn rect_on_paper(&self, a: [f32; 2], b: [f32; 2]) -> Rect {
        let p0 = self.to_screen(a);
        let p1 = self.to_screen(b);
        Rect {
            pos: dvec2(p0.x.min(p1.x), p0.y.min(p1.y)),
            size: dvec2((p1.x - p0.x).abs(), (p1.y - p0.y).abs()),
        }
    }

    fn visible(&self, r: &Rect) -> bool {
        let c = self.canvas;
        r.pos.x < c.pos.x + c.size.x
            && r.pos.y < c.pos.y + c.size.y
            && r.pos.x + r.size.x > c.pos.x
            && r.pos.y + r.size.y > c.pos.y
    }

    /// Paper weights become a fixed device-pixel hierarchy. Undoing the view
    /// zoom here is what keeps a fitted-out page from turning every cut edge
    /// into a heavy band; dividing by DPI converts the final device pixels
    /// back to the logical coordinates consumed by `DrawSheetLine`.
    fn stroke_width(&self, mm: f32, dpi: f64) -> f64 {
        screen_stroke_width(mm, self.zoom, dpi)
    }

    /// A dash length in paper millimetres → the period in points.
    fn dash_px(&self, mm: f32, dpi: f64) -> f64 {
        if mm <= 0.0 {
            0.0
        } else {
            (mm as f64 * self.zoom * 1.6).max(3.0 / dpi.max(1.0))
        }
    }

    fn draw_items(&mut self, cx: &mut Cx2d, sheet: &Sheet) {
        let dpi = cx.current_dpi_factor();
        let items = sheet.items.clone();
        for item in &items {
            match item {
                SheetItem::Fill { points, color, stroke } => {
                    let c = vec4(color[0], color[1], color[2], color[3]);
                    if let Some((a, b)) = as_rect(points) {
                        let r = self.rect_on_paper(a, b);
                        if self.visible(&r) {
                            // Round up to a visible band: a wall must read as
                            // solid at every zoom, never dither away.
                            let r = Rect {
                                pos: r.pos,
                                size: dvec2(r.size.x.max(0.8), r.size.y.max(0.8)),
                            };
                            self.draw_fill.color = c;
                            self.draw_fill.draw_abs(cx, r);
                        }
                    } else {
                        // No polygon rasteriser in the 2D pass: outline it.
                        self.poly(cx, points, true, c, 1.0 / dpi.max(1.0), 0.0);
                    }
                    if let Some(s) = stroke {
                        let col = vec4(s.color[0], s.color[1], s.color[2], s.color[3]);
                        let w = self.stroke_width(s.width_mm, dpi);
                        let d = self.dash_px(s.dash[0], dpi);
                        self.poly(cx, points, true, col, w, d);
                    }
                }
                SheetItem::Path { points, closed, stroke } => {
                    let col = vec4(stroke.color[0], stroke.color[1], stroke.color[2], stroke.color[3]);
                    let w = self.stroke_width(stroke.width_mm, dpi);
                    let d = self.dash_px(stroke.dash[0], dpi);
                    self.poly(cx, points, *closed, col, w, d);
                }
                SheetItem::Arc {
                    center,
                    radius,
                    start_deg,
                    end_deg,
                    stroke,
                } => {
                    let steps = 24;
                    let mut pts = Vec::with_capacity(steps + 1);
                    for i in 0..=steps {
                        let t = i as f32 / steps as f32;
                        let a = (start_deg + (end_deg - start_deg) * t).to_radians();
                        pts.push([center[0] + a.cos() * radius, center[1] + a.sin() * radius]);
                    }
                    let col = vec4(stroke.color[0], stroke.color[1], stroke.color[2], stroke.color[3]);
                    let w = self.stroke_width(stroke.width_mm, dpi);
                    let d = self.dash_px(stroke.dash[0], dpi);
                    self.poly(cx, &pts, false, col, w, d);
                }
                SheetItem::Hatch { points, color, .. } => {
                    let col = vec4(color[0], color[1], color[2], color[3] * 0.8);
                    let hairline = 1.0 / dpi.max(1.0);
                    self.poly(cx, points, true, col, hairline, 0.0);
                    // A 45° hatch across the loop's bounding box reads as a
                    // hatch without a scanline fill.
                    if let Some((a, b)) = as_rect(points) {
                        let step = 3.0f32;
                        let mut x = a[0];
                        while x < b[0] + (b[1] - a[1]) {
                            let p0 = [x.min(b[0]), a[1] + (x - b[0]).max(0.0)];
                            let p1 = [
                                (x - (b[1] - a[1])).max(a[0]),
                                (a[1] + (x - a[0]).min(b[1] - a[1])).min(b[1]),
                            ];
                            self.poly(cx, &[p0, p1], false, col, hairline, 0.0);
                            x += step;
                        }
                    }
                }
                SheetItem::Text {
                    pos,
                    text,
                    height_mm,
                    color,
                    ..
                } => {
                    let px = *height_mm as f64 * self.zoom;
                    if px < 4.0 {
                        continue;
                    }
                    let p = self.to_screen(*pos);
                    if p.x < self.canvas.pos.x - 200.0
                        || p.x > self.canvas.pos.x + self.canvas.size.x
                        || p.y < self.canvas.pos.y - 40.0
                        || p.y > self.canvas.pos.y + self.canvas.size.y
                    {
                        continue;
                    }
                    // Set the real font size rather than scaling a rasterised
                    // atlas: the glyphs stay crisp at every zoom.
                    // font_size is points and a cap height is ~0.72 of it.
                    self.draw_ink.text_style.font_size = (px / 0.72 * 0.75).clamp(3.0, 120.0) as f32;
                    self.draw_ink.color = vec4(color[0], color[1], color[2], color[3]);
                    self.draw_ink.draw_abs(cx, dvec2(p.x, p.y - px), text);
                }
            }
        }
    }

    fn draw_links(&mut self, cx: &mut Cx2d, sheet: &Sheet, state: &AppState) {
        let dpi = cx.current_dpi_factor().max(1.0);
        self.links.clear();
        let sel = &state.scene_state.selection;
        let links = sheet.links.clone();
        for l in &links {
            let r = self.rect_on_paper([l.rect_mm[0], l.rect_mm[1]], [l.rect_mm[2], l.rect_mm[3]]);
            if !self.visible(&r) {
                continue;
            }
            self.links.push((r, l.element));
            let selected = sel.contains(l.element);
            let active = sel.active == Some(l.element);
            let hovered = self.hover == Some(l.element);
            if !selected && !hovered {
                continue;
            }
            let color = if hovered && !selected {
                self.color_hover
            } else if active {
                self.color_select
            } else {
                vec4(
                    self.color_select.x,
                    self.color_select.y,
                    self.color_select.z,
                    0.65,
                )
            };
            let w = if active { 2.0 / dpi } else { 1.4 / dpi };
            let (a, b) = (r.pos, r.pos + r.size);
            self.seg(cx, a, dvec2(b.x, a.y), color, w, false);
            self.seg(cx, dvec2(b.x, a.y), b, color, w, false);
            self.seg(cx, b, dvec2(a.x, b.y), color, w, false);
            self.seg(cx, dvec2(a.x, b.y), a, color, w, false);
        }
    }

    fn cut_from_x(&self, x: f64) -> f32 {
        const CUT_MIN: f32 = 0.30;
        const CUT_MAX: f32 = 2.70;
        let r = self.slider;
        if r.size.x <= 1.0 {
            return self.plan_settings.cut_height;
        }
        let t = ((x - r.pos.x) / r.size.x).clamp(0.0, 1.0) as f32;
        CUT_MIN + t * (CUT_MAX - CUT_MIN)
    }

    fn draw_strip(&mut self, cx: &mut Cx2d, scope: &mut Scope, rect: Rect, active: usize) {
        self.draw_chip.color = self.color_strip;
        self.draw_chip.border_color = vec4(0.0, 0.0, 0.0, 0.0);
        self.draw_chip.draw_abs(
            cx,
            Rect {
                pos: rect.pos,
                size: dvec2(rect.size.x, TAB_H),
            },
        );
        let slider_w = 148.0;
        self.slider = Rect {
            pos: dvec2(rect.pos.x + rect.size.x - slider_w + 36.0, rect.pos.y + 8.0),
            size: dvec2(72.0, 6.0),
        };
        let tabs = self
            .sheets
            .iter()
            .map(|sheet| {
                FabOverflowTab::new(
                    sheet.name.clone(),
                    format!("Open {} sheet", sheet.name),
                )
            })
            .collect();
        if let Some(mut strip) = self.tab_strip.borrow_mut::<FabOverflowTabStrip>() {
            strip.set_tabs(cx, tabs, active);
        }
        let _ = self.tab_strip.draw_walk(
            cx,
            scope,
            Walk {
                abs_pos: Some(rect.pos),
                width: Size::Fixed((rect.size.x - slider_w).max(0.0)),
                height: Size::Fixed(TAB_H),
                ..Walk::default()
            },
        );
        // Cut-height slider, right of the tabs.
        const CUT_MIN: f32 = 0.30;
        const CUT_MAX: f32 = 2.70;
        let label = format!("cut {:.2} m", self.plan_settings.cut_height);
        self.draw_text.color = self.color_dim;
        self.draw_text.draw_abs(
            cx,
            dvec2(rect.pos.x + rect.size.x - 148.0, rect.pos.y + 5.0),
            &label,
        );
        let track = self.slider;
        self.draw_chip.color = vec4(0.18, 0.18, 0.18, 1.0);
        self.draw_chip.border_color = vec4(0.09, 0.09, 0.09, 1.0);
        self.draw_chip.draw_abs(cx, track);
        let t = ((self.plan_settings.cut_height - CUT_MIN) / (CUT_MAX - CUT_MIN)).clamp(0.0, 1.0) as f64;
        let kx = track.pos.x + t * track.size.x;
        self.draw_chip.color = self.color_accent;
        self.draw_chip.draw_abs(
            cx,
            Rect {
                pos: dvec2(kx - 4.0, track.pos.y - 3.0),
                size: dvec2(8.0, track.size.y + 6.0),
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sheet_strokes_are_constant_device_pixels() {
        for zoom in [MIN_ZOOM, 0.25, 1.0, MAX_ZOOM] {
            assert!((screen_stroke_width(0.15, zoom, 2.0) * 2.0 - 1.0).abs() < 1e-9);
            assert!((screen_stroke_width(0.50, zoom, 2.0) * 2.0 - 1.5).abs() < 1e-9);
            assert!(screen_stroke_width(10.0, zoom, 2.0) * 2.0 <= 1.5);
        }
    }
}

impl WidgetNode for FabSheetView {
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
}

impl Widget for FabSheetView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.tab_strip.handle_event(cx, event, scope);
        if let Event::Actions(actions) = event {
            if let Some(item) = actions.find_widget_action(self.tab_strip.widget_uid()) {
                if let FabOverflowTabAction::Selected(index) = item.cast() {
                    if let Some(sheet) = self.sheets.get(index) {
                        cx.action(ShellAction::SelectSheet(Some(sheet.id)));
                    }
                    self.area.redraw(cx);
                    return;
                }
            }
        }
        let tab_rect = self.tab_strip.area().rect(cx);
        match event.hits(cx, self.area) {
            Hit::FingerDown(fe) => {
                if tab_rect.contains(fe.abs) {
                    self.down_at = None;
                    return;
                }
                self.down_at = Some(fe.abs);
                let hit_slider = {
                    let r = self.slider;
                    Rect {
                        pos: dvec2(r.pos.x - 8.0, r.pos.y - 6.0),
                        size: dvec2(r.size.x + 16.0, r.size.y + 12.0),
                    }
                    .contains(fe.abs)
                };
                if hit_slider {
                    self.sliding = true;
                    self.plan_settings.cut_height = self.cut_from_x(fe.abs.x);
                    self.cached_cut = None;
                    self.down_at = None;
                    self.area.redraw(cx);
                    return;
                }
                self.drag = Some(fe.abs);
                cx.set_cursor(MouseCursor::Grabbing);
            }
            Hit::FingerMove(fe) => {
                if tab_rect.contains(fe.abs) {
                    return;
                }
                if self.sliding {
                    self.plan_settings.cut_height = self.cut_from_x(fe.abs.x);
                    self.cached_cut = None;
                    self.area.redraw(cx);
                    return;
                }
                if let Some(last) = self.drag {
                    self.pan += fe.abs - last;
                    self.drag = Some(fe.abs);
                    self.area.redraw(cx);
                }
            }
            Hit::FingerUp(fe) => {
                if tab_rect.contains(fe.abs) {
                    self.drag = None;
                    self.down_at = None;
                    return;
                }
                if self.sliding {
                    self.sliding = false;
                    self.plan_settings.cut_height = self.cut_from_x(fe.abs.x);
                    self.cached_cut = None;
                    self.area.redraw(cx);
                    return;
                }
                let click = self
                    .down_at
                    .map(|d| (fe.abs - d).length() < 4.0)
                    .unwrap_or(false);
                self.drag = None;
                self.down_at = None;
                cx.set_cursor(MouseCursor::Default);
                if click {
                    // Smallest link under the cursor wins, so a door inside a
                    // wall is reachable.
                    let mut best: Option<(f64, ElementId)> = None;
                    for (r, id) in &self.links {
                        if r.contains(fe.abs) {
                            let a = r.size.x * r.size.y;
                            if best.map_or(true, |b| a < b.0) {
                                best = Some((a, *id));
                            }
                        }
                    }
                    match best {
                        Some((_, id)) => {
                            cx.action(ShellAction::SelectOnly(id));
                            cx.action(ShellAction::RevealInOutliner(id));
                        }
                        None => {
                            if fe.tap_count >= 2 {
                                self.fitted_for = None;
                                self.area.redraw(cx);
                            }
                        }
                    }
                }
            }
            Hit::FingerHoverIn(fh) | Hit::FingerHoverOver(fh) => {
                if tab_rect.contains(fh.abs) {
                    return;
                }
                let mut hit = None;
                let mut best = f64::MAX;
                for (r, id) in &self.links {
                    if r.contains(fh.abs) {
                        let a = r.size.x * r.size.y;
                        if a < best {
                            best = a;
                            hit = Some(*id);
                        }
                    }
                }
                if hit != self.hover {
                    self.hover = hit;
                    cx.action(ShellAction::HoverElement(hit));
                    self.area.redraw(cx);
                }
                cx.set_cursor(if hit.is_some() {
                    MouseCursor::Hand
                } else {
                    MouseCursor::Grab
                });
            }
            Hit::FingerHoverOut(_) => {
                if self.hover.is_some() {
                    self.hover = None;
                    cx.action(ShellAction::HoverElement(None));
                    self.area.redraw(cx);
                }
            }
            Hit::FingerScroll(fs) => {
                if tab_rect.contains(fs.abs) {
                    return;
                }
                let factor = (1.0 - fs.scroll.y * 0.0025).clamp(0.5, 2.0);
                self.zoom_at(fs.abs, factor);
                self.area.redraw(cx);
            }
            _ => {}
        }
        let _ = scope;
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, self.layout);
        let rect = cx.turtle().rect();
        if rect.size.x < 4.0 || rect.size.y < 4.0 {
            cx.end_turtle_with_area(&mut self.area);
            return DrawStep::done();
        }
        // Background.
        self.draw_paper.color = self.color_bg;
        self.draw_paper.border_color = vec4(0.0, 0.0, 0.0, 0.0);
        self.draw_paper.draw_abs(cx, rect);

        let mut footer = String::new();
        let mut sheet: Option<Sheet> = None;
        let mut active = 0usize;
        let mut selection_target: Option<usize> = None;
        if let Some(state) = scope.data.get::<AppState>() {
            self.ensure_sheets(state);
            active = self.active_index(state);
            sheet = self.sheets.get(active).cloned();
            let act = state.scene_state.selection.active;
            if self.follow_selection && act != self.last_active {
                self.last_active = act;
                if let Some(id) = act {
                    let on_current = sheet
                        .as_ref()
                        .map(|s| s.links.iter().any(|l| l.element == id))
                        .unwrap_or(false);
                    if !on_current {
                        selection_target = self.sheets.iter().position(|s| {
                            s.links.iter().any(|l| l.element == id)
                        });
                    }
                }
            }
            footer = match &sheet {
                Some(s) => format!(
                    "{}  ·  1:{:.0}  ·  {:.0} × {:.0} mm  ·  {} items{}",
                    s.name,
                    s.scale,
                    s.size_mm[0],
                    s.size_mm[1],
                    s.items.len(),
                    if self.generated { "  ·  generated from the model" } else { "" }
                ),
                None => "No sheets in this model".to_string(),
            };
        }
        if let Some(i) = selection_target {
            if let Some(s) = self.sheets.get(i) {
                cx.action(ShellAction::SelectSheet(Some(s.id)));
            }
        }

        self.canvas = Rect {
            pos: dvec2(rect.pos.x, rect.pos.y + TAB_H),
            size: dvec2(rect.size.x, (rect.size.y - TAB_H - FOOT_H).max(4.0)),
        };
        self.draw_strip(cx, scope, rect, active);

        if let Some(s) = sheet {
            if self.fitted_for != Some((active, self.cached_revision.unwrap_or(0))) {
                self.fitted_for = Some((active, self.cached_revision.unwrap_or(0)));
                self.fit(&s);
            }
            // All absolute paper draws are bounded by the editor's canvas;
            // the tab strip and footer live outside this clip.
            cx.push_clip_rect(self.canvas);
            // Paper.
            let paper = self.rect_on_paper([0.0, 0.0], s.size_mm);
            self.draw_paper.color = vec4(0.957, 0.949, 0.933, 1.0);
            self.draw_paper.border_color = vec4(0.6, 0.6, 0.6, 1.0);
            self.draw_paper.draw_abs(cx, paper);
            self.draw_items(cx, &s);
            {
                let state = scope.data.get::<AppState>();
                if let Some(state) = state {
                    let selection_exists = !state.scene_state.selection.is_empty();
                    let _ = selection_exists;
                    self.draw_links(cx, &s, state);
                }
            }
            cx.pop_clip_rect();
        }

        self.draw_text.color = self.color_dim;
        self.draw_text.draw_abs(
            cx,
            dvec2(rect.pos.x + 8.0, rect.pos.y + rect.size.y - FOOT_H + 2.0),
            &footer,
        );
        if let Some(id) = self.hover {
            let name = scope
                .data
                .get::<AppState>()
                .and_then(|s| s.scene.element(id).map(|e| format!("{} · {}", e.name, e.class.label())));
            if let Some(name) = name {
                self.draw_text.color = self.color_text;
                self.draw_text.draw_abs(
                    cx,
                    dvec2(rect.pos.x + rect.size.x - 8.0 - name.chars().count() as f64 * 5.4, rect.pos.y + rect.size.y - FOOT_H + 2.0),
                    &name,
                );
            }
        }

        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }
}
