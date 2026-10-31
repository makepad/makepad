//! The orientation gizmo: a small picture of the world's axes as the camera
//! sees them, which turns the camera when it is clicked or dragged.
//!
//! Two faces of one thing:
//!
//! - [`ViewCubeStyle::Cube`], a labelled cube whose faces, edges and corners
//!   are all targets: 26 views, from straight on to a three-quarter corner.
//! - [`ViewCubeStyle::Balls`], the compact form: a ball per axis end, the
//!   positive ones labelled and solid, the negative ones hollow, near balls
//!   drawn larger than far ones.
//!
//! A click asks for the view from that direction, looking at the pivot from
//! the camera's current distance; a click on the view already showing asks
//! for the opposite one. A drag orbits the camera about the pivot: yaw about
//! the world's up axis, pitch about the camera's right. The gizmo only reads
//! the view's rotation, so it is drawn orthographically at any size.
use crate::camera::*;
use crate::prim::GizmoPrim;
use makepad_math::*;

/// Which picture the orientation gizmo draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ViewCubeStyle {
    #[default]
    Cube,
    Balls,
}

/// The world's up axis, which decides the face names and which way is up in
/// the views a click asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum UpAxis {
    #[default]
    Y,
    Z,
}

impl UpAxis {
    pub fn vector(self) -> Vec3f {
        match self {
            UpAxis::Y => vec3(0.0, 1.0, 0.0),
            UpAxis::Z => vec3(0.0, 0.0, 1.0),
        }
    }

    /// The screen's up for a camera looking straight along the up axis,
    /// chosen so +X reads to the right from above and from below.
    fn pole_up(self, dir: Vec3f) -> Vec3f {
        match self {
            UpAxis::Y => {
                if dir.y > 0.0 {
                    vec3(0.0, 0.0, -1.0)
                } else {
                    vec3(0.0, 0.0, 1.0)
                }
            }
            UpAxis::Z => {
                if dir.z > 0.0 {
                    vec3(0.0, 1.0, 0.0)
                } else {
                    vec3(0.0, -1.0, 0.0)
                }
            }
        }
    }

    /// The conventional name of the view from a face direction.
    pub fn face_name(self, dir: ViewDir) -> &'static str {
        match (self, dir.x, dir.y, dir.z) {
            (_, 1, 0, 0) => "Right",
            (_, -1, 0, 0) => "Left",
            (UpAxis::Y, 0, 1, 0) | (UpAxis::Z, 0, 0, 1) => "Top",
            (UpAxis::Y, 0, -1, 0) | (UpAxis::Z, 0, 0, -1) => "Bottom",
            (UpAxis::Y, 0, 0, 1) | (UpAxis::Z, 0, -1, 0) => "Front",
            (UpAxis::Y, 0, 0, -1) | (UpAxis::Z, 0, 1, 0) => "Back",
            _ => "",
        }
    }
}

/// A direction the camera can be asked to look from, toward the pivot. Each
/// component is -1, 0 or 1: one non-zero is a face, two an edge, three a
/// corner.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ViewDir {
    pub x: i8,
    pub y: i8,
    pub z: i8,
}

impl ViewDir {
    pub const POS_X: ViewDir = ViewDir::new(1, 0, 0);
    pub const NEG_X: ViewDir = ViewDir::new(-1, 0, 0);
    pub const POS_Y: ViewDir = ViewDir::new(0, 1, 0);
    pub const NEG_Y: ViewDir = ViewDir::new(0, -1, 0);
    pub const POS_Z: ViewDir = ViewDir::new(0, 0, 1);
    pub const NEG_Z: ViewDir = ViewDir::new(0, 0, -1);

    pub const fn new(x: i8, y: i8, z: i8) -> Self {
        Self { x, y, z }
    }

    /// Unit direction from the pivot to where the camera will stand.
    pub fn vector(self) -> Vec3f {
        safe_normalize(
            vec3(self.x as f32, self.y as f32, self.z as f32),
            vec3(0.0, 0.0, 1.0),
        )
    }

    pub fn opposite(self) -> Self {
        Self::new(-self.x, -self.y, -self.z)
    }

    pub fn is_face(self) -> bool {
        (self.x != 0) as u8 + (self.y != 0) as u8 + (self.z != 0) as u8 == 1
    }

    fn axis(i: usize, s: i8) -> Self {
        match i {
            0 => Self::new(s, 0, 0),
            1 => Self::new(0, s, 0),
            _ => Self::new(0, 0, s),
        }
    }

    fn add(self, o: ViewDir) -> Self {
        Self::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }

    fn scaled(self, s: i8) -> Self {
        Self::new(self.x * s, self.y * s, self.z * s)
    }
}

/// The orientation gizmo's configuration.
#[derive(Clone, Debug, PartialEq)]
pub struct ViewCube {
    pub style: ViewCubeStyle,
    pub up: UpAxis,
    pub axis_colors: [Vec4f; 3],
    pub face_color: Vec4f,
    pub edge_color: Vec4f,
    pub highlight: Vec4f,
    /// Face labels.
    pub text_color: Vec4f,
    /// Labels on the axis balls.
    pub ball_text_color: Vec4f,
    /// The disc behind the gizmo while the pointer is on it.
    pub backdrop: Vec4f,
    pub labels: bool,
}

impl Default for ViewCube {
    fn default() -> Self {
        Self {
            style: ViewCubeStyle::Cube,
            up: UpAxis::Y,
            axis_colors: [
                vec4(0.93, 0.29, 0.31, 1.0),
                vec4(0.47, 0.80, 0.27, 1.0),
                vec4(0.27, 0.54, 0.96, 1.0),
            ],
            face_color: vec4(0.30, 0.32, 0.36, 0.94),
            edge_color: vec4(0.06, 0.06, 0.08, 0.9),
            highlight: vec4(1.0, 0.83, 0.27, 1.0),
            text_color: vec4(0.95, 0.95, 0.95, 1.0),
            ball_text_color: vec4(0.06, 0.06, 0.08, 1.0),
            backdrop: vec4(0.55, 0.57, 0.62, 0.22),
            labels: true,
        }
    }
}

