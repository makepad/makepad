//! Small, lit 3D interaction shapes. One instanced quad shader evaluates signed distances in
//! an orthonormal room-space frame and writes the surface's projected depth for each eye.
//! The same retained list is submitted with LessEqual, then Greater without depth writes.
//! Submit after opaque meshes AND any field shading that writes scene depth.
//!
//! CPU work is linear in the submitted shapes, with capacity retained between frames. GPU work
//! is confined to each shape's projected bound; sphere tracing stops at a pixel-sized footprint
//! or the bound's exit (no iteration cap that can silently lose a thin/grazing surface).

use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.math.*
    use mod.shader.*

    let XrShapeKind = set_type_default() do #(XrShapeKind::script_api(vm))
    mod.draw.DrawXrShape = mod.std.set_type_default() do #(DrawXrShape::script_shader(vm)){
        ..mod.draw.DrawQuad
        alpha_blend: true
        backface_culling: false
        depth_write: true
        depth_out: fragment_depth(@any)
        shape: instance(XrShapeKind.Sphere)
        eye: varying(vec3f)
        ray_point: varying(vec3f)

        vertex: fn() {
            let camera = inverse(self.draw_pass.camera_view)
            self.eye = (camera * vec4(0.0, 0.0, 0.0, 1.0)).xyz
            let c = self.draw_pass.camera_view * vec4(self.center, 1.0)
            let uv = self.geom.pos * 2.0 - vec2(1.0, 1.0)
            // Project a conservative bounding sphere, including a margin for silhouette coverage.
            let r = self.bound * 1.2
            if c.z + r >= -0.001 {
                self.vertex_pos = vec4(uv, 0.0, 1.0)
                let w = camera * (inverse(self.draw_pass.camera_projection) * self.vertex_pos)
                self.ray_point = w.xyz / w.w
            } else {
                let extent = r * (vec2(0.0 - c.z) + abs(c.xy)) / max(0.0 - c.z - r, 0.00001)
                let v = vec4(c.xy + uv * extent, c.z, 1.0)
                self.ray_point = (camera * v).xyz
                self.vertex_pos = self.draw_pass.camera_projection * v
            }
        }

        local: fn(v: vec3) -> vec3 {
            return vec3(dot(v, self.axis_x), dot(v, self.axis_y), dot(v, self.axis_z))
        }
        field: fn(p: vec3) -> float {
            match self.shape {
                XrShapeKind.Sphere => { return length(p) - self.params.x }
                XrShapeKind.Capsule => {
                    return length(vec3(p.xy, p.z - clamp(p.z, 0.0 - self.params.y, self.params.y))) - self.params.x
                }
                XrShapeKind.Ring => {
                    return length(vec2(length(p.xy) - self.params.x, p.z)) - self.params.y
                }
                XrShapeKind.Arc => {
                    let a = atan2(p.y, p.x)
                    let angle = a + step(a, 0.0) * 6.28318530718
                    // Within the sweep the nearest point lies on the circle; outside it
                    // lies at an endpoint. Round caps are part of the same distance field.
                    if angle <= self.params.z {
                        return length(vec2(length(p.xy) - self.params.x, p.z)) - self.params.y
                    }
                    let start = vec3(self.params.x, 0.0, 0.0)
                    let end = vec3(cos(self.params.z), sin(self.params.z), 0.0) * self.params.x
                    return min(length(p - start), length(p - end)) - self.params.y
                }
                XrShapeKind.Cone => {
                    let q = vec2(length(p.xy), p.z)
                    let k1 = vec2(0.0, self.params.y)
                    let k2 = vec2(0.0 - self.params.x, 2.0 * self.params.y)
                    var radius = 0.0
                    if q.y < 0.0 { radius = self.params.x }
                    let ca = vec2(q.x - min(q.x, radius), abs(q.y) - self.params.y)
                    let cb = q - k1 + k2 * clamp(dot(k1 - q, k2) / dot(k2, k2), 0.0, 1.0)
                    var sign = 1.0
                    if cb.x < 0.0 && ca.y < 0.0 { sign = -1.0 }
                    return sign * sqrt(min(dot(ca, ca), dot(cb, cb)))
                }
                XrShapeKind.Disc => {
                    let q = vec2(length(p.xy) - self.params.x + self.params.z, abs(p.z) - self.params.y + self.params.z)
                    return length(max(q, vec2(0.0))) + min(max(q.x, q.y), 0.0) - self.params.z
                }
                XrShapeKind.Badge => {
                    let q = abs(p) - self.params.xyz + vec3(self.params.w)
                    return length(max(q, vec3(0.0))) + min(max(q.x, max(q.y, q.z)), 0.0) - self.params.w
                }
                _ => { return length(p) - self.params.x }
            }
        }

        fragment: fn() {
            let ro = self.local(self.eye - self.center)
            let rd_world = normalize(self.ray_point - self.eye)
            let rd = self.local(rd_world)
            let angular_px = max(length(dFdx(rd)), length(dFdy(rd)))
            let b = dot(ro, rd)
            let r = self.bound * 1.2
            let side = ro - rd * b
            let side2 = dot(side, side)
            let disc = r * r - side2
            if disc <= 0.0 { discard() }
            var t = max(0.0, 0.0 - b - sqrt(disc))
            let end = 0.0 - b + sqrt(disc)
            if end <= t { discard() }
            var at = t
            var coverage = 0.0
            match self.shape {
                XrShapeKind.Sphere => {
                    // Exact ray/sphere intersection, with analytic coverage at the tangent.
                    let edge = sqrt(side2) - self.params.x
                    let px = max(angular_px * max(0.0 - b, t), 0.000001)
                    coverage = clamp(0.5 - edge / px, 0.0, 1.0)
                    let root = sqrt(max(self.params.x * self.params.x - side2, 0.0))
                    at = 0.0 - b - root
                    if at < 0.0 { at = 0.0 - b + root }
                }
                _ => {
                    var closest = 1000000.0
                    // Distance/footprint gives continuous silhouette coverage, including a ray
                    // that narrowly misses the surface. Conservative steps cannot jump a tube.
                    while t <= end {
                        let px = max(angular_px * t, 0.000001)
                        let d = self.field(ro + rd * t)
                        let ratio = d / px
                        if ratio < closest { closest = ratio; at = t }
                        if ratio <= 0.01 { break }
                        if closest < 1.0 && ratio > closest + 1.0 { break }
                        t = t + max(d * 0.9, px * 0.02)
                    }
                    coverage = clamp(1.0 - closest, 0.0, 1.0)
                }
            }
            if coverage <= 0.0 { discard() }
            let p = ro + rd * at
            let px = max(angular_px * at, 0.000001)
            let e = px * 0.25
            let n = normalize(vec3(
                self.field(p + vec3(e, 0.0, 0.0)) - self.field(p - vec3(e, 0.0, 0.0)),
                self.field(p + vec3(0.0, e, 0.0)) - self.field(p - vec3(0.0, e, 0.0)),
                self.field(p + vec3(0.0, 0.0, e)) - self.field(p - vec3(0.0, 0.0, e))
            ))
            let world_normal = normalize(self.axis_x * n.x + self.axis_y * n.y + self.axis_z * n.z)
            let hit = vec4(self.center + self.axis_x * p.x + self.axis_y * p.y + self.axis_z * p.z, 1.0)
            let clip = self.draw_pass.camera_projection * (self.draw_pass.camera_view * hit)
            if clip.w <= 0.0 { discard() }
            self.depth_out = clip.z / clip.w
            let facing = abs(dot(n, rd))
            // Convert the grazing-normal band to pixels; a dark one-pixel outline at rest,
            // a bright pulsing outline during capture. Lighting matches DrawCube.
            let edge_px = facing * facing * self.edge_radius / (2.0 * px)
            let rim = 1.0 - smoothstep(self.outline_px, self.outline_px + 1.0, edge_px)
            let lit = self.color.xyz * (0.28 + 0.72 * max(dot(world_normal, normalize(self.light_dir)), 0.0))
            let rgb = mix(lit, self.outline.xyz, rim * self.outline.w)
            let alpha = self.color.w * coverage * self.opacity
            self.fb0 = depth_clip(hit, vec4(rgb * alpha, alpha), self.depth_clip)
        }
    }
    mod.widgets.XrShapes = set_type_default() do #(XrShapes::script_component(vm)){
        draw_shape: mod.draw.DrawXrShape{}
        draw_hidden: mod.draw.DrawXrShape{depth_write: false depth_test_greater: true}
        light_dir: vec3(0.35, 0.8, 0.45)
        hidden_alpha: 0.25
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Script, ScriptHook)]
#[repr(u32)]
pub enum XrShapeKind {
    #[pick]
    Sphere = 0,
    Capsule = 1,
    Ring = 2,
    Arc = 3,
    Cone = 4,
    Badge = 5,
    Disc = 6,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawXrShape {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    shape: XrShapeKind,
    #[live]
    center: Vec3f,
    #[live]
    axis_x: Vec3f,
    #[live]
    axis_y: Vec3f,
    #[live]
    axis_z: Vec3f,
    #[live]
    params: Vec4f,
    #[live]
    color: Vec4f,
    #[live]
    outline: Vec4f,
    #[live]
    outline_px: f32,
    #[live]
    edge_radius: f32,
    #[live]
    bound: f32,
    #[live]
    light_dir: Vec3f,
    #[live(1.0)]
    opacity: f32,
}

/// Straight colour and silhouette ink. The caller supplies its own UI's palette/state.
#[derive(Clone, Copy, Debug)]
pub struct XrShapeStyle {
    pub color: Vec4f,
    pub outline: Vec4f,
    pub outline_px: f32,
}

#[derive(Clone, Copy)]
struct Instance {
    kind: XrShapeKind,
    center: Vec3f,
    axes: [Vec3f; 3],
    params: Vec4f,
    bound: f32,
    edge_radius: f32,
    style: XrShapeStyle,
}

#[derive(Script, ScriptHook)]
pub struct XrShapes {
    #[live]
    draw_shape: DrawXrShape,
    #[live]
    draw_hidden: DrawXrShape,
    #[live]
    pub light_dir: Vec3f,
    #[live(0.25)]
    pub hidden_alpha: f32,
    #[rust]
    instances: Vec<Instance>,
}

const AXES: [Vec3f; 3] = [vec3f(1.0, 0.0, 0.0), vec3f(0.0, 1.0, 0.0), vec3f(0.0, 0.0, 1.0)];

fn frame(axis: Vec3f) -> [Vec3f; 3] {
    let z = if axis.length() > 1e-8 { axis.normalize() } else { AXES[2] };
    let x = Vec3f::cross(if z.y.abs() < 0.9 { AXES[1] } else { AXES[0] }, z).normalize();
    [x, Vec3f::cross(z, x), z]
}

impl XrShapes {
    /// Start a new list without discarding its allocation.
    pub fn clear(&mut self) { self.instances.clear(); }

