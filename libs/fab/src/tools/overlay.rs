//! Lane E: `FabToolOverlay` — everything the tools draw over a viewport.
//!
//! Lane D stacks one of these over each `FabViewport` (`ui/viewport_area.rs`,
//! `body.tool_overlay`). It is a 2D overlay: it never renders geometry, it
//! projects world points with `api::ViewProjector` and draws lines, markers,
//! labels and cards. It also **hosts the N-panel** (`FabToolPanel`) until
//! lane D's real N sidebar lands — set `hosts_panel: false` then.
//!
//! What it draws, all of it from `AppState` + `tools::session`:
//!
//! * finished measurements — dimension line, end ticks, halo label
//! * the measurement being placed — rubber band to the snapped cursor, live
//!   value, and the snap glyph that says *why* the point is where it is
//! * section planes (outline + normal arrow) and the section box (12 edges),
//!   with grab handles that `ToolSet` drags
//! * the exploded-storey diagram (until lane B's per-element lookup can move
//!   the geometry itself)
//! * the sun compass: today's sun path, the current sun, the readout
//! * the box-select rubber band, and the element info card
//!
//! It owns one `NextFrame` for the two animations that are lane E's — the
//! section animate-in and the sun day scrub — and only the overlay of the
//! *active* view drives them, so two viewports never double the clock.

use crate::api::*;
use crate::tools::{explode, info, measure, section, session, snap, sun_study};
use makepad_widgets::*;

script_mod! {
    use mod.prelude.fab.*
    use mod.widgets.*
    use mod.math.*
    use mod.shader.*
    use mod.draw

    // A screen-space segment: one quad over the segment's bounding box, the
    // distance to the line computed in the pixel shader. Dashed when dash > 0.
    mod.draw.DrawToolLine = mod.std.set_type_default() do #(DrawToolLine::script_shader(vm)){
        ..mod.draw.DrawQuad
        color: vec4(1.0, 1.0, 1.0, 1.0)
        line_a: vec2(0.0, 0.0)
        line_b: vec2(1.0, 1.0)
        line_width: 1.5
        dash: 0.0
        pixel: fn() {
            let p = self.pos * self.rect_size
            let ba = self.line_b - self.line_a
            let pa = p - self.line_a
            let h = clamp(dot(pa, ba) / max(dot(ba, ba), 0.0001), 0.0, 1.0)
            let d = length(pa - ba * h)
            let cover = 1.0 - smoothstep(self.line_width * 0.5 - 0.7, self.line_width * 0.5 + 0.7, d)
            let t = h * length(ba)
            let period = max(self.dash, 0.0001)
            let on = 1.0 - step(0.5, self.dash) * (1.0 - step(fract(t / period), 0.58))
            let a = cover * on * self.color.w
            return vec4(self.color.xyz * a, a)
        }
    }

    // Point marker. shape 0 = disc, 1 = diamond, 2 = square.
    mod.draw.DrawToolMarker = mod.std.set_type_default() do #(DrawToolMarker::script_shader(vm)){
        ..mod.draw.DrawQuad
        color: vec4(1.0, 1.0, 1.0, 1.0)
        shape: 0.0
        hollow: 0.0
        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            let w = self.rect_size.x
            let h = self.rect_size.y
            let r = min(w, h) * 0.5 - 1.0
            if self.shape < 0.5 {
                sdf.circle(w * 0.5, h * 0.5, r)
            } else {
                if self.shape < 1.5 {
                    sdf.rotate(0.785398, w * 0.5, h * 0.5)
                    sdf.box(w * 0.5 - r * 0.68, h * 0.5 - r * 0.68, r * 1.36, r * 1.36, 0.5)
                } else {
                    sdf.box(w * 0.5 - r * 0.78, h * 0.5 - r * 0.78, r * 1.56, r * 1.56, 0.5)
                }
            }
            sdf.fill_keep(vec4(self.color.xyz, self.color.w * (1.0 - self.hollow)))
            sdf.stroke(vec4(self.color.xyz, self.color.w), 1.25)
            return sdf.result
        }
    }

    // Backing card for the info panel and the compass.
    mod.draw.DrawToolCard = mod.std.set_type_default() do #(DrawToolCard::script_shader(vm)){
        ..mod.draw.DrawQuad
        color: vec4(0.1, 0.1, 0.1, 0.88)
        border_color: vec4(0.29, 0.29, 0.29, 1.0)
        radius: 4.0
        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, self.radius)
            sdf.fill_keep(self.color)
            sdf.stroke(self.border_color, 1.0)
            return sdf.result
        }
    }

    mod.widgets.FabToolOverlayBase = #(FabToolOverlay::register_widget(vm))
    mod.widgets.FabToolOverlay = set_type_default() do mod.widgets.FabToolOverlayBase{
        width: Fill
        height: Fill
        view: 0
        hosts_panel: true
        color_measure: fab.color_vp_measure
        color_section: fab.color_vp_section
        color_select: fab.color_vp_select
        color_dim: fab.color_text_dim
        color_text: fab.color_vp_text
        color_accent: fab.color_accent
        color_warning: fab.color_warning
        draw_line: mod.draw.DrawToolLine{}
        draw_marker: mod.draw.DrawToolMarker{}
        draw_card: mod.draw.DrawToolCard{
            color: vec4(0.09, 0.09, 0.09, 0.9)
            border_color: fab.color_border_light
            radius: fab.radius
        }
        draw_text: mod.draw.DrawText{
            text_style: theme.font_bold{
                font_size: fab.font_size_small
            }
            color: fab.color_vp_measure
        }
        draw_text_small: mod.draw.DrawText{
            text_style: theme.font_regular{
                font_size: fab.font_size_small
            }
            color: fab.color_text_dim
        }
        panel: FabToolPanel{}
    }
}

