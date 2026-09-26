//! A 3D transform gizmo and an orientation gizmo, as widgets that sit over a
//! 3D view.
//!
//! [`Gizmo3d`] moves, turns and scales one model matrix; [`ViewCube`] turns
//! the camera. Neither draws or knows the scene: the app hands them the
//! matrices it renders with and they hand back new ones through
//! [`GizmoAction`]. Put them in an `Overlay` flow over whatever draws the 3D
//! content, the `Gizmo3d` filling it so its viewport is the scene's.
//!
//! The picking, dragging, snapping and the picture itself are the pure math
//! of `makepad-gizmo` (re-exported here as [`makepad_gizmo`]); this file
//! paints its screen-space primitives with three small SDF shaders and
//! routes the pointer.
//!
//! # Getting the pointer only where it belongs
//!
//! The transform gizmo fills the whole view, but it only answers the pointer
//! over one of its handles: its hit test IS the gizmo's pick. A press
//! anywhere else falls through to the scene underneath, so the app's own
//! camera controls keep working around it. A press on a handle captures the
//! pointer for the drag. Escape during a drag puts the model back where the
//! drag found it. Holding the primary modifier (Ctrl, or Cmd on macOS)
//! inverts `snap` for as long as it is held.
//!
//! # Wiring it up
//!
//! ```ignore
//! let gizmo = self.ui.gizmo3d(cx, ids!(gizmo));
//! gizmo.set_perspective(cx, view, 45.0, 0.1, 1000.0);
//! gizmo.set_model(cx, model);
//! // ...and in handle_actions:
//! if let Some(model) = gizmo.changed(actions) { /* move the object */ }
//! if let Some(view) = self.ui.view_cube(cx, ids!(cube)).view_changed(actions) {
//!     /* move the camera, and hand the new view to both gizmos */
//! }
//! ```
use crate::{makepad_derive_widget::*, makepad_draw::*, widget::*};
pub use makepad_gizmo;
use makepad_gizmo::{
    interpolate_view, look_from, orbit_view, GizmoCamera, GizmoDragInfo, GizmoHandle, GizmoMode,
    GizmoPrim, GizmoRect, GizmoSnap, GizmoSpace, UpAxis, ViewCubeStyle, ViewDir,
};

