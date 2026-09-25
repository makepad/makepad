//! The camera a gizmo is seen through, rays under the pointer, and the small
//! amount of matrix and 2D arithmetic the gizmos share.
use makepad_math::*;

/// A rectangle in screen space: the viewport a camera renders into, or the
/// box an orientation gizmo is drawn in. Pixels, origin top left, y down.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GizmoRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Default for GizmoRect {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
        }
    }
}

impl GizmoRect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub fn center(&self) -> Vec2f {
        vec2(self.x + self.w * 0.5, self.y + self.h * 0.5)
    }

    pub fn contains(&self, p: Vec2f) -> bool {
        p.x >= self.x && p.y >= self.y && p.x <= self.x + self.w && p.y <= self.y + self.h
    }
}

/// A line of sight through a pixel.
///
/// Through a perspective camera it starts at the eye and only its forward
/// half is real. Through an orthographic camera every point along the line
/// projects to the same pixel, so both directions count.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ray {
    pub origin: Vec3f,
    /// Unit length.
    pub dir: Vec3f,
    /// True when points behind `origin` are on the line of sight too.
    pub both_ways: bool,
}

impl Ray {
    pub fn at(&self, t: f32) -> Vec3f {
        self.origin + self.dir * t
    }

    /// Where the ray meets the plane through `point` with normal `normal`,
    /// or None when it runs parallel to it or meets it behind the eye.
    pub fn intersect_plane(&self, point: Vec3f, normal: Vec3f) -> Option<Vec3f> {
        let denom = self.dir.dot(normal);
        if denom.abs() < 1.0e-6 {
            return None;
        }
        let t = (point - self.origin).dot(normal) / denom;
        if !self.both_ways && t < 0.0 {
            return None;
        }
        let p = self.at(t);
        if p.is_finite() {
            Some(p)
        } else {
            None
        }
    }

    /// The parameter along the line `p + d * s` of its point nearest this
    /// ray. None when the two are parallel. `d` need not be unit length; the
    /// answer is in multiples of it.
    pub fn closest_on_line(&self, p: Vec3f, d: Vec3f) -> Option<f32> {
        let w = p - self.origin;
        let a = d.dot(d);
        let b = d.dot(self.dir);
        let c = self.dir.dot(self.dir);
        let denom = a * c - b * b;
        if denom.abs() < 1.0e-9 {
            return None;
        }
        let e = d.dot(w);
        let f = self.dir.dot(w);
        Some((b * f - c * e) / denom)
    }
}

/// The matrices a scene is drawn with, and the viewport they land in.
///
/// Build it with [`GizmoCamera::new`], which also caches the inverses every
/// query needs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GizmoCamera {
    view: Mat4f,
    proj: Mat4f,
    viewport: GizmoRect,
    view_proj: Mat4f,
    inv_view_proj: Mat4f,
    inv_view: Mat4f,
}

impl Default for GizmoCamera {
    fn default() -> Self {
        Self::new(Mat4f::identity(), Mat4f::identity(), GizmoRect::default())
    }
}

impl GizmoCamera {
    pub fn new(view: Mat4f, proj: Mat4f, viewport: GizmoRect) -> Self {
        let view_proj = Mat4f::mul(&proj, &view);
        Self {
            view,
            proj,
            viewport,
            view_proj,
            inv_view_proj: view_proj.invert(),
            inv_view: view.invert(),
        }
    }

    pub fn view(&self) -> &Mat4f {
        &self.view
    }

    pub fn proj(&self) -> &Mat4f {
        &self.proj
    }

    pub fn viewport(&self) -> GizmoRect {
        self.viewport
    }

    /// An orthographic projection leaves w alone.
    pub fn is_ortho(&self) -> bool {
        self.proj.v[11].abs() < 1.0e-6 && (self.proj.v[15] - 1.0).abs() < 1.0e-6
    }

    /// The camera's position in the world.
    pub fn eye(&self) -> Vec3f {
        mat_col(&self.inv_view, 3)
    }

    /// The camera's right, up and back (toward the viewer) directions in the
    /// world, unit length.
    pub fn right(&self) -> Vec3f {
        safe_normalize(mat_col(&self.inv_view, 0), vec3(1.0, 0.0, 0.0))
    }

    pub fn up(&self) -> Vec3f {
        safe_normalize(mat_col(&self.inv_view, 1), vec3(0.0, 1.0, 0.0))
    }