#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawToolLine {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    pub color: Vec4f,
    #[live]
    pub line_a: Vec2f,
    #[live]
    pub line_b: Vec2f,
    #[live(1.5)]
    pub line_width: f32,
    #[live(0.0)]
    pub dash: f32,
}

#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawToolMarker {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    pub color: Vec4f,
    #[live(0.0)]
    pub shape: f32,
    #[live(0.0)]
    pub hollow: f32,
}

#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawToolCard {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    pub color: Vec4f,
    #[live]
    pub border_color: Vec4f,
    #[live(4.0)]
    pub radius: f32,
}

/// Marker shapes.
const DISC: f32 = 0.0;
const DIAMOND: f32 = 1.0;
const SQUARE: f32 = 2.0;

/// Inputs that affect a whole civil day's solar path. Clock, turbidity,
/// exposure, and the shadow toggle deliberately do not invalidate it.
#[derive(Clone, Copy, Debug, PartialEq)]
struct SunDayKey {
    date: SkyDate,
    tz_offset: f32,
    latitude: f32,
    longitude: f32,
    north_deg: f32,
}

impl From<SunSettings> for SunDayKey {
    fn from(sun: SunSettings) -> Self {
        Self {
            date: sun.date,
            tz_offset: sun.tz_offset,
            latitude: sun.latitude,
            longitude: sun.longitude,
            north_deg: sun.north_deg,
        }
    }
}

#[derive(Script, ScriptHook, WidgetRef, WidgetRegister)]
pub struct FabToolOverlay {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    /// Index into `AppState::views`.
    #[live(0)]
    view: usize,
    /// Draw the N-panel inside the overlay. Lane D sets this false once the
    /// real N sidebar hosts `FabToolPanel`.
    #[live(true)]
    hosts_panel: bool,
    #[live]
    color_measure: Vec4f,
    #[live]
    color_section: Vec4f,
    #[live]
    color_select: Vec4f,
    #[live]
    color_dim: Vec4f,
    #[live]
    color_text: Vec4f,
    #[live]
    color_accent: Vec4f,
    #[live]
    color_warning: Vec4f,
    #[live]
    draw_line: DrawToolLine,
    #[live]
    draw_marker: DrawToolMarker,
    #[live]
    draw_card: DrawToolCard,
    #[live]
    draw_text: DrawText,
    #[live]
    draw_text_small: DrawText,
    #[live]
    panel: WidgetRef,
    #[rust]
    area: Area,
    #[rust]
    next_frame: NextFrame,
    #[rust]
    last_time: f64,
    #[rust]
    sun_day_key: Option<SunDayKey>,
    #[rust]
    sun_day_path: Vec<(f32, Vec3f)>,
    #[rust]
    sun_day_times: Option<(f32, f32)>,
}

