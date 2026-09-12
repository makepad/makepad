//! The plan view: one architecture plan drawn on a camera-scaled canvas —
//! lane bands with their titles, uniform subsystem cards (kind icon, title,
//! summary, budget chip, a kind stripe) and orthogonal relations with
//! arrowheads. Hover reads a card's summary and first story sentence and
//! lights its one-hop neighbourhood (the light-up/dim law), a click pins a
//! card, a drag pans, the wheel zooms about the pointer, F fits, Escape
//! clears. Nothing is drawn at rest: the view redraws only when the plan,
//! the camera, the hover or the selection changed.
use super::plan::{DesignEdgeKind, DesignNodeKind, DesignScene};
use super::{ellipsize_to, load_design, plan_candidates, report_lines, wrap_lines, DesignBundle};
use makepad_widgets::makepad_draw::DrawSvg;
use makepad_widgets::makepad_platform::thread::SignalToUI;
use makepad_widgets::*;
use makepad_workspace::canvas_draw::{DrawCanvasCard, DrawCanvasGrid};
use makepad_workspace::camera::{Camera as PlanCamera, Geometry, MAX_ZOOM, MIN_ZOOM};
use makepad_workspace::presentation::Camera as ScreenCamera;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    set_type_default() do #(DrawPlanPlate::script_shader(vm)){
        ..mod.draw.DrawQuad
        color: theme.color_bg_container
        radius: 4.0
        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            sdf.box(0.0, 0.0, self.rect_size.x, self.rect_size.y, self.radius)
            sdf.fill(self.color)
            return sdf.result
        }
    }

    set_type_default() do #(DrawPlanEdge::script_shader(vm)){
        ..mod.draw.DrawQuad
        start: vec2(0.0, 0.0)
        end: vec2(0.0, 0.0)
        width: 1.5
        color: theme.color_text_disabled
        pattern: 0.0
        dash_period: 10.0
        dash_fill: 0.6
        pixel: fn() {
            let p = self.pos * self.rect_size + self.rect_pos
            let ab = self.end - self.start
            let l2 = max(dot(ab, ab), 0.000001)
            let t = clamp(dot(p - self.start, ab) / l2, 0.0, 1.0)
            let q = self.start + ab * t
            let d = length(p - q)
            let half = self.width * 0.5
            var cover = 1.0 - smoothstep(half - 0.75, half + 0.75, d)
            if self.pattern > 0.5 {
                let along = t * sqrt(l2)
                let ph = fract(along / self.dash_period)
                if ph > self.dash_fill {
                    cover = 0.0
                }
            }
            return vec4(self.color.rgb * self.color.a * cover, self.color.a * cover)
        }
    }

    mod.widgets.ArchitectureView = #(ArchitectureView::register_widget(vm)){
        width: Fill height: Fill
        draw_grid +: {
            cell: 48.0
            color_a: theme.color_bg_app
            color_b: mix(theme.color_bg_app, theme.color_text, 0.025)
        }
        draw_card +: {
            border_radius: 9.0
            shadow_radius: 8.0
        }
        draw_label +: {text_style: theme.font_regular{font_size: 9.0} color: theme.color_text}
        draw_bold +: {text_style: theme.font_bold{font_size: 9.0} color: theme.color_text}
        draw_meta +: {text_style: theme.font_regular{font_size: 7.5} color: theme.color_text_disabled}
        icon_component +: {svg: crate_resource("self:resources/icons/component.svg")}
        icon_thread +: {svg: crate_resource("self:resources/icons/thread.svg")}
        icon_queue +: {svg: crate_resource("self:resources/icons/queue.svg")}
        icon_store +: {svg: crate_resource("self:resources/icons/database.svg")}
        icon_memory +: {svg: crate_resource("self:resources/icons/memory.svg")}
        icon_gpu +: {svg: crate_resource("self:resources/icons/gpu.svg")}
        icon_io +: {svg: crate_resource("self:resources/icons/io.svg")}
        background: theme.color_bg_app
        surface: theme.color_bg_container
        inset: theme.color_inset
        ink: theme.color_text
        muted: theme.color_text_disabled
        focus: theme.color_focus
        shadow: theme.color_shadow
        kind_component: theme.color_map_kind_component
        kind_thread: theme.color_map_kind_thread
        kind_queue: theme.color_map_kind_queue
        kind_store: theme.color_map_kind_store
        kind_memory: theme.color_map_kind_memory
        kind_gpu: theme.color_map_kind_gpu
        kind_io: theme.color_map_kind_io
    }
}