    pub fn back(&self) -> Vec3f {
        safe_normalize(mat_col(&self.inv_view, 2), vec3(0.0, 0.0, 1.0))
    }

    /// Unit direction from `p` toward the camera: toward the eye through a
    /// perspective camera, the camera's back direction through an
    /// orthographic one.
    pub fn to_camera(&self, p: Vec3f) -> Vec3f {
        if self.is_ortho() {
            self.back()
        } else {
            safe_normalize(self.eye() - p, self.back())
        }
    }

    /// Clip-space coordinates of a world point.
    pub fn clip(&self, p: Vec3f) -> Vec4f {
        self.view_proj.transform_vec4(vec4(p.x, p.y, p.z, 1.0))
    }

    /// A world point on the screen, or None when it is behind the camera.
    pub fn project(&self, p: Vec3f) -> Option<Vec2f> {
        let c = self.clip(p);
        if c.w <= 1.0e-6 {
            return None;
        }
        let nx = c.x / c.w;
        let ny = c.y / c.w;
        let v = &self.viewport;
        let s = vec2(v.x + (nx * 0.5 + 0.5) * v.w, v.y + (0.5 - ny * 0.5) * v.h);
        if s.x.is_finite() && s.y.is_finite() {
            Some(s)
        } else {
            None
        }
    }

    /// Normalised device coordinates of a screen point.
    fn ndc(&self, s: Vec2f) -> Vec2f {
        let v = &self.viewport;
        vec2(
            (s.x - v.x) / v.w.max(1.0e-6) * 2.0 - 1.0,
            1.0 - (s.y - v.y) / v.h.max(1.0e-6) * 2.0,
        )
    }

    fn unproject(&self, ndc: Vec2f, z: f32) -> Vec3f {
        let p = self
            .inv_view_proj
            .transform_vec4(vec4(ndc.x, ndc.y, z, 1.0));
        let w = if p.w.abs() < 1.0e-9 { 1.0e-9 } else { p.w };
        vec3(p.x / w, p.y / w, p.z / w)
    }

    /// The line of sight through a screen point.
    pub fn ray(&self, s: Vec2f) -> Ray {
        let n = self.ndc(s);
        // Two depths well inside any depth range, near and far planes alike,
        // including an infinite far plane.
        let a = self.unproject(n, -0.5);
        let b = self.unproject(n, 0.5);
        let dir = safe_normalize(b - a, self.back() * -1.0);
        if self.is_ortho() {
            Ray {
                origin: a,
                dir,
                both_ways: true,
            }
        } else {
            Ray {
                origin: self.eye(),
                dir,
                both_ways: false,
            }
        }
    }

    /// How long one screen pixel is in the world at a point's depth, so a
    /// gizmo can keep a constant size on screen.
    pub fn world_per_pixel(&self, p: Vec3f) -> f32 {
        let w = self.clip(p).w;
        let w = if self.is_ortho() { 1.0 } else { w.max(1.0e-6) };
        let sy = self.proj.v[5].abs().max(1.0e-9);
        2.0 * w / (sy * self.viewport.h.max(1.0))
    }
}

// ---- matrix helpers -------------------------------------------------------

/// Column `i` of a matrix as a 3-vector: the image of an axis for i < 3,
/// the translation for i == 3.
pub fn mat_col(m: &Mat4f, i: usize) -> Vec3f {
    vec3(m.v[i * 4], m.v[i * 4 + 1], m.v[i * 4 + 2])
}

/// The translation of an affine matrix.
pub fn mat_translation(m: &Mat4f) -> Vec3f {
    mat_col(m, 3)
}

pub fn transform_point(m: &Mat4f, p: Vec3f) -> Vec3f {
    let r = m.transform_vec4(vec4(p.x, p.y, p.z, 1.0));
    vec3(r.x, r.y, r.z)
}

pub fn transform_dir(m: &Mat4f, d: Vec3f) -> Vec3f {
    let r = m.transform_vec4(vec4(d.x, d.y, d.z, 0.0));
    vec3(r.x, r.y, r.z)
}

