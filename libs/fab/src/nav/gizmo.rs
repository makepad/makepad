//! Lane C. The navigation gizmo and the view buttons under it.
//!
//! Axis balls X / Y / Z with their negative ghosts, depth-sorted and scaled by
//! depth so the near ones read as near; click a ball for its preset view, drag
//! anywhere on the gizmo to orbit, hover for the backdrop ring. Under it,
//! Fab's little column: zoom (drag), move (drag), frame all, and the
//! perspective / orthographic toggle.
//!
//! Placement note: lane D docks this in an overlay row that far-edge-aligns
//! (`align: Align{x: 1.0}`), and `DrawVector` — which every SVG icon rides
//! on — is submitted with `add_aligned_instance`, so a deferred turtle shift
//! displaces its geometry. The widget therefore takes the **full width** (no
//! slack for the parent to distribute, so no shift) and corner-pins its own
//! panel rect, exactly the fix `PerfGraph` uses.
//!
//! The balls, the axis lines and the button wells are `Sdf2d` because they are
//! procedural marks. The button glyphs are our own SVGs on `DrawSvg`, never
//! shader-drawn icons.

use crate::api::*;
use crate::nav::orbit;
use crate::nav::walk;
use makepad_widgets::tip::TipAction;
use makepad_widgets::*;

script_mod! {
    use mod.prelude.fab.*
    use mod.widgets.*
    use mod.math.*
    use mod.shader.*
    use mod.draw

    mod.draw.DrawGizmoBall = mod.std.set_type_default() do #(DrawGizmoBall::script_shader(vm)){
        ..mod.draw.DrawQuad
        color: vec4(1.0, 1.0, 1.0, 1.0)
        ghost: 0.0
        hover: 0.0
        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            let r = min(self.rect_size.x, self.rect_size.y) * 0.5 - 1.0
            sdf.circle(self.rect_size.x * 0.5, self.rect_size.y * 0.5, r)
            // Positive balls are solid; negatives are hollow until hovered,
            // which is exactly how Fab tells them apart.
            let bright = self.color.mix(vec4(1.0, 1.0, 1.0, 1.0), self.hover * 0.4)
            let hollow = vec4(self.color.xyz * 0.26, 0.85)
            let fill = bright.mix(hollow, self.ghost).mix(bright, self.hover)
            sdf.fill_keep(fill)
            sdf.stroke(vec4(bright.xyz, 0.9), 1.0)
            return sdf.result
        }
    }

    mod.draw.DrawGizmoLine = mod.std.set_type_default() do #(DrawGizmoLine::script_shader(vm)){
        ..mod.draw.DrawQuad
        color: vec4(1.0, 1.0, 1.0, 1.0)
        p0: vec2(0.0, 0.0)
        p1: vec2(1.0, 1.0)
        thickness: 2.0
        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            sdf.move_to(self.p0.x, self.p0.y)
            sdf.line_to(self.p1.x, self.p1.y)
            sdf.stroke(vec4(self.color.xyz * 0.85, 0.85), self.thickness)
            return sdf.result
        }
    }

    // The hover backdrop: invisible until the pointer is on the gizmo, then a
    // soft disc behind the balls — Fab's ring.
    mod.draw.DrawGizmoBackdrop = mod.std.set_type_default() do #(DrawGizmoBackdrop::script_shader(vm)){
        ..mod.draw.DrawQuad
        color: vec4(0.0, 0.0, 0.0, 0.0)
        hover: 0.0
        radius: 44.0
        opacity: 0.55
        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            sdf.circle(self.rect_size.x * 0.5, self.radius, self.radius - 1.0)
            sdf.fill(vec4(self.color.xyz, self.opacity * self.hover))
            return sdf.result
        }
    }

    mod.draw.DrawGizmoButton = mod.std.set_type_default() do #(DrawGizmoButton::script_shader(vm)){
        ..mod.draw.DrawQuad
        color: vec4(0.0, 0.0, 0.0, 0.0)
        color_hover: vec4(1.0, 1.0, 1.0, 1.0)
        color_down: vec4(0.0, 0.0, 0.0, 1.0)
        color_active: vec4(0.0, 0.0, 1.0, 1.0)
        border_color: vec4(0.0, 0.0, 0.0, 1.0)
        radius: 4.0
        hover: 0.0
        down: 0.0
        active: 0.0
        idle_alpha: 0.55
        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, self.radius)
            let idle = vec4(self.color.xyz, self.idle_alpha)
            let base = idle
                .mix(self.color_active, self.active)
                .mix(self.color_hover, self.hover)
                .mix(self.color_down, self.down)
            sdf.fill_keep(base)
            // A faint rim always, so the well reads on a bright viewport too.
            let rim = 0.45 + 0.55 * max(self.hover, self.active)
            sdf.stroke(vec4(self.border_color.xyz, self.border_color.w * rim), 1.0)
            return sdf.result
        }
    }

    mod.widgets.FabNavGizmoBase = #(FabNavGizmo::register_widget(vm))
    mod.widgets.FabNavGizmo = set_type_default() do mod.widgets.FabNavGizmoBase{
        width: Fill
        height: fab.gizmo_size
        view: 0
        ball_size: fab.gizmo_size
        button_size: fab.row_height
        button_gap: fab.pad_1
        draw_backdrop: mod.draw.DrawGizmoBackdrop{
            color: fab.color_editor
        }
        draw_ball: mod.draw.DrawGizmoBall{}
        draw_line: mod.draw.DrawGizmoLine{}
        draw_button: mod.draw.DrawGizmoButton{
            color: fab.color_editor
            color_hover: fab.color_button_hover
            color_down: fab.color_button_down
            color_active: fab.color_button_active
            border_color: fab.color_border
            radius: fab.radius
        }
        draw_text: mod.draw.DrawText{
            text_style: theme.font_bold{
                font_size: fab.font_size_small
            }
            color: #x1a1a1a
        }
        icon_zoom: mod.draw.DrawSvg{
            svg: crate_resource("self://resources/icons/zoom.svg")
            color: fab.color_text
        }
        icon_pan: mod.draw.DrawSvg{
            svg: crate_resource("self://resources/icons/pan.svg")
            color: fab.color_text
        }
        icon_camera: mod.draw.DrawSvg{
            svg: crate_resource("self://resources/icons/camera.svg")
            color: fab.color_text
        }
        icon_ortho: mod.draw.DrawSvg{
            svg: crate_resource("self://resources/icons/ortho.svg")
            color: fab.color_text
        }
        icon_persp: mod.draw.DrawSvg{
            svg: crate_resource("self://resources/icons/persp.svg")
            color: fab.color_text
        }
        color_x: fab.color_vp_axis_x
        color_y: fab.color_vp_axis_y
        color_z: fab.color_vp_axis_z
    }
}