type GizmoModel = makepad_gizmo::Gizmo;
type ViewCubeModel = makepad_gizmo::ViewCube;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.Gizmo3dMode = #(Gizmo3dMode::script_api(vm))
    mod.widgets.Gizmo3dSpace = #(Gizmo3dSpace::script_api(vm))
    mod.widgets.ViewCubeShape = #(ViewCubeShape::script_api(vm))
    mod.widgets.ViewCubeUp = #(ViewCubeUp::script_api(vm))

    // A straight stroke with round ends, over the segment's bounding box.
    set_type_default() do #(DrawGizmoStroke::script_shader(vm)){
        ..mod.draw.DrawQuad
        pixel: fn(){
            let p = self.pos * self.rect_size
            let pa = p - self.seg_a
            let ba = self.seg_b - self.seg_a
            let h = clamp(dot(pa, ba) / max(dot(ba, ba), 0.0001), 0.0, 1.0)
            let d = length(pa - ba * h)
            let half = max(self.width * 0.5, 0.5)
            let alpha = clamp(half - d + 0.5, 0.0, 1.0) * min(self.width, 1.0) * self.color.a
            return vec4(self.color.rgb * alpha, alpha)
        }
    }

    // A disc and/or a ring, optionally cut to an arc. Angles run from +x
    // toward +y, which on a y-down screen is clockwise.
    set_type_default() do #(DrawGizmoDisc::script_shader(vm)){
        ..mod.draw.DrawQuad
        pixel: fn(){
            let p = self.pos * self.rect_size - self.center
            let r = length(p)
            let d = r - self.radius
            var mask = 1.0
            if self.sweep < 6.283 {
                var a = atan2(p.y, p.x) - self.start
                if a < 0.0 {
                    a = a + 6.2831853
                }
                if a < 0.0 {
                    a = a + 6.2831853
                }
                // Signed distance to the arc's ends, in points along it,
                // so both ends feather like the edges do.
                var s = 0.0 - min(a - self.sweep, 6.2831853 - a) * r
                if a <= self.sweep {
                    s = min(a, self.sweep - a) * r
                }
                mask = clamp(s + 0.5, 0.0, 1.0)
            }
            let fa = clamp(0.5 - d, 0.0, 1.0) * self.fill_color.a
            let half = max(self.width * 0.5, 0.5)
            let sa = clamp(half - abs(d) + 0.5, 0.0, 1.0) * min(self.width, 1.0) * self.color.a
            let rgb = self.color.rgb * sa + self.fill_color.rgb * (fa * (1.0 - sa))
            let alpha = sa + fa * (1.0 - sa)
            return vec4(rgb * mask, alpha * mask)
        }
    }

    // A convex quad, filled and outlined; a triangle repeats its last point.
    set_type_default() do #(DrawGizmoFace::script_shader(vm)){
        ..mod.draw.DrawQuad
        edge_dist: fn(a: vec2, b: vec2, p: vec2, s: float) -> float {
            let e = b - a
            let l = length(e)
            var d = s * (e.x * (p.y - a.y) - e.y * (p.x - a.x)) / max(l, 0.001)
            // A repeated point is no edge at all.
            if l < 0.001 {
                d = 100000.0
            }
            return d
        }
        pixel: fn(){
            let p = self.pos * self.rect_size
            let e1 = self.p1 - self.p0
            let e2 = self.p2 - self.p0
            let e3 = self.p3 - self.p0
            let twice_area = e1.x * e2.y - e1.y * e2.x + e2.x * e3.y - e2.y * e3.x
            var s = 1.0
            if twice_area < 0.0 {
                s = 0.0 - 1.0
            }
            var d = self.edge_dist(self.p0, self.p1, p, s)
            d = min(d, self.edge_dist(self.p1, self.p2, p, s))
            d = min(d, self.edge_dist(self.p2, self.p3, p, s))
            d = min(d, self.edge_dist(self.p3, self.p0, p, s))
            let fa = clamp(d + 0.5, 0.0, 1.0) * self.fill_color.a
            let half = max(self.width * 0.5, 0.5)
            let sa = clamp(half - abs(d) + 0.5, 0.0, 1.0) * min(self.width, 1.0) * self.color.a
            let rgb = self.color.rgb * sa + self.fill_color.rgb * (fa * (1.0 - sa))
            let alpha = sa + fa * (1.0 - sa)
            return vec4(rgb, alpha)
        }
    }

    mod.widgets.Gizmo3dBase = #(Gizmo3d::register_widget(vm))

    /** A translate / rotate / scale gizmo for one model matrix, drawn over a
     * 3D view. Lay it over the view with Fill so its viewport is the view's;
     * it only takes the pointer over its handles. */
    mod.widgets.Gizmo3d = set_type_default() do mod.widgets.Gizmo3dBase{
        width: Fill
        height: Fill
        /** Translate, Rotate, Scale or Universal */
        mode: mod.widgets.Gizmo3dMode.Universal
        /** World or Local axes for translate and rotate; scale is always local */
        space: mod.widgets.Gizmo3dSpace.World
        /** snap while dragging; the primary modifier inverts it 0..1 step 1 */
        snap: false
        /** translation step in world units 0..10 step 0.05 */
        snap_translate: 0.25
        /** rotation step in degrees 0..90 step 1 */
        snap_rotate: 15.0
        /** scale-factor step 0..1 step 0.01 */
        snap_scale: 0.1
        /** axis length on screen in points 40..240 step 1 */
        size: 96.0
        /** stroke width of arrows and rings 1..6 step 0.5 */
        line_width: 2.5
        /** point arrows at the half of each axis facing the camera 0..1 step 1 */
        flip_axes: true
        /** show the drag's value under the gizmo 0..1 step 1 */
        show_readout: true
        color_x: #xee4a4f
        color_y: #x78cc45
        color_z: #x458af5
        color_screen: #xebebeb
        color_highlight: #xffd445
        color_text: #xffffff
        color_text_bg: #x00000099
        draw_text +: {
            text_style: theme.font_regular{font_size: 9.0}
            color: #xffffff
        }
    }

    mod.widgets.ViewCubeBase = #(ViewCube::register_widget(vm))

    /** An orientation gizmo: a view cube, or a ball of labelled axes. A
     * click turns the camera to that view (again, to the opposite one); a
     * drag orbits it about the pivot. */
    mod.widgets.ViewCube = set_type_default() do mod.widgets.ViewCubeBase{
        width: 104
        height: 104
        /** Cube (faces, edges and corners) or Balls (six axis ends) */
        shape: mod.widgets.ViewCubeShape.Cube
        /** the world's up axis: Y or Z */
        up: mod.widgets.ViewCubeUp.Y
        /** name the faces and the axes 0..1 step 1 */
        labels: true
        /** seconds a click takes to turn the camera; 0 jumps 0..2 step 0.05 */
        snap_seconds: 0.3
        /** radians of orbit per point of drag 0.001..0.05 step 0.001 */
        orbit_speed: 0.01
        color_x: #xee4a4f
        color_y: #x78cc45
        color_z: #x458af5
        color_face: #x4d525cf0
        color_edge: #x0f0f14e6
        color_highlight: #xffd445
        color_text: #xf2f2f2
        color_ball_text: #x0f0f14
        color_backdrop: #x8c919e38
        draw_text +: {
            text_style: theme.font_bold{font_size: 8.0}
            color: #xf2f2f2
        }
    }
}

/// What the transform gizmo manipulates.
#[derive(Copy, Clone, Debug, PartialEq, Script, ScriptHook, Default)]
pub enum Gizmo3dMode {
    Translate,
    Rotate,
    Scale,
    #[pick]
    #[default]
    Universal,
}

/// Which axes the transform gizmo's translate and rotate handles follow.
#[derive(Copy, Clone, Debug, PartialEq, Script, ScriptHook, Default)]
pub enum Gizmo3dSpace {
    #[pick]
    #[default]
    World,
    Local,
}

