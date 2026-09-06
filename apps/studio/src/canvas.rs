//! Alternate presentations of the same Dock-owned live widgets. The canvas
//! changes geometry and input coordinates, never document or process ownership.
use crate::canvas_input::{remap_event, sync_handled};
use crate::workspace::{self, Card, CardKind, Geometry, LayoutMode, Mode, Workspace};
use makepad_flowgraph::canvas::{DrawFlowCard, DrawFlowGrid};
use makepad_terminal::widget::MpTerm;
use makepad_widgets::makepad_platform::event::TouchState;
use makepad_widgets::*;
use std::collections::HashSet;

const ORIGIN: f64 = 32768.0;
const HEADER: f64 = 34.0;
const DETAIL_ZOOM: f64 = 0.45;
const EDITOR_DETAIL_ZOOM: f64 = 0.12;
fn detail_zoom(kind: CardKind) -> f64 {
    if matches!(kind, CardKind::Terminal | CardKind::Code) {
        EDITOR_DETAIL_ZOOM
    } else {
        DETAIL_ZOOM
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct CanvasCamera {
    pub view: Rect,
    pub pan: DVec2,
    pub scale: f64,
    rebase: DVec2,
}
impl CanvasCamera {
    fn new(view: Rect, c: workspace::Camera) -> Self {
        let world = dvec2(-c.pan_x / c.zoom, -c.pan_y / c.zoom);
        Self {
            view,
            pan: dvec2(c.pan_x, c.pan_y),
            scale: c.zoom,
            rebase: dvec2(
                (world.x / 8192.0).floor() * 8192.0,
                (world.y / 8192.0).floor() * 8192.0,
            ),
        }
    }
    pub fn screen_to_local(&self, p: DVec2) -> DVec2 {
        self.world_to_local((p - self.view.pos - self.pan) / self.scale)
    }
    pub fn local_to_screen(&self, p: DVec2) -> DVec2 {
        self.view.pos + self.pan + (p - dvec2(ORIGIN, ORIGIN) + self.rebase) * self.scale
    }
    fn world_to_local(&self, p: DVec2) -> DVec2 {
        p - self.rebase + dvec2(ORIGIN, ORIGIN)
    }
    fn world_at(&self, p: DVec2) -> DVec2 {
        (p - self.view.pos - self.pan) / self.scale
    }
    fn screen_rect(&self, g: Geometry) -> Rect {
        Rect {
            pos: self.view.pos + self.pan + dvec2(g.x, g.y) * self.scale,
            size: dvec2(g.w, g.h) * self.scale,
        }
    }
    fn local_rect(&self, g: Geometry) -> Rect {
        Rect {
            pos: self.world_to_local(dvec2(g.x, g.y)),
            size: dvec2(g.w, g.h),
        }
    }
    fn transform(&self) -> PopupAnchorTransform {
        PopupAnchorTransform {
            scale: self.scale,
            translation: self.view.pos
                + self.pan
                + (self.rebase - dvec2(ORIGIN, ORIGIN)) * self.scale,
        }
    }
    fn matrix(&self) -> Mat4f {
        let t = self.transform();
        let mut m = Mat4f::identity();
        m.v[0] = t.scale as f32;
        m.v[5] = t.scale as f32;
        m.v[12] = t.translation.x as f32;
        m.v[13] = t.translation.y as f32;
        m
    }
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    use mod.math.*
    mod.widgets.StudioSurface = #(StudioSurface::register_widget(vm)){
        width: Fill height: Fill
        background: theme.color_bg_app
        surface: theme.color_bg_container
        surface_hover: theme.color_outset_hover
        edge: theme.color_bevel_inset_2
        accent: theme.color_focus
        ink: theme.color_text
        muted: theme.color_text_disabled
        draw_shape +: {color: theme.color_bg_container}
        draw_grid +: {color_a: theme.color_bg_app color_b: mix(theme.color_bg_app, theme.color_text, 0.025)}
        draw_card +: {shadow_color: #0005}
        icon_terminal: mod.draw.DrawSvg{svg: crate_resource("self:resources/icons/terminal.svg")}
        icon_code: mod.draw.DrawSvg{svg: crate_resource("self:resources/icons/code.svg")}
        icon_system: mod.draw.DrawSvg{svg: crate_resource("self:resources/icons/system.svg")}
        icon_agent: mod.draw.DrawSvg{svg: crate_resource("self:resources/icons/agent.svg")}
        icon_run: mod.draw.DrawSvg{svg: crate_resource("self:resources/icons/run.svg")}
        draw_title +: {color: theme.color_text text_style: theme.font_bold{font_size: 12}}
        draw_detail +: {color: theme.color_text_disabled text_style: theme.font_regular{font_size: 10}}
        dock: Dock{}
    }
}

#[derive(Clone, Debug, Default)]
pub enum CanvasAction {
    Changed,
    Select(u64),
    Open(u64),
    Close(u64),
    #[default]
    None,
}
#[derive(Clone, Copy)]
enum Drag {
    Pan {
        start: DVec2,
        pan: DVec2,
    },
    Card {
        id: u64,
        start: DVec2,
        geometry: Geometry,
    },
    Resize {
        id: u64,
        start: DVec2,
        geometry: Geometry,
    },
    Minimap,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PointerRegion {
    Outside,
    Background,
    Navigator,
    NavigatorPanel,
    Close,
    Resize,
    Header,
    Frame,
    Body,
}

struct PointerCard {
    rect: Rect,
    caption: f64,
    close: Option<Rect>,
    body: Option<Rect>,
}

fn resize_grip(rect: Rect) -> Option<Rect> {
    (rect.size.x >= 48.0 && rect.size.y >= 48.0).then_some(Rect {
        pos: rect.pos + rect.size - dvec2(18.0, 18.0),
        size: dvec2(18.0, 18.0),
    })
}

fn pointer_region(p: DVec2, view: Rect, minimap: Rect, card: Option<PointerCard>) -> PointerRegion {
    if !view.contains(p) {
        return PointerRegion::Outside;
    }
    if minimap.contains(p) {
        return PointerRegion::Navigator;
    }
    if (Rect {
        pos: minimap.pos - dvec2(10.0, 28.0),
        size: minimap.size + dvec2(20.0, 38.0),
    })
    .contains(p)
    {
        return PointerRegion::NavigatorPanel;
    }
    let Some(card) = card.filter(|card| card.rect.contains(p)) else {
        return PointerRegion::Background;
    };
    if card.close.is_some_and(|r| r.contains(p)) {
        return PointerRegion::Close;
    }
    if resize_grip(card.rect).is_some_and(|r| r.contains(p)) {
        return PointerRegion::Resize;
    }
    if p.y < card.rect.pos.y + card.caption {
        return PointerRegion::Header;
    }
    if card.body.is_some_and(|r| r.contains(p)) {
        PointerRegion::Body
    } else {
        PointerRegion::Frame
    }
}

fn region_cursor(
    region: PointerRegion,
    layout: LayoutMode,
    pan: bool,
    drag: Option<Drag>,
) -> Option<MouseCursor> {
    match drag {
        Some(Drag::Resize { .. }) => return Some(MouseCursor::NwseResize),
        Some(Drag::Card { .. }) => return Some(MouseCursor::Move),
        Some(Drag::Pan { .. } | Drag::Minimap) => return Some(MouseCursor::Grabbing),
        None => {}
    }
    if region == PointerRegion::Outside {
        return None;
    }
    if pan && !matches!(region, PointerRegion::Close | PointerRegion::Resize) {
        return Some(MouseCursor::Grab);
    }
    match region {
        PointerRegion::Close => Some(MouseCursor::Hand),
        PointerRegion::Resize => Some(MouseCursor::NwseResize),
        PointerRegion::Navigator | PointerRegion::Background => Some(MouseCursor::Grab),
        PointerRegion::NavigatorPanel => Some(MouseCursor::Default),
        PointerRegion::Header | PointerRegion::Frame => Some(if layout == LayoutMode::Free {
            MouseCursor::Move
        } else {
            MouseCursor::Hand
        }),
        PointerRegion::Body | PointerRegion::Outside => None,
    }
}

#[derive(Script, ScriptHook, WidgetRegister, WidgetRef, WidgetSet)]
pub struct StudioSurface {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[live]
    dock: WidgetRef,
    #[live]
    draw_shape: DrawColor,
    #[live]
    draw_grid: DrawFlowGrid,
    #[live]
    draw_card: DrawFlowCard,
    #[live]
    draw_links: DrawVector,
    #[live]
    draw_marks: DrawVector,
    #[live]
    icon_terminal: DrawSvg,
    #[live]
    icon_code: DrawSvg,
    #[live]
    icon_system: DrawSvg,
    #[live]
    icon_agent: DrawSvg,
    #[live]
    icon_run: DrawSvg,
    #[live]
    draw_title: DrawText,
    #[live]
    draw_detail: DrawText,
    #[live]
    background: Vec4f,
    #[live]
    surface: Vec4f,
    #[live]
    surface_hover: Vec4f,
    #[live]
    edge: Vec4f,
    #[live]
    accent: Vec4f,
    #[live]
    ink: Vec4f,
    #[live]
    muted: Vec4f,
    #[rust]
    area: Area,
    #[rust]
    pub workspace: Workspace,
    #[rust]
    camera: CanvasCamera,
    #[rust]
    draw_list: Option<DrawList2d>,
    #[rust]
    selected: Option<u64>,
    #[rust]
    hovered: Option<u64>,
    #[rust]
    drag: Option<Drag>,
    #[rust]
    close_pressed: Option<u64>,
    #[rust]
    minimap: Rect,
    #[rust]
    map_bounds: Option<Geometry>,
    #[rust]
    warmed: HashSet<u64>,
    #[rust]
    fit_pending: bool,
    #[rust]
    focus_pending: Option<u64>,
    #[rust]
    space: bool,
    #[rust]
    body_capture: Option<u64>,
    #[rust]
    body_touch_capture: Option<(u64, u64)>,
}
impl WidgetNode for StudioSurface {
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }
    fn walk(&mut self, _: &mut Cx) -> Walk {
        self.walk
    }
    fn area(&self) -> Area {
        self.area
    }
    fn redraw(&mut self, cx: &mut Cx) {
        self.area.redraw(cx);
        self.dock.redraw(cx);
    }
    fn children(&self, visit: &mut dyn FnMut(LiveId, WidgetRef)) {
        visit(id!(dock), self.dock.clone());
    }
    fn find_widgets_from_point(&self, cx: &Cx, p: DVec2, found: &mut dyn FnMut(&WidgetRef)) {
        if self.workspace.mode == Mode::Structured {
            self.dock.find_widgets_from_point(cx, p, found);
        } else if self.camera.view.contains(p) && !self.minimap_panel().contains(p) {
            if let Some(id) = self
                .card_at(p)
                .filter(|id| self.camera.scale >= self.detail_scale(*id))
            {
                self.body(id)
                    .find_widgets_from_point(cx, self.camera.screen_to_local(p), found);
            }
        }
    }
}
impl StudioSurface {
    /// Full-workspace map bounds and the visible window, in world coordinates.
    pub fn navigator_geometry(&self) -> Option<(Geometry, Geometry)> {
        if self.workspace.mode != Mode::Canvas {
            return None;
        }
        let p = self.camera.world_at(self.camera.view.pos);
        Some((
            self.map_bounds?,
            Geometry {
                x: p.x,
                y: p.y,
                w: self.camera.view.size.x / self.camera.scale,
                h: self.camera.view.size.y / self.camera.scale,
            },
        ))
    }
    pub fn set_workspace(&mut self, cx: &mut Cx, workspace: Workspace) {
        self.workspace = workspace;
        self.changed(cx);
    }
    pub fn update_cards(&mut self, cx: &mut Cx, cards: Vec<Card>) -> Result<(), String> {
        if self.workspace.reconcile_cards(cards)? {
            self.area.redraw(cx);
        }
        Ok(())
    }
    pub fn set_mode(&mut self, cx: &mut Cx, mode: Mode) {
        if self.workspace.mode != mode {
            self.workspace.mode = mode;
            self.drag = None;
            self.space = false;
            self.body_capture = None;
            self.body_touch_capture = None;
            cx.set_key_focus(self.area);
            cx.hide_text_ime();
            for card in &self.workspace.cards {
                self.set_anchor(cx, &self.body(card.id), None);
            }
            self.dock.redraw(cx);
            self.changed(cx);
        }
    }
    pub fn set_layout(&mut self, cx: &mut Cx, layout: LayoutMode) {
        if self.workspace.set_layout(layout) {
            self.changed(cx);
        }
    }
    pub fn move_card(&mut self, cx: &mut Cx, id: u64, x: f64, y: f64) -> Result<(), String> {
        self.workspace.move_card(id, x, y)?;
        self.changed(cx);
        Ok(())
    }
    pub fn resize_card(&mut self, cx: &mut Cx, id: u64, w: f64, h: f64) -> Result<(), String> {
        self.workspace.resize_card(id, w, h)?;
        self.changed(cx);
        Ok(())
    }
    pub fn fit(&mut self, cx: &mut Cx) {
        if self.camera.view.size.x <= 1.0 {
            self.fit_pending = true;
            return;
        }
        if let Some(g) = self.workspace.bounds() {
            // Keep the fitted overview clear of the fixed navigation map.
            let v = self.camera.view.size;
            let usable = (v.x - 240.0).max(v.x * 0.6);
            let z = ((usable - 32.0).max(1.0) / g.w)
                .min((v.y - 64.0).max(1.0) / g.h)
                .clamp(workspace::MIN_ZOOM, 1.0);
            self.workspace.camera = workspace::Camera {
                zoom: z,
                pan_x: (usable - g.w * z) * 0.5 - g.x * z,
                pan_y: (v.y - g.h * z) * 0.5 - g.y * z,
            };
            self.changed(cx);
        }
    }
    pub fn focus_card(&mut self, cx: &mut Cx, id: u64) {
        self.selected = Some(id);
        if self.camera.view.size.x <= 1.0 {
            self.focus_pending = Some(id);
            return;
        }
        if let Some(g) = self.workspace.geometry(id) {
            self.fit_rect(cx, g);
        }
    }
    pub fn zoom_by(&mut self, cx: &mut Cx, factor: f64) {
        self.zoom_at(
            cx,
            self.camera.view.pos + self.camera.view.size * 0.5,
            factor,
        );
    }
    fn fit_rect(&mut self, cx: &mut Cx, g: Geometry) {
        let v = self.camera.view.size;
        let z = ((v.x - 64.0).max(1.0) / g.w)
            .min((v.y - 64.0).max(1.0) / g.h)
            .clamp(workspace::MIN_ZOOM, 1.0);
        self.workspace.camera = workspace::Camera {
            zoom: z,
            pan_x: (v.x - g.w * z) * 0.5 - g.x * z,
            pan_y: (v.y - g.h * z) * 0.5 - g.y * z,
        };
        self.changed(cx);
    }
    fn zoom_at(&mut self, cx: &mut Cx, p: DVec2, factor: f64) {
        let world = self.camera.world_at(p);
        let z =
            (self.workspace.camera.zoom * factor).clamp(workspace::MIN_ZOOM, workspace::MAX_ZOOM);
        let pan = p - self.camera.view.pos - world * z;
        let c = workspace::Camera {
            pan_x: pan.x,
            pan_y: pan.y,
            zoom: z,
        };
        if self.workspace.set_camera(c).is_ok() {
            self.changed(cx);
        }
    }
    fn changed(&mut self, cx: &mut Cx) {
        self.camera = CanvasCamera::new(self.camera.view, self.workspace.camera);
        self.area.redraw(cx);
        cx.widget_action(self.uid, CanvasAction::Changed);
    }
    fn body(&self, id: u64) -> WidgetRef {
        self.dock.as_dock().item(LiveId(id))
    }
    fn card_at(&self, p: DVec2) -> Option<u64> {
        let p = self.camera.world_at(p);
        self.workspace
            .cards
            .iter()
            .rev()
            .find(|c| {
                self.workspace
                    .geometry(c.id)
                    .is_some_and(|g| g.contains(p.x, p.y))
            })
            .map(|c| c.id)
    }
    fn detail_scale(&self, id: u64) -> f64 {
        self.workspace
            .cards
            .iter()
            .find(|c| c.id == id)
            .map(|c| detail_zoom(c.kind))
            .unwrap_or(DETAIL_ZOOM)
    }
    fn pointer_region(&self, p: DVec2) -> PointerRegion {
        let card = self.card_at(p).and_then(|id| {
            let rect = self.camera.screen_rect(self.workspace.geometry(id)?);
            let scale = self.camera.scale;
            let detail = scale >= self.detail_scale(id);
            Some(PointerCard {
                rect,
                caption: caption_height(rect, scale, self.detail_scale(id)),
                close: self.close_rect(id, rect),
                body: (detail && !self.body(id).is_empty()).then_some(Rect {
                    pos: rect.pos + dvec2(14.0, HEADER + 14.0) * scale,
                    size: rect.size - dvec2(28.0, HEADER + 42.0) * scale,
                }),
            })
        });
        pointer_region(p, self.camera.view, self.minimap, card)
    }
    fn over_scrollable_content(&self, p: DVec2) -> bool {
        if self.camera.scale < EDITOR_DETAIL_ZOOM || self.minimap_panel().contains(p) {
            return false;
        }
        let Some(id) = self.card_at(p) else {
            return false;
        };
        let Some(card) = self.workspace.cards.iter().find(|c| c.id == id) else {
            return false;
        };
        if !matches!(card.kind, CardKind::Terminal | CardKind::Code) {
            return false;
        }
        let Some(g) = self.workspace.geometry(id) else {
            return false;
        };
        let p = self.camera.world_at(p);
        p.x >= g.x + 14.0
            && p.x <= g.right() - 14.0
            && p.y >= g.y + HEADER + 14.0
            && p.y <= g.bottom() - 28.0
    }
    fn set_anchor(
        &self,
        cx: &mut Cx,
        body: &WidgetRef,
        anchor: Option<(Area, PopupAnchorTransform)>,
    ) {
        if let Some(mut term) = body.widget(cx, ids!(term)).borrow_mut::<MpTerm>() {
            term.canvas_ime_anchor = anchor;
        }
        if let Some(mut editor) = body.borrow_mut::<crate::document::StudioCodeEditor>() {
            editor.set_canvas_anchor(anchor);
        }
    }
    fn mini_pan(&mut self, cx: &mut Cx, p: DVec2) {
        let Some(g) = self.map_bounds else { return };
        let x = g.x + (p.x - self.minimap.pos.x) / self.minimap.size.x * g.w;
        let y = g.y + (p.y - self.minimap.pos.y) / self.minimap.size.y * g.h;
        let c = &mut self.workspace.camera;
        c.pan_x = self.camera.view.size.x * 0.5 - x * c.zoom;
        c.pan_y = self.camera.view.size.y * 0.5 - y * c.zoom;
        self.changed(cx);
    }
    fn shape(&mut self, cx: &mut Cx2d, rect: Rect, color: Vec4f) {
        self.draw_shape.color = color;
        self.draw_shape.draw_abs(cx, rect);
    }
    fn plate(&mut self, cx: &mut Cx2d, rect: Rect, scale: f64, id: Option<u64>) {
        let selected = id.is_some() && id == self.selected;
        let hovered = id.is_some() && id == self.hovered;
        if rect.size.x * scale < 3.0 || rect.size.y * scale < 3.0 {
            self.shape(
                cx,
                Rect {
                    pos: rect.pos,
                    size: dvec2(2.0, 2.0) / scale,
                },
                if selected { self.accent } else { self.muted },
            );
            return;
        }
        self.draw_card.color = if hovered {
            self.surface_hover
        } else {
            self.surface
        };
        self.draw_card.border_color = self.edge;
        self.draw_card.border_size = (1.0 / scale) as f32;
        self.draw_card.border_radius = (4.0 * scale.sqrt() / scale).min(rect.size.y * 0.4) as f32;
        self.draw_card.shadow_radius = (12.0 / scale) as f32;
        self.draw_card.shadow_offset = vec2(0.0, (2.0 / scale) as f32);
        self.draw_card.outline_color = tint_alpha(self.accent, if selected { 1.0 } else { 0.4 });
        self.draw_card.outline_size = if selected || hovered {
            (2.0 / scale) as f32
        } else {
            0.0
        };
        self.draw_card.draw_abs(cx, rect);
    }
    fn kind_color(&self, kind: CardKind) -> Vec4f {
        match kind {
            CardKind::Terminal => vec4(0.25, 0.73, 0.66, 1.0),
            CardKind::Code => vec4(0.90, 0.70, 0.26, 1.0),
            CardKind::Agent => vec4(0.55, 0.49, 0.96, 1.0),
            CardKind::System => vec4(0.35, 0.62, 1.0, 1.0),
            CardKind::Run | CardKind::App => vec4(0.95, 0.60, 0.29, 1.0),
            CardKind::Test => vec4(0.30, 0.77, 0.42, 1.0),
        }
    }
    fn kind_icon(&mut self, cx: &mut Cx2d, kind: CardKind, rect: Rect) {
        let color = self.kind_color(kind);
        let icon = match kind {
            CardKind::Terminal => &mut self.icon_terminal,
            CardKind::Code => &mut self.icon_code,
            CardKind::Agent => &mut self.icon_agent,
            CardKind::System => &mut self.icon_system,
            CardKind::Run | CardKind::Test | CardKind::App => &mut self.icon_run,
        };
        icon.color = color;
        icon.draw_abs(cx, rect);
    }
    fn draw_connections(&mut self, cx: &mut Cx2d) {
        self.draw_links.begin();
        let view = self.camera.view;
        for (parent, child) in self.workspace.edges() {
            let (Some(a), Some(b)) = (
                self.workspace.geometry(parent),
                self.workspace.geometry(child),
            ) else {
                continue;
            };
            let a = self.camera.screen_rect(a);
            let b = self.camera.screen_rect(b);
            let ac = caption_height(a, self.camera.scale, self.detail_scale(parent));
            let bc = caption_height(b, self.camera.scale, self.detail_scale(child));
            let from = a.pos + dvec2(a.size.x, ac + (a.size.y - ac).max(0.0) * 0.25);
            let to = b.pos + dvec2(0.0, bc + (b.size.y - bc).max(0.0) * 0.25);
            // Cull whole offscreen routes, then bound huge offscreen endpoints.
            if from.x.max(to.x) < view.pos.x - 160.0
                || from.x.min(to.x) > view.pos.x + view.size.x + 160.0
                || from.y.max(to.y) < view.pos.y - 160.0
                || from.y.min(to.y) > view.pos.y + view.size.y + 160.0
            {
                continue;
            }
            let clamp = |p: DVec2| {
                dvec2(
                    p.x.clamp(view.pos.x - 2048.0, view.pos.x + view.size.x + 2048.0),
                    p.y.clamp(view.pos.y - 2048.0, view.pos.y + view.size.y + 2048.0),
                )
            };
            let a = clamp(from);
            let b = clamp(to);
            let selected = self.selected == Some(parent) || self.selected == Some(child);
            let kind = self
                .workspace
                .cards
                .iter()
                .find(|c| c.id == child)
                .map(|c| c.kind)
                .unwrap_or(CardKind::System);
            let color = self.kind_color(kind);
            let bend = ((b.x - a.x).abs() * 0.45).clamp(32.0, 160.0);
            self.draw_links
                .set_color(color.x, color.y, color.z, if selected { 0.8 } else { 0.35 });
            self.draw_links.move_to(a.x as f32, a.y as f32);
            self.draw_links.bezier_to(
                (a.x + bend) as f32,
                a.y as f32,
                (b.x - bend) as f32,
                b.y as f32,
                b.x as f32,
                b.y as f32,
            );
            self.draw_links.stroke(if selected { 2.0 } else { 1.5 });
        }
        self.draw_links.end(cx);
    }
    fn close_rect(&self, id: u64, rect: Rect) -> Option<Rect> {
        let caption = caption_height(rect, self.camera.scale, self.detail_scale(id));
        if self.body(id).is_empty() || rect.size.x < 100.0 || caption < 18.0 {
            return None;
        }
        Some(Rect {
            pos: rect.pos + dvec2(rect.size.x - 23.0, (caption - 18.0) * 0.5),
            size: dvec2(18.0, 18.0),
        })
    }
    fn close_at(&self, p: DVec2) -> Option<u64> {
        if !self.camera.view.contains(p) || self.minimap_panel().contains(p) {
            return None;
        }
        let id = self.card_at(p)?;
        let rect = self.camera.screen_rect(self.workspace.geometry(id)?);
        self.close_rect(id, rect)
            .filter(|r| r.contains(p))
            .map(|_| id)
    }
    fn connector_marks(&mut self, cx: &mut Cx2d, rect: Rect, card: &Card, caption: f64) {
        let color = self.kind_color(card.kind);
        self.draw_marks.begin();
        for (show, x) in [
            (card.parent.is_some(), rect.pos.x),
            (
                self.workspace
                    .cards
                    .iter()
                    .any(|c| c.parent == Some(card.id)),
                rect.pos.x + rect.size.x,
            ),
        ] {
            if !show || rect.size.y < 35.0 {
                continue;
            }
            let y = rect.pos.y + caption + (rect.size.y - caption).max(0.0) * 0.25;
            self.draw_marks
                .set_color(self.surface.x, self.surface.y, self.surface.z, 1.0);
            self.draw_marks.circle(x as f32, y as f32, 4.0);
            self.draw_marks.fill();
            self.draw_marks.set_color(color.x, color.y, color.z, 0.9);
            self.draw_marks.circle(x as f32, y as f32, 4.0);
            self.draw_marks.stroke(1.5);
        }
        if let Some(close) = self.close_rect(card.id, rect) {
            let p = close.pos + dvec2(5.0, 5.0);
            self.draw_marks
                .set_color(self.muted.x, self.muted.y, self.muted.z, 1.0);
            self.draw_marks.move_to(p.x as f32, p.y as f32);
            self.draw_marks
                .line_to((p.x + 8.0) as f32, (p.y + 8.0) as f32);
            self.draw_marks.move_to((p.x + 8.0) as f32, p.y as f32);
            self.draw_marks.line_to(p.x as f32, (p.y + 8.0) as f32);
            self.draw_marks.stroke(1.5);
        }
        if let Some(grip) = resize_grip(rect) {
            let corner = grip.pos + grip.size - dvec2(4.0, 4.0);
            self.draw_marks
                .set_color(self.muted.x, self.muted.y, self.muted.z, 1.0);
            for size in [5.0, 10.0] {
                self.draw_marks
                    .move_to((corner.x - size) as f32, corner.y as f32);
                self.draw_marks
                    .line_to(corner.x as f32, (corner.y - size) as f32);
            }
            self.draw_marks.stroke(1.5);
        }
        self.draw_marks.end(cx);
    }
    fn overview(&mut self, cx: &mut Cx2d, rect: Rect, card: &Card) {
        if rect.size.x < 40.0 || rect.size.y < 35.0 {
            self.plate(cx, rect, 1.0, Some(card.id));
            if rect.size.x >= 40.0 && rect.size.y >= 18.0 {
                cx.push_clip_rect(rect);
                self.draw_detail.draw_abs(
                    cx,
                    rect.pos + dvec2(7.0, (rect.size.y - 10.0) * 0.5),
                    &short_text(&card.title, ((rect.size.x - 14.0) / 5.5).max(1.0) as usize),
                );
                cx.pop_clip_rect();
            }
            return;
        }
        let caption = 24.0;
        let body = Rect {
            pos: rect.pos + dvec2(0.0, caption),
            size: rect.size - dvec2(0.0, caption),
        };
        self.plate(cx, body, 1.0, Some(card.id));
        cx.push_clip_rect(rect);
        self.kind_icon(
            cx,
            card.kind,
            Rect {
                pos: rect.pos + dvec2(2.0, 3.0),
                size: dvec2(15.0, 15.0),
            },
        );
        let title_width = if self.close_rect(card.id, rect).is_some() {
            rect.size.x - 28.0
        } else {
            rect.size.x
        };
        cx.push_clip_rect(Rect {
            pos: rect.pos,
            size: dvec2(title_width.max(0.0), rect.size.y),
        });
        self.draw_title.draw_abs(
            cx,
            rect.pos + dvec2(23.0, 5.0),
            &short_text(
                &card.title,
                ((rect.size.x
                    - if self.body(card.id).is_empty() {
                        28.0
                    } else {
                        54.0
                    })
                    / 6.0)
                    .max(1.0) as usize,
            ),
        );
        cx.pop_clip_rect();
        if body.size.y >= 25.0 {
            self.draw_detail
                .draw_abs(cx, body.pos + dvec2(11.0, 10.0), card.kind.as_str());
        }
        if body.size.y >= 52.0 {
            let detail = card.detail.lines().next().unwrap_or("");
            let cols = ((body.size.x - 22.0) / 5.5).max(1.0) as usize;
            self.draw_detail
                .draw_abs(cx, body.pos + dvec2(11.0, 30.0), &short_text(detail, cols));
        }
        cx.pop_clip_rect();
        self.connector_marks(cx, rect, card, caption);
    }
    fn minimap_panel(&self) -> Rect {
        Rect {
            pos: self.minimap.pos - dvec2(10.0, 28.0),
            size: self.minimap.size + dvec2(20.0, 38.0),
        }
    }

    fn draw_minimap(&mut self, cx: &mut Cx2d) {
        let v = self.camera.view;
        self.minimap = Rect {
            pos: v.pos + v.size - dvec2(200.0, 148.0),
            size: dvec2(184.0, 124.0),
        };
        self.plate(cx, self.minimap_panel(), 1.0, None);
        self.draw_detail
            .draw_abs(cx, self.minimap.pos - dvec2(0.0, 19.0), "Navigator");
        let g = minimap_bounds(self.workspace.bounds(), self.minimap.size);
        self.map_bounds = Some(g);
        let world = self.camera.world_at(v.pos);
        cx.push_clip_rect(self.minimap);
        let sx = self.minimap.size.x / g.w;
        let sy = self.minimap.size.y / g.h;
        for index in 0..self.workspace.cards.len() {
            let id = self.workspace.cards[index].id;
            let kind = self.workspace.cards[index].kind;
            if let Some(r) = self.workspace.geometry(id) {
                let rect = Rect {
                    pos: self.minimap.pos + dvec2((r.x - g.x) * sx, (r.y - g.y) * sy),
                    size: dvec2((r.w * sx).max(2.0), (r.h * sy).max(2.0)),
                };
                self.shape(
                    cx,
                    rect,
                    if self.selected == Some(id) {
                        self.accent
                    } else {
                        self.kind_color(kind)
                    },
                );
            }
        }
        let r = Rect {
            pos: self.minimap.pos + dvec2((world.x - g.x) * sx, (world.y - g.y) * sy),
            size: dvec2(
                v.size.x / self.camera.scale * sx,
                v.size.y / self.camera.scale * sy,
            ),
        };
        self.shape(cx, r, tint_alpha(self.accent, 0.10));
        for rr in [
            Rect {
                pos: r.pos,
                size: dvec2(r.size.x, 1.5),
            },
            Rect {
                pos: r.pos + dvec2(0.0, r.size.y - 1.5),
                size: dvec2(r.size.x, 1.5),
            },
            Rect {
                pos: r.pos,
                size: dvec2(1.5, r.size.y),
            },
            Rect {
                pos: r.pos + dvec2(r.size.x - 1.5, 0.0),
                size: dvec2(1.5, r.size.y),
            },
        ] {
            self.shape(cx, rr, self.accent);
        }
        cx.pop_clip_rect();
    }
}
impl Widget for StudioSurface {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let view = cx.walk_turtle(walk);
        cx.add_rect_area(&mut self.area, view);
        self.camera = CanvasCamera::new(view, self.workspace.camera);
        if self.workspace.mode == Mode::Canvas {
            cx.push_clip_rect(view);
            let cell = 24.0 * self.camera.scale;
            let cell = cell * 2.0_f64.powf((14.0 / cell).log2().ceil().max(0.0));
            self.draw_grid.cell = cell as f32;
            // Some themes use transparent alternating-row colors. The canvas
            // grid is opaque, so keep those tokens from whitening the result
            // when the quad blends over the window clear color.
            self.draw_grid.color_a.w = 1.0;
            self.draw_grid.color_b.w = 1.0;
            self.draw_grid.origin = vec2(
                ((view.pos.x + self.camera.pan.x).rem_euclid(cell * 2.0)) as f32,
                ((view.pos.y + self.camera.pan.y).rem_euclid(cell * 2.0)) as f32,
            );
            self.draw_grid.draw_abs(cx, view);
            self.draw_connections(cx);
            cx.pop_clip_rect();
        }
        let mut list = self.draw_list.take().unwrap_or_else(|| DrawList2d::new(cx));
        list.begin_always(cx);
        if self.workspace.mode == Mode::Structured {
            cx.begin_turtle(
                Walk {
                    abs_pos: Some(view.pos),
                    width: Size::Fixed(view.size.x),
                    height: Size::Fixed(view.size.y),
                    ..Default::default()
                },
                Layout::flow_overlay(),
            );
            self.dock.draw_walk_all(cx, scope, Walk::fill());
            cx.end_turtle();
            list.end(cx);
            list.set_view_transform(cx, &Mat4f::identity());
            self.draw_list = Some(list);
            return DrawStep::done();
        }
        cx.begin_root_turtle(
            dvec2(
                (view.size.x / self.camera.scale + ORIGIN * 2.0).max(65536.0),
                (view.size.y / self.camera.scale + ORIGIN * 2.0).max(65536.0),
            ),
            Layout::flow_overlay(),
        );
        let local = Rect {
            pos: self.camera.screen_to_local(view.pos),
            size: view.size / self.camera.scale,
        };
        cx.push_clip_rect(local);
        let mut overview_cards = Vec::new();
        let mut detail_marks = Vec::new();
        for index in 0..self.workspace.cards.len() {
            let id = self.workspace.cards[index].id;
            let Some(g) = self.workspace.geometry(id) else {
                continue;
            };
            let screen = self.camera.screen_rect(g);
            let visible = screen.pos.x < view.pos.x + view.size.x
                && screen.pos.y < view.pos.y + view.size.y
                && screen.pos.x + screen.size.x > view.pos.x
                && screen.pos.y + screen.size.y > view.pos.y;
            let body = self.body(id);
            if !visible && (body.is_empty() || self.warmed.contains(&id)) {
                continue;
            }
            // Copy text only for visible cards or a live widget's first draw.
            // Offscreen terminal sessions still receive events below.
            let card = self.workspace.cards[index].clone();
            let r = self.camera.local_rect(g);
            let overview = self.camera.scale < detail_zoom(card.kind);
            if overview && visible {
                overview_cards.push((screen, card.clone()));
            }
            if overview && (body.is_empty() || self.warmed.contains(&card.id)) {
                continue;
            }
            if overview {
                cx.push_clip_rect(Rect {
                    pos: r.pos,
                    size: dvec2(0.0, 0.0),
                });
            }
            let plate = Rect {
                pos: r.pos + dvec2(0.0, HEADER),
                size: r.size - dvec2(0.0, HEADER),
            };
            self.plate(cx, plate, self.camera.scale, Some(card.id));
            cx.push_clip_rect(r);
            self.kind_icon(
                cx,
                card.kind,
                Rect {
                    pos: r.pos + dvec2(2.0, 7.0),
                    size: dvec2(18.0, 18.0),
                },
            );
            let title_cols =
                ((g.w - if g.w > 180.0 { 130.0 } else { 36.0 }) / 7.0).max(1.0) as usize;
            self.draw_title.draw_abs(
                cx,
                r.pos + dvec2(28.0, 10.0),
                &short_text(&card.title, title_cols),
            );
            if g.w > 180.0 {
                self.draw_detail
                    .draw_abs(cx, r.pos + dvec2(g.w - 108.0, 11.0), card.kind.as_str());
            }
            let content = Rect {
                pos: plate.pos + dvec2(14.0, 14.0),
                size: dvec2(
                    (plate.size.x - 28.0).max(1.0),
                    (plate.size.y - 42.0).max(1.0),
                ),
            };
            if !body.is_empty() && (!overview || !self.warmed.contains(&card.id)) {
                self.set_anchor(cx, &body, Some((self.area, self.camera.transform())));
                cx.push_clip_rect(content);
                cx.begin_turtle(
                    Walk {
                        abs_pos: Some(content.pos),
                        width: Size::Fixed(content.size.x),
                        height: Size::Fixed(content.size.y),
                        ..Default::default()
                    },
                    Layout {
                        flow: Flow::Down,
                        clip_x: true,
                        clip_y: true,
                        ..Default::default()
                    },
                );
                let focus = cx.key_focus();
                body.draw_walk_all(cx, scope, Walk::fill());
                if self.selected != Some(card.id) && cx.key_focus() != focus {
                    cx.set_key_focus(focus);
                }
                cx.end_turtle();
                cx.pop_clip_rect();
                self.warmed.insert(card.id);
            } else {
                let text = if body.is_empty() {
                    card.detail.clone()
                } else {
                    format!("{}\n\nZoom in or double-click to work here", card.detail)
                };
                for (i, line) in wrap_lines(
                    &text,
                    ((g.w - 32.0) / 6.0) as usize,
                    ((g.h - 80.0) / 18.0) as usize,
                )
                .iter()
                .enumerate()
                {
                    self.draw_detail.draw_abs(
                        cx,
                        content.pos + dvec2(12.0, 12.0 + i as f64 * 18.0),
                        line,
                    );
                }
            }
            let footer = if body.is_empty() {
                format!("{:x}", card.id)
            } else {
                card.detail
                    .lines()
                    .next()
                    .unwrap_or("Live workspace item")
                    .to_string()
            };
            if !body.is_empty() {
                self.draw_detail.draw_abs(
                    cx,
                    r.pos + dvec2(14.0, g.h - 18.0),
                    &short_text(&footer, ((g.w - 28.0) / 6.0).max(1.0) as usize),
                );
            }
            if !overview && visible {
                detail_marks.push((screen, card));
            }
            cx.pop_clip_rect();
            if overview {
                cx.pop_clip_rect();
            }
        }
        cx.pop_clip_rect();
        cx.end_pass_sized_turtle();
        list.end(cx);
        list.set_view_transform(cx, &self.camera.matrix());
        self.draw_list = Some(list);
        // Keep overview typography readable without scaling glyph atlases.
        cx.push_clip_rect(view);
        self.draw_title.text_style.font_size = 10.5;
        self.draw_detail.text_style.font_size = 9.0;
        for (rect, card) in overview_cards {
            self.overview(cx, rect, &card);
        }
        for (rect, card) in detail_marks {
            self.connector_marks(cx, rect, &card, HEADER * self.camera.scale);
        }
        self.draw_title.text_style.font_size = 12.0;
        self.draw_detail.text_style.font_size = 10.0;
        cx.pop_clip_rect();
        // Navigation chrome stays screen-sized, outside the camera list.
        self.draw_minimap(cx);
        let hint = if view.size.x < 900.0 {
            "Scroll to zoom · Drag to pan"
        } else {
            "Scroll to zoom · Drag background to pan · Double-click to focus"
        };
        cx.push_clip_rect(Rect {
            pos: view.pos,
            size: dvec2(
                (self.minimap_panel().pos.x - view.pos.x - 8.0).max(0.0),
                view.size.y,
            ),
        });
        self.draw_detail.draw_abs(
            cx,
            view.pos + dvec2(14.0, view.size.y - 24.0),
            &format!(
                "{} · {} cards · {hint}",
                workspace::zoom_label(self.workspace.camera.zoom),
                self.workspace.cards.len()
            ),
        );
        cx.pop_clip_rect();
        if let Some(id) = self.focus_pending.take() {
            self.focus_card(cx, id);
        }
        if self.fit_pending {
            self.fit_pending = false;
            self.fit(cx);
        }
        DrawStep::done()
    }
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if self.workspace.mode == Mode::Structured {
            self.dock.handle_event(cx, event, scope);
            return;
        }
        let pointer = match event {
            Event::MouseDown(e) => Some(e.abs),
            Event::MouseMove(e) => Some(e.abs),
            Event::MouseUp(e) => Some(e.abs),
            Event::Scroll(e) => Some(e.abs),
            Event::TouchUpdate(e) => self
                .body_touch_capture
                .and_then(|(uid, _)| e.touches.iter().find(|t| t.uid == uid))
                .or_else(|| e.touches.iter().find(|t| t.state == TouchState::Start))
                .or_else(|| e.touches.iter().find(|t| t.state != TouchState::Stable))
                .map(|t| t.abs),
            _ => None,
        };
        if matches!(event, Event::MouseMove(_) | Event::MouseLeave(_)) {
            let hovered = pointer
                .filter(|p| self.camera.view.contains(*p) && !self.minimap_panel().contains(*p))
                .and_then(|p| self.card_at(p));
            if self.hovered != hovered {
                self.hovered = hovered;
                self.area.redraw(cx);
            }
        }
        let nav_scroll = matches!(event,Event::Scroll(e) if e.modifiers.control || e.modifiers.logo || !self.over_scrollable_content(e.abs));
        let middle = matches!(event,Event::MouseDown(e) if e.button==MouseButton::MIDDLE);
        let region = pointer.map(|p| self.pointer_region(p));
        let over_grip = region == Some(PointerRegion::Resize);
        let close_card = pointer.and_then(|p| self.close_at(p));
        let chrome = region.is_some_and(|region| {
            !matches!(
                region,
                PointerRegion::Outside | PointerRegion::Body | PointerRegion::Background
            )
        });
        let input = event.requires_visibility()
            || matches!(
                event,
                Event::MouseUp(_)
                    | Event::MouseLeave(_)
                    | Event::LongPress(_)
                    | Event::SelectionHandleDrag(_)
                    | Event::Drag(_)
                    | Event::Drop(_)
                    | Event::DragEnd
                    | Event::KeyDown(_)
                    | Event::KeyUp(_)
                    | Event::TextInput(_)
                    | Event::TextRangeReplace(_)
                    | Event::TextCopy(_)
                    | Event::TextCut(_)
                    | Event::ImeAction(_)
            );
        let captured_card = match event {
            Event::MouseMove(_) | Event::MouseUp(_) | Event::MouseLeave(_) => self.body_capture,
            Event::TouchUpdate(e) => self
                .body_touch_capture
                .filter(|(uid, _)| e.touches.iter().any(|t| t.uid == *uid))
                .map(|(_, card)| card),
            _ => None,
        };
        let captured = captured_card.is_some();
        let top_card = pointer.and_then(|p| self.card_at(p));
        // Record the press before child dispatch, then let the body claim key
        // focus. Selection of a card must not depend on its body ignoring input.
        let body_press = match event {
            Event::MouseDown(e) if e.handled.get().is_empty() => Some((e.abs, None)),
            Event::TouchUpdate(e) if self.body_touch_capture.is_none() => e
                .touches
                .iter()
                .find(|t| t.state == TouchState::Start && t.handled.get().is_empty())
                .map(|t| (t.abs, Some(t.uid))),
            _ => None,
        }
        .filter(|(p, _)| self.camera.view.contains(*p) && !chrome && !middle && !self.space);
        if let Some((p, _)) = body_press {
            if let Some(id) = self.card_at(p) {
                self.selected = Some(id);
                cx.widget_action(self.uid, CanvasAction::Select(id));
                self.area.redraw(cx);
            }
        }
        if !input {
            // Deliver async PTY signals, document updates and timers once to
            // every resident tab, including offscreen/overview cards.
            for card in &self.workspace.cards {
                self.body(card.id).handle_event(cx, event, scope);
            }
        } else if captured
            || (self.drag.is_none()
                && !chrome
                && !nav_scroll
                && !middle
                && !self.space
                && pointer.is_none_or(|p| self.camera.view.contains(p))
                && self.camera.scale >= EDITOR_DETAIL_ZOOM)
        {
            let mapped = remap_event(event, &self.camera);
            let delivered = mapped.as_ref().unwrap_or(event);
            for card in self.workspace.cards.iter().rev() {
                if !captured && pointer.is_some() && top_card != Some(card.id) {
                    continue;
                }
                if captured_card == Some(card.id)
                    || (!captured
                        && self.camera.scale >= detail_zoom(card.kind)
                        && self.workspace.geometry(card.id).is_some_and(|g| {
                            let r = self.camera.screen_rect(g);
                            r.pos.x + r.size.x > self.camera.view.pos.x
                                && r.pos.y + r.size.y > self.camera.view.pos.y
                                && r.pos.x < self.camera.view.pos.x + self.camera.view.size.x
                                && r.pos.y < self.camera.view.pos.y + self.camera.view.size.y
                        }))
                {
                    self.body(card.id).handle_event(cx, delivered, scope);
                }
            }
            if let Some(mapped) = mapped.as_ref() {
                sync_handled(event, mapped, &self.camera);
            }
            if let Some((p, touch_uid)) = body_press {
                match event {
                    Event::MouseDown(e) if !e.handled.get().is_empty() => {
                        self.body_capture = self.card_at(p)
                    }
                    Event::TouchUpdate(e) => {
                        if let Some(uid) = touch_uid.filter(|uid| {
                            e.touches
                                .iter()
                                .any(|t| t.uid == *uid && !t.handled.get().is_empty())
                        }) {
                            self.body_touch_capture = self.card_at(p).map(|card| (uid, card));
                        }
                    }
                    _ => {}
                }
            }
        }
        // MouseLeave reports hover-out, not release: retain capture so an up
        // outside the viewport or over navigation chrome still reaches its owner.
        if matches!(event, Event::MouseUp(_)) {
            self.body_capture = None;
        }
        if let Event::TouchUpdate(e) = event {
            if self.body_touch_capture.is_some_and(|(uid, _)| {
                e.touches
                    .iter()
                    .any(|t| t.uid == uid && t.state == TouchState::Stop)
            }) {
                self.body_touch_capture = None;
            }
        }
        let scroll_handled =
            matches!(event,Event::Scroll(e) if e.handled_x.get()||e.handled_y.get());
        let hit = event.hits(cx, self.area);
        // Event::hits respects overlay handling and sweep locks. Only a hit
        // owned by this surface may change the cursor; background signals and
        // keyboard delivery to resident editors must not affect pointer chrome.
        let cursor_hit = matches!(
            &hit,
            Hit::FingerHoverIn(_)
                | Hit::FingerHoverOver(_)
                | Hit::FingerDown(_)
                | Hit::FingerMove(_)
                | Hit::FingerUp(_)
                | Hit::FingerScroll(_)
        );
        match hit {
            Hit::FingerDown(e) => {
                cx.set_key_focus(self.area);
                cx.hide_text_ime();
                self.close_pressed = close_card;
                if close_card.is_some() {
                    self.drag = None;
                } else if self.minimap.contains(e.abs) {
                    self.drag = Some(Drag::Minimap);
                    self.mini_pan(cx, e.abs);
                } else if self.minimap_panel().contains(e.abs) {
                    self.drag = None;
                } else if !middle && !self.space {
                    if let Some(id) = self.card_at(e.abs) {
                        self.selected = Some(id);
                        cx.widget_action(self.uid, CanvasAction::Select(id));
                        if over_grip {
                            if let Some(g) = self.workspace.geometry(id) {
                                self.drag = Some(Drag::Resize {
                                    id,
                                    start: e.abs,
                                    geometry: g,
                                });
                            }
                        } else if e.tap_count >= 2 {
                            self.focus_card(cx, id);
                            cx.widget_action(self.uid, CanvasAction::Open(id));
                        } else if self.workspace.layout == LayoutMode::Free {
                            if let Some(g) = self.workspace.geometry(id) {
                                self.drag = Some(Drag::Card {
                                    id,
                                    start: e.abs,
                                    geometry: g,
                                });
                            }
                        }
                        self.area.redraw(cx);
                    } else {
                        self.drag = Some(Drag::Pan {
                            start: e.abs,
                            pan: self.camera.pan,
                        });
                    }
                } else {
                    self.drag = Some(Drag::Pan {
                        start: e.abs,
                        pan: self.camera.pan,
                    });
                }
            }
            Hit::FingerMove(e) => match self.drag {
                Some(Drag::Pan { start, pan }) => {
                    let p = pan + e.abs - start;
                    let c = workspace::Camera {
                        pan_x: p.x,
                        pan_y: p.y,
                        zoom: self.workspace.camera.zoom,
                    };
                    if self.workspace.set_camera(c).is_ok() {
                        self.changed(cx);
                    }
                }
                Some(Drag::Card {
                    id,
                    start,
                    geometry: g,
                }) => {
                    let p = (e.abs - start) / self.camera.scale;
                    let _ = self.move_card(cx, id, g.x + p.x, g.y + p.y);
                }
                Some(Drag::Resize {
                    id,
                    start,
                    geometry: g,
                }) => {
                    let p = (e.abs - start) / self.camera.scale;
                    let _ = self.resize_card(
                        cx,
                        id,
                        (g.w + p.x).clamp(160.0, 4096.0),
                        (g.h + p.y).clamp(120.0, 4096.0),
                    );
                }
                Some(Drag::Minimap) => self.mini_pan(cx, e.abs),
                None => {}
            },
            Hit::FingerUp(e) => {
                self.drag = None;
                if let Some(id) = self
                    .close_pressed
                    .take()
                    .filter(|id| self.close_at(e.abs) == Some(*id))
                {
                    cx.widget_action(self.uid, CanvasAction::Close(id));
                }
            }
            Hit::FingerScroll(e) if !scroll_handled => {
                if nav_scroll {
                    let anchor = if self.minimap_panel().contains(e.abs) {
                        self.camera.view.pos + self.camera.view.size * 0.5
                    } else {
                        e.abs
                    };
                    let delta = if e.scroll.y.abs() > 0.0 {
                        e.scroll.y
                    } else {
                        e.scroll.x
                    };
                    self.zoom_at(cx, anchor, (-delta * 0.006).exp());
                }
            }
            Hit::KeyDown(e) => match e.key_code {
                KeyCode::Space => self.space = true,
                KeyCode::KeyF => self.fit(cx),
                KeyCode::Escape => {
                    self.drag = None;
                    self.close_pressed = None;
                    self.space = false;
                }
                _ => {}
            },
            Hit::KeyUp(e) if e.key_code == KeyCode::Space => self.space = false,
            _ => {}
        }
        if cursor_hit {
            if let Some(cursor) = pointer.and_then(|p| {
                region_cursor(
                    self.pointer_region(p),
                    self.workspace.layout,
                    self.space,
                    self.drag,
                )
            }) {
                cx.set_cursor(cursor);
            }
        }
    }
}
fn short_text(text: &str, chars: usize) -> String {
    let mut input = text.chars();
    let mut s: String = input.by_ref().take(chars).collect();
    if input.next().is_some() {
        s.push('…');
    }
    s
}
fn wrap_lines(text: &str, width: usize, max_lines: usize) -> Vec<String> {
    let width = width.max(8);
    let mut out = Vec::new();
    for line in text.lines() {
        if out.len() >= max_lines {
            break;
        }
        if line.is_empty() {
            out.push(String::new());
            continue;
        }
        let mut chars = line.chars().peekable();
        while chars.peek().is_some() && out.len() < max_lines {
            out.push(chars.by_ref().take(width).collect());
        }
    }
    out
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pointer_chrome_overrides_text_and_navigator_masks_live_body() {
        let view = Rect {
            pos: dvec2(0.0, 0.0),
            size: dvec2(1000.0, 800.0),
        };
        let minimap = Rect {
            pos: dvec2(600.0, 350.0),
            size: dvec2(184.0, 124.0),
        };
        let card = || PointerCard {
            rect: Rect {
                pos: dvec2(100.0, 100.0),
                size: dvec2(720.0, 460.0),
            },
            caption: 34.0,
            close: Some(Rect {
                pos: dvec2(797.0, 108.0),
                size: dvec2(18.0, 18.0),
            }),
            body: Some(Rect {
                pos: dvec2(114.0, 148.0),
                size: dvec2(692.0, 384.0),
            }),
        };
        let region = |p| pointer_region(p, view, minimap, Some(card()));
        let cursor = |p, layout| region_cursor(region(p), layout, false, None);
        // A native editor may leave Text selected. Chrome must replace it.
        assert_eq!(
            cursor(dvec2(805.0, 116.0), LayoutMode::Auto).unwrap_or(MouseCursor::Text),
            MouseCursor::Hand
        );
        assert_eq!(
            cursor(dvec2(815.0, 555.0), LayoutMode::Auto).unwrap_or(MouseCursor::Text),
            MouseCursor::NwseResize
        );
        assert_eq!(
            cursor(dvec2(815.0, 555.0), LayoutMode::Free),
            Some(MouseCursor::NwseResize)
        );
        assert_eq!(
            cursor(dvec2(200.0, 115.0), LayoutMode::Auto),
            Some(MouseCursor::Hand)
        );
        assert_eq!(
            cursor(dvec2(200.0, 115.0), LayoutMode::Free),
            Some(MouseCursor::Move)
        );
        assert_eq!(
            cursor(dvec2(200.0, 200.0), LayoutMode::Auto),
            None,
            "the native editor owns its body cursor"
        );
        assert_eq!(
            region(dvec2(650.0, 380.0)),
            PointerRegion::Navigator,
            "the overlay covers an otherwise live body point"
        );
        assert_eq!(
            cursor(dvec2(650.0, 380.0), LayoutMode::Auto),
            Some(MouseCursor::Grab)
        );
        assert_eq!(
            cursor(dvec2(650.0, 335.0), LayoutMode::Auto),
            Some(MouseCursor::Default)
        );
        assert_eq!(
            cursor(dvec2(20.0, 200.0), LayoutMode::Auto),
            Some(MouseCursor::Grab)
        );
        assert_eq!(
            cursor(dvec2(-1.0, 200.0), LayoutMode::Auto),
            None,
            "other UI owns pointers outside the canvas"
        );
    }

    #[test]
    fn resize_grip_stays_usable_in_screen_points_when_zoomed_out() {
        let rect = Rect {
            pos: dvec2(120.0, 100.0),
            size: dvec2(720.0, 460.0) * EDITOR_DETAIL_ZOOM,
        };
        let grip = resize_grip(rect).unwrap();
        assert_eq!(grip.size, dvec2(18.0, 18.0));
        assert!(grip.contains(rect.pos + rect.size - dvec2(12.0, 12.0)));
        assert!(!grip.contains(rect.pos + rect.size - dvec2(21.0, 21.0)));
        assert!(
            resize_grip(Rect {
                size: dvec2(30.0, 25.0),
                ..rect
            })
            .is_none(),
            "tiny overview dots must not become resize targets"
        );
    }

    #[test]
    fn camera_roundtrip_at_extreme_pan_and_zoom() {
        for zoom in [workspace::MIN_ZOOM, 0.01, 0.2, 1.0, 8.0] {
            for pan in [-9_000_000.0, 0.0, 9_000_000.0] {
                let c = CanvasCamera::new(
                    Rect {
                        pos: dvec2(140.0, 80.0),
                        size: dvec2(1000.0, 700.0),
                    },
                    workspace::Camera {
                        pan_x: pan,
                        pan_y: -pan,
                        zoom,
                    },
                );
                let p = dvec2(511.0, 344.0);
                assert!((c.local_to_screen(c.screen_to_local(p)) - p).length() < 0.00001);
            }
        }
    }
}

fn tint_alpha(mut color: Vec4f, alpha: f32) -> Vec4f {
    color.w *= alpha;
    color
}

fn caption_height(rect: Rect, scale: f64, detail_scale: f64) -> f64 {
    if scale >= detail_scale {
        HEADER * scale
    } else if rect.size.x >= 40.0 && rect.size.y >= 35.0 {
        24.0
    } else {
        0.0
    }
}

/// A stable projection of the entire workspace. Panning/zooming only changes
/// the window overlay, never these bounds or the node positions in the map.
fn minimap_bounds(bounds: Option<Geometry>, size: DVec2) -> Geometry {
    let g = bounds.unwrap_or(Geometry {
        x: 0.0,
        y: 0.0,
        w: 1024.0,
        h: 768.0,
    });
    let margin = (g.w.max(g.h) * 0.5).max(48.0);
    let aspect = size.x / size.y;
    let mut w = g.w + margin * 2.0;
    let mut h = g.h + margin * 2.0;
    if w / h > aspect {
        h = w / aspect;
    } else {
        w = h * aspect;
    }
    Geometry {
        x: g.x + (g.w - w) * 0.5,
        y: g.y + (g.h - h) * 0.5,
        w,
        h,
    }
}
