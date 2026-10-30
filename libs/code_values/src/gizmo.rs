//! 3D values edited in a small 3D view: a point ([`Control::Point3`]), a
//! direction ([`Control::Direction`]) and a path of keys
//! ([`Control::Path3`]). The view is drawn into its own pass (lines and
//! discs through the line renderer) only while its popup is open, and only
//! when what it shows changed; between changes the last picture is reused.
//!
//! - drag: the point (or the key under the pointer) moves in the plane
//!   facing you; on an axis handle, along that axis;
//! - alt-drag: along the axis the drag first heads along;
//! - right-drag, ctrl-drag (touchpads) or a drag on empty space: orbit;
//!   scroll: closer, further; a ctrl-click stays a click;
//! - shift: whole steps of the grid.

use crate::data::*;
use crate::*;
use makepad_render_lines::{Blend, LineBatch, LineRenderer, LinePoint, LineStyle, LineView, PointBatch, PointStyle, Segment, Sprite, SpriteShape};

/// The 3D popup's size, its header and its footer.
pub(crate) const SPACE_W: f64 = 340.0;
pub(crate) const SPACE_H: f64 = 318.0;
const HEAD: f64 = 26.0;
const FOOT: f64 = 18.0;
const FOV: f64 = 38.0;

pub(crate) fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
pub(crate) fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
pub(crate) fn mul(a: [f64; 3], s: f64) -> [f64; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}
pub(crate) fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
pub(crate) fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
const AXES: [[f64; 3]; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

/// How the view looks at its subject.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Orbit {
    pub yaw: f64,
    pub pitch: f64,
    pub dist: f64,
    pub target: [f64; 3],
}

impl Orbit {
    /// The calm three-quarter view the editors open with.
    pub fn framing(target: [f64; 3], radius: f64) -> Self {
        let half = (FOV.to_radians() * 0.5).tan();
        Self { yaw: 0.62, pitch: 0.42, dist: radius.max(1e-3) / half * 1.15, target }
    }
}

/// A camera laid out over a rect of the screen.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Cam {
    pub eye: [f64; 3],
    pub fwd: [f64; 3],
    pub right: [f64; 3],
    pub up: [f64; 3],
    pub tan_y: f64,
    pub tan_x: f64,
    pub rect: Rect,
    pub orbit: Orbit,
}

impl Cam {
    pub fn new(orbit: &Orbit, rect: Rect) -> Self {
        let (cp, sp) = (orbit.pitch.cos(), orbit.pitch.sin());
        let back = [cp * orbit.yaw.sin(), sp, cp * orbit.yaw.cos()];
        let eye = add(orbit.target, mul(back, orbit.dist));
        let fwd = mul(back, -1.0);
        let right = normalize3(cross(fwd, [0.0, 1.0, 0.0]));
        let up = cross(right, fwd);
        let tan_y = (FOV.to_radians() * 0.5).tan();
        let tan_x = tan_y * rect.size.x / rect.size.y.max(1.0);
        Self { eye, fwd, right, up, tan_y, tan_x, rect, orbit: *orbit }
    }

    /// Where a world point lands on screen, and how far in front it is.
    pub fn project(&self, p: [f64; 3]) -> Option<(DVec2, f64)> {
        let rel = sub(p, self.eye);
        let z = dot(rel, self.fwd);
        if z < self.orbit.dist * 0.01 {
            return None;
        }
        let x = dot(rel, self.right) / (z * self.tan_x);
        let y = dot(rel, self.up) / (z * self.tan_y);
        let r = self.rect;
        Some((dvec2(r.pos.x + (x + 1.0) * 0.5 * r.size.x, r.pos.y + (1.0 - y) * 0.5 * r.size.y), z))
    }

    /// The ray through a screen point: from the eye, unit length.
    pub fn ray(&self, abs: DVec2) -> ([f64; 3], [f64; 3]) {
        let r = self.rect;
        let nx = (abs.x - r.pos.x) / r.size.x * 2.0 - 1.0;
        let ny = 1.0 - (abs.y - r.pos.y) / r.size.y * 2.0;
        let d = add(self.fwd, add(mul(self.right, nx * self.tan_x), mul(self.up, ny * self.tan_y)));
        (self.eye, normalize3(d))
    }

    /// The renderer's matrices for this camera.
    pub fn matrices(&self) -> (Mat4f, Mat4f) {
        let f = |v: [f64; 3]| vec3f(v[0] as f32, v[1] as f32, v[2] as f32);
        let view = Mat4f::look_at(f(self.eye), f(self.orbit.target), vec3f(0.0, 1.0, 0.0));
        let near = (self.orbit.dist * 0.01) as f32;
        let proj = Mat4f::perspective(FOV as f32, (self.rect.size.x / self.rect.size.y.max(1.0)) as f32, near, (self.orbit.dist * 50.0) as f32);
        (view, proj)
    }

    /// The point where the ray through `abs` meets the plane through `at`
    /// facing the camera.
    pub fn on_plane(&self, abs: DVec2, at: [f64; 3]) -> [f64; 3] {
        let (o, d) = self.ray(abs);
        let denom = dot(d, self.fwd);
        let t = if denom.abs() < 1e-9 { 0.0 } else { dot(sub(at, o), self.fwd) / denom };
        add(o, mul(d, t))
    }