/// The orientation gizmo's picture.
#[derive(Copy, Clone, Debug, PartialEq, Script, ScriptHook, Default)]
pub enum ViewCubeShape {
    #[pick]
    #[default]
    Cube,
    Balls,
}

/// The world's up axis, for the orientation gizmo.
#[derive(Copy, Clone, Debug, PartialEq, Script, ScriptHook, Default)]
pub enum ViewCubeUp {
    #[pick]
    #[default]
    Y,
    Z,
}

impl From<Gizmo3dMode> for GizmoMode {
    fn from(m: Gizmo3dMode) -> Self {
        match m {
            Gizmo3dMode::Translate => GizmoMode::Translate,
            Gizmo3dMode::Rotate => GizmoMode::Rotate,
            Gizmo3dMode::Scale => GizmoMode::Scale,
            Gizmo3dMode::Universal => GizmoMode::Universal,
        }
    }
}

impl From<Gizmo3dSpace> for GizmoSpace {
    fn from(s: Gizmo3dSpace) -> Self {
        match s {
            Gizmo3dSpace::World => GizmoSpace::World,
            Gizmo3dSpace::Local => GizmoSpace::Local,
        }
    }
}

impl From<ViewCubeUp> for UpAxis {
    fn from(u: ViewCubeUp) -> Self {
        match u {
            ViewCubeUp::Y => UpAxis::Y,
            ViewCubeUp::Z => UpAxis::Z,
        }
    }
}

/// What the gizmos report.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum GizmoAction {
    #[default]
    None,
    /// A handle was pressed and a drag began.
    DragStart,
    /// The drag moved the model to this matrix. An Escape that cancels the
    /// drag sends the matrix it started from.
    Changed(Mat4f),
    /// The drag finished, or was cancelled.
    DragEnd,
    /// The orientation gizmo turned the camera to this view matrix.
    ViewChanged(Mat4f),
}

/// A straight stroke from `seg_a` to `seg_b`, both relative to the quad.
#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawGizmoStroke {
    #[deref]
    pub draw_super: DrawQuad,
    #[live]
    pub seg_a: Vec2f,
    #[live]
    pub seg_b: Vec2f,
    #[live]
    pub color: Vec4f,
    #[live(2.0)]
    pub width: f32,
}

/// A disc, ring or arc about `center`, relative to the quad.
#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawGizmoDisc {
    #[deref]
    pub draw_super: DrawQuad,
    #[live]
    pub center: Vec2f,
    #[live]
    pub radius: f32,
    #[live]
    pub width: f32,
    #[live]
    pub color: Vec4f,
    #[live]
    pub fill_color: Vec4f,
    #[live]
    pub start: f32,
    #[live(7.0)]
    pub sweep: f32,
}

/// A convex quad `p0..p3`, relative to the quad it is drawn in.
#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawGizmoFace {
    #[deref]
    pub draw_super: DrawQuad,
    #[live]
    pub p0: Vec2f,
    #[live]
    pub p1: Vec2f,
    #[live]
    pub p2: Vec2f,
    #[live]
    pub p3: Vec2f,
    #[live]
    pub fill_color: Vec4f,
    #[live]
    pub color: Vec4f,
    #[live]
    pub width: f32,
}

fn text_size(dt: &DrawText, cx: &mut Cx2d, text: &str) -> DVec2 {
    let laidout = dt.layout(cx, 0.0, 0.0, None, false, Align::default(), text);
    let scale = dt.font_scale as f64;
    dvec2(
        laidout.size_in_lpxs.width as f64 * scale,
        laidout.size_in_lpxs.height as f64 * scale,
    )
}

fn rect_between(lo: Vec2f, hi: Vec2f) -> Rect {
    Rect {
        pos: dvec2(lo.x as f64, lo.y as f64),
        size: dvec2((hi.x - lo.x).max(1.0) as f64, (hi.y - lo.y).max(1.0) as f64),
    }
}