    pub fn sphere(&mut self, at: Vec3f, radius: f32, style: XrShapeStyle) {
        if radius <= 0.0 { return; }
        self.instances.push(Instance { kind: XrShapeKind::Sphere, center: at, axes: AXES, params: vec4(radius, 0.0, 0.0, 0.0), bound: radius, edge_radius: radius, style });
    }

    pub fn segment(&mut self, a: Vec3f, b: Vec3f, radius: f32, style: XrShapeStyle) {
        let half = (b - a).length() * 0.5;
        if half < 1e-8 { self.sphere(a, radius, style); return; }
        self.instances.push(Instance { kind: XrShapeKind::Capsule, center: (a + b) * 0.5, axes: frame(b - a), params: vec4(radius, half, 0.0, 0.0), bound: half + radius, edge_radius: radius, style });
    }

    /// Cone with its apex at `tip` and a circular base centred at `base`.
    pub fn cone(&mut self, base: Vec3f, tip: Vec3f, radius: f32, style: XrShapeStyle) {
        let half = (tip - base).length() * 0.5;
        if half < 1e-8 || radius <= 0.0 { return; }
        self.instances.push(Instance { kind: XrShapeKind::Cone, center: (base + tip) * 0.5, axes: frame(tip - base), params: vec4(radius, half, 0.0, 0.0), bound: half.hypot(radius), edge_radius: radius, style });
    }