impl FabToolOverlay {
    fn is_active(&self, state: &AppState) -> bool {
        state.active_view.min(state.views.len().saturating_sub(1)) == self.view
    }

    fn show_panel(&self, state: &AppState) -> bool {
        self.hosts_panel
            && self.is_active(state)
            && (state.ui.sidebar_open || state.ui.workspace == Workspace::SunStudy)
    }

    // ---- primitives ---------------------------------------------------

    fn seg(&mut self, cx: &mut Cx2d, a: DVec2, b: DVec2, color: Vec4f, width: f64, dash: bool) {
        self.seg_dash(cx, a, b, color, width, if dash { 9.0 } else { 0.0 })
    }

    /// `dash_px` is the dash period in points; 0 draws a solid line.
    fn seg_dash(&mut self, cx: &mut Cx2d, a: DVec2, b: DVec2, color: Vec4f, width: f64, dash_px: f64) {
        let pad = width + 3.0;
        let pos = dvec2(a.x.min(b.x) - pad, a.y.min(b.y) - pad);
        let size = dvec2(
            (a.x - b.x).abs() + pad * 2.0,
            (a.y - b.y).abs() + pad * 2.0,
        );
        if size.x > 8000.0 || size.y > 8000.0 || !size.x.is_finite() || !size.y.is_finite() {
            return;
        }
        self.draw_line.color = color;
        self.draw_line.line_a = vec2((a.x - pos.x) as f32, (a.y - pos.y) as f32);
        self.draw_line.line_b = vec2((b.x - pos.x) as f32, (b.y - pos.y) as f32);
        self.draw_line.line_width = width as f32;
        self.draw_line.dash = dash_px as f32;
        self.draw_line.draw_abs(cx, Rect { pos, size });
    }

    fn marker(&mut self, cx: &mut Cx2d, p: DVec2, r: f64, color: Vec4f, shape: f32, hollow: bool) {
        if !p.x.is_finite() || !p.y.is_finite() {
            return;
        }
        self.draw_marker.color = color;
        self.draw_marker.shape = shape;
        self.draw_marker.hollow = if hollow { 1.0 } else { 0.0 };
        self.draw_marker.draw_abs(
            cx,
            Rect {
                pos: p - dvec2(r, r),
                size: dvec2(r * 2.0, r * 2.0),
            },
        );
    }

    /// Text with a dark halo, so it reads over any shading mode.
    fn halo_text(&mut self, cx: &mut Cx2d, p: DVec2, text: &str, color: Vec4f) {
        let dark = vec4(0.04, 0.04, 0.04, 0.9);
        self.draw_text.color = dark;
        for (dx, dy) in [(-1.0, 0.0), (1.0, 0.0), (0.0, -1.0), (0.0, 1.0)] {
            self.draw_text.draw_abs(cx, p + dvec2(dx, dy), text);
        }
        self.draw_text.color = color;
        self.draw_text.draw_abs(cx, p, text);
    }

    fn small_text(&mut self, cx: &mut Cx2d, p: DVec2, text: &str, color: Vec4f) {
        self.draw_text_small.color = color;
        self.draw_text_small.draw_abs(cx, p, text);
    }

    /// Project a world point, applying the element's explode offset so the
    /// overlay stays glued to where lane B draws the geometry.
    fn polyline(&mut self, cx: &mut Cx2d, pts: &[DVec2], closed: bool, color: Vec4f, width: f64, dash: bool) {
        if pts.len() < 2 {
            return;
        }
        for w in pts.windows(2) {
            self.seg(cx, w[0], w[1], color, width, dash);
        }
        if closed && pts.len() > 2 {
            self.seg(cx, pts[pts.len() - 1], pts[0], color, width, dash);
        }
    }

    // ---- measurements --------------------------------------------------