/// A flat rounded plate: lane bands, chips and the hover plate.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawPlanPlate {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    pub color: Vec4f,
    #[live]
    pub radius: f32,
}

/// One relation segment: an anti-aliased line between two screen points
/// inside its padded bounding quad, solid or dashed.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawPlanEdge {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    pub start: Vec2f,
    #[live]
    pub end: Vec2f,
    #[live]
    pub width: f32,
    #[live]
    pub color: Vec4f,
    #[live]
    pub pattern: f32,
    #[live]
    pub dash_period: f32,
    #[live]
    pub dash_fill: f32,
}

/// World units (px at zoom 1) of the typography inside a card.
const TITLE_UNITS: f64 = 17.0;
const SUMMARY_UNITS: f64 = 12.0;
const LANE_TITLE_UNITS: f64 = 14.0;
const CHIP_UNITS: f64 = 10.5;
const EDGE_LABEL_UNITS: f64 = 10.0;
const CARD_PAD: f64 = 12.0;
const ICON_UNITS: f64 = 28.0;
/// Text under this many px is not emitted (the card still reads as a shape).
const MIN_TEXT_PX: f64 = 3.5;
/// Cards, relations and text outside a hovered or selected node's one-hop
/// neighbourhood.
const DIM: f32 = 0.35;
const EDGE_ALPHA: f32 = 0.6;
const EDGE_DIM: f32 = 0.14;
/// A press that travels this far is a pan, not a click.
const DRAG_THRESHOLD_PX: f64 = 4.0;
/// The plan fits with this margin (px).
const FIT_MARGIN: f64 = 28.0;

#[derive(Clone, Debug, Default, PartialEq)]
pub enum ArchitectureViewAction {
    /// The plan loaded, failed or was replaced.
    PlanChanged,
    /// The pinned card changed (or was cleared).
    SelectionChanged,
    #[default]
    None,
}

#[derive(Script, ScriptHook, Widget)]
pub struct ArchitectureView {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_grid: DrawCanvasGrid,
    #[live]
    draw_card: DrawCanvasCard,
    #[live]
    draw_plate: DrawPlanPlate,
    #[live]
    draw_edge: DrawPlanEdge,
    #[live]
    draw_label: DrawText,
    #[live]
    draw_bold: DrawText,
    #[live]
    draw_meta: DrawText,
    #[live]
    icon_component: DrawSvg,
    #[live]
    icon_thread: DrawSvg,
    #[live]
    icon_queue: DrawSvg,
    #[live]
    icon_store: DrawSvg,
    #[live]
    icon_memory: DrawSvg,
    #[live]
    icon_gpu: DrawSvg,
    #[live]
    icon_io: DrawSvg,
    #[live]
    background: Vec4f,
    #[live]
    surface: Vec4f,
    #[live]
    inset: Vec4f,
    #[live]
    ink: Vec4f,
    #[live]
    muted: Vec4f,
    #[live]
    focus: Vec4f,
    #[live]
    shadow: Vec4f,
    #[live]
    kind_component: Vec4f,
    #[live]
    kind_thread: Vec4f,
    #[live]
    kind_queue: Vec4f,
    #[live]
    kind_store: Vec4f,
    #[live]
    kind_memory: Vec4f,
    #[live]
    kind_gpu: Vec4f,
    #[live]
    kind_io: Vec4f,
    /// The workspace root the plans are read from.
    #[rust]
    root: Option<PathBuf>,
    /// The active file (repository-relative) the plan follows.
    #[rust]
    context_path: Option<String>,
    /// The candidates the last load was asked for.
    #[rust]
    requested: Option<Vec<PathBuf>>,
    #[rust]
    camera: PlanCamera,
    #[rust]
    view: Rect,
    #[rust]
    bundle: Option<Arc<DesignBundle>>,
    #[rust]
    error: Option<String>,
    #[rust]
    loading: Option<Receiver<(u64, Result<DesignBundle, String>)>>,
    #[rust]
    job_seq: u64,
    #[rust]
    hover: Option<usize>,
    #[rust]
    selected: Option<usize>,
    #[rust]
    fit_pending: bool,
    /// A press on the map: its start and the camera pan then; `dragging`
    /// once it travelled past the click threshold.
    #[rust]
    press: Option<(DVec2, DVec2)>,
    #[rust]
    dragging: bool,
}