    pub fn ring(&mut self, at: Vec3f, axis: Vec3f, radius: f32, tube: f32, style: XrShapeStyle) {
        self.instances.push(Instance { kind: XrShapeKind::Ring, center: at, axes: frame(axis), params: vec4(radius, tube, 0.0, 0.0), bound: radius + tube, edge_radius: tube, style });
    }

    /// Round-capped arc, measured from `start` around `axis`, in radians.
    pub fn arc(&mut self, at: Vec3f, axis: Vec3f, start: Vec3f, radius: f32, sweep: f32, tube: f32, style: XrShapeStyle) {
        if sweep.abs() >= std::f32::consts::TAU { self.ring(at, axis, radius, tube, style); return; }
        let z = frame(axis)[2] * if sweep < 0.0 { -1.0 } else { 1.0 };
        let radial = start - z * start.dot(z);
        let x = if radial.length() > 1e-8 { radial.normalize() } else { frame(z)[0] };
        self.instances.push(Instance { kind: XrShapeKind::Arc, center: at, axes: [x, Vec3f::cross(z, x), z], params: vec4(radius, tube, sweep.abs(), 0.0), bound: radius + tube, edge_radius: tube, style });
    }

    /// A flat circular badge with rounded edges, facing `normal`.
    pub fn disc(&mut self, at: Vec3f, normal: Vec3f, radius: f32, half_depth: f32, style: XrShapeStyle) {
        if radius <= 0.0 || half_depth <= 0.0 { return; }
        let round = half_depth.min(radius) * 0.5;
        self.instances.push(Instance { kind: XrShapeKind::Disc, center: at, axes: frame(normal), params: vec4(radius, half_depth, round, 0.0), bound: radius.hypot(half_depth), edge_radius: radius, style });
    }