/// Paint a gizmo's picture, first primitive to last. Shared by both gizmo
/// widgets, and usable by an app that draws simple overlays (a wireframe, a
/// grid) with the same primitives.
pub fn paint_gizmo_prims(
    cx: &mut Cx2d,
    prims: &[GizmoPrim],
    line: &mut DrawGizmoStroke,
    disc: &mut DrawGizmoDisc,
    face: &mut DrawGizmoFace,
    text: &mut DrawText,
) {
    for prim in prims {
        let (lo, hi) = prim.bounds();
        match prim {
            GizmoPrim::Line { a, b, width, color } => {
                if color.w <= 0.002 {
                    continue;
                }
                line.seg_a = *a - lo;
                line.seg_b = *b - lo;
                line.width = *width;
                line.color = *color;
                line.draw_abs(cx, rect_between(lo, hi));
            }
            GizmoPrim::Circle {
                center,
                radius,
                width,
                color,
                fill,
            } => {
                disc.center = *center - lo;
                disc.radius = *radius;
                disc.width = *width;
                disc.color = *color;
                disc.fill_color = *fill;
                disc.start = 0.0;
                disc.sweep = 7.0;
                disc.draw_abs(cx, rect_between(lo, hi));
            }
            GizmoPrim::Arc {
                center,
                radius,
                start,
                sweep,
                width,
                color,
            } => {
                let (start, sweep) = if *sweep < 0.0 {
                    (start + sweep, -sweep)
                } else {
                    (*start, *sweep)
                };
                disc.center = *center - lo;
                disc.radius = *radius;
                disc.width = *width;
                disc.color = *color;
                disc.fill_color = vec4(0.0, 0.0, 0.0, 0.0);
                disc.start = start.rem_euclid(std::f32::consts::TAU);
                disc.sweep = sweep.min(7.0);
                disc.draw_abs(cx, rect_between(lo, hi));
            }
            GizmoPrim::Quad {
                pts,
                fill,
                width,
                color,
            } => {
                face.p0 = pts[0] - lo;
                face.p1 = pts[1] - lo;
                face.p2 = pts[2] - lo;
                face.p3 = pts[3] - lo;
                face.fill_color = *fill;
                face.color = *color;
                face.width = *width;
                face.draw_abs(cx, rect_between(lo, hi));
            }
            GizmoPrim::Text {
                pos,
                text: s,
                color,
                align,
                backdrop,
            } => {
                if s.is_empty() {
                    continue;
                }
                let size = text_size(text, cx, s);
                let top_left = dvec2(
                    pos.x as f64 - size.x * align.x as f64,
                    pos.y as f64 - size.y * align.y as f64,
                );
                if backdrop.w > 0.0 {
                    let pad = dvec2(5.0, 3.0);
                    let a = top_left - pad;
                    let b = top_left + size + pad;
                    let (a, b) = (vec2(a.x as f32, a.y as f32), vec2(b.x as f32, b.y as f32));
                    let bg = GizmoPrim::Quad {
                        pts: [a, vec2(b.x, a.y), b, vec2(a.x, b.y)],
                        fill: *backdrop,
                        width: 0.0,
                        color: *backdrop,
                    };
                    paint_gizmo_prims(cx, &[bg], line, disc, face, text);
                }
                text.color = *color;
                text.draw_abs(cx, top_left, s);
            }
        }
    }
}

fn gizmo_rect(r: Rect) -> GizmoRect {
    GizmoRect::new(
        r.pos.x as f32,
        r.pos.y as f32,
        r.size.x as f32,
        r.size.y as f32,
    )
}

fn vec2_of(p: DVec2) -> Vec2f {
    vec2(p.x as f32, p.y as f32)
}

fn positive(v: f64) -> Option<f32> {
    if v > 0.0 {
        Some(v as f32)
    } else {
        None
    }
}

/// How the transform gizmo projects: a matrix the app built, or a
/// perspective it builds from the widget's own aspect at every draw.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GizmoProjection {
    Matrix(Mat4f),
    Perspective { fov_y_deg: f32, near: f32, far: f32 },
}

impl Default for GizmoProjection {
    fn default() -> Self {
        GizmoProjection::Perspective {
            fov_y_deg: 45.0,
            near: 0.1,
            far: 1000.0,
        }
    }
}

impl GizmoProjection {
    /// The projection matrix for a viewport of this size.
    pub fn matrix(&self, viewport: GizmoRect) -> Mat4f {
        match *self {
            GizmoProjection::Matrix(m) => m,
            GizmoProjection::Perspective {
                fov_y_deg,
                near,
                far,
            } => Mat4f::perspective(fov_y_deg, viewport.w / viewport.h.max(1.0), near, far),
        }
    }
}

/// The transform gizmo widget. See the module documentation.
#[derive(Script, ScriptHook, WidgetRef, WidgetRegister)]
pub struct Gizmo3d {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[live]
    draw_line: DrawGizmoStroke,
    #[live]
    draw_disc: DrawGizmoDisc,
    #[live]
    draw_face: DrawGizmoFace,
    #[live]
    draw_text: DrawText,
    /// Translate, Rotate, Scale or Universal.
    #[live]
    pub mode: Gizmo3dMode,
    /// World or Local axes for translate and rotate.
    #[live]
    pub space: Gizmo3dSpace,
    /// Snap while dragging. The primary modifier inverts it.
    #[live]
    pub snap: bool,
    /// Translation step in world units; zero never snaps translation.
    #[live(0.25)]
    pub snap_translate: f64,
    /// Rotation step in degrees; zero never snaps rotation.
    #[live(15.0)]
    pub snap_rotate: f64,
    /// Scale-factor step; zero never snaps scale.
    #[live(0.1)]
    pub snap_scale: f64,
    /// Axis length on screen, in points.
    #[live(96.0)]
    pub size: f64,
    #[live(2.5)]
    pub line_width: f64,
    #[live(true)]
    pub flip_axes: bool,
    #[live(true)]
    pub show_readout: bool,
    #[live]
    pub color_x: Vec4f,
    #[live]
    pub color_y: Vec4f,
    #[live]
    pub color_z: Vec4f,
    #[live]
    pub color_screen: Vec4f,
    #[live]
    pub color_highlight: Vec4f,
    #[live]
    pub color_text: Vec4f,
    #[live]
    pub color_text_bg: Vec4f,
    #[rust]
    gizmo: GizmoModel,
    #[rust]
    view: Mat4f,
    #[rust]
    projection: GizmoProjection,
    #[rust]
    model: Mat4f,
    #[rust]
    hover: Option<GizmoHandle>,
    #[rust]
    area: Area,
    #[rust]
    cancel_scope: Option<CancelScope>,
}