    fn draw_measurements(&mut self, cx: &mut Cx2d, state: &AppState, proj: &ViewProjector) {
        let units = session::with(|s| s.units(&state.scene.units));
        let color = self.color_measure;
        for m in &state.measurements {
            let pts: Vec<DVec2> = m.points.iter().filter_map(|p| proj.project(*p)).collect();
            if pts.len() < m.points.len() || pts.is_empty() {
                continue;
            }
            let closed = m.kind == MeasureKind::Area;
            self.polyline(cx, &pts, closed, color, 1.6, false);
            for p in &pts {
                self.marker(cx, *p, 3.5, color, DISC, false);
            }
            // Extension ticks on a distance, the way a dimension line reads.
            if m.kind == MeasureKind::Distance && pts.len() == 2 {
                let d = pts[1] - pts[0];
                let len = d.length();
                if len > 1.0 {
                    let n = dvec2(-d.y / len, d.x / len) * 4.0;
                    self.seg(cx, pts[0] - n, pts[0] + n, color, 1.6, false);
                    self.seg(cx, pts[1] - n, pts[1] + n, color, 1.6, false);
                }
            }
            let anchor = match m.kind {
                MeasureKind::Angle => pts.get(1).copied(),
                _ => {
                    let mut c = dvec2(0.0, 0.0);
                    for p in &pts {
                        c += *p;
                    }
                    Some(c / pts.len() as f64)
                }
            };
            if let Some(a) = anchor {
                let text = measure::format(m.kind, m.value, &units);
                self.halo_text(cx, a + dvec2(7.0, -13.0), &text, color);
            }
        }
    }

    fn draw_draft(&mut self, cx: &mut Cx2d, state: &AppState, proj: &ViewProjector) {
        let (draft_points, draft_snaps, kind, preview) = session::with(|s| {
            (
                s.measure.points.clone(),
                s.measure.snaps.clone(),
                s.measure.kind,
                s.measure.preview,
            )
        });
        let color = self.color_measure;
        let pts: Vec<DVec2> = draft_points.iter().filter_map(|p| proj.project(*p)).collect();
        for (i, p) in pts.iter().enumerate() {
            let shape = draft_snaps
                .get(i)
                .map(|k| snap_shape(*k))
                .unwrap_or(DISC);
            self.marker(cx, *p, 4.0, color, shape, false);
        }
        self.polyline(cx, &pts, false, color, 1.6, false);

        // Rubber band from the last placed point to the snapped cursor.
        if let Some(h) = preview {
            if let Some(s) = proj.project(h.point) {
                if let Some(last) = pts.last().copied() {
                    self.seg(cx, last, s, color, 1.4, true);
                    if kind == MeasureKind::Area && pts.len() >= 2 {
                        self.seg(cx, s, pts[0], color, 1.0, true);
                    }
                    let units = session::with(|s| s.units(&state.scene.units));
                    let value = measure::value_of(kind, &draft_live(&draft_points, h.point, kind));
                    let text = measure::format(kind, value, &units);
                    self.halo_text(cx, s + dvec2(10.0, -16.0), &text, color);
                }
                // The snap glyph: shape + one-letter tag says why it landed.
                self.marker(cx, s, 6.0, self.color_accent, snap_shape(h.kind), true);
                self.small_text(cx, s + dvec2(9.0, 1.0), snap::tag(h.kind), self.color_accent);
            }
        }
    }

    // ---- section --------------------------------------------------------

    fn draw_section(&mut self, cx: &mut Cx2d, state: &AppState, proj: &ViewProjector) {
        let sec = &state.scene_state.section;
        if !sec.enabled {
            return;
        }
        let bounds = state.scene.bounds;
        if aabb_is_empty(&bounds) {
            return;
        }
        let color = self.color_section;
        for p in sec.planes.iter().filter(|p| p.enabled) {
            let quad = section::plane_quad(&p.plane, &bounds);
            let pts: Vec<DVec2> = quad.iter().filter_map(|q| proj.project(*q)).collect();
            if pts.len() == 4 {
                self.polyline(cx, &pts, true, color, 1.4, true);
            }
            // Normal arrow: which half survives.
            let anchor = section::plane_anchor(&p.plane, &bounds);
            let n = section::normal(&p.plane).normalize();
            let tip = anchor + n * (aabb_radius(&bounds) * 0.18);
            if let (Some(a), Some(b)) = (proj.project(anchor), proj.project(tip)) {
                self.seg(cx, a, b, color, 1.6, false);
                let d = b - a;
                let l = d.length().max(1e-6);
                let u = d / l;
                let v = dvec2(-u.y, u.x);
                self.seg(cx, b, b - u * 7.0 + v * 4.0, color, 1.6, false);
                self.seg(cx, b, b - u * 7.0 - v * 4.0, color, 1.6, false);
            }
        }
        if let Some(b) = sec.boxed {
            let corners = section::box_corners(&b);
            let pts: Vec<Option<DVec2>> = corners.iter().map(|c| proj.project(*c)).collect();
            for (i, j) in section::BOX_EDGES {
                if let (Some(a), Some(b)) = (pts[i], pts[j]) {
                    self.seg(cx, a, b, color, 1.3, true);
                }
            }
        }
        // Handles.
        let hover = session::with(|s| s.section_hover);
        for (handle, world, _) in section::handles(sec, &bounds) {
            if let Some(s) = proj.project(world) {
                let on = hover == Some(handle);
                let c = if on { self.color_accent } else { color };
                self.marker(cx, s, if on { 7.0 } else { 5.5 }, c, SQUARE, !on);
            }
        }
    }