/// The theme role a node kind draws with.
fn kind_index(kind: DesignNodeKind) -> usize {
    match kind {
        DesignNodeKind::Component => 0,
        DesignNodeKind::Thread => 1,
        DesignNodeKind::Queue => 2,
        DesignNodeKind::Store => 3,
        DesignNodeKind::Memory => 4,
        DesignNodeKind::Gpu => 5,
        DesignNodeKind::Io => 6,
    }
}

/// The dash pattern of a relation kind: reads/writes dashed, allocation
/// dotted, the rest solid.
fn edge_pattern(kind: DesignEdgeKind) -> f32 {
    match kind {
        DesignEdgeKind::Reads | DesignEdgeKind::Writes => 1.0,
        DesignEdgeKind::AllocatesFrom => 2.0,
        _ => 0.0,
    }
}

fn with_alpha(c: Vec4f, a: f32) -> Vec4f {
    Vec4f { x: c.x, y: c.y, z: c.z, w: a }
}

fn mix4(a: Vec4f, b: Vec4f, t: f32) -> Vec4f {
    Vec4f { x: a.x + (b.x - a.x) * t, y: a.y + (b.y - a.y) * t, z: a.z + (b.z - a.z) * t, w: a.w + (b.w - a.w) * t }
}

fn geometry(r: Rect) -> Geometry {
    Geometry { x: r.pos.x, y: r.pos.y, w: r.size.x, h: r.size.y }
}

/// The two wings of an arrowhead at `tip`, pointing along `dir`.
fn arrowhead(tip: DVec2, dir: DVec2, size: f64) -> [DVec2; 2] {
    let len = dir.length().max(1e-6);
    let d = dir / len;
    let n = dvec2(-d.y, d.x);
    let back = tip - d * size;
    [back + n * size * 0.45, back - n * size * 0.45]
}

impl ArchitectureView {
    /// The workspace root; the plan reloads when it changes.
    pub fn configure(&mut self, cx: &mut Cx, root: PathBuf) {
        if self.root.as_ref() == Some(&root) {
            return;
        }
        self.root = Some(root);
        self.requested = None;
        self.request_load(cx, true);
    }

    /// The active file the plan follows (repository-relative): the deepest
    /// crate plan on its path, else the platform's.
    pub fn set_context_path(&mut self, cx: &mut Cx, path: Option<String>) {
        if self.context_path == path {
            return;
        }
        self.context_path = path;
        self.request_load(cx, false);
    }

    /// Re-read the current plan (its file changed on disk).
    pub fn reload(&mut self, cx: &mut Cx) {
        self.request_load(cx, true);
    }

    /// Fit the whole plan into the view on the next draw.
    pub fn fit(&mut self, cx: &mut Cx) {
        self.fit_pending = true;
        self.redraw(cx);
    }

    pub fn bundle(&self) -> Option<Arc<DesignBundle>> {
        self.bundle.clone()
    }

    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    /// The side report's lines for the current state.
    pub fn report(&self) -> Vec<String> {
        report_lines(self.bundle.as_deref(), self.error.as_deref(), self.loading.is_some(), self.selected)
    }

    /// One line for the status strip: the plan, its size and its freshness.
    pub fn status_line(&self) -> String {
        match (&self.bundle, &self.error) {
            (Some(b), _) => {
                let s = &b.scene;
                let warn = if b.validation.warnings.is_empty() { String::new() } else { format!(" · {} warning{}", b.validation.warnings.len(), if b.validation.warnings.len() == 1 { "" } else { "s" }) };
                format!("{} · {} · {} subsystems · {} relations · {} lanes · {}{}", s.title, s.scope, s.nodes.len(), s.edges.len(), s.lanes.len(), super::freshness_text(&b.changes), warn)
            }
            (None, Some(e)) => e.clone(),
            (None, None) => "Loading the architecture plan…".into(),
        }
    }

    /// Ask a worker for the plan the context names (or the default);
    /// `force` re-asks even for the same candidates.
    fn request_load(&mut self, cx: &mut Cx, force: bool) {
        let Some(root) = self.root.clone() else {
            self.error = Some("No workspace root".into());
            return;
        };
        let candidates = plan_candidates(self.context_path.as_deref());
        if !force && self.requested.as_ref() == Some(&candidates) {
            return;
        }
        self.requested = Some(candidates.clone());
        self.job_seq += 1;
        let id = self.job_seq;
        let (tx, rx) = channel();
        let spawned = std::thread::Builder::new().name("director-architecture".into()).spawn(move || {
            let result = load_design(root, candidates);
            let _ = tx.send((id, result));
            SignalToUI::set_ui_signal();
        });
        match spawned {
            Ok(_) => {
                self.loading = Some(rx);
                self.error = None;
            }
            Err(e) => self.error = Some(format!("Plan loader thread: {e}")),
        }
        self.redraw(cx);
    }