impl Gizmo3d {
    /// Copy the live properties onto the gizmo math.
    fn sync(&mut self) {
        let g = &mut self.gizmo;
        g.mode = self.mode.into();
        g.space = self.space.into();
        g.size_px = self.size.max(8.0) as f32;
        g.flip_axes = self.flip_axes;
        g.show_text = self.show_readout;
        g.style.axis_colors = [self.color_x, self.color_y, self.color_z];
        g.style.screen_color = self.color_screen;
        g.style.highlight = self.color_highlight;
        g.style.text_color = self.color_text;
        g.style.text_backdrop = self.color_text_bg;
        g.style.line_width = self.line_width.max(0.5) as f32;
    }

    fn snap_for(&self, modifiers: &KeyModifiers) -> GizmoSnap {
        if self.snap != modifiers.is_primary() {
            GizmoSnap {
                translate: positive(self.snap_translate),
                rotate_deg: positive(self.snap_rotate),
                scale: positive(self.snap_scale),
            }
        } else {
            GizmoSnap::default()
        }
    }

    fn camera_for(&self, rect: Rect) -> GizmoCamera {
        let viewport = gizmo_rect(rect);
        GizmoCamera::new(self.view, self.projection.matrix(viewport), viewport)
    }

    /// The camera the scene is drawn with: a view and a projection matrix.
    pub fn set_camera(&mut self, cx: &mut Cx, view: Mat4f, proj: Mat4f) {
        self.view = view;
        self.projection = GizmoProjection::Matrix(proj);
        self.area.redraw(cx);
    }

    /// The camera as a view matrix and a perspective the widget completes
    /// with its own aspect ratio.
    pub fn set_perspective(
        &mut self,
        cx: &mut Cx,
        view: Mat4f,
        fov_y_deg: f32,
        near: f32,
        far: f32,
    ) {
        self.view = view;
        self.projection = GizmoProjection::Perspective {
            fov_y_deg,
            near,
            far,
        };
        self.area.redraw(cx);
    }

    /// A new view matrix, keeping the projection.
    pub fn set_view(&mut self, cx: &mut Cx, view: Mat4f) {
        self.view = view;
        self.area.redraw(cx);
    }

    pub fn view(&self) -> Mat4f {
        self.view
    }

    /// The model matrix to manipulate.
    pub fn set_model(&mut self, cx: &mut Cx, model: Mat4f) {
        self.model = model;
        self.area.redraw(cx);
    }

    pub fn model(&self) -> Mat4f {
        self.model
    }

    pub fn set_mode(&mut self, cx: &mut Cx, mode: Gizmo3dMode) {
        self.mode = mode;
        self.area.redraw(cx);
    }

    pub fn set_space(&mut self, cx: &mut Cx, space: Gizmo3dSpace) {
        self.space = space;
        self.area.redraw(cx);
    }

    pub fn is_dragging(&self) -> bool {
        self.gizmo.is_dragging()
    }

    /// The handle under the pointer.
    pub fn hovered(&self) -> Option<GizmoHandle> {
        self.hover
    }

    /// What the drag in progress has done so far.
    pub fn drag_info(&self) -> Option<GizmoDragInfo> {
        self.gizmo.drag_info()
    }

    fn finish_drag(&mut self, cx: &mut Cx) {
        if let Some(scope) = self.cancel_scope.take() {
            cx.end_cancel_scope(scope);
        }
        cx.widget_action(self.uid, GizmoAction::DragEnd);
        cx.set_cursor(MouseCursor::Default);
        self.area.redraw(cx);
    }
}