    // ---- explode --------------------------------------------------------

    fn draw_explode(&mut self, cx: &mut Cx2d, state: &AppState, proj: &ViewProjector) {
        let ex = state.scene_state.explode;
        if ex.amount <= 0.001 {
            return;
        }
        let color = self.color_dim;
        for (name, b, offset) in explode::story_boxes(&state.scene, &ex) {
            let moved = Aabb {
                min: b.min + offset,
                max: b.max + offset,
            };
            let corners = section::box_corners(&moved);
            let pts: Vec<Option<DVec2>> = corners.iter().map(|c| proj.project(*c)).collect();
            for (i, j) in section::BOX_EDGES {
                if let (Some(a), Some(c)) = (pts[i], pts[j]) {
                    self.seg(cx, a, c, color, 1.0, true);
                }
            }
            if let Some(p) = pts[7] {
                self.small_text(cx, p + dvec2(4.0, -12.0), &name, self.color_text);
            }
        }
    }

    // ---- sun ------------------------------------------------------------

    fn draw_sun_compass(&mut self, cx: &mut Cx2d, state: &AppState, rect: Rect) {
        let sun = state.sun;
        let day_key = SunDayKey::from(sun);
        if self.sun_day_key != Some(day_key) {
            self.sun_day_key = Some(day_key);
            self.sun_day_path = sun_study::day_path(&sun, 0.25);
            self.sun_day_times = sun_study::sun_times(&sun);
        }
        let r = 46.0f64;
        let c = dvec2(rect.pos.x + r + 22.0, rect.pos.y + rect.size.y - r - 34.0);
        self.draw_card.draw_abs(
            cx,
            Rect {
                pos: dvec2(c.x - r - 10.0, c.y - r - 10.0),
                size: dvec2(r * 2.0 + 20.0, r * 2.0 + 38.0),
            },
        );
        // horizon circle
        let dim = self.color_dim;
        let mut ring = Vec::with_capacity(33);
        for i in 0..=32 {
            let a = i as f64 / 32.0 * std::f64::consts::TAU;
            ring.push(c + dvec2(a.sin() * r, -a.cos() * r));
        }
        self.polyline(cx, &ring, false, dim, 1.0, false);
        for (label, ang) in [("N", 0.0), ("E", 90.0), ("S", 180.0), ("W", 270.0)] {
            let a = (ang as f64).to_radians();
            let p = c + dvec2(a.sin() * (r + 1.0), -a.cos() * (r + 1.0));
            let q = c + dvec2(a.sin() * (r - 5.0), -a.cos() * (r - 5.0));
            self.seg(cx, p, q, dim, 1.0, false);
            let t = c + dvec2(a.sin() * (r - 14.0) - 3.0, -a.cos() * (r - 14.0) - 6.0);
            self.small_text(cx, t, label, dim);
        }

        // today's sun path: radius = (90 - altitude) / 90
        let project_sun = |d: Vec3f| -> DVec2 {
            let alt = d.z.clamp(-1.0, 1.0).asin().to_degrees() as f64;
            let az = (d.x.atan2(d.y) as f64).to_degrees().to_radians();
            let rr = ((90.0 - alt) / 90.0).clamp(0.0, 1.0) * r;
            c + dvec2(az.sin() * rr, -az.cos() * rr)
        };
        let path: Vec<DVec2> = self
            .sun_day_path
            .iter()
            .map(|(_, d)| project_sun(*d))
            .collect();
        self.polyline(cx, &path, false, self.color_warning, 1.3, false);

        let d = sun.direction();
        if d.z > 0.0 {
            let p = project_sun(d);
            self.seg(cx, c, p, self.color_warning, 1.0, true);
            self.marker(cx, p, 5.0, self.color_warning, DISC, false);
        }
        let text = sun_study::describe(&sun);
        self.small_text(
            cx,
            dvec2(c.x - r - 4.0, c.y + r + 8.0),
            &text,
            self.color_text,
        );
        if let Some((rise, set)) = self.sun_day_times {
            self.small_text(
                cx,
                dvec2(c.x - r - 4.0, c.y + r + 20.0),
                &format!(
                    "sunrise {}   sunset {}",
                    sun_study::clock(rise),
                    sun_study::clock(set)
                ),
                self.color_dim,
            );
        }
    }

