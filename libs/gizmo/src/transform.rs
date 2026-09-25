//! The transform gizmo: arrows, planes, rings and boxes that move, turn and
//! scale a model matrix.
//!
//! # Handles
//!
//! - **Translate**: an arrow per axis, a square per plane (YZ, ZX, XY) and a
//!   dot at the centre that moves in the screen plane.
//! - **Rotate**: a ring per axis, only its half facing the camera, and an
//!   outer screen-space ring that turns about the line of sight.
//! - **Scale**: a stick with a box per local axis and a centre dot that
//!   scales uniformly. Scale is always along the model's own axes.
//! - **Universal**: the translate handles, the rotation rings further out and
//!   the scale boxes between them.
//!
//! An axis nearly end-on to the camera fades out and stops being pickable,
//! and so does a plane seen edge-on. Arrows and sticks flip to the half of
//! their axis facing the camera; the flip is frozen for the length of a
//! drag, so a handle never jumps under the pointer.
//!
//! # Dragging
//!
//! Translation follows the pointer's ray onto a plane: the handle's own plane
//! for a plane handle, the screen plane for the centre dot, and for an axis
//! the plane through that axis that faces the camera most, with the hit then
//! taken along the axis. A rotation reads the angle the pointer sweeps on the
//! ring's plane, unwrapped so it can go round more than once; a ring seen
//! nearly edge-on switches to reading the pointer along the ring's tangent
//! instead. An axis scale is the ratio of where the pointer's ray meets the
//! axis now to where it met it on the press; the uniform scale reads the
//! pointer's travel right and up.
//!
//! Snapping rounds the translation per axis of the gizmo's space, the angle
//! to a step in degrees, and a scale factor to a step away from one.
use crate::camera::*;
use crate::prim::GizmoPrim;
use makepad_math::*;
use std::f32::consts::{PI, TAU};

/// What the gizmo manipulates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum GizmoMode {
    #[default]
    Translate,
    Rotate,
    Scale,
    /// Translate, rotate and scale handles at once.
    Universal,
}

/// Which axes the translate and rotate handles follow. Scale always follows
/// the model's own axes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum GizmoSpace {
    #[default]
    World,
    Local,
}

/// Snapping steps; `None` leaves that operation continuous.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct GizmoSnap {
    /// World units, per axis of the gizmo's space.
    pub translate: Option<f32>,
    /// Degrees.
    pub rotate_deg: Option<f32>,
    /// A step of the scale factor away from one.
    pub scale: Option<f32>,
}

/// One handle of the transform gizmo.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GizmoHandle {
    TranslateX,
    TranslateY,
    TranslateZ,
    /// The plane with normal X.
    TranslateYZ,
    /// The plane with normal Y.
    TranslateZX,
    /// The plane with normal Z.
    TranslateXY,
    /// The centre dot: moves in the plane facing the camera.
    TranslateScreen,
    RotateX,
    RotateY,
    RotateZ,
    /// The outer ring: turns about the line of sight.
    RotateScreen,
    ScaleX,
    ScaleY,
    ScaleZ,
    ScaleUniform,
}

impl GizmoHandle {
    /// The axis a handle belongs to: its own axis, or for a plane its normal.
    pub fn axis(self) -> Option<usize> {
        use GizmoHandle::*;
        match self {
            TranslateX | TranslateYZ | RotateX | ScaleX => Some(0),
            TranslateY | TranslateZX | RotateY | ScaleY => Some(1),
            TranslateZ | TranslateXY | RotateZ | ScaleZ => Some(2),
            TranslateScreen | RotateScreen | ScaleUniform => None,
        }
    }

    pub fn is_translate(self) -> bool {
        use GizmoHandle::*;
        matches!(
            self,
            TranslateX
                | TranslateY
                | TranslateZ
                | TranslateYZ
                | TranslateZX
                | TranslateXY
                | TranslateScreen
        )
    }

    pub fn is_rotate(self) -> bool {
        use GizmoHandle::*;
        matches!(self, RotateX | RotateY | RotateZ | RotateScreen)
    }

    pub fn is_scale(self) -> bool {
        use GizmoHandle::*;
        matches!(self, ScaleX | ScaleY | ScaleZ | ScaleUniform)
    }

    fn is_plane(self) -> bool {
        use GizmoHandle::*;
        matches!(self, TranslateYZ | TranslateZX | TranslateXY)
    }