impl WidgetNode for Gizmo3d {
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

impl Widget for Gizmo3d {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if let Event::KeyDown(ke) = event {
            if ke.key_code == KeyCode::Escape
                && self.gizmo.is_dragging()
                && self
                    .cancel_scope
                    .as_ref()
                    .is_some_and(|s| cx.owns_cancel(s))
            {
                if let Some(start) = self.gizmo.cancel_drag() {
                    self.model = start;
                    cx.widget_action(self.uid, GizmoAction::Changed(start));
                }
                self.finish_drag(cx);
                return;
            }
        }
        if !self.area.is_valid(cx) {
            return;
        }
        self.sync();
        let cam = self.camera_for(self.area.rect(cx));
        let area = self.area;
        // The hit test is the pick: a press off every handle is not ours and
        // goes on to the scene underneath.
        let hit = {
            let this = &*self;
            event.hits_with_test(cx, area, |abs, _, _| {
                this.gizmo.hover(&cam, &this.model, vec2_of(abs)).is_some()
            })
        };
        match hit {
            Hit::FingerHoverIn(fh) | Hit::FingerHoverOver(fh) => {
                if self.gizmo.is_dragging() {
                    return;
                }
                let h = self.gizmo.hover(&cam, &self.model, vec2_of(fh.abs));
                if h != self.hover {
                    self.hover = h;
                    self.area.redraw(cx);
                }
                cx.set_cursor(MouseCursor::Hand);
            }
            Hit::FingerHoverOut(_) => {
                if self.hover.is_some() && !self.gizmo.is_dragging() {
                    self.hover = None;
                    self.area.redraw(cx);
                }
            }
            Hit::FingerDown(fe) if fe.device.is_primary_hit() => {
                self.gizmo.snap = self.snap_for(&fe.modifiers);
                if let Some(h) = self.gizmo.begin_drag(&cam, &self.model, vec2_of(fe.abs)) {
                    self.hover = Some(h);
                    if self.cancel_scope.is_none() {
                        self.cancel_scope = Some(self.begin_cancel_scope(cx));
                    }
                    cx.widget_action(self.uid, GizmoAction::DragStart);
                    cx.set_cursor(MouseCursor::Grabbing);
                    self.area.redraw(cx);
                }
            }
            Hit::FingerMove(fe) => {
                if !self.gizmo.is_dragging() {
                    return;
                }
                self.gizmo.snap = self.snap_for(&fe.modifiers);
                if let Some(model) = self.gizmo.drag(&cam, vec2_of(fe.abs)) {
                    if model != self.model {
                        self.model = model;
                        cx.widget_action(self.uid, GizmoAction::Changed(model));
                    }
                }
                cx.set_cursor(MouseCursor::Grabbing);
                self.area.redraw(cx);
            }
            Hit::FingerUp(fe) => {
                if !self.gizmo.is_dragging() {
                    return;
                }
                if let Some(model) = self.gizmo.end_drag() {
                    if model != self.model {
                        self.model = model;
                        cx.widget_action(self.uid, GizmoAction::Changed(model));
                    }
                }
                self.hover = self.gizmo.hover(&cam, &self.model, vec2_of(fe.abs));
                self.finish_drag(cx);
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle_with_area(&mut self.area, walk);
        self.sync();
        let cam = self.camera_for(rect);
        let prims = self.gizmo.geometry(&cam, &self.model, self.hover);
        paint_gizmo_prims(
            cx,
            &prims,
            &mut self.draw_line,
            &mut self.draw_disc,
            &mut self.draw_face,
            &mut self.draw_text,
        );
        DrawStep::done()
    }

    fn snapshot_value(&self, _cx: &Cx) -> Option<String> {
        let t = makepad_gizmo::mat_translation(&self.model);
        let state = match (self.gizmo.active(), self.hover) {
            (Some(h), _) => format!(" dragging {:?}", h),
            (None, Some(h)) => format!(" hover {:?}", h),
            (None, None) => String::new(),
        };
        Some(format!(
            "{:?} {:?} at {:.3} {:.3} {:.3}{}",
            self.mode, self.space, t.x, t.y, t.z, state
        ))
    }
}

impl Gizmo3dRef {
    pub fn set_camera(&self, cx: &mut Cx, view: Mat4f, proj: Mat4f) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_camera(cx, view, proj);
        }
    }

    pub fn set_perspective(&self, cx: &mut Cx, view: Mat4f, fov_y_deg: f32, near: f32, far: f32) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_perspective(cx, view, fov_y_deg, near, far);
        }
    }

    pub fn set_view(&self, cx: &mut Cx, view: Mat4f) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_view(cx, view);
        }
    }

    pub fn set_model(&self, cx: &mut Cx, model: Mat4f) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_model(cx, model);
        }
    }

    pub fn model(&self) -> Mat4f {
        self.borrow().map(|inner| inner.model).unwrap_or_default()
    }

    pub fn set_mode(&self, cx: &mut Cx, mode: Gizmo3dMode) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_mode(cx, mode);
        }
    }

    pub fn set_space(&self, cx: &mut Cx, space: Gizmo3dSpace) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_space(cx, space);
        }
    }

    pub fn is_dragging(&self) -> bool {
        self.borrow()
            .map(|inner| inner.is_dragging())
            .unwrap_or(false)
    }

    pub fn drag_info(&self) -> Option<GizmoDragInfo> {
        self.borrow().and_then(|inner| inner.drag_info())
    }

    /// The last model matrix a drag produced in these actions.
    pub fn changed(&self, actions: &Actions) -> Option<Mat4f> {
        actions
            .filter_widget_actions_cast::<GizmoAction>(self.widget_uid())
            .filter_map(|a| match a {
                GizmoAction::Changed(m) => Some(m),
                _ => None,
            })
            .last()
    }

    pub fn drag_started(&self, actions: &Actions) -> bool {
        actions
            .filter_widget_actions_cast::<GizmoAction>(self.widget_uid())
            .any(|a| a == GizmoAction::DragStart)
    }

    pub fn drag_ended(&self, actions: &Actions) -> bool {
        actions
            .filter_widget_actions_cast::<GizmoAction>(self.widget_uid())
            .any(|a| a == GizmoAction::DragEnd)
    }
}