    /// The sun ray that casts the active element's shadow: from the top of its
    /// bounds down to where the shadow lands. Honest even before lane B's
    /// shadow maps follow the sun.
    fn draw_sun_ray(&mut self, cx: &mut Cx2d, state: &AppState, proj: &ViewProjector) {
        let Some(id) = state.scene_state.selection.active else {
            return;
        };
        let Some(top) = info::anchor(&state.scene, id) else {
            return;
        };
        let Some(ground) = sun_study::ground_shadow(&state.sun, top) else {
            return;
        };
        let (Some(a), Some(b)) = (proj.project(top), proj.project(ground)) else {
            return;
        };
        self.seg(cx, a, b, self.color_warning, 1.2, true);
        self.marker(cx, b, 4.0, self.color_warning, DISC, true);
        self.small_text(cx, b + dvec2(7.0, -6.0), "shadow", self.color_warning);
    }

    // ---- info card -------------------------------------------------------

    fn draw_info_card(&mut self, cx: &mut Cx2d, state: &AppState, proj: &ViewProjector, rect: Rect) {
        let Some(id) = state.scene_state.selection.active else {
            return;
        };
        let units = session::with(|s| s.units(&state.scene.units));
        let Some(card) = info::card_for(&state.scene, id, &units) else {
            return;
        };
        let anchor = info::anchor(&state.scene, id).and_then(|p| proj.project(p));
        let w = 232.0;
        let h = 34.0 + card.rows.len() as f64 * 15.0;
        let mut pos = match anchor {
            Some(a) => a + dvec2(16.0, -h * 0.5),
            None => rect.pos + dvec2(rect.size.x - w - 16.0, 60.0),
        };
        pos.x = pos.x.clamp(rect.pos.x + 6.0, rect.pos.x + rect.size.x - w - 6.0);
        pos.y = pos.y.clamp(rect.pos.y + 6.0, rect.pos.y + rect.size.y - h - 6.0);
        if let Some(a) = anchor {
            self.seg(cx, a, pos + dvec2(0.0, h * 0.5), self.color_select, 1.0, true);
            self.marker(cx, a, 4.0, self.color_select, DIAMOND, true);
        }
        self.draw_card.draw_abs(
            cx,
            Rect {
                pos,
                size: dvec2(w, h),
            },
        );
        self.draw_text.color = self.color_text;
        self.draw_text.draw_abs(cx, pos + dvec2(9.0, 6.0), &card.title);
        self.small_text(cx, pos + dvec2(9.0, 19.0), &card.subtitle, self.color_dim);
        let mut y = 36.0;
        for (k, v) in &card.rows {
            self.small_text(cx, pos + dvec2(9.0, y), k, self.color_dim);
            self.small_text(cx, pos + dvec2(96.0, y), v, self.color_text);
            y += 15.0;
        }
    }

    // ---- animation -------------------------------------------------------

    /// Advance the section animate-in and the day scrub. Returns true while
    /// something still wants frames.
    fn animate(&mut self, cx: &mut Cx, state: &mut AppState, dt: f32) -> bool {
        let mut wants = false;

        let step = session::with(|s| {
            let anim = s.section_anim.as_mut()?;
            anim.t = (anim.t + dt / anim.duration.max(1e-3)).min(1.0);
            let done = anim.t >= 1.0;
            let f = section::ease(anim.t);
            let next = section::lerp_section(&anim.from, &anim.to, f);
            if done {
                s.section_anim = None;
            }
            Some((next, done))
        });
        if let Some((next, done)) = step {
            cx.action(ShellAction::SetSection(next));
            wants |= !done;
        }

        let play = session::with(|s| s.sun_play);
        if play.playing && play.owner == self.view {
            let speed = if play.speed > 0.0 {
                play.speed
            } else {
                sun_study::PLAY_HOURS_PER_SECOND
            };
            let next = sun_study::advance(&state.sun, dt, speed);
            cx.action(ShellAction::SetSun(next));
            wants = true;
        }
        wants
    }
}