    /// The worker's answer (on the UI signal).
    fn pump(&mut self, cx: &mut Cx) {
        let Some(rx) = &self.loading else { return };
        let Ok((id, result)) = rx.try_recv() else { return };
        self.loading = None;
        if id != self.job_seq {
            return;
        }
        match result {
            Ok(bundle) => {
                // a different plan than before: the selection is void, the
                // camera refits
                if self.bundle.as_ref().is_none_or(|b| b.path != bundle.path) {
                    self.selected = None;
                    self.hover = None;
                    self.fit_pending = true;
                }
                self.bundle = Some(Arc::new(bundle));
                self.error = None;
            }
            Err(e) => self.error = Some(e),
        }
        cx.widget_action(self.widget_uid(), ArchitectureViewAction::PlanChanged);
        self.redraw(cx);
    }

    fn screen_camera(&self) -> ScreenCamera {
        ScreenCamera::new(self.view, self.camera)
    }

    fn fit_now(&mut self) {
        let Some(b) = self.bundle.as_ref() else { return };
        let bounds = b.scene.bounds;
        let usable = self.view.size - dvec2(FIT_MARGIN * 2.0, FIT_MARGIN * 2.0);
        if usable.x <= 1.0 || usable.y <= 1.0 {
            return;
        }
        let zoom = (usable.x / bounds.size.x.max(1.0)).min(usable.y / bounds.size.y.max(1.0)).clamp(MIN_ZOOM, 1.5);
        let pan = (self.view.size - bounds.size * zoom) * 0.5 - bounds.pos * zoom;
        self.camera = PlanCamera { pan_x: pan.x, pan_y: pan.y, zoom };
        self.fit_pending = false;
    }