/// A click's turn of the camera, in progress.
#[derive(Clone, Copy, Debug)]
struct ViewTurn {
    from: Mat4f,
    to: Mat4f,
    start: f64,
}

/// The orientation gizmo widget. See the module documentation.
#[derive(Script, ScriptHook, WidgetRef, WidgetRegister)]
pub struct ViewCube {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[live]
    draw_line: DrawGizmoStroke,
    #[live]
    draw_disc: DrawGizmoDisc,
    #[live]
    draw_face: DrawGizmoFace,
    #[live]
    draw_text: DrawText,
    /// Cube or Balls.
    #[live]
    pub shape: ViewCubeShape,
    /// The world's up axis.
    #[live]
    pub up: ViewCubeUp,
    #[live(true)]
    pub labels: bool,
    /// Seconds a click takes to turn the camera; zero jumps.
    #[live(0.3)]
    pub snap_seconds: f64,
    /// Radians of orbit per point dragged.
    #[live(0.01)]
    pub orbit_speed: f64,
    #[live]
    pub color_x: Vec4f,
    #[live]
    pub color_y: Vec4f,
    #[live]
    pub color_z: Vec4f,
    #[live]
    pub color_face: Vec4f,
    #[live]
    pub color_edge: Vec4f,
    #[live]
    pub color_highlight: Vec4f,
    #[live]
    pub color_text: Vec4f,
    #[live]
    pub color_ball_text: Vec4f,
    #[live]
    pub color_backdrop: Vec4f,
    #[rust]
    cube: ViewCubeModel,
    #[rust]
    view: Mat4f,
    #[rust]
    pivot: Vec3f,
    #[rust]
    hover: Option<ViewDir>,
    #[rust]
    hot: bool,
    #[rust]
    area: Area,
    #[rust]
    press: Option<DVec2>,
    #[rust]
    last: DVec2,
    #[rust]
    dragged: bool,
    #[rust]
    turn: Option<ViewTurn>,
    #[rust]
    next_frame: NextFrame,
    /// Whether anything has handed the gizmo a view yet. Until something
    /// does it shows a three-quarter view rather than a flat face.
    #[rust]
    has_view: bool,
}

impl ViewCube {
    fn sync(&mut self) {
        let c = &mut self.cube;
        c.style = match self.shape {
            ViewCubeShape::Cube => ViewCubeStyle::Cube,
            ViewCubeShape::Balls => ViewCubeStyle::Balls,
        };
        c.up = self.up.into();
        c.labels = self.labels;
        c.axis_colors = [self.color_x, self.color_y, self.color_z];
        c.face_color = self.color_face;
        c.edge_color = self.color_edge;
        c.highlight = self.color_highlight;
        c.text_color = self.color_text;
        c.ball_text_color = self.color_ball_text;
        c.backdrop = self.color_backdrop;
    }

    /// The camera's view matrix, for the gizmo to show. Call it whenever the
    /// camera moves.
    pub fn set_view(&mut self, cx: &mut Cx, view: Mat4f) {
        self.has_view = true;
        if self.view != view {
            self.view = view;
            self.area.redraw(cx);
        }
    }

    pub fn view(&self) -> Mat4f {
        self.view
    }

    /// Front, right and above, whichever way is up.
    fn default_view(&self) -> Mat4f {
        let up = UpAxis::from(self.up);
        let dir = match up {
            UpAxis::Y => vec3(1.0, 0.8, 1.3),
            UpAxis::Z => vec3(1.0, -1.3, 0.8),
        };
        look_from(dir, self.pivot, 5.0, up)
    }

    /// The point the camera orbits and looks at.
    pub fn set_pivot(&mut self, cx: &mut Cx, pivot: Vec3f) {
        self.pivot = pivot;
        self.area.redraw(cx);
    }

    /// Turn the camera to look from `dir`, as a click on it would.
    pub fn snap_to(&mut self, cx: &mut Cx, dir: ViewDir) {
        self.sync();
        let target = self.cube.snap_view(&self.view, self.pivot, dir);
        self.start_turn(cx, target);
    }

    fn start_turn(&mut self, cx: &mut Cx, target: Mat4f) {
        if self.snap_seconds <= 0.0 {
            self.turn = None;
            self.emit_view(cx, target);
        } else {
            self.turn = Some(ViewTurn {
                from: self.view,
                to: target,
                start: cx.seconds_since_app_start(),
            });
            self.next_frame = cx.new_next_frame();
        }
    }

    fn emit_view(&mut self, cx: &mut Cx, view: Mat4f) {
        self.view = view;
        self.has_view = true;
        cx.widget_action(self.uid, GizmoAction::ViewChanged(view));
        self.area.redraw(cx);
    }
}