    /// A thin rounded square badge, with a caller-defined facing frame.
    pub fn badge(&mut self, at: Vec3f, right: Vec3f, up: Vec3f, half: Vec3f, rounding: f32, style: XrShapeStyle) {
        let x = right.normalize();
        let z = Vec3f::cross(x, up).normalize();
        self.instances.push(Instance { kind: XrShapeKind::Badge, center: at, axes: [x, Vec3f::cross(z, x), z], params: vec4(half.x, half.y, half.z, rounding.min(half.x.min(half.y).min(half.z))), bound: half.length(), edge_radius: half.x.min(half.y), style });
    }

    /// Two batched draws of one list. The hidden pass never changes the scene's depth.
    pub fn draw(&mut self, cx: &mut Cx3d) {
        if self.instances.is_empty() { return; }
        for (draw, opacity) in [(&mut self.draw_shape, 1.0), (&mut self.draw_hidden, self.hidden_alpha)] {
            draw.light_dir = self.light_dir;
            draw.opacity = opacity;
            cx.new_draw_call(&mut draw.draw_vars);
            let Some(mut many) = cx.begin_many_instances(&mut draw.draw_vars) else { continue };
            for i in &self.instances {
                draw.shape = i.kind;
                draw.center = i.center;
                [draw.axis_x, draw.axis_y, draw.axis_z] = i.axes;
                draw.params = i.params;
                draw.bound = i.bound;
                draw.edge_radius = i.edge_radius;
                draw.color = i.style.color;
                draw.outline = i.style.outline;
                draw.outline_px = i.style.outline_px;
                many.instances.extend_from_slice(draw.draw_vars.as_slice());
            }
            let area = cx.end_many_instances(many);
            draw.draw_vars.area = cx.update_area_refs(draw.draw_vars.area, area);
        }
    }
}