/// Marker shape for a snap kind, so the glyph says what it caught.
fn snap_shape(kind: SnapKind) -> f32 {
    match kind {
        SnapKind::Vertex => SQUARE,
        SnapKind::EdgeMidpoint => DIAMOND,
        SnapKind::Edge => DIAMOND,
        SnapKind::Face => DISC,
        SnapKind::Ground => DISC,
    }
}

/// The point list a live value is computed from (draft + cursor).
fn draft_live(points: &[Vec3f], preview: Vec3f, _kind: MeasureKind) -> Vec<Vec3f> {
    let mut v = points.to_vec();
    v.push(preview);
    v
}

impl WidgetNode for FabToolOverlay {
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
        self.panel.redraw(cx);
    }
}

impl Widget for FabToolOverlay {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if let Some(ne) = self.next_frame.is_event(event) {
            let dt = if self.last_time > 0.0 {
                (ne.time - self.last_time).clamp(0.0, 0.25) as f32
            } else {
                1.0 / 60.0
            };
            self.last_time = ne.time;
            let mut wants = false;
            if let Some(state) = scope.data.get_mut::<AppState>() {
                wants = self.animate(cx, state, dt);
            }
            if wants {
                self.next_frame = cx.new_next_frame();
            } else {
                self.last_time = 0.0;
            }
        }
        // Insurance for the day scrub's frame chain: while THIS overlay
        // drives the play clock, re-arm from any event. A chain that only
        // re-arms inside its own NextFrame dies the moment one frame is
        // missed (a display-link stall, a redraw that skipped the overlay)
        // — and then the sun only moved while something else redrew the
        // view.
        if session::with(|s| s.sun_play.playing && s.sun_play.owner == self.view) {
            self.next_frame = cx.new_next_frame();
        }
        let show = scope
            .data
            .get::<AppState>()
            .map(|s| self.show_panel(s))
            .unwrap_or(false);
        if show {
            self.panel.handle_event(cx, event, scope);
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, self.layout);
        let rect = cx.turtle().rect();
        let mut show_panel = false;
        let mut wants_frames = false;
        if rect.size.x > 2.0 && rect.size.y > 2.0 {
            if let Some(state) = scope.data.get_mut::<AppState>() {
                let state: &AppState = state;
                let vs = state.view_at(self.view);
                let proj = ViewProjector::new(vs.camera, rect);
                let active = self.is_active(state);
                show_panel = self.show_panel(state);
                wants_frames = session::with(|s| s.wants_frames()) && active;

                if vs.overlays.measurements {
                    self.draw_measurements(cx, state, &proj);
                    if matches!(state.tool, Tool::Measure(_)) {
                        self.draw_draft(cx, state, &proj);
                    }
                }
                if vs.overlays.section_planes {
                    self.draw_section(cx, state, &proj);
                }
                self.draw_explode(cx, state, &proj);

                if active {
                    if session::with(|s| s.info_card) {
                        self.draw_info_card(cx, state, &proj, rect);
                    }
                    if show_panel || state.ui.workspace == Workspace::SunStudy {
                        self.draw_sun_compass(cx, state, rect);
                        self.draw_sun_ray(cx, state, &proj);
                    }
                    // Box-select rubber band.
                    if let Some((a, b)) = session::with(|s| s.box_select) {
                        let pts = [a, dvec2(b.x, a.y), b, dvec2(a.x, b.y)];
                        let c = self.color_select;
                        self.polyline(cx, &pts, true, c, 1.0, true);
                    }
                    // Tool feedback under the view label (D owns the two lines
                    // above it; this is the third).
                    let hint = session::with(|s| s.hint.clone());
                    if !hint.is_empty() {
                        let p = rect.pos + dvec2(44.0, 46.0);
                        self.small_text(cx, p, &hint, self.color_measure);
                    }
                }
            }
        }
        if show_panel {
            self.panel.draw_all(cx, scope);
        }
        cx.end_turtle_with_area(&mut self.area);
        if wants_frames {
            self.next_frame = cx.new_next_frame();
        }
        DrawStep::done()
    }
}