    /// Where along the line `at + s·axis` the ray through `abs` passes
    /// closest: `s`.
    pub fn on_axis(&self, abs: DVec2, at: [f64; 3], axis: [f64; 3]) -> Option<f64> {
        let (o, d) = self.ray(abs);
        let w0 = sub(at, o);
        let b = dot(axis, d);
        let denom = 1.0 - b * b;
        if denom.abs() < 1e-6 {
            return None;
        }
        Some((b * dot(d, w0) - dot(axis, w0)) / denom)
    }

    /// The unit sphere's point under `abs` (the front hit, or the nearest
    /// point of its outline when the ray misses).
    pub fn on_sphere(&self, abs: DVec2) -> [f64; 3] {
        let (o, d) = self.ray(abs);
        let b = dot(o, d);
        let c = dot(o, o) - 1.0;
        let disc = b * b - c;
        if disc >= 0.0 {
            let t = -b - disc.sqrt();
            return normalize3(add(o, mul(d, t)));
        }
        normalize3(add(o, mul(d, -b)))
    }
}

/// What a 3D popup edits.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum SpaceKind {
    Point,
    Direction,
    Path,
}

/// What is under the pointer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Grab {
    Key(usize),
    /// A key's handle along an axis.
    Axis(usize, usize),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum DragMode {
    Plane,
    Axis(usize),
    /// Alt held: along the axis the drag first heads along.
    AxisPending,
    Orbit,
    Sphere,
}