/// A rotation of `angle` radians about the unit `axis`, right handed.
pub fn rotation_matrix(axis: Vec3f, angle: f32) -> Mat4f {
    let a = safe_normalize(axis, vec3(0.0, 0.0, 1.0));
    let (s, c) = angle.sin_cos();
    let t = 1.0 - c;
    let (x, y, z) = (a.x, a.y, a.z);
    Mat4f {
        v: [
            c + t * x * x,
            t * x * y + s * z,
            t * x * z - s * y,
            0.0,
            t * x * y - s * z,
            c + t * y * y,
            t * y * z + s * x,
            0.0,
            t * x * z + s * y,
            t * y * z - s * x,
            c + t * z * z,
            0.0,
            0.0,
            0.0,
            0.0,
            1.0,
        ],
    }
}

/// A scale along the three axes.
pub fn scale_matrix(s: Vec3f) -> Mat4f {
    Mat4f::nonuniform_scaled_translation(s, vec3(0.0, 0.0, 0.0))
}

/// The view matrix of a camera at `eye` with the given unit right, up and
/// back (toward the viewer) directions.
pub fn view_from_frame(right: Vec3f, up: Vec3f, back: Vec3f, eye: Vec3f) -> Mat4f {
    Mat4f {
        v: [
            right.x,
            up.x,
            back.x,
            0.0,
            right.y,
            up.y,
            back.y,
            0.0,
            right.z,
            up.z,
            back.z,
            0.0,
            -right.dot(eye),
            -up.dot(eye),
            -back.dot(eye),
            1.0,
        ],
    }
}

/// A camera's unit right, up and back directions and its eye, from its view
/// matrix.
pub fn frame_of_view(view: &Mat4f) -> (Vec3f, Vec3f, Vec3f, Vec3f) {
    let inv = view.invert();
    (
        safe_normalize(mat_col(&inv, 0), vec3(1.0, 0.0, 0.0)),
        safe_normalize(mat_col(&inv, 1), vec3(0.0, 1.0, 0.0)),
        safe_normalize(mat_col(&inv, 2), vec3(0.0, 0.0, 1.0)),
        mat_col(&inv, 3),
    )
}

/// `v` at unit length, or `fallback` when it has none to speak of.
pub fn safe_normalize(v: Vec3f, fallback: Vec3f) -> Vec3f {
    let l = v.length();
    if l > 1.0e-9 && l.is_finite() {
        v * (1.0 / l)
    } else {
        fallback
    }
}

// ---- 2D helpers -----------------------------------------------------------

pub(crate) fn cross2(a: Vec2f, b: Vec2f) -> f32 {
    a.x * b.y - a.y * b.x
}

pub(crate) fn dot2(a: Vec2f, b: Vec2f) -> f32 {
    a.x * b.x + a.y * b.y
}

pub(crate) fn norm2(v: Vec2f) -> Vec2f {
    let l = v.length();
    if l > 1.0e-6 {
        v * (1.0 / l)
    } else {
        vec2(1.0, 0.0)
    }
}

pub(crate) fn dist_to_segment(p: Vec2f, a: Vec2f, b: Vec2f) -> f32 {
    let ab = b - a;
    let l2 = dot2(ab, ab);
    let t = if l2 > 1.0e-9 {
        (dot2(p - a, ab) / l2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (p - (a + ab * t)).length()
}

/// Whether `p` is inside a convex polygon given in either winding.
pub(crate) fn inside_convex(p: Vec2f, pts: &[Vec2f]) -> bool {
    let n = pts.len();
    if n < 3 {
        return false;
    }
    let mut sign = 0.0f32;
    for i in 0..n {
        let a = pts[i];
        let b = pts[(i + 1) % n];
        if (b - a).length() < 1.0e-6 {
            continue;
        }
        let c = cross2(b - a, p - a);
        if c.abs() < 1.0e-9 {
            continue;
        }
        if sign == 0.0 {
            sign = c.signum();
        } else if c.signum() != sign {
            return false;
        }
    }
    sign != 0.0
}

/// Distance from `p` to a convex polygon: zero inside, to the nearest edge
/// outside.
pub(crate) fn dist_to_convex(p: Vec2f, pts: &[Vec2f]) -> f32 {
    if inside_convex(p, pts) {
        return 0.0;
    }
    let n = pts.len();
    let mut best = f32::MAX;
    for i in 0..n {
        best = best.min(dist_to_segment(p, pts[i], pts[(i + 1) % n]));
    }
    best
}

pub(crate) fn fade_in(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

pub(crate) fn with_alpha(c: Vec4f, a: f32) -> Vec4f {
    vec4(c.x, c.y, c.z, c.w * a)
}

pub(crate) fn mix4(a: Vec4f, b: Vec4f, t: f32) -> Vec4f {
    a + (b - a) * t
}