const AXES: [Vec3f; 3] = [
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

/// Where the gizmo's zones fall along a face: a band at each edge and the
/// middle, in face coordinates from -1 to 1.
const ZONES: [f32; 4] = [-1.0, -0.62, 0.62, 1.0];

/// How the camera sees a direction: right, up and toward the viewer.
#[derive(Clone, Copy)]
struct Basis {
    right: Vec3f,
    up: Vec3f,
    back: Vec3f,
}

impl Basis {
    fn of(view: &Mat4f) -> Self {
        let (right, up, back, _) = frame_of_view(view);
        Self { right, up, back }
    }

    fn cam(&self, d: Vec3f) -> Vec3f {
        vec3(d.dot(self.right), d.dot(self.up), d.dot(self.back))
    }
}

struct Ball {
    dir: ViewDir,
    pos: Vec2f,
    radius: f32,
    depth: f32,
    axis: usize,
    positive: bool,
}

struct Zone {
    dir: ViewDir,
    pts: [Vec2f; 4],
}

struct Face {
    dir: ViewDir,
    axis: usize,
    depth: f32,
    outline: [Vec2f; 4],
    center: Vec2f,
    zones: Vec<Zone>,
}

impl ViewCube {
    fn radius(rect: GizmoRect) -> f32 {
        (rect.w.min(rect.h) * 0.5).max(1.0)
    }

    /// Whether a point is on the gizmo's round face: the region a drag
    /// orbits from.
    pub fn contains(&self, rect: GizmoRect, mouse: Vec2f) -> bool {
        (mouse - rect.center()).length() <= Self::radius(rect)
    }

    fn balls(&self, basis: &Basis, rect: GizmoRect) -> Vec<Ball> {
        let r = Self::radius(rect);
        let center = rect.center();
        let arm = r * 0.68;
        let mut out = Vec::with_capacity(6);
        for (i, axis) in AXES.iter().enumerate() {
            for positive in [true, false] {
                let d = if positive { *axis } else { *axis * -1.0 };
                let c = basis.cam(d);
                let size = if positive { 0.17 } else { 0.13 };
                out.push(Ball {
                    dir: ViewDir::axis(i, if positive { 1 } else { -1 }),
                    pos: center + vec2(c.x, -c.y) * arm,
                    radius: r * size * (0.85 + 0.15 * c.z),
                    depth: c.z,
                    axis: i,
                    positive,
                });
            }
        }
        out.sort_by(|a, b| {
            a.depth
                .partial_cmp(&b.depth)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        out
    }

    fn faces(&self, basis: &Basis, rect: GizmoRect) -> Vec<Face> {
        let r = Self::radius(rect);
        let center = rect.center();
        // Half the cube's side, so its corners reach 0.87 of the radius.
        let h = 0.5;
        let to_screen = |p: Vec3f| {
            let c = basis.cam(p);
            center + vec2(c.x, -c.y) * r
        };
        let mut out = Vec::new();
        for i in 0..3 {
            for s in [1i8, -1i8] {
                let n = AXES[i] * s as f32;
                let depth = n.dot(basis.back);
                if depth <= 0.02 {
                    continue;
                }
                let iu = (i + 1) % 3;
                let iv = (i + 2) % 3;
                let u = AXES[iu];
                let v = AXES[iv];
                let at = |x: f32, y: f32| to_screen(n * h + u * (x * h) + v * (y * h));
                let mut zones = Vec::with_capacity(9);
                for a in 0..3 {
                    for b in 0..3 {
                        let (x0, x1) = (ZONES[a], ZONES[a + 1]);
                        let (y0, y1) = (ZONES[b], ZONES[b + 1]);
                        let dir = ViewDir::axis(i, s)
                            .add(ViewDir::axis(iu, 1).scaled(a as i8 - 1))
                            .add(ViewDir::axis(iv, 1).scaled(b as i8 - 1));
                        zones.push(Zone {
                            dir,
                            pts: [at(x0, y0), at(x1, y0), at(x1, y1), at(x0, y1)],
                        });
                    }
                }
                out.push(Face {
                    dir: ViewDir::axis(i, s),
                    axis: i,
                    depth,
                    outline: [at(-1.0, -1.0), at(1.0, -1.0), at(1.0, 1.0), at(-1.0, 1.0)],
                    center: to_screen(n * h),
                    zones,
                });
            }
        }
        out.sort_by(|a, b| {
            a.depth
                .partial_cmp(&b.depth)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        out
    }

    /// The view target under the pointer, if any.
    pub fn hover(&self, view: &Mat4f, rect: GizmoRect, mouse: Vec2f) -> Option<ViewDir> {
        if !self.contains(rect, mouse) {
            return None;
        }
        let basis = Basis::of(view);
        match self.style {
            ViewCubeStyle::Balls => self
                .balls(&basis, rect)
                .iter()
                .rev()
                .find(|b| (mouse - b.pos).length() <= b.radius + 2.0)
                .map(|b| b.dir),
            ViewCubeStyle::Cube => {
                for face in self.faces(&basis, rect).iter().rev() {
                    for zone in &face.zones {
                        if inside_convex(mouse, &zone.pts) {
                            return Some(zone.dir);
                        }
                    }
                }
                None
            }
        }
    }

    /// The gizmo's picture for this view. `hot` shows the backdrop, for
    /// while the pointer is on the gizmo.
    pub fn geometry(
        &self,
        view: &Mat4f,
        rect: GizmoRect,
        hover: Option<ViewDir>,
        hot: bool,
    ) -> Vec<GizmoPrim> {
        let mut out = Vec::new();
        let basis = Basis::of(view);
        let center = rect.center();
        let r = Self::radius(rect);
        if hot && self.backdrop.w > 0.0 {
            out.push(GizmoPrim::disc(center, r, self.backdrop));
        }
        match self.style {
            ViewCubeStyle::Balls => self.ball_geometry(&basis, rect, hover, &mut out),
            ViewCubeStyle::Cube => self.cube_geometry(&basis, rect, hover, &mut out),
        }
        out
    }

    fn ball_geometry(
        &self,
        basis: &Basis,
        rect: GizmoRect,
        hover: Option<ViewDir>,
        out: &mut Vec<GizmoPrim>,
    ) {
        let center = rect.center();
        let balls = self.balls(basis, rect);
        for (n, ball) in balls.iter().enumerate() {
            let color = self.axis_colors[ball.axis];
            let lit = hover == Some(ball.dir);
            if ball.positive {
                out.push(GizmoPrim::line(
                    center,
                    ball.pos,
                    2.0,
                    with_alpha(color, 0.9),
                ));
            }
            let (fill, stroke) = if ball.positive {
                let fill = if lit {
                    mix4(color, vec4(1.0, 1.0, 1.0, 1.0), 0.35)
                } else {
                    color
                };
                (
                    fill,
                    if lit {
                        self.highlight
                    } else {
                        with_alpha(color, 0.0)
                    },
                )
            } else {
                let fill = vec4(color.x * 0.3, color.y * 0.3, color.z * 0.3, 0.85);
                (
                    if lit { mix4(fill, color, 0.6) } else { fill },
                    if lit { self.highlight } else { color },
                )
            };
            out.push(GizmoPrim::Circle {
                center: ball.pos,
                radius: ball.radius,
                width: if lit || !ball.positive { 1.5 } else { 0.0 },
                color: stroke,
                fill,
            });
            if !self.labels {
                continue;
            }
            // Text paints over every shape, so a label whose ball is covered
            // by a nearer one is left out rather than shown through it.
            let covered = balls[n + 1..]
                .iter()
                .any(|near| (near.pos - ball.pos).length() < near.radius);
            if covered {
                continue;
            }
            let name = ["X", "Y", "Z"][ball.axis];
            if ball.positive {
                out.push(GizmoPrim::text(
                    ball.pos,
                    name,
                    self.ball_text_color,
                    vec2(0.5, 0.5),
                ));
            } else if lit {
                out.push(GizmoPrim::text(
                    ball.pos,
                    format!("-{}", name),
                    self.text_color,
                    vec2(0.5, 0.5),
                ));
            }
        }
    }

    fn cube_geometry(
        &self,
        basis: &Basis,
        rect: GizmoRect,
        hover: Option<ViewDir>,
        out: &mut Vec<GizmoPrim>,
    ) {
        let faces = self.faces(basis, rect);
        for face in &faces {
            let tint = mix4(self.face_color, self.axis_colors[face.axis], 0.22);
            // Faces turned toward the viewer read a little lighter, so the
            // cube reads as a solid rather than a flat hexagon.
            let shade = mix4(tint, vec4(1.0, 1.0, 1.0, tint.w), 0.12 * face.depth);
            for zone in &face.zones {
                let fill = if hover == Some(zone.dir) {
                    self.highlight
                } else {
                    shade
                };
                out.push(GizmoPrim::Quad {
                    pts: zone.pts,
                    fill,
                    width: 0.0,
                    color: fill,
                });
            }
            out.push(GizmoPrim::Quad {
                pts: face.outline,
                fill: vec4(0.0, 0.0, 0.0, 0.0),
                width: 1.0,
                color: self.edge_color,
            });
        }
        if self.labels {
            for face in &faces {
                let a = fade_in(0.35, 0.7, face.depth);
                if a <= 0.01 {
                    continue;
                }
                let name = self.up.face_name(face.dir);
                let color = if hover == Some(face.dir) {
                    self.ball_text_color
                } else {
                    self.text_color
                };
                out.push(GizmoPrim::text(
                    face.center,
                    name,
                    with_alpha(color, a),
                    vec2(0.5, 0.5),
                ));
            }
        }
    }

    /// The direction a click on `dir` asks for: `dir` itself, or its
    /// opposite when the camera already looks from there.
    pub fn click_target(&self, view: &Mat4f, dir: ViewDir) -> ViewDir {
        let (_, _, back, _) = frame_of_view(view);
        if back.dot(dir.vector()) > 0.9999 {
            dir.opposite()
        } else {
            dir
        }
    }

    /// The view from `dir`, looking at `pivot` from the camera's current
    /// distance to it.
    pub fn snap_view(&self, view: &Mat4f, pivot: Vec3f, dir: ViewDir) -> Mat4f {
        look_from(dir.vector(), pivot, view_distance(view, pivot), self.up)
    }
}

/// How far a camera stands from a point.
pub fn view_distance(view: &Mat4f, pivot: Vec3f) -> f32 {
    let (_, _, _, eye) = frame_of_view(view);
    (eye - pivot).length().max(1.0e-3)
}

/// A camera standing `distance` from `pivot` in the unit direction `dir`,
/// looking at it, with the world's `up` up (or, looking straight along it,
/// the pole's up).
pub fn look_from(dir: Vec3f, pivot: Vec3f, distance: f32, up: UpAxis) -> Mat4f {
    let dir = safe_normalize(dir, vec3(0.0, 0.0, 1.0));
    let world_up = up.vector();
    let screen_up = if dir.dot(world_up).abs() > 0.999 {
        up.pole_up(dir)
    } else {
        world_up
    };
    Mat4f::look_at(pivot + dir * distance, pivot, screen_up)
}

/// Orbit a camera about `pivot`: `yaw` radians about the world's up axis and
/// `pitch` radians about the camera's own right axis.
pub fn orbit_view(view: &Mat4f, pivot: Vec3f, world_up: Vec3f, yaw: f32, pitch: f32) -> Mat4f {
    let (right, up, back, eye) = frame_of_view(view);
    let rot = Mat4f::mul(
        &rotation_matrix(world_up, yaw),
        &rotation_matrix(right, pitch),
    );
    let back = safe_normalize(transform_dir(&rot, back), back);
    let up = transform_dir(&rot, up);
    let right = safe_normalize(Vec3f::cross(up, back), right);
    let up = Vec3f::cross(back, right);
    let eye = pivot + transform_dir(&rot, eye - pivot);
    view_from_frame(right, up, back, eye)
}

/// A view part of the way from `from` to `to`, turning about `pivot`: the
/// orientation slerps, and the eye swings round the pivot rather than
/// cutting across. `t` runs 0 to 1.
pub fn interpolate_view(from: &Mat4f, to: &Mat4f, pivot: Vec3f, t: f32) -> Mat4f {
    let t = t.clamp(0.0, 1.0);
    let (r0, u0, b0, e0) = frame_of_view(from);
    let (r1, u1, b1, e1) = frame_of_view(to);
    let q0 = quat_from_basis(r0, u0, b0);
    let q1 = quat_from_basis(r1, u1, b1);
    let q = quat_slerp(q0, q1, t);
    let right = quat_rotate(q, vec3(1.0, 0.0, 0.0));
    let up = quat_rotate(q, vec3(0.0, 1.0, 0.0));
    let back = quat_rotate(q, vec3(0.0, 0.0, 1.0));
    // Each end's eye, held in its own camera's frame and carried round by
    // the current orientation: exact at both ends, a swing between.
    let a = quat_rotate(q, quat_rotate(quat_conj(q0), e0 - pivot));
    let b = quat_rotate(q, quat_rotate(quat_conj(q1), e1 - pivot));
    let eye = pivot + a * (1.0 - t) + b * t;
    view_from_frame(right, up, back, eye)
}

// ---- quaternions, just what the interpolation needs -----------------------

/// The rotation whose matrix has columns `r`, `u` and `b`.
fn quat_from_basis(r: Vec3f, u: Vec3f, b: Vec3f) -> Quat {
    let (m00, m01, m02) = (r.x, u.x, b.x);
    let (m10, m11, m12) = (r.y, u.y, b.y);
    let (m20, m21, m22) = (r.z, u.z, b.z);
    let trace = m00 + m11 + m22;
    let q = if trace > 0.0 {
        let s = (trace + 1.0).sqrt() * 2.0;
        Quat {
            w: 0.25 * s,
            x: (m21 - m12) / s,
            y: (m02 - m20) / s,
            z: (m10 - m01) / s,
        }
    } else if m00 > m11 && m00 > m22 {
        let s = (1.0 + m00 - m11 - m22).max(1.0e-12).sqrt() * 2.0;
        Quat {
            w: (m21 - m12) / s,
            x: 0.25 * s,
            y: (m01 + m10) / s,
            z: (m02 + m20) / s,
        }
    } else if m11 > m22 {
        let s = (1.0 + m11 - m00 - m22).max(1.0e-12).sqrt() * 2.0;
        Quat {
            w: (m02 - m20) / s,
            x: (m01 + m10) / s,
            y: 0.25 * s,
            z: (m12 + m21) / s,
        }
    } else {
        let s = (1.0 + m22 - m00 - m11).max(1.0e-12).sqrt() * 2.0;
        Quat {
            w: (m10 - m01) / s,
            x: (m02 + m20) / s,
            y: (m12 + m21) / s,
            z: 0.25 * s,
        }
    };
    quat_normalize(q)
}

fn quat_normalize(q: Quat) -> Quat {
    let l = (q.x * q.x + q.y * q.y + q.z * q.z + q.w * q.w).sqrt();
    if l > 1.0e-12 {
        Quat {
            x: q.x / l,
            y: q.y / l,
            z: q.z / l,
            w: q.w / l,
        }
    } else {
        Quat::default()
    }
}

fn quat_conj(q: Quat) -> Quat {
    Quat {
        x: -q.x,
        y: -q.y,
        z: -q.z,
        w: q.w,
    }
}

fn quat_rotate(q: Quat, v: Vec3f) -> Vec3f {
    let qv = vec3(q.x, q.y, q.z);
    let t = Vec3f::cross(qv, v) * 2.0;
    v + t * q.w + Vec3f::cross(qv, t)
}

fn quat_slerp(a: Quat, b: Quat, t: f32) -> Quat {
    let mut b = b;
    let mut d = a.x * b.x + a.y * b.y + a.z * b.z + a.w * b.w;
    if d < 0.0 {
        b = Quat {
            x: -b.x,
            y: -b.y,
            z: -b.z,
            w: -b.w,
        };
        d = -d;
    }
    let (wa, wb) = if d > 0.9995 {
        (1.0 - t, t)
    } else {
        let theta = d.clamp(-1.0, 1.0).acos();
        let s = theta.sin();
        (((1.0 - t) * theta).sin() / s, (t * theta).sin() / s)
    };
    quat_normalize(Quat {
        x: a.x * wa + b.x * wb,
        y: a.y * wa + b.y * wb,
        z: a.z * wa + b.z * wb,
        w: a.w * wa + b.w * wb,
    })
}