#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawGizmoBall {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    color: Vec4f,
    #[live]
    ghost: f32,
    #[live]
    hover: f32,
}

#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawGizmoLine {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    color: Vec4f,
    #[live]
    p0: Vec2f,
    #[live]
    p1: Vec2f,
    #[live]
    thickness: f32,
}

#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawGizmoBackdrop {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    color: Vec4f,
    #[live]
    hover: f32,
    #[live]
    radius: f32,
    #[live]
    opacity: f32,
}

#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawGizmoButton {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    color: Vec4f,
    #[live]
    color_hover: Vec4f,
    #[live]
    color_down: Vec4f,
    #[live]
    color_active: Vec4f,
    #[live]
    border_color: Vec4f,
    #[live]
    radius: f32,
    #[live]
    hover: f32,
    #[live]
    down: f32,
    #[live]
    active: f32,
    #[live]
    idle_alpha: f32,
}

/// The buttons under the gizmo, top to bottom.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NavButton {
    Zoom,
    Move,
    FrameAll,
    Ortho,
}

impl NavButton {
    const ALL: [NavButton; 4] = [
        NavButton::Zoom,
        NavButton::Move,
        NavButton::FrameAll,
        NavButton::Ortho,
    ];

    fn hint(self, ortho: bool) -> &'static str {
        match self {
            NavButton::Zoom => "Zoom view",
            NavButton::Move => "Pan view",
            NavButton::FrameAll => "Frame all (Home)",
            NavButton::Ortho => {
                if ortho {
                    "Switch to perspective (5)"
                } else {
                    "Switch to orthographic (5)"
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GizmoDrag {
    Orbit,
    Zoom,
    Move,
}

#[derive(Script, ScriptHook, WidgetRef, WidgetRegister)]
pub struct FabNavGizmo {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[live(0)]
    view: usize,
    #[live(96.0)]
    ball_size: f64,
    #[live(20.0)]
    button_size: f64,
    #[live(4.0)]
    button_gap: f64,
    #[live]
    draw_backdrop: DrawGizmoBackdrop,
    #[live]
    draw_ball: DrawGizmoBall,
    #[live]
    draw_line: DrawGizmoLine,
    #[live]
    draw_button: DrawGizmoButton,
    #[live]
    draw_text: DrawText,
    #[live]
    draw_list: DrawList2d,
    #[live]
    icon_zoom: DrawSvg,
    #[live]
    icon_pan: DrawSvg,
    #[live]
    icon_camera: DrawSvg,
    #[live]
    icon_ortho: DrawSvg,
    #[live]
    icon_persp: DrawSvg,
    #[live]
    color_x: Vec4f,
    #[live]
    color_y: Vec4f,
    #[live]
    color_z: Vec4f,
    #[rust]
    area: Area,
    #[rust]
    ball_rect: Rect,
    #[rust]
    balls: Vec<(PresetView, DVec2, f64)>,
    #[rust]
    buttons: Vec<(NavButton, Rect)>,
    #[rust]
    drag: Option<GizmoDrag>,
    #[rust]
    drag_start: Option<DVec2>,
    #[rust]
    last_pos: Option<DVec2>,
    #[rust]
    dragged: bool,
    #[rust]
    hover_ball: Option<usize>,
    #[rust]
    hover_button: Option<NavButton>,
    #[rust]
    press_button: Option<NavButton>,
    #[rust(false)]
    hovered: bool,
}

impl FabNavGizmo {
    /// The full column: the ball box plus the button stack under it.
    fn column_height(&self) -> f64 {
        self.ball_size
            + NavButton::ALL.len() as f64 * (self.button_size + self.button_gap)
    }

    /// View-space direction of a world axis. `z` grows toward the viewer.
    fn axis_screen(cam: &Camera, axis: Vec3f) -> Vec3f {
        let r = cam.right();
        let u = cam.true_up();
        let f = cam.forward();
        vec3(axis.dot(r), axis.dot(u), -axis.dot(f))
    }

    fn ball_radius(&self, depth: f64, ghost: bool) -> f64 {
        let base = if ghost {
            self.ball_size * 0.072
        } else {
            self.ball_size * 0.094
        };
        // Near balls read bigger, which is the whole trick of the widget.
        base * (0.78 + 0.22 * (depth * 0.5 + 0.5))
    }

    fn nearest_ball(&self, pos: DVec2) -> Option<usize> {
        let mut best = None;
        let mut best_d = f64::MAX;
        for (i, (_, p, depth)) in self.balls.iter().enumerate() {
            let d = (pos - *p).length();
            let r = self.ball_radius(*depth, false) + 2.0;
            if d < r && d < best_d {
                best_d = d;
                best = Some(i);
            }
        }
        best
    }

    fn button_at(&self, pos: DVec2) -> Option<NavButton> {
        self.buttons
            .iter()
            .find(|(_, r)| r.contains(pos))
            .map(|(b, _)| *b)
    }

    /// The zoom / move buttons drive the camera straight: they are pure
    /// transforms with no controller state behind them, and this keeps the
    /// locked viewports in step without a round trip through the app.
    fn drag_camera(&self, cx: &mut Cx, state: &mut AppState, drag: GizmoDrag, delta: DVec2) {
        let view = self.view;
        let preset = state.view_at(view).preset;
        let mut cam = state.view_at(view).camera;
        match drag {
            GizmoDrag::Zoom => orbit::dolly(&mut cam, 1.01f32.powf(delta.y as f32), None),
            GizmoDrag::Move => orbit::pan(
                &mut cam,
                delta.x as f32,
                delta.y as f32,
                self.ball_rect.size.y.max(1.0) as f32 * 8.0,
            ),
            GizmoDrag::Orbit => return,
        }
        let vs = state.view_at_mut(view);
        vs.camera = cam;
        vs.mark_camera_changed();
        vs.preset = preset;
        state.sync_locked_cameras(view);
        cx.redraw_all();
    }

    /// Crosshair at the 3D pane centre. Drawn on an overlay list so the
    /// gizmo dock's Fit-height clip does not swallow it.
    fn draw_walk_crosshair(&mut self, cx: &mut Cx2d) {
        let dock = cx.turtle().rect();
        let w = cx
            .find_base_width(Base::Full)
            .filter(|v| v.is_finite() && *v > 8.0)
            .unwrap_or(dock.size.x);
        let h = cx
            .find_base_height(Base::Full)
            .filter(|v| v.is_finite() && *v > 8.0 && *v < 1.0e6)
            .unwrap_or_else(|| {
                let mh = cx.compute_max_height_from_ancestors();
                if mh.is_finite() && mh > 8.0 && mh < 1.0e6 {
                    mh
                } else {
                    dock.size.y.max(120.0)
                }
            });
        let center = dvec2(dock.pos.x + w * 0.5, dock.pos.y + h * 0.5);
        self.draw_list.begin_overlay_reuse(cx);
        let pass = cx.current_pass_size();
        cx.begin_root_turtle(pass, Layout::flow_overlay());
        let r_hi = 5.0;
        let r_lo = 2.0;
        self.draw_ball.ghost = 0.0;
        self.draw_ball.hover = 0.0;
        self.draw_ball.color = vec4(0.04, 0.04, 0.04, 0.9);
        self.draw_ball.draw_abs(
            cx,
            Rect {
                pos: center - dvec2(r_hi, r_hi),
                size: dvec2(r_hi * 2.0, r_hi * 2.0),
            },
        );
        self.draw_ball.color = vec4(1.0, 1.0, 1.0, 0.95);
        self.draw_ball.draw_abs(
            cx,
            Rect {
                pos: center - dvec2(r_lo, r_lo),
                size: dvec2(r_lo * 2.0, r_lo * 2.0),
            },
        );
        cx.end_turtle();
        self.draw_list.end(cx);
    }
}

impl WidgetNode for FabNavGizmo {
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }
    fn walk(&mut self, _cx: &mut Cx) -> Walk {
        Walk {
            height: Size::Fixed(self.column_height()),
            ..self.walk
        }
    }
    fn area(&self) -> Area {
        self.area
    }
    fn redraw(&mut self, cx: &mut Cx) {
        self.area.redraw(cx);
    }
}

impl Widget for FabNavGizmo {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        // The navigator lives on the viewport widget, so window-blur cannot
        // be delivered as ViewportInput. Unlock the OS pointer here and
        // post a mailbox the navigator consumes on the next frame.
        match event {
            Event::WindowLostFocus(_) | Event::Pause | Event::Background => {
                walk::request_capture_release();
                cx.lock_mouse_pointer(false);
                cx.set_cursor(MouseCursor::Default);
            }
            _ => {}
        }
        match event.hits(cx, self.area) {
            Hit::FingerDown(fe) => {
                self.drag_start = Some(fe.abs);
                self.last_pos = Some(fe.abs);
                self.dragged = false;
                self.press_button = self.button_at(fe.abs);
                self.drag = match self.press_button {
                    Some(NavButton::Zoom) => Some(GizmoDrag::Zoom),
                    Some(NavButton::Move) => Some(GizmoDrag::Move),
                    Some(_) => None,
                    None => Some(GizmoDrag::Orbit),
                };
                self.area.redraw(cx);
            }
            Hit::FingerMove(fe) => {
                let Some(last) = self.last_pos else {
                    return;
                };
                let delta = fe.abs - last;
                self.last_pos = Some(fe.abs);
                if let Some(start) = self.drag_start {
                    if (fe.abs - start).length() > 3.0 {
                        self.dragged = true;
                    }
                }
                if !self.dragged {
                    return;
                }
                match self.drag {
                    Some(GizmoDrag::Orbit) => {
                        cx.action(ShellAction::OrbitBy(
                            self.view,
                            delta.x as f32,
                            delta.y as f32,
                        ));
                        cx.set_cursor(MouseCursor::Grabbing);
                    }
                    Some(drag) => {
                        if let Some(state) = scope.data.get_mut::<AppState>() {
                            self.drag_camera(cx, state, drag, delta);
                        }
                        cx.set_cursor(if drag == GizmoDrag::Move {
                            MouseCursor::Move
                        } else {
                            MouseCursor::NsResize
                        });
                    }
                    None => {}
                }
            }
            Hit::FingerUp(fe) => {
                if !self.dragged {
                    match self.press_button {
                        Some(NavButton::FrameAll) => {
                            cx.action(ShellAction::FrameAll(self.view));
                        }
                        Some(NavButton::Ortho) => {
                            cx.action(ShellAction::ToggleOrtho(self.view));
                        }
                        Some(_) => {}
                        None => {
                            if let Some(i) = self.nearest_ball(fe.abs) {
                                cx.action(ShellAction::PresetView(self.view, self.balls[i].0));
                            }
                        }
                    }
                }
                self.drag = None;
                self.drag_start = None;
                self.last_pos = None;
                self.dragged = false;
                self.press_button = None;
                cx.set_cursor(MouseCursor::Default);
                self.area.redraw(cx);
            }
            Hit::FingerHoverIn(fh) | Hit::FingerHoverOver(fh) => {
                let ball = self.nearest_ball(fh.abs);
                let button = self.button_at(fh.abs);
                if ball != self.hover_ball || button != self.hover_button || !self.hovered {
                    if button != self.hover_button {
                        if self.hover_button.is_some() {
                            cx.widget_action(self.uid, TipAction::HoverOut);
                        }
                        if let Some(b) = button {
                            let ortho = scope
                                .data
                                .get_mut::<AppState>()
                                .map(|s| s.view_at(self.view).camera.ortho)
                                .unwrap_or(false);
                            if let Some((_, rect)) = self.buttons.iter().find(|(item, _)| *item == b) {
                                cx.widget_action(
                                    self.uid,
                                    TipAction::HoverIn(b.hint(ortho).to_string(), *rect),
                                );
                            }
                        }
                    }
                    self.hover_ball = ball;
                    self.hover_button = button;
                    self.hovered = true;
                    if let Some(b) = button {
                        let ortho = scope
                            .data
                            .get_mut::<AppState>()
                            .map(|s| s.view_at(self.view).camera.ortho)
                            .unwrap_or(false);
                        cx.action(ShellAction::StatusHint(b.hint(ortho).to_string()));
                    }
                    self.area.redraw(cx);
                }
                cx.set_cursor(if ball.is_some() || button.is_some() {
                    MouseCursor::Hand
                } else {
                    MouseCursor::Grab
                });
            }
            Hit::FingerHoverOut(_) => {
                if self.hover_button.is_some() {
                    cx.widget_action(self.uid, TipAction::HoverOut);
                }
                self.hover_ball = None;
                self.hover_button = None;
                self.hovered = false;
                self.area.redraw(cx);
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let walk = Walk {
            height: Size::Fixed(self.column_height()),
            ..walk
        };
        let outer = cx.walk_turtle(walk);
        let Some(state) = scope.data.get_mut::<AppState>() else {
            return DrawStep::done();
        };
        if !state.view_at(self.view).overlays.nav_gizmo {
            self.area = Area::Empty;
            self.balls.clear();
            self.buttons.clear();
            return DrawStep::done();
        }
        let cam = state.view_at(self.view).camera;
        let ortho = cam.ortho;
        let walk_hud = state.view_at(self.view).nav_mode == NavMode::Walk;

        // Corner-pin: the widget owns the whole width so the parent's far-edge
        // align has no slack to shift the vector geometry with.
        let size = self.ball_size;
        let x0 = outer.pos.x + (outer.size.x - size).max(0.0);
        self.ball_rect = Rect {
            pos: dvec2(x0, outer.pos.y),
            size: dvec2(size, size),
        };
        let panel = Rect {
            pos: self.ball_rect.pos,
            size: dvec2(size, self.column_height()),
        };

        // Backdrop first, so the balls sit on it. It spans the whole column
        // (its shader only paints the disc at the top) and so it is also the
        // widget's hit area: ball box plus button stack, one rect.
        self.draw_backdrop.hover = if self.hovered { 1.0 } else { 0.0 };
        self.draw_backdrop.radius = (size * 0.5) as f32;
        self.draw_backdrop.draw_abs(cx, panel);
        self.area = self.draw_backdrop.area();

        let center = self.ball_rect.pos + self.ball_rect.size * 0.5;
        let radius = size * 0.5 - self.ball_size * 0.115;
        let axes = [
            (PresetView::Right, vec3(1.0, 0.0, 0.0), self.color_x, "X", false),
            (PresetView::Back, vec3(0.0, 1.0, 0.0), self.color_y, "Y", false),
            (PresetView::Top, vec3(0.0, 0.0, 1.0), self.color_z, "Z", false),
            (PresetView::Left, vec3(-1.0, 0.0, 0.0), self.color_x, "", true),
            (PresetView::Front, vec3(0.0, -1.0, 0.0), self.color_y, "", true),
            (PresetView::Bottom, vec3(0.0, 0.0, -1.0), self.color_z, "", true),
        ];
        let mut items: Vec<(PresetView, DVec2, f64, Vec4f, &str, bool)> = axes
            .iter()
            .map(|(p, a, c, l, ghost)| {
                let s = Self::axis_screen(&cam, *a);
                let pos = center + dvec2(s.x as f64 * radius, -(s.y as f64) * radius);
                (*p, pos, s.z as f64, *c, *l, *ghost)
            })
            .collect();
        items.sort_by(|a, b| a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal));

        // One anti-aliased quad per positive axis, not a trail of dots.
        for (_, pos, _, color, _, ghost) in &items {
            if *ghost {
                continue;
            }
            let d = *pos - center;
            if d.length() < 1.0 {
                continue;
            }
            let pad = 3.0;
            let min = dvec2(center.x.min(pos.x), center.y.min(pos.y)) - dvec2(pad, pad);
            let max = dvec2(center.x.max(pos.x), center.y.max(pos.y)) + dvec2(pad, pad);
            let rect = Rect {
                pos: min,
                size: max - min,
            };
            self.draw_line.color = *color;
            self.draw_line.p0 = vec2((center.x - min.x) as f32, (center.y - min.y) as f32);
            self.draw_line.p1 = vec2((pos.x - min.x) as f32, (pos.y - min.y) as f32);
            self.draw_line.thickness = 1.6;
            self.draw_line.draw_abs(cx, rect);
        }

        self.balls.clear();
        for (i, (preset, pos, depth, color, label, ghost)) in items.iter().enumerate() {
            let r = self.ball_radius(*depth, *ghost);
            self.draw_ball.color = *color;
            self.draw_ball.ghost = if *ghost { 1.0 } else { 0.0 };
            self.draw_ball.hover = if self.hover_ball == Some(i) { 1.0 } else { 0.0 };
            self.draw_ball.draw_abs(
                cx,
                Rect {
                    pos: *pos - dvec2(r, r),
                    size: dvec2(r * 2.0, r * 2.0),
                },
            );
            if !label.is_empty() {
                self.draw_text
                    .draw_abs(cx, *pos + dvec2(-r * 0.38, -r * 0.75), label);
            }
            self.balls.push((*preset, *pos, *depth));
        }

        // The button column, right-aligned under the balls.
        self.buttons.clear();
        let bs = self.button_size;
        let mut y = self.ball_rect.pos.y + size + self.button_gap;
        for button in NavButton::ALL {
            let rect = Rect {
                pos: dvec2(panel.pos.x + size - bs, y),
                size: dvec2(bs, bs),
            };
            let hovered = self.hover_button == Some(button);
            self.draw_button.hover = if hovered { 1.0 } else { 0.0 };
            self.draw_button.down = if self.press_button == Some(button) { 1.0 } else { 0.0 };
            self.draw_button.active = if button == NavButton::Ortho && ortho { 1.0 } else { 0.0 };
            self.draw_button.draw_abs(cx, rect);
            let inset = 3.0;
            let icon_rect = Rect {
                pos: rect.pos + dvec2(inset, inset),
                size: rect.size - dvec2(inset * 2.0, inset * 2.0),
            };
            let icon = match button {
                NavButton::Zoom => &mut self.icon_zoom,
                NavButton::Move => &mut self.icon_pan,
                NavButton::FrameAll => &mut self.icon_camera,
                NavButton::Ortho => {
                    if ortho {
                        &mut self.icon_ortho
                    } else {
                        &mut self.icon_persp
                    }
                }
            };
            icon.draw_abs(cx, icon_rect);
            self.buttons.push((button, rect));
            y += bs + self.button_gap;
        }

        if walk_hud {
            self.draw_walk_crosshair(cx);
        }

        DrawStep::done()
    }
}