impl WidgetNode for ViewCube {
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

impl Widget for ViewCube {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if let Some(ne) = self.next_frame.is_event(event) {
            if let Some(turn) = self.turn {
                let t = ((ne.time - turn.start) / self.snap_seconds.max(1.0e-3)).clamp(0.0, 1.0);
                // Ease in and out, so the turn starts and lands gently.
                let e = (t * t * (3.0 - 2.0 * t)) as f32;
                let view = if t >= 1.0 {
                    turn.to
                } else {
                    interpolate_view(&turn.from, &turn.to, self.pivot, e)
                };
                self.emit_view(cx, view);
                if t >= 1.0 {
                    self.turn = None;
                } else {
                    self.next_frame = cx.new_next_frame();
                }
            }
        }
        if !self.area.is_valid(cx) {
            return;
        }
        self.sync();
        let rect = gizmo_rect(self.area.rect(cx));
        // Only the round face is the gizmo; the corners of its box are not.
        let hit = event.hits_with_test(cx, self.area, |abs, r, _| {
            let c = r.pos + r.size * 0.5;
            (abs - c).length() <= r.size.x.min(r.size.y) * 0.5
        });
        match hit {
            Hit::FingerHoverIn(fh) | Hit::FingerHoverOver(fh) => {
                let h = self.cube.hover(&self.view, rect, vec2_of(fh.abs));
                if h != self.hover || !self.hot {
                    self.hover = h;
                    self.hot = true;
                    self.area.redraw(cx);
                }
                if self.press.is_none() {
                    cx.set_cursor(if h.is_some() {
                        MouseCursor::Hand
                    } else {
                        MouseCursor::Grab
                    });
                }
            }
            Hit::FingerHoverOut(_) => {
                if self.hot || self.hover.is_some() {
                    self.hot = false;
                    self.hover = None;
                    self.area.redraw(cx);
                }
            }
            Hit::FingerDown(fe) if fe.device.is_primary_hit() => {
                self.press = Some(fe.abs);
                self.last = fe.abs;
                self.dragged = false;
                self.turn = None;
            }
            Hit::FingerMove(fe) => {
                let Some(press) = self.press else {
                    return;
                };
                if !self.dragged && (fe.abs - press).length() > 3.0 {
                    self.dragged = true;
                }
                if self.dragged {
                    let d = fe.abs - self.last;
                    let speed = self.orbit_speed as f32;
                    let up = UpAxis::from(self.up).vector();
                    let view = orbit_view(
                        &self.view,
                        self.pivot,
                        up,
                        -(d.x as f32) * speed,
                        -(d.y as f32) * speed,
                    );
                    self.hover = None;
                    self.emit_view(cx, view);
                    cx.set_cursor(MouseCursor::Grabbing);
                }
                self.last = fe.abs;
            }
            Hit::FingerUp(fe) => {
                if !self.dragged && fe.is_over {
                    if let Some(dir) = self.cube.hover(&self.view, rect, vec2_of(fe.abs)) {
                        let dir = self.cube.click_target(&self.view, dir);
                        let target = self.cube.snap_view(&self.view, self.pivot, dir);
                        self.start_turn(cx, target);
                    }
                }
                self.press = None;
                self.dragged = false;
                self.hover = if fe.is_over {
                    self.cube.hover(&self.view, rect, vec2_of(fe.abs))
                } else {
                    None
                };
                self.area.redraw(cx);
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle_with_area(&mut self.area, walk);
        self.sync();
        if !self.has_view {
            self.view = self.default_view();
            self.has_view = true;
        }
        let prims = self
            .cube
            .geometry(&self.view, gizmo_rect(rect), self.hover, self.hot);
        paint_gizmo_prims(
            cx,
            &prims,
            &mut self.draw_line,
            &mut self.draw_disc,
            &mut self.draw_face,
            &mut self.draw_text,
        );
        DrawStep::done()
    }

    fn snapshot_value(&self, _cx: &Cx) -> Option<String> {
        let (_, _, back, _) = makepad_gizmo::frame_of_view(&self.view);
        Some(format!(
            "{:?} from {:.2} {:.2} {:.2}{}",
            self.shape,
            back.x,
            back.y,
            back.z,
            match self.hover {
                Some(d) => format!(" hover {} {} {}", d.x, d.y, d.z),
                None => String::new(),
            }
        ))
    }
}

impl ViewCubeRef {
    pub fn set_view(&self, cx: &mut Cx, view: Mat4f) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_view(cx, view);
        }
    }

    pub fn view(&self) -> Mat4f {
        self.borrow().map(|inner| inner.view).unwrap_or_default()
    }

    pub fn set_pivot(&self, cx: &mut Cx, pivot: Vec3f) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_pivot(cx, pivot);
        }
    }

    pub fn snap_to(&self, cx: &mut Cx, dir: ViewDir) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.snap_to(cx, dir);
        }
    }

    /// The last view the gizmo turned the camera to in these actions.
    pub fn view_changed(&self, actions: &Actions) -> Option<Mat4f> {
        actions
            .filter_widget_actions_cast::<GizmoAction>(self.widget_uid())
            .filter_map(|a| match a {
                GizmoAction::ViewChanged(v) => Some(v),
                _ => None,
            })
            .last()
    }
}