    fn zoom_at(&mut self, p: DVec2, factor: f64) {
        let old = self.camera.zoom;
        let new = (old * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        if (new - old).abs() < 1e-9 {
            return;
        }
        let local = p - self.view.pos;
        let pan = dvec2(self.camera.pan_x, self.camera.pan_y);
        let world = (local - pan) / old;
        let next = local - world * new;
        self.camera = PlanCamera { pan_x: next.x, pan_y: next.y, zoom: new };
    }

    /// The node the pointer is over, in the plan's world.
    fn node_at(&self, p: DVec2) -> Option<usize> {
        if !self.view.contains(p) {
            return None;
        }
        let b = self.bundle.as_ref()?;
        let world = self.screen_camera().world_at(p);
        b.scene.node_at(world)
    }

    /// The focus the dim law works from: the selection, else the hover.
    fn focus_node(&self) -> Option<usize> {
        self.selected.or(self.hover)
    }

    /// A kind's colour role (the theme's `color_map_kind_*` roles).
    fn kind_color(&self, kind: DesignNodeKind) -> Vec4f {
        match kind_index(kind) {
            0 => self.kind_component,
            1 => self.kind_thread,
            2 => self.kind_queue,
            3 => self.kind_store,
            4 => self.kind_memory,
            5 => self.kind_gpu,
            _ => self.kind_io,
        }
    }

    /// A text run's width at `px` (bold or regular), measured on the UI
    /// thread: the plan has a few dozen short strings.
    fn measure(&mut self, cx: &mut Cx2d, bold: bool, px: f64, text: &str) -> f64 {
        let draw = if bold { &mut self.draw_bold } else { &mut self.draw_label };
        draw.text_style.font_size = (px * 0.75) as f32;
        draw.prepare_single_line_run(cx, text).map(|r| r.width_in_lpxs as f64).unwrap_or(text.chars().count() as f64 * px * 0.55)
    }

    fn draw_plate(&mut self, cx: &mut Cx2d, r: Rect, color: Vec4f, radius: f32) {
        self.draw_plate.color = color;
        self.draw_plate.radius = radius;
        self.draw_plate.draw_abs(cx, r);
    }

    /// One polyline as anti-aliased segment quads.
    fn draw_polyline(&mut self, cx: &mut Cx2d, pts: &[DVec2], cull: Rect, color: Vec4f, width: f32, alpha: f32, pattern: f32) {
        for w in pts.windows(2) {
            let (a, b) = (w[0], w[1]);
            let len = (b - a).length();
            if len < 0.01 {
                continue;
            }
            let pad = (width as f64 * 0.5 + 2.0).max(2.0);
            let min = dvec2(a.x.min(b.x), a.y.min(b.y)) - dvec2(pad, pad);
            let max = dvec2(a.x.max(b.x), a.y.max(b.y)) + dvec2(pad, pad);
            let quad = Rect { pos: min, size: max - min };
            if !quad.intersects(cull) {
                continue;
            }
            let d = &mut self.draw_edge;
            d.start = vec2(a.x as f32, a.y as f32);
            d.end = vec2(b.x as f32, b.y as f32);
            d.color = with_alpha(color, alpha);
            d.width = width;
            d.pattern = pattern;
            d.dash_period = if pattern > 1.5 { 4.0 } else { 10.0 };
            d.dash_fill = 0.6;
            d.draw_abs(cx, quad);
        }
    }

    fn draw_icon(&mut self, cx: &mut Cx2d, kind: DesignNodeKind, r: Rect, tint: Vec4f) {
        let icon = match kind_index(kind) {
            0 => &mut self.icon_component,
            1 => &mut self.icon_thread,
            2 => &mut self.icon_queue,
            3 => &mut self.icon_store,
            4 => &mut self.icon_memory,
            5 => &mut self.icon_gpu,
            _ => &mut self.icon_io,
        };
        icon.color = tint;
        icon.draw_abs(cx, r);
    }

    /// The structure: lane bands, relations with arrowheads, cards with
    /// their kind stripe, icon and budget chip, then the labels.
    fn draw_plan(&mut self, cx: &mut Cx2d, scene: &DesignScene) {
        let cam = self.screen_camera();
        let z = self.camera.zoom;
        let view = self.view;
        let cull = Rect { pos: view.pos - view.size, size: view.size * 3.0 };
        let focus = self.focus_node();
        let (lit_nodes, lit_edges) = match focus {
            Some(i) => {
                let (n, e) = scene.neighbourhood(i);
                (Some(n), Some(e))
            }
            None => (None, None),
        };
        // lanes: a quiet band with a firmer title strip
        for li in &scene.lane_order {
            let (rect, titled) = if *li < scene.lanes.len() { (scene.lanes[*li].rect, true) } else { (scene.unlaned.unwrap_or_default(), false) };
            let r = cam.screen_rect(geometry(rect));
            if !r.intersects(cull) {
                continue;
            }
            let radius = (12.0 * z).clamp(2.0, 14.0) as f32;
            self.draw_plate(cx, r, with_alpha(mix4(self.background, self.inset, 0.85), 0.9), radius);
            if titled {
                let title_h = super::layout::LANE_TITLE_H * z;
                let strip = Rect { pos: r.pos, size: dvec2(r.size.x, title_h.min(r.size.y)) };
                self.draw_plate(cx, strip, with_alpha(mix4(self.inset, self.ink, 0.06), 0.9), radius);
            }
        }
        // relations beneath the cards, the lit ones on top of the quiet
        let mut order: Vec<usize> = (0..scene.edges.len()).collect();
        order.sort_by_key(|ei| lit_edges.as_ref().is_some_and(|s| s.contains(ei)));
        for ei in order {
            let e = &scene.edges[ei];
            if e.points.len() < 2 {
                continue;
            }
            let lit = lit_edges.as_ref().is_none_or(|s| s.contains(&ei));
            let color = if focus.is_some() && lit { self.kind_color(scene.nodes[e.from].kind) } else { self.muted };
            let alpha = if focus.is_some() && !lit { EDGE_DIM } else if lit && focus.is_some() { 0.95 } else { EDGE_ALPHA };
            let width = if focus.is_some() && lit { 2.2 } else { 1.4 };
            let pts: Vec<DVec2> = e.points.iter().map(|p| cam.local_to_screen(cam.world_to_local(*p))).collect();
            self.draw_polyline(cx, &pts, cull, color, width, alpha, edge_pattern(e.kind));
            // the arrowhead at the target port, along the last segment
            let n = pts.len();
            let dir = pts[n - 1] - pts[n - 2];
            let head = arrowhead(pts[n - 1], dir, (8.0 * z).clamp(5.0, 12.0));
            self.draw_polyline(cx, &[head[0], pts[n - 1], head[1]], cull, color, width, alpha, 0.0);
        }
        // cards
        let pad = CARD_PAD * z;
        let icon = ICON_UNITS * z;
        for (i, n) in scene.nodes.iter().enumerate() {
            let r = cam.screen_rect(geometry(n.rect));
            if !r.intersects(cull) {
                continue;
            }
            let k = self.kind_color(n.kind);
            let lit = lit_nodes.as_ref().is_none_or(|s| s.contains(&i));
            let dim = if lit { 1.0 } else { DIM };
            let selected = self.selected == Some(i);
            let hovered = self.hover == Some(i);
            let d = &mut self.draw_card;
            d.color = with_alpha(mix4(self.surface, k, if hovered || selected { 0.24 } else { 0.14 }), dim);
            d.border_color = with_alpha(k, if lit { 0.85 } else { 0.4 } * dim);
            d.border_size = 1.0;
            d.border_radius = (9.0 * z).clamp(2.0, 12.0) as f32;
            d.outline_color = self.focus;
            d.outline_size = if selected { 2.0 } else { 0.0 };
            d.shadow_color = with_alpha(self.shadow, 0.35 * dim);
            d.shadow_radius = (8.0 * z).clamp(1.0, 10.0) as f32;
            d.draw_abs(cx, r);
            // the kind stripe down the left edge
            let stripe = Rect { pos: r.pos + dvec2(0.0, r.size.y * 0.12), size: dvec2((4.0 * z).clamp(1.5, 5.0), r.size.y * 0.76) };
            self.draw_plate(cx, stripe, with_alpha(k, dim), 1.0);
            // the kind icon, top-left inside the padding
            if icon >= 6.0 {
                let ir = Rect { pos: r.pos + dvec2(pad + 2.0 * z, pad * 0.85), size: dvec2(icon, icon) };
                self.draw_icon(cx, n.kind, ir, with_alpha(k, dim));
            }
            // the budget chip's plate, bottom-left
            if let Some(b) = &n.budget {
                let px = CHIP_UNITS * z;
                if px >= MIN_TEXT_PX {
                    let text = ellipsize_to(&mut |t| self.measure(cx, false, px, t), b, r.size.x - 2.0 * pad - 8.0 * z);
                    let w = self.measure(cx, false, px, &text) + 10.0 * z;
                    let h = px * 1.35 + 4.0 * z;
                    let chip = Rect { pos: dvec2(r.pos.x + pad + 2.0 * z, r.pos.y + r.size.y - pad * 0.7 - h), size: dvec2(w, h) };
                    self.draw_plate(cx, chip, with_alpha(mix4(self.surface, k, 0.32), dim), (h * 0.5) as f32);
                }
            }
        }
        // lane titles
        let lane_px = LANE_TITLE_UNITS * z;
        if lane_px >= MIN_TEXT_PX {
            for lane in &scene.lanes {
                let r = cam.screen_rect(geometry(lane.rect));
                if !r.intersects(cull) {
                    continue;
                }
                let title_h = super::layout::LANE_TITLE_H * z;
                let text = ellipsize_to(&mut |t| self.measure(cx, true, lane_px, t), &lane.title.to_uppercase(), r.size.x - 28.0 * z);
                self.draw_bold.text_style.font_size = (lane_px * 0.75) as f32;
                self.draw_bold.color = with_alpha(self.muted, 0.95);
                self.draw_bold.draw_abs(cx, dvec2(r.pos.x + 14.0 * z, r.pos.y + (title_h - lane_px * 1.35) * 0.5), &text);
            }
        }
        // card text
        let title_px = TITLE_UNITS * z;
        let summary_px = SUMMARY_UNITS * z;
        let chip_px = CHIP_UNITS * z;
        for (i, n) in scene.nodes.iter().enumerate() {
            let r = cam.screen_rect(geometry(n.rect));
            if !r.intersects(cull) {
                continue;
            }
            let lit = lit_nodes.as_ref().is_none_or(|s| s.contains(&i));
            let dim = if lit { 1.0 } else { DIM };
            let text_x = r.pos.x + pad + icon + 10.0 * z;
            let avail = (r.pos.x + r.size.x - pad - text_x).max(8.0);
            let mut y = r.pos.y + pad * 0.8;
            if title_px >= MIN_TEXT_PX {
                let text = ellipsize_to(&mut |t| self.measure(cx, true, title_px, t), &n.title, avail);
                self.draw_bold.text_style.font_size = (title_px * 0.75) as f32;
                self.draw_bold.color = with_alpha(self.ink, dim);
                self.draw_bold.draw_abs(cx, dvec2(text_x, y), &text);
                y += title_px * 1.5;
            }
            if summary_px >= MIN_TEXT_PX {
                let has_budget = n.budget.is_some();
                let lines = wrap_lines(&mut |t| self.measure(cx, false, summary_px, t), &n.summary, avail, if has_budget { 2 } else { 3 });
                self.draw_label.text_style.font_size = (summary_px * 0.75) as f32;
                self.draw_label.color = with_alpha(self.ink, 0.82 * dim);
                for line in lines {
                    self.draw_label.draw_abs(cx, dvec2(text_x, y), &line);
                    y += summary_px * 1.4;
                }
            }
            if let Some(b) = &n.budget {
                if chip_px >= MIN_TEXT_PX {
                    let text = ellipsize_to(&mut |t| self.measure(cx, false, chip_px, t), b, r.size.x - 2.0 * pad - 8.0 * z);
                    let h = chip_px * 1.35 + 4.0 * z;
                    self.draw_label.text_style.font_size = (chip_px * 0.75) as f32;
                    self.draw_label.color = with_alpha(self.ink, 0.9 * dim);
                    self.draw_label.draw_abs(cx, dvec2(r.pos.x + pad + 7.0 * z, r.pos.y + r.size.y - pad * 0.7 - h + 2.0 * z), &text);
                }
            }
        }
        // relation labels at the route's middle segment
        let edge_px = EDGE_LABEL_UNITS * z;
        if edge_px >= MIN_TEXT_PX {
            for e in &scene.edges {
                let Some(label) = &e.label else { continue };
                if e.points.len() < 2 {
                    continue;
                }
                let mid = e.points.len() / 2;
                let (a, b) = (e.points[mid - 1], e.points[mid]);
                let p = cam.local_to_screen(cam.world_to_local((a + b) * 0.5));
                if !cull.contains(p) {
                    continue;
                }
                let text = ellipsize_to(&mut |t| self.measure(cx, false, edge_px, t), label, 160.0 * z.max(0.5));
                let w = self.measure(cx, false, edge_px, &text);
                let h = edge_px * 1.35;
                self.draw_plate(cx, Rect { pos: p - dvec2(w * 0.5 + 3.0, h * 0.5 + 1.0), size: dvec2(w + 6.0, h + 2.0) }, with_alpha(self.background, 0.85), 2.0);
                self.draw_label.text_style.font_size = (edge_px * 0.75) as f32;
                self.draw_label.color = with_alpha(self.muted, 1.0);
                self.draw_label.draw_abs(cx, p - dvec2(w * 0.5, h * 0.5), &text);
            }
        }
    }

    /// The hover plate: the card's summary and the first sentence of its
    /// story, screen-sized, beside the card.
    fn draw_hover(&mut self, cx: &mut Cx2d, scene: &DesignScene, index: usize) {
        let Some(n) = scene.nodes.get(index) else { return };
        let view = self.view;
        let r = self.screen_camera().screen_rect(geometry(n.rect));
        if !r.intersects(view) {
            return;
        }
        let px = 10.0;
        let text_h = px * 1.4;
        let width = 340.0f64.min(view.size.x - 16.0);
        let mut lines = wrap_lines(&mut |t| self.measure(cx, false, px, t), &n.summary, width - 16.0, 2);
        let story = super::plan::first_sentence(&n.story);
        if !story.is_empty() && story != n.summary {
            lines.extend(wrap_lines(&mut |t| self.measure(cx, false, px, t), &story, width - 16.0, 3));
        }
        let (outgoing, incoming) = scene.edges_of(index).iter().fold((0usize, 0usize), |(o, i), (_, out)| if *out { (o + 1, i) } else { (o, i + 1) });
        let head = format!("{} · {outgoing} out · {incoming} in", n.kind.as_str());
        let h = text_h * (lines.len() as f64 + 1.0) + 12.0;
        let lo = view.pos.x + 4.0;
        let hi = (view.pos.x + view.size.x - width - 4.0).max(lo);
        let above = r.pos.y - h - 8.0;
        let y = if above >= view.pos.y + 4.0 { above } else { (r.pos.y + r.size.y + 6.0).min(view.pos.y + view.size.y - h - 6.0) };
        let pos = dvec2(r.pos.x.clamp(lo, hi), y);
        self.draw_plate(cx, Rect { pos, size: dvec2(width, h) }, with_alpha(self.surface, 0.97), 4.0);
        let k = self.kind_color(n.kind);
        self.draw_plate(cx, Rect { pos: pos + dvec2(4.0, 4.0), size: dvec2(3.0, h - 8.0) }, with_alpha(k, 0.9), 1.5);
        self.draw_meta.text_style.font_size = (px * 0.75) as f32;
        self.draw_meta.color = self.muted;
        self.draw_meta.draw_abs(cx, pos + dvec2(12.0, 6.0), &head);
        self.draw_label.text_style.font_size = (px * 0.75) as f32;
        self.draw_label.color = self.ink;
        for (li, line) in lines.iter().enumerate() {
            self.draw_label.draw_abs(cx, pos + dvec2(12.0, 6.0 + text_h * (li as f64 + 1.0)), line);
        }
    }
}

impl Widget for ArchitectureView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if let Event::Signal = event {
            self.pump(cx);
        }
        let area = self.draw_grid.area();
        match event.hits(cx, area) {
            Hit::FingerHoverIn(e) | Hit::FingerHoverOver(e) => {
                let hovered = if self.dragging { self.hover } else { self.node_at(e.abs) };
                if hovered != self.hover {
                    self.hover = hovered;
                    self.redraw(cx);
                }
                cx.set_cursor(if hovered.is_some() { MouseCursor::Hand } else { MouseCursor::Default });
            }
            Hit::FingerHoverOut(_) => {
                if self.hover.take().is_some() {
                    self.redraw(cx);
                }
            }
            Hit::FingerDown(e) => {
                cx.set_key_focus(area);
                cx.hide_text_ime();
                self.press = Some((e.abs, dvec2(self.camera.pan_x, self.camera.pan_y)));
                self.dragging = false;
            }
            Hit::FingerMove(e) => {
                if let Some((start, pan)) = self.press {
                    if self.dragging || (e.abs - start).length() >= DRAG_THRESHOLD_PX {
                        self.dragging = true;
                        let next = pan + (e.abs - start);
                        self.camera.pan_x = next.x;
                        self.camera.pan_y = next.y;
                        self.redraw(cx);
                    }
                }
            }
            Hit::FingerUp(e) => {
                if self.press.take().is_some() && !self.dragging {
                    let hit = self.node_at(e.abs);
                    let next = if hit == self.selected { None } else { hit };
                    if next != self.selected {
                        self.selected = next;
                        cx.widget_action(self.widget_uid(), ArchitectureViewAction::SelectionChanged);
                    }
                }
                self.dragging = false;
                self.redraw(cx);
            }
            Hit::FingerScroll(e) => {
                if e.modifiers.control || e.modifiers.logo || e.modifiers.alt {
                    self.zoom_at(e.abs, (-e.scroll.y * 0.005).exp().clamp(0.1, 10.0));
                } else {
                    self.camera.pan_x -= e.scroll.x;
                    self.camera.pan_y -= e.scroll.y;
                }
                self.redraw(cx);
            }
            Hit::KeyDown(e) => match e.key_code {
                KeyCode::KeyF => {
                    self.fit_pending = true;
                    self.redraw(cx);
                }
                KeyCode::Escape => {
                    if self.selected.take().is_some() {
                        cx.widget_action(self.widget_uid(), ArchitectureViewAction::SelectionChanged);
                        self.redraw(cx);
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let view = cx.walk_turtle(walk);
        self.view = view;
        self.draw_grid.origin = vec2((view.pos.x + self.camera.pan_x) as f32, (view.pos.y + self.camera.pan_y) as f32);
        self.draw_grid.cell = (48.0 * self.camera.zoom).clamp(8.0, 256.0) as f32;
        self.draw_grid.draw_abs(cx, view);
        let Some(bundle) = self.bundle.clone() else {
            let text = match (&self.error, self.loading.is_some()) {
                (Some(e), _) => e.clone(),
                (None, true) => "Loading the architecture plan…".to_string(),
                (None, false) => "No architecture plan loaded".to_string(),
            };
            self.draw_bold.text_style.font_size = 11.0 * 0.75;
            self.draw_bold.color = self.muted;
            self.draw_bold.draw_abs(cx, view.pos + dvec2(24.0, 24.0), &text);
            return DrawStep::done();
        };
        if self.fit_pending {
            self.fit_now();
        }
        self.draw_plan(cx, &bundle.scene);
        if !self.dragging {
            if let Some(i) = self.hover {
                self.draw_hover(cx, &bundle.scene, i);
            }
        }
        DrawStep::done()
    }
}