#[derive(Clone, Debug)]
pub(crate) struct SpaceDrag {
    pub mode: DragMode,
    pub key: usize,
    pub from: DVec2,
    /// The key's value and its numbers as written when the drag began.
    pub start: [f64; 3],
    pub written: Vec<String>,
    /// Where the press met the drag's plane or axis.
    pub hit0: [f64; 3],
    pub s0: f64,
    pub orbit0: Orbit,
    /// Ctrl held: an orbit once the drag travels 4 points.
    pub orbit_on_drag: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct SpaceState {
    pub kind: SpaceKind,
    pub orbit: Orbit,
    /// The grid's half size: a round number around what is shown.
    pub extent: f64,
    pub hover: Option<Grab>,
    pub drag: Option<SpaceDrag>,
    /// The path's key last picked (its handles show).
    pub selected: usize,
}

/// A colour for the view's target (premultiplied; the target is shown as
/// it is drawn).
fn pm(rgb: u32, a: f32) -> [f32; 4] {
    let c = Vec4f::from_u32((rgb << 8) | 0xff);
    [c.x * a, c.y * a, c.z * a, a]
}

pub(crate) const AXIS_INK: [u32; 3] = [0xe5737a, 0x8fd18a, 0x6fa8f0];
const ORANGE: u32 = 0xff7a2e;
const TEAL: u32 = 0x4ec9b0;
const PALE: u32 = 0xdcdcaa;

/// The keys a 3D value has in `source` at `start`: each key's numbers.
pub(crate) fn keys_of(kind: SpaceKind, source: &str, start: usize) -> Vec<Vec<(std::ops::Range<usize>, f64)>> {
    match kind {
        SpaceKind::Point | SpaceKind::Direction => {
            let v = vector_items(source, start);
            if v.len() == 3 { vec![v] } else { Vec::new() }
        }
        SpaceKind::Path => path_items(source, start),
    }
}

fn value3(items: &[(std::ops::Range<usize>, f64)]) -> [f64; 3] {
    [items[0].1, items[1].1, items[2].1]
}

impl SpaceState {
    /// A view framing the value at `start`.
    pub fn open(kind: SpaceKind, source: &str, start: usize) -> Self {
        let keys: Vec<[f64; 3]> = keys_of(kind, source, start).iter().map(|k| value3(k)).collect();
        let (orbit, extent) = match kind {
            SpaceKind::Direction => (Orbit::framing([0.0; 3], 1.35), 1.0),
            _ => {
                // The keys and the origin, framed.
                let mut lo = [0.0f64; 3];
                let mut hi = [0.0f64; 3];
                for k in &keys {
                    for a in 0..3 {
                        lo[a] = lo[a].min(k[a]);
                        hi[a] = hi[a].max(k[a]);
                    }
                }
                let reach = (0..3).map(|a| lo[a].abs().max(hi[a].abs())).fold(0.0, f64::max);
                let extent = nice_ceil(reach.max(0.5));
                // The ground's grid and what stands on it, whole.
                let top = hi[1].max(0.0).min(extent);
                let centre = [0.0, top * 0.5 + lo[1].min(0.0) * 0.5, 0.0];
                (Orbit::framing(centre, extent * 1.05), extent)
            }
        };
        Self { kind, orbit, extent, hover: None, drag: None, selected: 0 }
    }
}

/// The view's rect inside the popup's.
pub(crate) fn view_rect(rect: Rect) -> Rect {
    Rect { pos: rect.pos + dvec2(6.0, HEAD), size: rect.size - dvec2(12.0, HEAD + FOOT) }
}

impl CodeValues {
    /// Hit test: the key or handle under `abs`.
    fn space_hit(&self, st: &SpaceState, cam: &Cam, keys: &[[f64; 3]], abs: DVec2) -> Option<Grab> {
        let handle = st.extent * 0.28;
        let mut best: Option<(f64, Grab)> = None;
        let mut consider = |d: f64, g: Grab, reach: f64| {
            if d < reach && best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, g));
            }
        };
        for (k, p) in keys.iter().enumerate() {
            let Some((c, _)) = cam.project(*p) else { continue };
            consider((c - abs).length(), Grab::Key(k), 10.0);
            let handles = st.kind == SpaceKind::Point || (st.kind == SpaceKind::Path && k == st.selected);
            if handles {
                for (a, axis) in AXES.iter().enumerate() {
                    if let Some((e, _)) = cam.project(add(*p, mul(*axis, handle))) {
                        // Distance to the handle's segment (past the key's disc).
                        let ab = e - c;
                        let ac = abs - c;
                        let t = ((ac.x * ab.x + ac.y * ab.y) / (ab.x * ab.x + ab.y * ab.y).max(1e-9)).clamp(0.0, 1.0);
                        let d = (c + ab * t - abs).length();
                        if t > 0.25 {
                            consider(d + 2.0, Grab::Axis(k, a), 7.0);
                        }
                    }
                }
            }
        }
        best.map(|(_, g)| g)
    }

    /// Draw the 3D popup: the view (re-rendered only when it changed), its
    /// labels, header and footer.
    pub(crate) fn draw_space_popup(&mut self, cx: &mut Cx2d, site: usize, rect: Rect, st: &SpaceState) {
        self.boxed(cx, rect, vec4(0.08, 0.09, 0.11, 1.0), vec4(1.0, 1.0, 1.0, 0.16), 6.0);
        let start = self.sites.get(site).map_or(0, |s| s.range.start);
        let keys: Vec<Vec<(std::ops::Range<usize>, f64)>> = keys_of(st.kind, &self.source, start);
        let points: Vec<[f64; 3]> = keys.iter().map(|k| value3(k)).collect();
        let view = view_rect(rect);
        let cam = Cam::new(&st.orbit, view);
        // What the picture shows: when it is what was drawn last, the last
        // picture stands.
        let lit_key = st.drag.as_ref().map(|d| (d.key, d.mode)).map(|(k, m)| format!("{k}{m:?}")).unwrap_or_default();
        let text = self.sites.get(site).map(|s| {
            let end = keys.last().and_then(|k| k.last()).map_or(s.range.end, |(r, _)| r.end);
            self.source[s.range.start..end.max(s.range.start)].to_string()
        }).unwrap_or_default();
        let stamp = format!("{text}|{:?}|{:?}|{lit_key}|{}|{:?}|{}", st.orbit, st.hover, st.selected, view.size, cx.current_dpi_factor());
        let redraw = self.space_drawn.as_deref() != Some(stamp.as_str());
        if !self.space_ready {
            self.space_ready = true;
            self.space_color = Texture::new_with_format(cx, TextureFormat::RenderBGRAu8 { size: TextureSize::Auto, initial: true }).label(cx, "gizmo/space-color");
            self.space_depth = Texture::new_with_format(cx, TextureFormat::DepthD32 { size: TextureSize::Auto, initial: true }).label(cx, "gizmo/space-depth");
        }
        if self.lines.is_none() {
            self.lines = LineRenderer::new(cx.cx);
        }
        let ready = self.lines.as_ref().is_some_and(|l| l.ready(cx, false));
        if redraw && self.lines.is_some() {
            // Until its shaders are built a picture is not whole: drawn
            // again on the next frame.
            self.space_drawn = ready.then_some(stamp);
            self.space_renders += 1;
            self.render_space(cx, st, &cam, &points);
        }
        if !ready {
            // The renderer is made, and its shaders built, on a later frame.
            cx.redraw_all();
        }
        self.draw_tex.draw_vars.set_texture(0, &self.space_color);
        self.draw_tex.draw_abs(cx, view);
        cx.set_pass_area(&self.space_pass, self.draw_tex.area());
        self.boxed(cx, view, vec4(0.0, 0.0, 0.0, 0.0), vec4(1.0, 1.0, 1.0, 0.10), 3.0);
        // Labels over the picture: the axes' ends, the keys' numbers.
        let axis_reach = if st.kind == SpaceKind::Direction { 1.32 } else { st.extent * 1.06 };
        for (a, name) in ["x", "y", "z"].iter().enumerate() {
            if let Some((p, _)) = cam.project(mul(AXES[a], axis_reach)) {
                if view.contains(p) {
                    self.draw_glyph.color = Vec4f::from_u32((AXIS_INK[a] << 8) | 0xff);
                    self.draw_glyph.draw_abs(cx, p + dvec2(3.0, -6.0), name);
                }
            }
        }
        if st.kind == SpaceKind::Path {
            let lit = st.drag.as_ref().map(|d| d.key).or(match st.hover {
                Some(Grab::Key(k) | Grab::Axis(k, _)) => Some(k),
                None => None,
            });
            for (k, p) in points.iter().enumerate() {
                if let Some((c, _)) = cam.project(*p) {
                    if view.contains(c) {
                        self.draw_glyph.color = if lit == Some(k) { vec4(1.0, 0.62, 0.32, 1.0) } else { vec4(0.62, 0.66, 0.72, 0.9) };
                        self.draw_glyph.draw_abs(cx, c + dvec2(7.0, 2.0), &k.to_string());
                    }
                }
            }
        }
        // The header: the value, its axes in their colours.
        let shown_key = match st.kind {
            SpaceKind::Path => st.drag.as_ref().map(|d| d.key).or(match st.hover {
                Some(Grab::Key(k) | Grab::Axis(k, _)) => Some(k),
                None => None,
            }).unwrap_or(st.selected),
            _ => 0,
        };
        let mut x = rect.pos.x + 10.0;
        let ty = rect.pos.y + 7.0;
        let title = match st.kind {
            SpaceKind::Point => "point".to_string(),
            SpaceKind::Direction => {
                let d = points.first().copied().map(normalize3).unwrap_or([0.0, 1.0, 0.0]);
                let (az, el) = az_el(d);
                format!("direction  az {az:.0}°  el {el:.0}°")
            }
            SpaceKind::Path => format!("key {shown_key} of {}", points.len()),
        };
        self.draw_label.color = vec4(0.86, 0.88, 0.9, 1.0);
        self.draw_label.draw_abs(cx, dvec2(x, ty), &title);
        x = rect.pos.x + rect.size.x - 10.0;
        if let Some(key) = keys.get(shown_key) {
            // Right-aligned: x …  y …  z …
            let parts: Vec<String> = key.iter().map(|(r, _)| self.source[r.clone()].to_string()).collect();
            let widths: Vec<f64> = parts.iter().map(|p| self.text_w(cx, p, true)).collect();
            let total: f64 = widths.iter().sum::<f64>() + 3.0 * 10.0 + 2.0 * 8.0;
            x -= total;
            for a in 0..3 {
                self.draw_glyph.color = Vec4f::from_u32((AXIS_INK[a] << 8) | 0xff);
                self.draw_glyph.draw_abs(cx, dvec2(x, ty + 1.0), ["x", "y", "z"][a]);
                self.draw_glyph.color = vec4(0.88, 0.9, 0.93, 1.0);
                self.draw_glyph.draw_abs(cx, dvec2(x + 8.0, ty + 1.0), &parts[a]);
                x += 10.0 + widths[a] + 8.0;
            }
        }
        let hint = match st.kind {
            SpaceKind::Direction => "drag on the sphere · right- or ctrl-drag orbit · shift 15° steps",
            SpaceKind::Point => "drag · alt or a handle: one axis · right- or ctrl-drag orbits · shift snaps",
            SpaceKind::Path => "drag a key · alt or a handle: one axis · drag around to orbit",
        };
        self.draw_glyph.color = vec4(0.5, 0.53, 0.58, 1.0);
        self.draw_glyph.draw_abs(cx, dvec2(rect.pos.x + 10.0, rect.pos.y + rect.size.y - FOOT + 3.0), hint);
    }

    /// The view's picture, into its pass.
    fn render_space(&mut self, cx: &mut Cx2d, st: &SpaceState, cam: &Cam, points: &[[f64; 3]]) {
        let size = cam.rect.size;
        self.space_pass.set_size(cx, size);
        let bg = vec4(0.047, 0.055, 0.067, 1.0);
        self.space_pass.set_color_texture(cx, &self.space_color, DrawPassClearColor::ClearWith(bg));
        self.space_pass.set_depth_texture(cx, &self.space_depth, DrawPassClearDepth::ClearWith(1.0));
        cx.make_child_pass(&self.space_pass);
        cx.begin_pass(&self.space_pass, None);
        self.space_pass.set_size(cx, size);
        let f3 = |v: [f64; 3]| [v[0] as f32, v[1] as f32, v[2] as f32];
        let mut faint = LineBatch::new(LineStyle::default());
        let mut lines = LineBatch::new(LineStyle::default());
        let mut dashed = LineBatch::new(LineStyle { dash: 4.0, gap: 3.0, ..LineStyle::default() });
        let mut dots = PointBatch::new(PointStyle { blend: Blend::Over, shape: SpriteShape::Disc, ..PointStyle::default() });
        let e = st.extent;
        let dragging = st.drag.as_ref().map(|d| (d.key, d.mode));
        match st.kind {
            SpaceKind::Point | SpaceKind::Path => {
                // The ground: a grid on y = 0, every fifth line stronger.
                let step = e / 5.0;
                for i in -5i32..=5 {
                    if i == 0 {
                        continue;
                    }
                    let v = i as f64 * step;
                    let c = pm(0xffffff, if i.abs() == 5 { 0.22 } else { 0.14 });
                    faint.push(Segment::new(f3([v, 0.0, -e]), f3([v, 0.0, e]), 1.0, c));
                    faint.push(Segment::new(f3([-e, 0.0, v]), f3([e, 0.0, v]), 1.0, c));
                }
                // The axes through the origin.
                lines.push(Segment::new(f3([-e, 0.0, 0.0]), f3([e, 0.0, 0.0]), 1.4, pm(AXIS_INK[0], 0.7)));
                lines.push(Segment::new(f3([0.0, 0.0, -e]), f3([0.0, 0.0, e]), 1.4, pm(AXIS_INK[2], 0.7)));
                lines.push(Segment::new(f3([0.0, 0.0, 0.0]), f3([0.0, e, 0.0]), 1.4, pm(AXIS_INK[1], 0.7)));
                if st.kind == SpaceKind::Path && points.len() > 1 {
                    // Its shadow on the ground, then the path.
                    let shadow: Vec<LinePoint> = points.iter().map(|p| LinePoint::new(f3([p[0], 0.0, p[2]]), 1.2, pm(TEAL, 0.28))).collect();
                    faint.push_polyline(&shadow, 0.0);
                    let path: Vec<LinePoint> = points.iter().map(|p| LinePoint::new(f3(*p), 2.2, pm(ORANGE, 0.95))).collect();
                    lines.push_polyline(&path, 0.0);
                }
                for (k, p) in points.iter().enumerate() {
                    // A drop to the ground, its foot.
                    if p[1].abs() > 1e-9 {
                        dashed.push_path(&[f3(*p), f3([p[0], 0.0, p[2]])], 1.0, pm(TEAL, 0.6), false);
                    }
                    dots.push(Sprite::new(f3([p[0], 0.0, p[2]]), 5.0, pm(TEAL, 0.55)));
                    let lit = dragging.is_some_and(|(dk, _)| dk == k) || matches!(st.hover, Some(Grab::Key(h) | Grab::Axis(h, _)) if h == k);
                    let is_point = st.kind == SpaceKind::Point;
                    let size = if lit { 13.0 } else if is_point { 11.0 } else { 8.5 };
                    dots.push(Sprite::new(f3(*p), size + 3.0, [0.0, 0.0, 0.0, 0.7]));
                    dots.push(Sprite::new(f3(*p), size, pm(if lit || is_point { ORANGE } else { PALE }, 1.0)));
                    // Handles: along each axis from the point (or the path's
                    // picked key).
                    if is_point || k == st.selected {
                        for a in 0..3 {
                            let on = matches!(st.hover, Some(Grab::Axis(h, ha)) if h == k && ha == a) || dragging == Some((k, DragMode::Axis(a)));
                            let end = add(*p, mul(AXES[a], e * 0.28));
                            lines.push(Segment::new(f3(add(*p, mul(AXES[a], e * 0.06))), f3(end), if on { 3.2 } else { 2.0 }, pm(AXIS_INK[a], if on { 1.0 } else { 0.85 })));
                            dots.push(Sprite::new(f3(end), if on { 8.0 } else { 6.0 }, pm(AXIS_INK[a], 1.0)));
                        }
                    }
                }
            }
            SpaceKind::Direction => {
                // The sphere: its great circles, rings of latitude.
                let ring = |batch: &mut LineBatch, f: &dyn Fn(f64) -> [f64; 3], w: f32, c: [f32; 4]| {
                    let pts: Vec<[f32; 3]> = (0..=64).map(|i| f3(f(i as f64 / 64.0 * std::f64::consts::TAU))).collect();
                    batch.push_path(&pts, w, c, false);
                };
                for lat in [-60.0f64, -30.0, 30.0, 60.0] {
                    let (y, r) = (lat.to_radians().sin(), lat.to_radians().cos());
                    ring(&mut faint, &|t| [r * t.cos(), y, r * t.sin()], 1.0, pm(0xffffff, 0.06));
                }
                ring(&mut faint, &|t| [t.cos(), t.sin(), 0.0], 1.0, pm(0xffffff, 0.16));
                ring(&mut faint, &|t| [0.0, t.sin(), t.cos()], 1.0, pm(0xffffff, 0.16));
                // Its outline as seen from here.
                let (eye, r, u) = (cam.eye, cam.right, cam.up);
                let d = len3(eye);
                let rim = (1.0 - 1.0 / (d * d)).max(0.0).sqrt();
                let centre = mul(eye, 1.0 / (d * d));
                ring(&mut lines, &|t| add(centre, add(mul(r, rim * t.cos()), mul(u, rim * t.sin()))), 1.2, pm(0xffffff, 0.3));
                ring(&mut lines, &|t| [t.cos(), 0.0, t.sin()], 1.2, pm(0xffffff, 0.26));
                for a in 0..3 {
                    lines.push(Segment::new([0.0; 3], f3(mul(AXES[a], 1.25)), 1.2, pm(AXIS_INK[a], 0.6)));
                }
                if let Some(p) = points.first() {
                    let d = normalize3(*p);
                    let flat = [d[0], 0.0, d[2]];
                    if len3(flat) > 1e-6 {
                        dashed.push_path(&[[0.0; 3], f3(flat)], 1.0, pm(TEAL, 0.7), false);
                        dashed.push_path(&[f3(d), f3(flat)], 1.0, pm(TEAL, 0.5), false);
                    }
                    let lit = dragging.is_some() || st.hover.is_some();
                    lines.push(Segment::new([0.0; 3], f3(d), if lit { 3.2 } else { 2.6 }, pm(ORANGE, 1.0)));
                    dots.push(Sprite::new([0.0; 3], 6.0, pm(0xffffff, 0.6)));
                    dots.push(Sprite::new(f3(d), if lit { 15.0 } else { 13.0 }, [0.0, 0.0, 0.0, 0.7]));
                    dots.push(Sprite::new(f3(d), if lit { 12.0 } else { 10.0 }, pm(ORANGE, 1.0)));
                }
            }
        }
        if let Some(renderer) = &mut self.lines {
            let cx3d = &mut Cx3d::new(cx.cx);
            self.space_list.begin_always(cx3d);
            let dpi = cx3d.cx.current_dpi_factor() as f32;
            let (view, proj) = cam.matrices();
            let lv = LineView::world(&view, &proj, (size.x as f32 * dpi, size.y as f32 * dpi), dpi);
            renderer.draw_lines(cx3d.cx, &lv, &faint);
            renderer.draw_lines(cx3d.cx, &lv, &dashed);
            renderer.draw_lines(cx3d.cx, &lv, &lines);
            renderer.draw_points(cx3d.cx, &lv, &dots);
            self.space_list.end(cx3d);
        }
        cx.end_pass(&self.space_pass);
    }

    /// The 3D popup's events.
    pub(crate) fn space_event(&mut self, cx: &mut Cx, event: &Event, site: usize, rect: Rect, mut st: SpaceState) -> CodeValuesResponse {
        let mut r = CodeValuesResponse::default();
        let start = self.sites.get(site).map_or(0, |s| s.range.start);
        let keys = keys_of(st.kind, &self.source, start);
        let points: Vec<[f64; 3]> = keys.iter().map(|k| value3(k)).collect();
        let view = view_rect(rect);
        let cam = Cam::new(&st.orbit, view);
        let snap_step = if st.kind == SpaceKind::Direction { 0.0 } else { nice_step(st.extent * 2.0) };
        let set = |this: &mut Self, st: SpaceState| this.popup = Some(Popup::Space { site, rect, space: Box::new(st) });
        match event {
            Event::MouseDown(e) if !rect.contains(e.abs) => {
                self.close(&mut r);
                r.consumed = true;
                r.redraw = true;
            }
            Event::MouseDown(e) => {
                r.consumed = true;
                if !view.contains(e.abs) {
                    return r;
                }
                let grab = self.space_hit(&st, &cam, &points, e.abs);
                let orbit = e.button.is_secondary();
                let (mode, key) = match st.kind {
                    _ if orbit => (DragMode::Orbit, 0),
                    SpaceKind::Direction => (DragMode::Sphere, 0),
                    SpaceKind::Point => match grab {
                        Some(Grab::Axis(_, a)) => (DragMode::Axis(a), 0),
                        _ if e.modifiers.alt => (DragMode::AxisPending, 0),
                        _ => (DragMode::Plane, 0),
                    },
                    SpaceKind::Path => match grab {
                        Some(Grab::Axis(k, a)) => (DragMode::Axis(a), k),
                        Some(Grab::Key(k)) if e.modifiers.alt => (DragMode::AxisPending, k),
                        Some(Grab::Key(k)) => (DragMode::Plane, k),
                        None => (DragMode::Orbit, st.selected),
                    },
                };
                if mode != DragMode::Orbit && keys.get(key).is_none() {
                    return r;
                }
                if st.kind == SpaceKind::Path && mode != DragMode::Orbit {
                    st.selected = key;
                }
                let (start_v, written) = match keys.get(key) {
                    Some(k) => (value3(k), k.iter().map(|(rg, _)| self.source[rg.clone()].to_string()).collect()),
                    None => ([0.0; 3], Vec::new()),
                };
                let s0 = match mode {
                    DragMode::Axis(a) => cam.on_axis(e.abs, start_v, AXES[a]).unwrap_or(0.0),
                    _ => 0.0,
                };
                if mode != DragMode::Orbit {
                    let k = &keys[key];
                    let span = k[0].0.start..k[2].0.end;
                    let id = self.next_gesture();
                    let text = self.source[span.clone()].to_string();
                    r.focus = Some(span.start);
                    self.gesture = Some(Gesture { id, span, last: text.clone(), original: text, scrub: None });
                }
                st.drag = Some(SpaceDrag { mode, key, from: e.abs, start: start_v, written, hit0: cam.on_plane(e.abs, start_v), s0, orbit0: st.orbit,
                    orbit_on_drag: e.modifiers.control && mode != DragMode::Orbit });
                set(self, st);
                r.redraw = true;
            }
            Event::MouseMove(e) => {
                r.consumed = true;
                if let Some(mut d) = st.drag.clone() {
                    if d.orbit_on_drag {
                        if (e.abs - d.from).length() < 4.0 {
                            return r;
                        }
                        d.mode = DragMode::Orbit;
                        d.orbit_on_drag = false;
                        st.drag = Some(d.clone());
                    }
                    if d.mode == DragMode::AxisPending {
                        if (e.abs - d.from).length() < 4.0 {
                            return r;
                        }
                        // The axis whose screen direction the drag follows.
                        let m = (e.abs - d.from).normalize();
                        let base = cam.project(d.start).map(|p| p.0);
                        let mut best = (0usize, -1.0f64);
                        for a in 0..3 {
                            if let (Some(p0), Some((p1, _))) = (base, cam.project(add(d.start, mul(AXES[a], st.extent * 0.25)))) {
                                let s = p1 - p0;
                                let score = if s.length() > 1e-6 { ((s.x * m.x + s.y * m.y) / s.length()).abs() * s.length().min(40.0) } else { 0.0 };
                                if score > best.1 {
                                    best = (a, score);
                                }
                            }
                        }
                        d.mode = DragMode::Axis(best.0);
                        d.s0 = cam.on_axis(d.from, d.start, AXES[best.0]).unwrap_or(0.0);
                        st.drag = Some(d.clone());
                    }
                    if d.mode == DragMode::Orbit {
                        let delta = e.abs - d.from;
                        st.orbit.yaw = d.orbit0.yaw - delta.x * 0.01;
                        st.orbit.pitch = (d.orbit0.pitch + delta.y * 0.01).clamp(-1.45, 1.45);
                    } else {
                        let cam0 = Cam::new(&st.orbit, view);
                        self.space_move(&mut r, &cam0, &d, e.abs, e.modifiers.shift, false, snap_step);
                    }
                    set(self, st);
                    r.redraw = true;
                } else {
                    let over = if view.contains(e.abs) { self.space_hit(&st, &cam, &points, e.abs) } else { None };
                    let over = if st.kind == SpaceKind::Direction { over.map(|_| Grab::Key(0)) } else { over };
                    if over != st.hover {
                        st.hover = over;
                        set(self, st);
                        r.redraw = true;
                    }
                    if over.is_some() {
                        cx.set_cursor(MouseCursor::Hand);
                    }
                }
            }
            Event::MouseUp(e) => {
                r.consumed = true;
                if let Some(d) = st.drag.take() {
                    if !matches!(d.mode, DragMode::Orbit | DragMode::AxisPending) {
                        self.space_move(&mut r, &cam, &d, e.abs, e.modifiers.shift, true, snap_step);
                    }
                    self.gesture = None;
                    set(self, st);
                    r.redraw = true;
                }
            }
            Event::Scroll(e) if view.contains(e.abs) => {
                let k = if e.scroll.y > 0.0 { 1.12 } else if e.scroll.y < 0.0 { 1.0 / 1.12 } else { 1.0 };
                st.orbit.dist = (st.orbit.dist * k).clamp(st.extent * 0.3, st.extent * 60.0 + 10.0);
                set(self, st);
                r.consumed = true;
                r.redraw = true;
            }
            Event::KeyDown(k) if k.key_code == KeyCode::Escape || k.key_code == KeyCode::ReturnKey => {
                if st.drag.is_some() && k.key_code == KeyCode::Escape {
                    // Back to where the drag began, and closed.
                    self.revert(&mut r);
                } else {
                    self.close(&mut r);
                }
                r.consumed = true;
                r.redraw = true;
            }
            Event::Scroll(_) | Event::KeyDown(_) | Event::KeyUp(_) | Event::TextInput(_) => r.consumed = true,
            _ => {}
        }
        if r.redraw {
            self.pop_list.redraw(cx);
        }
        r
    }

    /// A drag step: the key's new value, written (only its changed numbers;
    /// what is between them kept).
    fn space_move(&mut self, r: &mut CodeValuesResponse, cam: &Cam, d: &SpaceDrag, abs: DVec2, shift: bool, done: bool, step: f64) {
        if d.written.len() != 3 {
            return;
        }
        let span = cam.orbit.dist * cam.tan_y * 2.0;
        let mut v = match d.mode {
            DragMode::Plane => add(d.start, sub(cam.on_plane(abs, d.start), d.hit0)),
            DragMode::Axis(a) => match cam.on_axis(abs, d.start, AXES[a]) {
                Some(s) => add(d.start, mul(AXES[a], s - d.s0)),
                None => d.start,
            },
            DragMode::Sphere => {
                let mut u = cam.on_sphere(abs);
                if shift {
                    // Whole 15° steps of azimuth and elevation.
                    let (az, el) = az_el(u);
                    let q = |deg: f64| (deg / 15.0).round() * 15.0;
                    u = from_az_el(q(az), q(el));
                }
                redirect(u, d.start)
            }
            DragMode::AxisPending | DragMode::Orbit => return,
        };
        let mut texts: Vec<Option<String>> = Vec::with_capacity(3);
        for a in 0..3 {
            let moved = (v[a] - d.start[a]).abs() > 1e-12;
            if !moved {
                texts.push(None);
                continue;
            }
            let text = if d.mode == DragMode::Sphere {
                fmt_like(v[a], &d.written[a], 0.1)
            } else if shift && step > 0.0 {
                v[a] = (v[a] / step).round() * step;
                fmt_snapped(v[a], step)
            } else {
                fmt_like(v[a], &d.written[a], span)
            };
            texts.push((text != d.written[a]).then_some(text));
        }
        // The whole key is rewritten from its numbers as written when the
        // drag began: unchanged ones byte for byte.
        let Some(g) = self.gesture.clone() else { return };
        let src = &self.source;
        let now = vector_like(src, g.span.start);
        if now.len() != 3 {
            return;
        }
        let mut out = String::new();
        for a in 0..3 {
            if a > 0 {
                out.push_str(&src[now[a - 1].0.end..now[a].0.start]);
            }
            out.push_str(texts[a].as_deref().unwrap_or(&d.written[a]));
        }
        if done && out == g.original && g.last == g.original {
            // A press that moved nothing writes nothing.
            self.gesture = None;
            return;
        }
        self.write(r, out, done);
    }
}