    /// A short name for readouts.
    pub fn label(self) -> &'static str {
        use GizmoHandle::*;
        match self {
            TranslateX | RotateX | ScaleX => "X",
            TranslateY | RotateY | ScaleY => "Y",
            TranslateZ | RotateZ | ScaleZ => "Z",
            TranslateYZ => "YZ",
            TranslateZX => "ZX",
            TranslateXY => "XY",
            TranslateScreen | RotateScreen => "View",
            ScaleUniform => "XYZ",
        }
    }

    fn translate_axis(i: usize) -> Self {
        [Self::TranslateX, Self::TranslateY, Self::TranslateZ][i]
    }

    fn translate_plane(i: usize) -> Self {
        [Self::TranslateYZ, Self::TranslateZX, Self::TranslateXY][i]
    }

    fn rotate_axis(i: usize) -> Self {
        [Self::RotateX, Self::RotateY, Self::RotateZ][i]
    }

    fn scale_axis(i: usize) -> Self {
        [Self::ScaleX, Self::ScaleY, Self::ScaleZ][i]
    }
}

/// Colours and sizes of the transform gizmo.
#[derive(Clone, Debug, PartialEq)]
pub struct GizmoStyle {
    pub axis_colors: [Vec4f; 3],
    /// The screen-plane dot and ring.
    pub screen_color: Vec4f,
    /// A hovered or dragged handle.
    pub highlight: Vec4f,
    pub text_color: Vec4f,
    pub text_backdrop: Vec4f,
    /// Stroke width of arrows, sticks and rings, in pixels.
    pub line_width: f32,
    /// How near the pointer has to be to a handle, in pixels.
    pub pick_px: f32,
    /// Opacity of the plane squares' fill.
    pub plane_alpha: f32,
    /// Opacity of the handles not being dragged, while one is.
    pub inactive_alpha: f32,
}

impl Default for GizmoStyle {
    fn default() -> Self {
        Self {
            axis_colors: [
                vec4(0.93, 0.29, 0.31, 1.0),
                vec4(0.47, 0.80, 0.27, 1.0),
                vec4(0.27, 0.54, 0.96, 1.0),
            ],
            screen_color: vec4(0.92, 0.92, 0.92, 1.0),
            highlight: vec4(1.0, 0.83, 0.27, 1.0),
            text_color: vec4(1.0, 1.0, 1.0, 1.0),
            text_backdrop: vec4(0.0, 0.0, 0.0, 0.6),
            line_width: 2.5,
            pick_px: 7.0,
            plane_alpha: 0.35,
            inactive_alpha: 0.2,
        }
    }
}

/// What a drag in progress has done so far, for readouts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GizmoDragInfo {
    pub handle: GizmoHandle,
    pub start_model: Mat4f,
    pub model: Mat4f,
    /// The translation, as components along the gizmo space's axes.
    pub translation: Vec3f,
    /// The rotation angle, in degrees.
    pub angle_deg: f32,
    /// The scale factors along the model's axes.
    pub scale: Vec3f,
}

/// The transform gizmo: its configuration, and the drag in progress.
#[derive(Clone, Debug)]
pub struct Gizmo {
    pub mode: GizmoMode,
    pub space: GizmoSpace,
    pub snap: GizmoSnap,
    /// Length of an axis on screen, in pixels.
    pub size_px: f32,
    /// Point arrows and sticks at the half of their axis facing the camera.
    pub flip_axes: bool,
    /// Show a readout of the drag under the gizmo.
    pub show_text: bool,
    pub style: GizmoStyle,
    drag: Option<DragState>,
}

