//! Screen-space reflections: half-resolution trace/resolve, full-resolution
//! environment replacement. Applied HDR colour and half-resolution history
//! ping-pong together: the trace never reads the target this frame writes.
//! After lazy creation, recording is fixed-size stack work and handle clones;
//! no scene traversal, readback or per-frame payload allocation.
//!
//! Why only the lit lanes write the surface and environment-specular targets,
//! and the pass relies on per-draw output masks: the alternative, every draw
//! in a scene pass writing every attachment, would make each draw type that
//! can appear there (sky, particles, decals, water, labels, a host's own
//! draws, and every shader written later) declare outputs it has nothing to
//! do with, and one that forgot would fail only where masks are missing: the
//! least tested devices. With masks the knowledge stays in the lanes that
//! reflect. The price is one capability (`GpuInfo::mixed_output_passes`): on
//! a GPU without it reflections stay off and say why.

use makepad_draw::*;
pub use makepad_scene::SsrSettings;
use crate::volume_fog::VolumeFogCamera as SsrCamera;

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw

    mod.draw.SsrCamera = {
        ..mod.draw.VolumeFogCamera,
        u_clip: uniform(mat4x4f(1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0))
        u_previous_clip: uniform(mat4x4f(1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0))
        u_previous_eye: uniform(vec4(0.0, 0.0, 0.0, 1.0))
        u_previous_axis: uniform(vec4(0.0, 0.0, 1.0, 0.0))
        u_previous_ref: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        // strength, max ray distance in metres, frame, history valid
        u_ssr: uniform(vec4(0.0, 30.0, 0.0, 0.0))
        ssr_uv: fn(h: vec4) -> vec2 {
            return vec2(h.x / h.w * 0.5 + 0.5, 0.5 - h.y / h.w * 0.5)
        }
        ssr_inside: fn(uv: vec2) -> bool {
            return uv.x >= 0.0 && uv.x <= 1.0 && uv.y >= 0.0 && uv.y <= 1.0
        }
        ssr_normal: fn(encoded: vec2) -> vec3 {
            let e = encoded * 2.0 - vec2(1.0, 1.0)
            var n = vec3(e.x, e.y, 1.0 - abs(e.x) - abs(e.y))
            if n.z < 0.0 {
                let signs = vec2(mix(-1.0, 1.0, step(0.0, n.x)), mix(-1.0, 1.0, step(0.0, n.y)))
                n = vec3((vec2(1.0, 1.0) - abs(n.yx)) * signs, n.z)
            }
            return normalize(n)
        }
        ssr_distance: fn(p: vec3) -> float {
            return self.fog_scene_depth(p, self.u_eye, self.u_axis, self.u_ref)
        }
        vertex: fn() {
            self.pos = self.geom.pos
            self.world = vec4(self.geom.pos.x, self.geom.pos.y, 0.0, 1.0)
            self.vertex_pos = vec4(self.geom.pos.x * 2.0 - 1.0, 1.0 - self.geom.pos.y * 2.0, 0.0, 1.0)
        }
    }

    mod.draw.DrawSsrTrace = mod.std.set_type_default() do #(DrawSsrTrace::script_shader(vm)){
        ..mod.draw.DrawQuad,
        ..mod.draw.SsrCamera,
        color_format: @Rgba16F
        depth_clip: 0.0
        alpha_blend: false
        depth_tex: texture_2d(float)
        surface_tex: texture_2d(float)
        previous_scene: texture_2d(float)
        previous_depth: texture_2d(float)
        frag: fragment_position()
        // Perspective-correct world point on a uniformly stepped screen ray.
        ssr_point: fn(a: vec4, b: vec4, t: float) -> vec3 {
            let h = mix(a, b, t)
            return h.xyz / max(h.w, 0.0000001)
        }
        ssr_hit_position: fn(uv: vec2) -> vec4 {
            if !self.ssr_inside(uv) { return vec4(0.0, 0.0, 0.0, 0.0) }
            let raw = self.depth_tex.sample_nearest(uv).x
            if self.background(raw) > 0.5 { return vec4(0.0, 0.0, 0.0, 0.0) }
            let p = self.unproject(uv, raw)
            return vec4(p, self.ssr_distance(p))
        }
        // Non-PBR hit surfaces (including emissive lamps) have no SURFACE
        // normal. SSAO's four depth differences choose the nearer valid
        // depth on each axis; a one-pixel sliver faces the camera.
        ssr_hit_normal: fn(uv: vec2, p: vec3) -> vec3 {
            let texel = vec2(1.0, 1.0) / self.depth_tex.size()
            let l = self.ssr_hit_position(uv - vec2(texel.x, 0.0))
            let r = self.ssr_hit_position(uv + vec2(texel.x, 0.0))
            let t = self.ssr_hit_position(uv - vec2(0.0, texel.y))
            let b = self.ssr_hit_position(uv + vec2(0.0, texel.y))
            let depth = self.ssr_distance(p)
            var dx = r.xyz - p
            var dy = b.xyz - p
            if r.w <= 0.0 || (l.w > 0.0 && abs(l.w - depth) < abs(r.w - depth)) { dx = p - l.xyz }
            if b.w <= 0.0 || (t.w > 0.0 && abs(t.w - depth) < abs(b.w - depth)) { dy = p - t.xyz }
            var n = cross(dy, dx)
            let incident = self.ray_dir(uv)
            if length(n) <= 0.000001 * length(dx) * length(dy) { n = incident * -1.0 }
            n = normalize(n)
            if dot(n, incident) > 0.0 { n = n * -1.0 }
            return n
        }
        pixel: fn() -> vec4 {
            let none = vec4(0.0, 0.0, 0.0, 0.0)
            if self.u_ssr.w < 0.5 { return none }
            let surface = self.surface_tex.sample_nearest(self.pos)
            if surface.w <= 0.5 || surface.z <= 0.35 { return none }
            let raw = self.depth_tex.sample_nearest(self.pos).x
            if self.background(raw) > 0.5 { return none }
            let p = self.unproject(self.pos, raw)
            let n = self.ssr_normal(surface.xy)
            let incident = self.ray_dir(self.pos)
            if dot(n, incident) >= 0.0 { return none }
            let ray = normalize(incident - n * (2.0 * dot(n, incident)))
            let start = p + n * 0.02
            let start_clip = self.u_clip * vec4(start, 1.0)
            var reach = self.u_ssr.y
            let end_clip = self.u_clip * vec4(start + ray * reach, 1.0)
            // Clip the ray to the near plane before projecting its endpoint.
            let near_z = self.u_depth_map.y
            let a_near = start_clip.z - near_z * start_clip.w
            let b_near = end_clip.z - near_z * end_clip.w
            if b_near < 0.0 { reach = reach * clamp(a_near / max(a_near - b_near, 0.000001) * 0.99, 0.0, 1.0) }
            let end = start + ray * reach
            let finish_clip = self.u_clip * vec4(end, 1.0)
            if start_clip.w <= 0.000001 || finish_clip.w <= 0.000001 { return none }
            let uv0 = self.ssr_uv(start_clip)
            let uv1 = self.ssr_uv(finish_clip)
            let a = vec4(start, 1.0) / start_clip.w
            let b = vec4(end, 1.0) / finish_clip.w
            let noise = fract(52.9829189 * fract(dot(self.frag.xy + vec2(self.u_ssr.z * 5.588238, self.u_ssr.z * 5.588238), vec2(0.06711056, 0.00583715))))
            var before = 0.0
            var before_gap = -0.02
            for i in 0..24 {
                let along = (float(i) + 1.0 - noise) / 24.0
                let uv = mix(uv0, uv1, along)
                if !self.ssr_inside(uv) { return none }
                let d = self.depth_tex.sample_nearest(uv).x
                let q = self.ssr_point(a, b, along)
                var gap = -1.0
                if self.background(d) < 0.5 { gap = self.ssr_distance(q) - self.ssr_distance(self.unproject(uv, d)) }
                if gap >= 0.0 && before_gap < 0.0 {
                    var lo = before
                    var hi = along
                    var hit_uv = uv
                    var hit_raw = d
                    var hit_gap = gap
                    for j in 0..4 {
                        let mid = (lo + hi) * 0.5
                        let mu = mix(uv0, uv1, mid)
                        let md = self.depth_tex.sample_nearest(mu).x
                        var mg = -1.0
                        if self.background(md) < 0.5 { mg = self.ssr_distance(self.ssr_point(a, b, mid)) - self.ssr_distance(self.unproject(mu, md)) }
                        if mg >= 0.0 { hi = mid; hit_uv = mu; hit_raw = md; hit_gap = mg }
                        else { lo = mid }
                    }
                    if hit_gap > 0.3 { return none }
                    let hit = self.unproject(hit_uv, hit_raw)
                    let hit_surface = self.surface_tex.sample_nearest(hit_uv)
                    var hit_n = vec3(0.0, 1.0, 0.0)
                    if hit_surface.w > 0.5 { hit_n = self.ssr_normal(hit_surface.xy) }
                    else { hit_n = self.ssr_hit_normal(hit_uv, hit) }
                    if dot(hit_n, ray) >= 0.0 { return none }
                    let old = self.u_previous_clip * vec4(hit, 1.0)
                    if old.w <= 0.000001 { return none }
                    let old_uv = self.ssr_uv(old)
                    if !self.ssr_inside(old_uv) { return none }
                    let expected = self.fog_scene_depth(hit, self.u_previous_eye, self.u_previous_axis, self.u_previous_ref)
                    let previous = self.previous_depth.sample_nearest(old_uv)
                    if previous.y > 0.5 || abs(previous.x - expected) > max(expected, 0.001) * 0.1 { return none }
                    let edge_uv = min(min(hit_uv, vec2(1.0, 1.0) - hit_uv), min(old_uv, vec2(1.0, 1.0) - old_uv))
                    let edge = clamp(min(edge_uv.x, edge_uv.y) / 0.08, 0.0, 1.0)
                    let length_fade = clamp(1.0 - length(hit - p) / self.u_ssr.y, 0.0, 1.0)
                    let rough_fade = clamp((surface.z - 0.35) / 0.65, 0.0, 1.0)
                    return vec4(self.previous_scene.sample(old_uv).xyz, edge * length_fade * rough_fade)
                }
                before = along
                before_gap = gap
            }
            return none
        }
        fragment: fn() { self.fb0 = self.pixel() }
    }

    mod.draw.DrawSsrResolve = mod.std.set_type_default() do #(DrawSsrResolve::script_shader(vm)){
        ..mod.draw.DrawQuad,
        ..mod.draw.SsrCamera,
        color_format: @Rgba16F
        depth_clip: 0.0
        alpha_blend: false
        fb1: fragment_output(1, vec4f)
        trace_tex: texture_2d(float)
        history_tex: texture_2d(float)
        history_depth: texture_2d(float)
        depth_tex: texture_2d(float)
        surface_tex: texture_2d(float)
        fragment: fn() {
            let raw = self.depth_tex.sample_nearest(self.pos).x
            let background = self.background(raw)
            var p = self.ray_origin(self.pos)
            if background < 0.5 { p = self.unproject(self.pos, raw) }
            let surface = self.surface_tex.sample_nearest(self.pos)
            let eligible = surface.w > 0.5 && surface.z > 0.35 && background < 0.5
            let depth = self.ssr_distance(p)
            // Background and reflectivity are separate: an unlit lamp can
            // be a ray HIT even though it is not a reflection receiver.
            var tag = 0.0
            if eligible { tag = 1.0 }
            self.fb1 = vec4(depth, background, tag, 1.0)
            var value = self.trace_tex.sample_nearest(self.pos)
            if !eligible { self.fb0 = vec4(0.0, 0.0, 0.0, 0.0); return }
            let old = self.u_previous_clip * vec4(p, 1.0)
            if self.u_ssr.w > 0.5 && old.w > 0.000001 {
                let uv = self.ssr_uv(old)
                if self.ssr_inside(uv) {
                    let previous = self.history_depth.sample_nearest(uv)
                    let expected = self.fog_scene_depth(p, self.u_previous_eye, self.u_previous_axis, self.u_previous_ref)
                    if previous.y < 0.5 && previous.z > 0.5 && abs(previous.x - expected) <= max(expected, 0.001) * 0.1 {
                        let history = self.history_tex.sample_nearest(uv)
                        let confidence = mix(value.w, history.w, 0.9)
                        let radiance = mix(value.xyz * value.w, history.xyz * history.w, 0.9)
                        value = vec4(radiance / max(confidence, 0.000001), confidence)
                    }
                }
            }
            self.fb0 = value
        }
    }

    mod.draw.DrawSsrApply = mod.std.set_type_default() do #(DrawSsrApply::script_shader(vm)){
        ..mod.draw.DrawQuad,
        ..mod.draw.SsrCamera,
        color_format: @Rgba16F
        depth_clip: 0.0
        alpha_blend: false
        scene_tex: texture_2d(float)
        reflection_tex: texture_2d(float)
        reflection_depth: texture_2d(float)
        depth_tex: texture_2d(float)
        surface_tex: texture_2d(float)
        envspec_tex: texture_2d(float)
        // Premultiplied radiance/confidence; four depth-aware nearest taps
        // per sample, with the current receiver's depth for ALL five taps.
        ssr_upsample: fn(at: vec2, depth: float) -> vec4 {
            if !self.ssr_inside(at) { return vec4(0.0, 0.0, 0.0, 0.0) }
            let size = self.reflection_tex.size()
            let pixel = at * size - vec2(0.5, 0.5)
            let base = floor(pixel)
            let f = fract(pixel)
            var sum = vec4(0.0, 0.0, 0.0, 0.0)
            var weight = 0.0
            var closest = vec4(0.0, 0.0, 0.0, 0.0)
            var best = 0.1
            for i in 0..4 {
                let y = floor(float(i) * 0.5)
                let x = float(i) - y * 2.0
                let uv = (clamp(base + vec2(x, y), vec2(0.0, 0.0), size - vec2(1.0, 1.0)) + vec2(0.5, 0.5)) / size
                let d = self.reflection_depth.sample_nearest(uv)
                let r = self.reflection_tex.sample_nearest(uv)
                let value = vec4(r.xyz * r.w, r.w)
                let gap = abs(d.x - depth) / max(depth, 0.001) + d.y + (1.0 - d.z)
                if gap < best { best = gap; closest = value }
                let spatial = mix(1.0 - f.x, f.x, x) * mix(1.0 - f.y, f.y, y)
                var w = 0.0
                if gap <= 0.1 { w = spatial * exp(clamp(0.0 - gap * 100.0, -80.0, 0.0)) }
                sum = sum + value * w
                weight = weight + w
            }
            if weight > 0.000001 { return sum / weight }
            return closest
        }
        pixel: fn() -> vec4 {
            let scene = self.scene_tex.sample(self.pos)
            let surface = self.surface_tex.sample_nearest(self.pos)
            if surface.w <= 0.5 || surface.z <= 0.35 { return scene }
            let raw = self.depth_tex.sample_nearest(self.pos).x
            if self.background(raw) > 0.5 { return scene }
            let depth = self.ssr_distance(self.unproject(self.pos, raw))
            let radius = mix(4.0, 0.0, clamp((surface.z - 0.35) / 0.65, 0.0, 1.0))
            let step_uv = vec2(radius, radius) / self.reflection_tex.size()
            var reflection = self.ssr_upsample(self.pos, depth) * 0.5
            reflection = reflection + self.ssr_upsample(self.pos + vec2(step_uv.x, 0.0), depth) * 0.125
            reflection = reflection + self.ssr_upsample(self.pos - vec2(step_uv.x, 0.0), depth) * 0.125
            reflection = reflection + self.ssr_upsample(self.pos + vec2(0.0, step_uv.y), depth) * 0.125
            reflection = reflection + self.ssr_upsample(self.pos - vec2(0.0, step_uv.y), depth) * 0.125
            let c = clamp(reflection.w * self.u_ssr.x, 0.0, 1.0)
            if c <= 0.0 { return scene }
            let env = self.envspec_tex.sample_nearest(self.pos)
            let radiance = reflection.xyz / max(reflection.w, 0.000001)
            // ENVSPEC.a is achromatic, including for coloured metals.
            return vec4(max(scene.xyz - env.xyz * c + radiance * (env.w * c), vec3(0.0, 0.0, 0.0)), scene.w)
        }
        fragment: fn() { self.fb0 = self.pixel() }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSsrTrace { #[deref] pub draw_super: DrawQuad }
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSsrResolve { #[deref] pub draw_super: DrawQuad }
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSsrApply { #[deref] pub draw_super: DrawQuad }

#[derive(Default)]
pub struct SsrHistory { previous: Option<SsrCamera>, frame: u32 }
pub struct SsrUniforms {
    pub settings: SsrSettings,
    pub camera: SsrCamera,
    pub previous: SsrCamera,
    pub history: bool,
    pub frame: u32,
    pub mixed_output_passes: bool,
}
impl SsrUniforms {
    pub fn active(&self) -> bool { self.settings.active() && self.mixed_output_passes }
}
impl SsrHistory {
    pub fn uniforms(&self, settings: SsrSettings, camera: SsrCamera, mixed_output_passes: bool) -> SsrUniforms {
        SsrUniforms { settings: settings.sanitized(), camera, previous: self.previous.unwrap_or(camera), history: self.previous.is_some(), frame: self.frame, mixed_output_passes }
    }
    pub fn commit(&mut self, uniforms: &SsrUniforms) {
        if uniforms.active() {
            self.previous = Some(uniforms.camera);
            self.frame = self.frame.wrapping_add(1);
        } else { *self = Self::default(); }
    }
}

struct Stage { pass: DrawPass, list: DrawList }
impl Stage {
    fn new(cx: &mut Cx, name: &str) -> Self {
        Self { pass: DrawPass::new_with_name(cx, name), list: DrawList::new(cx) }
    }
    fn record(&mut self, cx: &mut Cx2d, size: DVec2, targets: &[&Texture], draw: &mut DrawQuad) {
        self.pass.set_size(cx, size);
        self.pass.clear_color_textures(cx.cx);
        for target in targets {
            self.pass.add_color_texture(cx.cx, target, DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 0.0)));
        }
        self.pass.clear_depth_texture(cx.cx);
        cx.begin_pass(&self.pass, Some(1.0));
        self.pass.set_size(cx, size);
        self.list.begin_always(cx);
        cx.begin_root_turtle(cx.current_pass_size(), Layout::flow_overlay());
        draw.draw_abs(cx, Rect { pos: dvec2(0.0, 0.0), size });
        cx.end_pass_sized_turtle();
        self.list.end(cx);
        cx.end_pass(&self.pass);
    }
}
struct Resources {
    trace: Stage, resolve: Stage, apply: Stage,
    raw: Texture, reflection: [Texture; 2], depth: [Texture; 2], applied: [Texture; 2],
    draw_trace: Box<DrawSsrTrace>, draw_resolve: Box<DrawSsrResolve>, draw_apply: Box<DrawSsrApply>,
}
#[derive(Default)]
pub struct SsrPass {
    resources: Option<Resources>, history: SsrHistory, size: DVec2, flip: usize, output_valid: bool,
}
impl SsrPass {
    pub fn ensure(&mut self, cx: &mut Cx) -> bool {
        if self.resources.is_some() { return true; }
        let Some((trace, resolve, apply)) = cx.try_with_vm(|vm| (
            Box::new(DrawSsrTrace::script_new_with_default(vm)),
            Box::new(DrawSsrResolve::script_new_with_default(vm)),
            Box::new(DrawSsrApply::script_new_with_default(vm)),
        )) else { return false; };
        let target = |cx: &mut Cx| Texture::new_with_format(cx, TextureFormat::RenderRGBAf16 { size: TextureSize::Auto, initial: true });
        self.resources = Some(Resources {
            trace: Stage::new(cx, "reflection trace"), resolve: Stage::new(cx, "reflection resolve"), apply: Stage::new(cx, "reflection apply"),
            raw: target(cx), reflection: [target(cx), target(cx)], depth: [target(cx), target(cx)], applied: [target(cx), target(cx)],
            draw_trace: trace, draw_resolve: resolve, draw_apply: apply,
        });
        true
    }
    pub fn reset_history(&mut self) { self.history = SsrHistory::default(); self.output_valid = false; }
    pub fn begin_frame(&mut self) { self.output_valid = false; }
    pub fn disable(&mut self, cx: &mut Cx) {
        if let Some(r) = &self.resources {
            for stage in [&r.trace, &r.resolve, &r.apply] {
                stage.pass.clear_color_textures(cx);
                stage.pass.withdraw(cx);
            }
        }
        *self = Self::default();
    }
    pub fn applied(&self) -> Option<&Texture> {
        self.resources.as_ref().filter(|_| self.output_valid).map(|r| &r.applied[self.flip])
    }
    pub fn parent_apply(&self, cx: &mut Cx, parent: DrawPassId) {
        if let Some(r) = &self.resources { r.apply.pass.set_pass_parent(cx, parent); }
    }
    pub fn run(&mut self, cx: &mut Cx2d, frame: SsrFrame<'_>) -> Option<DrawPassId> {
        if !frame.settings.active() || !cx.cx.gpu_info().mixed_output_passes { self.disable(cx.cx); return None; }
        if !self.ensure(cx.cx) { return None; }
        let pixels = frame.size * cx.current_dpi_factor();
        let pixels = dvec2(pixels.x.floor().max(1.0), pixels.y.floor().max(1.0));
        if self.size != pixels { self.reset_history(); self.size = pixels; }
        let half = dvec2((pixels.x * 0.5).floor().max(1.0), (pixels.y * 0.5).floor().max(1.0));
        let uniforms = self.history.uniforms(frame.settings, frame.camera, cx.cx.gpu_info().mixed_output_passes);
        let previous = self.flip;
        self.flip ^= 1;
        let r = self.resources.as_mut().unwrap();
        r.trace.pass.set_pass_parent(cx.cx, r.resolve.pass.draw_pass_id());
        r.resolve.pass.set_pass_parent(cx.cx, r.apply.pass.draw_pass_id());
        r.apply.pass.set_pass_parent(cx.cx, frame.parent);
        let depth_map = match cx.cx.gpu_backend() { GpuBackend::OpenGl | GpuBackend::WebGl => [2.0, -1.0], _ => [1.0, 0.0] };
        for dv in [&mut r.draw_trace.draw_super.draw_vars, &mut r.draw_resolve.draw_super.draw_vars, &mut r.draw_apply.draw_super.draw_vars] {
            dv.set_uniform(cx.cx, live_id!(u_world_from_clip), &uniforms.camera.general.world_from_clip);
            dv.set_uniform(cx.cx, live_id!(u_clip), &uniforms.camera.general.clip_from_world);
            dv.set_uniform(cx.cx, live_id!(u_eye), &uniforms.camera.eye);
            dv.set_uniform(cx.cx, live_id!(u_axis), &uniforms.camera.axis);
            dv.set_uniform(cx.cx, live_id!(u_ref), &uniforms.camera.reference);
            dv.set_uniform(cx.cx, live_id!(u_previous_clip), &uniforms.previous.general.clip_from_world);
            dv.set_uniform(cx.cx, live_id!(u_previous_eye), &uniforms.previous.eye);
            dv.set_uniform(cx.cx, live_id!(u_previous_axis), &uniforms.previous.axis);
            dv.set_uniform(cx.cx, live_id!(u_previous_ref), &uniforms.previous.reference);
            dv.set_uniform(cx.cx, live_id!(u_depth_map), &depth_map);
            dv.set_uniform(cx.cx, live_id!(u_ssr), &[uniforms.settings.strength, uniforms.settings.max_distance, (uniforms.frame % 4096) as f32, if uniforms.history { 1.0 } else { 0.0 }]);
        }
        let dv = &mut r.draw_trace.draw_super.draw_vars;
        dv.set_texture(0, frame.depth);
        dv.set_texture(1, frame.surface);
        dv.set_texture(2, &r.applied[previous]);
        dv.set_texture(3, &r.depth[previous]);
        r.trace.record(cx, half, &[&r.raw], &mut r.draw_trace.draw_super);
        let dv = &mut r.draw_resolve.draw_super.draw_vars;
        dv.set_texture(0, &r.raw);
        dv.set_texture(1, &r.reflection[previous]);
        dv.set_texture(2, &r.depth[previous]);
        dv.set_texture(3, frame.depth);
        dv.set_texture(4, frame.surface);
        r.resolve.record(cx, half, &[&r.reflection[self.flip], &r.depth[self.flip]], &mut r.draw_resolve.draw_super);
        let dv = &mut r.draw_apply.draw_super.draw_vars;
        dv.set_texture(0, frame.scene);
        dv.set_texture(1, &r.reflection[self.flip]);
        dv.set_texture(2, &r.depth[self.flip]);
        dv.set_texture(3, frame.depth);
        dv.set_texture(4, frame.surface);
        dv.set_texture(5, frame.envspec);
        // The applied target IS the pass's private HDR copy for the next
        // trace; writing the other half avoids a redundant full-screen copy.
        r.apply.record(cx, pixels, &[&r.applied[self.flip]], &mut r.draw_apply.draw_super);
        self.history.commit(&uniforms);
        self.output_valid = true;
        Some(r.trace.pass.draw_pass_id())
    }
}
pub struct SsrFrame<'a> {
    pub settings: SsrSettings, pub camera: SsrCamera,
    pub depth: &'a Texture, pub scene: &'a Texture, pub surface: &'a Texture, pub envspec: &'a Texture,
    pub size: DVec2, pub parent: DrawPassId,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_gate_and_previous_view_projection() {
        let mut history = SsrHistory::default();
        let first = SsrCamera::from_clip(Mat4f::identity());
        let off = history.uniforms(SsrSettings::default(), first, true);
        assert!(!off.active(), "default settings allocate no reflection chain");
        history.commit(&off);
        let settings = SsrSettings { strength: 1.0, ..Default::default() };
        let unsupported = history.uniforms(settings, first, false);
        assert!(!unsupported.active(), "a GPU without mixed outputs must run nothing");
        history.commit(&unsupported);
        let on = history.uniforms(settings, first, true);
        assert!(on.active());
        assert!(!on.history, "disabled/unsupported frames must not seed history");
        history.commit(&on);
        let mut moved = Mat4f::identity();
        moved.v[12] = 2.0;
        let second = history.uniforms(settings, SsrCamera::from_clip(moved), true);
        assert!(second.history);
        assert_eq!(second.previous.general.clip_from_world, first.general.clip_from_world,
            "trace and resolve must reproject through the preceding frame's view-projection");
        assert_eq!(second.frame, 1);
        let before_record = history.uniforms(settings, first, true);
        assert_eq!(before_record.previous.general.clip_from_world, first.general.clip_from_world,
            "preparing uniforms must not advance history before recording");
        history.commit(&second);
        assert_eq!(history.uniforms(settings, first, true).previous.general.clip_from_world, moved.v);
        history.commit(&unsupported);
        assert!(!history.uniforms(settings, first, true).history,
            "losing the capability invalidates retained history");
    }
}