/// The three numbers from byte `start` (a key's first number) on.
pub(crate) fn vector_like(source: &str, start: usize) -> Vec<(std::ops::Range<usize>, f64)> {
    let mut out = Vec::new();
    let b = source.as_bytes();
    let mut i = start;
    while out.len() < 3 && i < b.len() {
        let c = b[i];
        if c.is_ascii_digit() || c == b'.' || (c == b'-' && b.get(i + 1).is_some_and(|n| n.is_ascii_digit() || *n == b'.')) {
            let from = i;
            i += 1;
            while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.' || b[i] == b'e' || (b[i] == b'-' && b[i - 1] == b'e')) {
                i += 1;
            }
            if let Ok(v) = source[from..i].parse::<f64>() {
                out.push((from..i, v));
            }
        } else if c == b')' || c == b']' {
            break;
        } else {
            i += 1;
        }
    }
    out
}

/// A direction's azimuth (from +z toward +x) and elevation, in degrees.
pub(crate) fn az_el(d: [f64; 3]) -> (f64, f64) {
    let d = normalize3(d);
    (d[0].atan2(d[2]).to_degrees(), d[1].clamp(-1.0, 1.0).asin().to_degrees())
}

pub(crate) fn from_az_el(az: f64, el: f64) -> [f64; 3] {
    let (a, e) = (az.to_radians(), el.to_radians());
    [e.cos() * a.sin(), e.sin(), e.cos() * a.cos()]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cam() -> Cam {
        Cam::new(&Orbit::framing([0.5, 0.2, -0.3], 2.0), Rect { pos: dvec2(100.0, 50.0), size: dvec2(320.0, 240.0) })
    }

    #[test]
    fn a_ray_lands_where_its_point_is_drawn() {
        let c = cam();
        for p in [[0.0, 0.0, 0.0], [1.0, 0.5, -1.0], [-0.7, 1.2, 0.4]] {
            let (s, _) = c.project(p).unwrap();
            let (o, d) = c.ray(s);
            // The point lies on the ray through its own projection.
            let t = dot(sub(p, o), d);
            let q = add(o, mul(d, t));
            assert!(len3(sub(p, q)) < 1e-9, "{p:?} off its ray by {}", len3(sub(p, q)));
        }
        // The matrices agree with the projection.
        let (view, proj) = c.matrices();
        let vp = Mat4f::mul(&proj, &view);
        let p = [1.0, 0.5, -1.0];
        let clip = vp.transform_vec4(vec4(p[0] as f32, p[1] as f32, p[2] as f32, 1.0));
        let (s, _) = c.project(p).unwrap();
        let sx = c.rect.pos.x + ((clip.x / clip.w) as f64 + 1.0) * 0.5 * c.rect.size.x;
        let sy = c.rect.pos.y + (1.0 - (clip.y / clip.w) as f64) * 0.5 * c.rect.size.y;
        assert!((sx - s.x).abs() < 0.05 && (sy - s.y).abs() < 0.05, "{sx},{sy} vs {s:?}");
    }

    #[test]
    fn drags_follow_the_pointer() {
        let c = cam();
        let p = [0.3, 0.4, 0.1];
        let (s, _) = c.project(p).unwrap();
        // The plane through the point gives the point back under itself.
        assert!(len3(sub(c.on_plane(s, p), p)) < 1e-9);
        // Along x: the pointer over x + 0.5 reads s = 0.5.
        let (s2, _) = c.project(add(p, [0.5, 0.0, 0.0])).unwrap();
        assert!((c.on_axis(s2, p, AXES[0]).unwrap() - 0.5).abs() < 1e-6);
        // The sphere: its point under itself.
        let d = normalize3([0.4, 0.7, 0.6]);
        let c0 = Cam::new(&Orbit::framing([0.0; 3], 1.35), c.rect);
        let (sd, _) = c0.project(d).unwrap();
        assert!(len3(sub(c0.on_sphere(sd), d)) < 1e-6);
    }

    #[test]
    fn azimuth_and_elevation_round_trip() {
        let (az, el) = az_el([0.0, 0.0, 1.0]);
        assert!(az.abs() < 1e-9 && el.abs() < 1e-9);
        let d = from_az_el(64.0, 20.0);
        let (az, el) = az_el(d);
        assert!((az - 64.0).abs() < 1e-9 && (el - 20.0).abs() < 1e-9);
    }

    #[test]
    fn keys_are_read_from_the_source() {
        let src = "path: [vec3(0, 1, 2), vec3( 3.5, -4, 5 )]";
        let keys = keys_of(SpaceKind::Path, src, src.find('[').unwrap());
        assert_eq!(keys.len(), 2);
        assert_eq!(&src[keys[1][1].0.clone()], "-4");
        let k1 = keys[1][0].0.start;
        assert_eq!(vector_like(src, k1).iter().map(|(r, _)| &src[r.clone()]).collect::<Vec<_>>(), vec!["3.5", "-4", "5"]);
        let st = SpaceState::open(SpaceKind::Path, src, src.find('[').unwrap());
        assert_eq!(st.extent, 5.0);
    }
}