impl Default for Gizmo {
    fn default() -> Self {
        Self {
            mode: GizmoMode::Translate,
            space: GizmoSpace::World,
            snap: GizmoSnap::default(),
            size_px: 100.0,
            flip_axes: true,
            show_text: true,
            style: GizmoStyle::default(),
            drag: None,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct DragState {
    handle: GizmoHandle,
    start_model: Mat4f,
    model: Mat4f,
    start_mouse: Vec2f,
    origin: Vec3f,
    unit: f32,
    /// The gizmo space's axes on the press; translation snaps along these.
    space_axes: [Vec3f; 3],
    tflips: [bool; 3],
    sflips: [bool; 3],
    /// Translate axis, rotation axis or scale axis, world, unit length.
    axis: Vec3f,
    /// Normal of the plane the pointer's ray is intersected with.
    plane_n: Vec3f,
    start_hit: Vec3f,
    // translate
    delta: Vec3f,
    // rotate
    start_vec: Vec3f,
    last_angle: f32,
    raw_angle: f32,
    angle: f32,
    /// Edge-on rings read the pointer along this screen direction, over this
    /// many pixels per radian.
    tangent: Option<(Vec2f, f32)>,
    ring_radius: f32,
    // scale
    start_t: f32,
    factors: Vec3f,
}

/// Where the gizmo stands this frame.
#[derive(Clone, Copy, Debug)]
struct Frame {
    origin: Vec3f,
    /// Translate and rotate axes: world or local, unit length, unflipped.
    axes: [Vec3f; 3],
    /// The model's own axes, unit length.
    local: [Vec3f; 3],
    /// World length of `size_px` at the origin.
    unit: f32,
    center: Vec2f,
    to_cam: Vec3f,
    /// Scale of the fixed-size marks.
    k: f32,
}

#[derive(Clone, Debug)]
enum Shape {
    Arrow {
        from: Vec2f,
        to: Vec2f,
        head: [Vec2f; 3],
    },
    Plane {
        pts: [Vec2f; 4],
    },
    Dot {
        center: Vec2f,
        radius: f32,
    },
    Stick {
        from: Vec2f,
        to: Vec2f,
        half: f32,
        shaft: bool,
    },
    Ring {
        pts: Vec<Vec2f>,
        front: Vec<bool>,
    },
    Circle {
        center: Vec2f,
        radius: f32,
    },
}

#[derive(Clone, Debug)]
struct Piece {
    handle: GizmoHandle,
    shape: Shape,
    alpha: f32,
    /// Preference when several handles are within reach: inside a small
    /// handle beats being near a long one.
    bias: f32,
    /// Paint order: layer first, then far to near.
    layer: u8,
    depth: f32,
}

const WORLD_AXES: [Vec3f; 3] = [
    Vec3f {
        x: 1.0,
        y: 0.0,
        z: 0.0,
    },
    Vec3f {
        x: 0.0,
        y: 1.0,
        z: 0.0,
    },
    Vec3f {
        x: 0.0,
        y: 0.0,
        z: 1.0,
    },
];

const RING_SEGMENTS: usize = 64;

fn wrap_angle(mut a: f32) -> f32 {
    while a > PI {
        a -= TAU;
    }
    while a < -PI {
        a += TAU;
    }
    a
}

fn snap_to(v: f32, step: Option<f32>) -> f32 {
    match step {
        Some(s) if s > 0.0 => (v / s).round() * s,
        _ => v,
    }
}

fn component(v: Vec3f, i: usize) -> f32 {
    [v.x, v.y, v.z][i]
}

fn with_component(v: Vec3f, i: usize, c: f32) -> Vec3f {
    let mut a = [v.x, v.y, v.z];
    a[i] = c;
    vec3(a[0], a[1], a[2])
}

/// Any unit vector perpendicular to `a`.
fn perpendicular(a: Vec3f) -> Vec3f {
    let other = if a.x.abs() < 0.9 {
        vec3(1.0, 0.0, 0.0)
    } else {
        vec3(0.0, 1.0, 0.0)
    };
    safe_normalize(Vec3f::cross(a, other), vec3(0.0, 0.0, 1.0))
}

/// The component of `v` across the unit `a`.
fn across(v: Vec3f, a: Vec3f) -> Vec3f {
    v - a * v.dot(a)
}

fn square(center: Vec2f, half: f32) -> [Vec2f; 4] {
    [
        vec2(center.x - half, center.y - half),
        vec2(center.x + half, center.y - half),
        vec2(center.x + half, center.y + half),
        vec2(center.x - half, center.y + half),
    ]
}

impl Gizmo {
    pub fn new(mode: GizmoMode) -> Self {
        Self {
            mode,
            ..Self::default()
        }
    }

    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// The handle being dragged.
    pub fn active(&self) -> Option<GizmoHandle> {
        self.drag.map(|d| d.handle)
    }

    /// What the drag in progress has done so far.
    pub fn drag_info(&self) -> Option<GizmoDragInfo> {
        let d = self.drag.as_ref()?;
        Some(GizmoDragInfo {
            handle: d.handle,
            start_model: d.start_model,
            model: d.model,
            translation: vec3(
                d.delta.dot(d.space_axes[0]),
                d.delta.dot(d.space_axes[1]),
                d.delta.dot(d.space_axes[2]),
            ),
            angle_deg: d.angle.to_degrees(),
            scale: d.factors,
        })
    }

    fn frame(&self, cam: &GizmoCamera, model: &Mat4f) -> Option<Frame> {
        let origin = mat_translation(model);
        let center = cam.project(origin)?;
        let local = [
            safe_normalize(mat_col(model, 0), WORLD_AXES[0]),
            safe_normalize(mat_col(model, 1), WORLD_AXES[1]),
            safe_normalize(mat_col(model, 2), WORLD_AXES[2]),
        ];
        let axes = match self.space {
            GizmoSpace::World => WORLD_AXES,
            GizmoSpace::Local => local,
        };
        let size = self.size_px.max(8.0);
        let unit = cam.world_per_pixel(origin) * size;
        if !unit.is_finite() || unit <= 0.0 {
            return None;
        }
        Some(Frame {
            origin,
            axes,
            local,
            unit,
            center,
            to_cam: cam.to_camera(origin),
            k: (size / 100.0).clamp(0.5, 2.0),
        })
    }

    fn flips(&self, f: &Frame) -> ([bool; 3], [bool; 3]) {
        if let Some(d) = &self.drag {
            return (d.tflips, d.sflips);
        }
        let mut t = [false; 3];
        let mut s = [false; 3];
        if self.flip_axes {
            for i in 0..3 {
                t[i] = f.axes[i].dot(f.to_cam) < 0.0;
                s[i] = f.local[i].dot(f.to_cam) < 0.0;
            }
        }
        (t, s)
    }

    fn has_translate(&self) -> bool {
        matches!(self.mode, GizmoMode::Translate | GizmoMode::Universal)
    }

    fn has_rotate(&self) -> bool {
        matches!(self.mode, GizmoMode::Rotate | GizmoMode::Universal)
    }

    fn has_scale(&self) -> bool {
        matches!(self.mode, GizmoMode::Scale | GizmoMode::Universal)
    }

    fn ring_radius(&self) -> f32 {
        if self.mode == GizmoMode::Universal {
            1.3
        } else {
            1.0
        }
    }

    fn screen_ring_px(&self) -> f32 {
        self.size_px.max(8.0) * 1.2
    }

    /// How faded an axis is by how short it looks: an axis pointing at the
    /// camera is a dot, and a dot cannot be dragged along.
    fn axis_alpha(&self, cam: &GizmoCamera, f: &Frame, dir: Vec3f) -> f32 {
        match cam.project(f.origin + dir * f.unit) {
            Some(tip) => {
                let ratio = (tip - f.center).length() / self.size_px.max(8.0);
                fade_in(0.08, 0.2, ratio)
            }
            None => 0.0,
        }
    }

    /// Every visible handle, with its screen shape.
    fn pieces(&self, cam: &GizmoCamera, f: &Frame) -> Vec<Piece> {
        let mut out = Vec::new();
        let (tflips, sflips) = self.flips(f);
        let sign = |flip: bool| if flip { -1.0 } else { 1.0 };
        let k = f.k;
        let o = f.origin;

        if self.mode == GizmoMode::Rotate {
            out.push(Piece {
                handle: GizmoHandle::RotateScreen,
                shape: Shape::Circle {
                    center: f.center,
                    radius: self.screen_ring_px(),
                },
                alpha: 1.0,
                bias: 0.0,
                layer: 0,
                depth: 0.0,
            });
        }

        if self.has_rotate() {
            let r = self.ring_radius() * f.unit;
            for i in 0..3 {
                let u = f.axes[(i + 1) % 3];
                let v = f.axes[(i + 2) % 3];
                let mut pts = Vec::with_capacity(RING_SEGMENTS + 1);
                let mut front = Vec::with_capacity(RING_SEGMENTS + 1);
                let mut ok = true;
                for s in 0..=RING_SEGMENTS {
                    let t = s as f32 / RING_SEGMENTS as f32 * TAU;
                    let p = o + (u * t.cos() + v * t.sin()) * r;
                    match cam.project(p) {
                        Some(sp) => pts.push(sp),
                        None => {
                            ok = false;
                            break;
                        }
                    }
                    // The half facing the camera, judged from the gizmo's centre
                    // so a ring seen face-on is front all the way round.
                    front.push((p - o).dot(f.to_cam) >= -0.02 * r);
                }
                if !ok {
                    continue;
                }
                out.push(Piece {
                    handle: GizmoHandle::rotate_axis(i),
                    shape: Shape::Ring { pts, front },
                    alpha: 1.0,
                    // Where rings cross, the one seen face-on wins: an
                    // edge-on ring is a line that could be any of them.
                    bias: 0.5 + 0.5 * f.axes[i].dot(f.to_cam).abs(),
                    layer: 1,
                    depth: 0.0,
                });
            }
        }

        if self.has_translate() {
            for i in 0..3 {
                let j = (i + 1) % 3;
                let l = (i + 2) % 3;
                let dj = f.axes[j] * sign(tflips[j]);
                let dl = f.axes[l] * sign(tflips[l]);
                let facing = f.axes[i].dot(f.to_cam).abs();
                let alpha = fade_in(0.12, 0.3, facing);
                if alpha < 0.02 {
                    continue;
                }
                let corners = [(0.22, 0.22), (0.5, 0.22), (0.5, 0.5), (0.22, 0.5)];
                let mut pts = [vec2(0.0, 0.0); 4];
                let mut ok = true;
                for (n, (a, b)) in corners.iter().enumerate() {
                    match cam.project(o + (dj * *a + dl * *b) * f.unit) {
                        Some(p) => pts[n] = p,
                        None => ok = false,
                    }
                }
                if !ok {
                    continue;
                }
                let mid = o + (dj + dl) * (0.36 * f.unit);
                out.push(Piece {
                    handle: GizmoHandle::translate_plane(i),
                    shape: Shape::Plane { pts },
                    alpha,
                    bias: 1.5,
                    layer: 2,
                    depth: (mid - o).dot(f.to_cam),
                });
            }
            for i in 0..3 {
                let dir = f.axes[i] * sign(tflips[i]);
                let alpha = self.axis_alpha(cam, f, dir);
                if alpha < 0.02 {
                    continue;
                }
                let (Some(from), Some(to)) = (
                    cam.project(o + dir * (0.14 * f.unit)),
                    cam.project(o + dir * f.unit),
                ) else {
                    continue;
                };
                let d2 = norm2(to - from);
                let perp = vec2(-d2.y, d2.x);
                let head_len = 13.0 * k;
                let half = 5.0 * k;
                let base = to - d2 * head_len;
                out.push(Piece {
                    handle: GizmoHandle::translate_axis(i),
                    shape: Shape::Arrow {
                        from,
                        to,
                        head: [to, base + perp * half, base - perp * half],
                    },
                    alpha,
                    bias: 1.0,
                    layer: 3,
                    depth: dir.dot(f.to_cam),
                });
            }
        }

        if self.has_scale() {
            let universal = self.mode == GizmoMode::Universal;
            let factors = match &self.drag {
                Some(d) if d.handle.is_scale() => d.factors,
                _ => vec3(1.0, 1.0, 1.0),
            };
            for i in 0..3 {
                let dir = f.local[i] * sign(sflips[i]);
                let alpha = self.axis_alpha(cam, f, dir);
                if alpha < 0.02 {
                    continue;
                }
                let reach = if universal { 1.15 } else { 1.0 } * component(factors, i);
                let (Some(from), Some(to)) = (
                    cam.project(o + dir * (0.14 * f.unit)),
                    cam.project(o + dir * (reach * f.unit)),
                ) else {
                    continue;
                };
                out.push(Piece {
                    handle: GizmoHandle::scale_axis(i),
                    shape: Shape::Stick {
                        from,
                        to,
                        half: 5.0 * k,
                        shaft: !universal,
                    },
                    alpha,
                    bias: 2.0,
                    layer: 3,
                    depth: dir.dot(f.to_cam),
                });
            }
        }

        if self.mode == GizmoMode::Scale {
            out.push(Piece {
                handle: GizmoHandle::ScaleUniform,
                shape: Shape::Dot {
                    center: f.center,
                    radius: 8.0 * k,
                },
                alpha: 1.0,
                bias: 3.0,
                layer: 4,
                depth: 0.0,
            });
        } else if self.has_translate() {
            out.push(Piece {
                handle: GizmoHandle::TranslateScreen,
                shape: Shape::Dot {
                    center: f.center,
                    radius: 6.5 * k,
                },
                alpha: 1.0,
                bias: 3.0,
                layer: 4,
                depth: 0.0,
            });
        }

        out.sort_by(|a, b| {
            a.layer.cmp(&b.layer).then(
                a.depth
                    .partial_cmp(&b.depth)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
        });
        out
    }

    fn pick_dist(shape: &Shape, m: Vec2f) -> f32 {
        match shape {
            Shape::Arrow { from, to, head } => {
                dist_to_segment(m, *from, *to).min(dist_to_convex(m, head))
            }
            Shape::Plane { pts } => dist_to_convex(m, pts),
            Shape::Dot { center, radius } => ((m - *center).length() - radius).max(0.0),
            Shape::Stick {
                from,
                to,
                half,
                shaft,
            } => {
                let b = dist_to_convex(m, &square(*to, *half));
                if *shaft {
                    b.min(dist_to_segment(m, *from, *to))
                } else {
                    b
                }
            }
            Shape::Ring { pts, front } => {
                let mut best = f32::MAX;
                for i in 0..pts.len().saturating_sub(1) {
                    if front[i] && front[i + 1] {
                        best = best.min(dist_to_segment(m, pts[i], pts[i + 1]));
                    }
                }
                best
            }
            Shape::Circle { center, radius } => ((m - *center).length() - radius).abs(),
        }
    }

    /// The handle under the pointer, if any.
    pub fn hover(&self, cam: &GizmoCamera, model: &Mat4f, mouse: Vec2f) -> Option<GizmoHandle> {
        let f = self.frame(cam, model)?;
        let tol = self.style.pick_px.max(1.0);
        let mut best: Option<(f32, GizmoHandle)> = None;
        for p in self.pieces(cam, &f) {
            if p.alpha < 0.3 {
                continue;
            }
            let d = Self::pick_dist(&p.shape, mouse);
            if d > tol {
                continue;
            }
            let score = d - p.bias;
            if best.map_or(true, |(s, _)| score < s) {
                best = Some((score, p.handle));
            }
        }
        best.map(|(_, h)| h)
    }

    /// Start dragging whatever handle is under the pointer. Returns the
    /// handle, or None when the press missed the gizmo.
    pub fn begin_drag(
        &mut self,
        cam: &GizmoCamera,
        model: &Mat4f,
        mouse: Vec2f,
    ) -> Option<GizmoHandle> {
        let handle = self.hover(cam, model, mouse)?;
        self.begin_drag_handle(cam, model, mouse, handle)
    }

    /// Start dragging a given handle from this pointer position, whether or
    /// not the pointer is on it: a keyboard "grab", for one.
    pub fn begin_drag_handle(
        &mut self,
        cam: &GizmoCamera,
        model: &Mat4f,
        mouse: Vec2f,
        handle: GizmoHandle,
    ) -> Option<GizmoHandle> {
        self.drag = None;
        let f = self.frame(cam, model)?;
        let (tflips, sflips) = self.flips(&f);
        let ray = cam.ray(mouse);
        let o = f.origin;
        let mut d = DragState {
            handle,
            start_model: *model,
            model: *model,
            start_mouse: mouse,
            origin: o,
            unit: f.unit,
            space_axes: f.axes,
            tflips,
            sflips,
            axis: f.to_cam,
            plane_n: f.to_cam,
            start_hit: o,
            delta: vec3(0.0, 0.0, 0.0),
            start_vec: perpendicular(f.to_cam),
            last_angle: 0.0,
            raw_angle: 0.0,
            angle: 0.0,
            tangent: None,
            ring_radius: self.ring_radius(),
            start_t: f.unit,
            factors: vec3(1.0, 1.0, 1.0),
        };
        use GizmoHandle::*;
        match handle {
            TranslateX | TranslateY | TranslateZ | ScaleX | ScaleY | ScaleZ => {
                let i = handle.axis().unwrap_or(0);
                let a = if handle.is_scale() {
                    f.local[i]
                } else {
                    f.axes[i]
                };
                // The plane through the axis that faces the camera most.
                let n = across(f.to_cam, a);
                if n.length() < 1.0e-4 {
                    return None;
                }
                d.axis = a;
                d.plane_n = safe_normalize(n, f.to_cam);
                d.start_hit = ray.intersect_plane(o, d.plane_n)?;
                if handle.is_scale() {
                    let t0 = (d.start_hit - o).dot(a);
                    d.start_t = if t0.abs() < 1.0e-4 * f.unit {
                        f.unit
                    } else {
                        t0
                    };
                }
            }
            TranslateYZ | TranslateZX | TranslateXY => {
                let i = handle.axis().unwrap_or(0);
                d.plane_n = f.axes[i];
                d.start_hit = ray.intersect_plane(o, d.plane_n)?;
            }
            TranslateScreen => {
                d.plane_n = f.to_cam;
                d.start_hit = ray.intersect_plane(o, d.plane_n)?;
            }
            RotateX | RotateY | RotateZ | RotateScreen => {
                let a = match handle.axis() {
                    Some(i) => f.axes[i],
                    None => f.to_cam,
                };
                d.axis = a;
                d.plane_n = a;
                if handle == RotateScreen {
                    d.ring_radius = self.screen_ring_px() / self.size_px.max(8.0);
                }
                let facing = if cam.is_ortho() {
                    cam.back().dot(a).abs()
                } else {
                    ray.dir.dot(a).abs()
                };
                let hit = if facing > 0.2 {
                    ray.intersect_plane(o, a)
                } else {
                    None
                };
                match hit.map(|h| across(h - o, a)) {
                    Some(v) if v.length() > 1.0e-5 * f.unit => {
                        d.start_vec = safe_normalize(v, perpendicular(a));
                    }
                    _ => {
                        // Edge-on: find the grabbed point of the ring and read
                        // the pointer along the ring's tangent there.
                        let u = perpendicular(a);
                        let v = Vec3f::cross(a, u);
                        let r = d.ring_radius * f.unit;
                        let mut best = (f32::MAX, u);
                        for s in 0..RING_SEGMENTS {
                            let t = s as f32 / RING_SEGMENTS as f32 * TAU;
                            let dir = u * t.cos() + v * t.sin();
                            if let Some(sp) = cam.project(o + dir * r) {
                                let dist = (sp - mouse).length();
                                if dist < best.0 {
                                    best = (dist, dir);
                                }
                            }
                        }
                        let grab = o + best.1 * r;
                        let tangent = Vec3f::cross(a, best.1);
                        let (Some(s0), Some(s1)) =
                            (cam.project(grab), cam.project(grab + tangent * (0.1 * r)))
                        else {
                            return None;
                        };
                        d.start_vec = best.1;
                        d.tangent = Some((norm2(s1 - s0), (d.ring_radius * self.size_px).max(1.0)));
                    }
                }
            }
            ScaleUniform => {}
        }
        self.drag = Some(d);
        Some(handle)
    }

    /// Follow the pointer. Returns the model matrix the drag makes, or None
    /// when no drag is in progress.
    pub fn drag(&mut self, cam: &GizmoCamera, mouse: Vec2f) -> Option<Mat4f> {
        let snap = self.snap;
        let size = self.size_px.max(8.0);
        let d = self.drag.as_mut()?;
        let ray = cam.ray(mouse);
        let o = d.origin;
        use GizmoHandle::*;
        match d.handle {
            TranslateX | TranslateY | TranslateZ => {
                if let Some(hit) = ray.intersect_plane(o, d.plane_n) {
                    d.delta = d.axis * (hit - d.start_hit).dot(d.axis);
                }
            }
            TranslateYZ | TranslateZX | TranslateXY | TranslateScreen => {
                if let Some(hit) = ray.intersect_plane(o, d.plane_n) {
                    d.delta = across(hit - d.start_hit, d.plane_n);
                }
            }
            RotateX | RotateY | RotateZ | RotateScreen => {
                if let Some((dir, px_per_rad)) = d.tangent {
                    d.raw_angle = dot2(mouse - d.start_mouse, dir) / px_per_rad;
                } else if let Some(hit) = ray.intersect_plane(o, d.axis) {
                    let v = across(hit - o, d.axis);
                    if v.length() > 1.0e-5 * d.unit {
                        let v = safe_normalize(v, d.start_vec);
                        let now = Vec3f::cross(d.start_vec, v)
                            .dot(d.axis)
                            .atan2(d.start_vec.dot(v));
                        d.raw_angle += wrap_angle(now - d.last_angle);
                        d.last_angle = now;
                    }
                }
            }
            ScaleX | ScaleY | ScaleZ => {
                if let Some(hit) = ray.intersect_plane(o, d.plane_n) {
                    let i = d.handle.axis().unwrap_or(0);
                    let t = (hit - o).dot(d.axis);
                    d.factors = with_component(vec3(1.0, 1.0, 1.0), i, t / d.start_t);
                }
            }
            ScaleUniform => {
                let m = mouse - d.start_mouse;
                let f = 1.0 + (m.x - m.y) / size;
                d.factors = vec3(f, f, f);
            }
        }

        // Snap, then compose onto the model the drag started from.
        if d.handle.is_translate() {
            let mut delta = vec3(0.0, 0.0, 0.0);
            for axis in d.space_axes.iter() {
                delta += *axis * snap_to(d.delta.dot(*axis), snap.translate);
            }
            d.delta = delta;
            d.model = Mat4f::mul(&Mat4f::translation(delta), &d.start_model);
        } else if d.handle.is_rotate() {
            let step = snap.rotate_deg.map(|deg| deg.to_radians());
            d.angle = snap_to(d.raw_angle, step);
            let rot = Mat4f::mul(
                &Mat4f::translation(o),
                &Mat4f::mul(
                    &rotation_matrix(d.axis, d.angle),
                    &Mat4f::translation(o * -1.0),
                ),
            );
            d.model = Mat4f::mul(&rot, &d.start_model);
        } else {
            let mut f = d.factors;
            for i in 0..3 {
                let c = 1.0 + snap_to(component(f, i) - 1.0, snap.scale);
                f = with_component(f, i, c.max(0.001));
            }
            d.factors = f;
            d.model = Mat4f::mul(&d.start_model, &scale_matrix(f));
        }
        Some(d.model)
    }

    /// Finish the drag. Returns the final model matrix.
    pub fn end_drag(&mut self) -> Option<Mat4f> {
        self.drag.take().map(|d| d.model)
    }

    /// Abandon the drag. Returns the model matrix it started from.
    pub fn cancel_drag(&mut self) -> Option<Mat4f> {
        self.drag.take().map(|d| d.start_model)
    }

    /// The gizmo's picture for this camera and model, `hover` highlighted and
    /// the dragged handle, if any, lit while the others fade.
    pub fn geometry(
        &self,
        cam: &GizmoCamera,
        model: &Mat4f,
        hover: Option<GizmoHandle>,
    ) -> Vec<GizmoPrim> {
        let mut out = Vec::new();
        let Some(f) = self.frame(cam, model) else {
            return out;
        };
        let active = self.active();
        let st = &self.style;
        let lw = st.line_width;

        if let Some(d) = &self.drag {
            self.drag_underlay(cam, &f, d, &mut out);
        }

        for p in self.pieces(cam, &f) {
            let base = match p.handle.axis() {
                Some(i) => st.axis_colors[i],
                None => st.screen_color,
            };
            let lit = match active {
                Some(a) => a == p.handle,
                None => hover == Some(p.handle),
            };
            let mut alpha = p.alpha;
            if active.is_some() && !lit {
                alpha *= st.inactive_alpha;
            }
            let c = with_alpha(if lit { st.highlight } else { base }, alpha);
            match &p.shape {
                Shape::Arrow { from, to, head } => {
                    let d2 = norm2(*to - *from);
                    let base_pt = head[1] + (head[2] - head[1]) * 0.5;
                    // Stop the shaft inside the head so its round end does not
                    // poke through the point.
                    let shaft_end = base_pt + d2 * 1.0;
                    out.push(GizmoPrim::line(*from, shaft_end, lw, c));
                    out.push(GizmoPrim::triangle(head[0], head[1], head[2], c));
                }
                Shape::Plane { pts } => {
                    let fill_a = if lit { 0.6 } else { st.plane_alpha };
                    out.push(GizmoPrim::Quad {
                        pts: *pts,
                        fill: with_alpha(c, fill_a),
                        width: 1.0,
                        color: c,
                    });
                }
                Shape::Dot { center, radius } => {
                    let fill = if p.handle == GizmoHandle::ScaleUniform {
                        with_alpha(c, 0.85)
                    } else {
                        with_alpha(c, if lit { 0.45 } else { 0.18 })
                    };
                    out.push(GizmoPrim::Circle {
                        center: *center,
                        radius: *radius,
                        width: lw * 0.8,
                        color: c,
                        fill,
                    });
                }
                Shape::Stick {
                    from,
                    to,
                    half,
                    shaft,
                } => {
                    if *shaft {
                        out.push(GizmoPrim::line(*from, *to, lw, c));
                    }
                    out.push(GizmoPrim::Quad {
                        pts: square(*to, *half),
                        fill: c,
                        width: 0.0,
                        color: c,
                    });
                }
                Shape::Ring { pts, front } => {
                    for i in 0..pts.len().saturating_sub(1) {
                        if front[i] && front[i + 1] {
                            out.push(GizmoPrim::line(pts[i], pts[i + 1], lw, c));
                        }
                    }
                }
                Shape::Circle { center, radius } => {
                    out.push(GizmoPrim::ring(*center, *radius, lw, c));
                }
            }
        }

        if let Some(d) = &self.drag {
            if self.show_text {
                let text = self.readout(d);
                if !text.is_empty() {
                    let below = if self.mode == GizmoMode::Rotate {
                        self.screen_ring_px()
                    } else {
                        self.size_px.max(8.0) * self.ring_radius()
                    };
                    out.push(GizmoPrim::Text {
                        pos: f.center + vec2(0.0, below + 12.0 * f.k),
                        text,
                        color: st.text_color,
                        align: vec2(0.5, 0.0),
                        backdrop: st.text_backdrop,
                    });
                }
            }
        }
        out
    }

    /// What a drag leaves behind it: the path of a move, the swept angle of a
    /// turn.
    fn drag_underlay(&self, cam: &GizmoCamera, f: &Frame, d: &DragState, out: &mut Vec<GizmoPrim>) {
        let st = &self.style;
        if d.handle.is_translate() {
            if let Some(start) = cam.project(d.origin) {
                let c = with_alpha(st.screen_color, 0.55);
                out.push(GizmoPrim::line(start, f.center, 1.5, c));
                out.push(GizmoPrim::disc(start, 3.0 * f.k, c));
            }
        } else if d.handle.is_rotate() && d.angle.abs() > 1.0e-4 {
            let color = match d.handle.axis() {
                Some(i) => st.axis_colors[i],
                None => st.screen_color,
            };
            let r = d.ring_radius * f.unit;
            let u = d.start_vec;
            let v = Vec3f::cross(d.axis, u);
            let steps = ((d.angle.abs() / (PI / 32.0)).ceil() as usize).clamp(1, 256);
            let mut pts = Vec::with_capacity(steps + 1);
            for s in 0..=steps {
                let t = d.angle * s as f32 / steps as f32;
                match cam.project(d.origin + (u * t.cos() + v * t.sin()) * r) {
                    Some(p) => pts.push(p),
                    None => return,
                }
            }
            let fill = with_alpha(color, 0.25);
            for s in 0..steps {
                out.push(GizmoPrim::triangle(f.center, pts[s], pts[s + 1], fill));
            }
            let edge = with_alpha(color, 0.9);
            out.push(GizmoPrim::line(f.center, pts[0], 1.5, edge));
            out.push(GizmoPrim::line(f.center, pts[steps], 1.5, edge));
        }
    }

    fn readout(&self, d: &DragState) -> String {
        let name = ["X", "Y", "Z"];
        if d.handle.is_translate() {
            let comps: Vec<usize> = match d.handle.axis() {
                Some(i) if d.handle.is_plane() => vec![(i + 1) % 3, (i + 2) % 3],
                Some(i) => vec![i],
                None => vec![0, 1, 2],
            };
            let mut comps = comps;
            comps.sort();
            comps
                .iter()
                .map(|i| format!("{} {:.3}", name[*i], d.delta.dot(d.space_axes[*i])))
                .collect::<Vec<_>>()
                .join("   ")
        } else if d.handle.is_rotate() {
            format!("{} {:.1}\u{b0}", d.handle.label(), d.angle.to_degrees())
        } else {
            let i = d.handle.axis().unwrap_or(0);
            format!("{} {:.3}", d.handle.label(), component(d.factors, i))
        }
    }
}
