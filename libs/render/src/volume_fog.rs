//! Height fog: half-resolution march -> temporal resolve -> HDR fog apply.
//!
//! The resolve writes fog and endpoint depth to separate RGBA16F attachments,
//! ping-ponged together. The apply is the composite's depth-aware 2x2 upsample,
//! before bloom, metering and tone mapping. No pass samples its own output.
//! Local lights are deliberately UNSHADOWED; only the sun samples the CSM atlas.
//! Targets/draws are lazy and released on disable. Steady-state CPU work is
//! fixed uniform writes plus selecting eight lights from the existing frame list;
//! no per-frame collections or payload allocations.

use makepad_draw::*;
use makepad_scene::VolumeFog;

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw

    mod.draw.VolumeFogCamera = {
        u_world_from_clip: uniform(mat4x4f(1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0))
        u_eye: uniform(vec4(0.0, 0.0, 0.0, 1.0))
        u_axis: uniform(vec4(0.0, 0.0, 1.0, 0.0))
        u_ref: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        // density, height, falloff, max distance
        u_medium: uniform(vec4(0.0, 0.0, 0.0, 80.0))
        // Hardware depth -> clip z (GL maps clip z to 0..1).
        u_depth_map: uniform(vec2(1.0, 0.0))

        unproject: fn(uv: vec2f, raw: float) -> vec3f {
            let h = self.u_world_from_clip * vec4(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, raw * self.u_depth_map.x + self.u_depth_map.y, 1.0)
            var w = max(abs(h.w), 0.0000001)
            if h.w < 0.0 { w = 0.0 - w }
            return h.xyz / w
        }
        ray_origin: fn(uv: vec2f) -> vec3f {
            if self.u_eye.w > 0.5 { return self.u_eye.xyz }
            return self.unproject(uv, 0.0)
        }
        ray_dir: fn(uv: vec2f) -> vec3f {
            let ray = self.unproject(uv, 0.75) - self.unproject(uv, 0.25)
            return ray / max(length(ray), 0.000001)
        }
        // A zero depth is also treated as background; neither background
        // representation is ever divided by or unprojected at infinity.
        background: fn(raw: float) -> float {
            if raw <= 0.0 || raw >= 0.99999 { return 1.0 }
            return 0.0
        }
        march_distance: fn(uv: vec2f, raw: float) -> float {
            if self.background(raw) > 0.5 { return self.u_medium.w }
            return min(length(self.unproject(uv, raw) - self.ray_origin(uv)), self.u_medium.w)
        }
        fog_scene_depth: fn(p: vec3f, eye: vec4f, axis: vec4f, reference: vec4f) -> float {
            if eye.w > 0.5 { return length(p - eye.xyz) }
            return max(dot(reference.xyz - p, axis.xyz), 0.0)
        }
    }

    mod.draw.DrawVolumeFogMarch = mod.std.set_type_default() do #(DrawVolumeFogMarch::script_shader(vm)){
        ..mod.draw.DrawQuad,
        ..mod.draw.VolumeFogCamera,
        ..mod.draw.SunCascades,
        color_format: @Rgba16F
        depth_clip: 0.0
        alpha_blend: false
        depth_tex: texture_2d(float)
        csm_map: texture_depth(float)
        light_dir: uniform(vec3(0.0, 1.0, 0.0))
        sun_color: uniform(vec3(0.0, 0.0, 0.0))
        sun_sky: uniform(vec3(0.0, 0.0, 0.0))
        u_albedo: uniform(vec3(1.0, 1.0, 1.0))
        // g, sun gain, frame index, local count
        u_scatter: uniform(vec4(0.6, 1.0, 0.0, 0.0))
        frag: fragment_position()
        u_local_pos0: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        u_local_color0: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        u_local_dir0: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        u_local_pos1: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        u_local_color1: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        u_local_dir1: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        u_local_pos2: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        u_local_color2: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        u_local_dir2: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        u_local_pos3: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        u_local_color3: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        u_local_dir3: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        u_local_pos4: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        u_local_color4: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        u_local_dir4: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        u_local_pos5: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        u_local_color5: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        u_local_dir5: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        u_local_pos6: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        u_local_color6: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        u_local_dir6: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        u_local_pos7: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        u_local_color7: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        u_local_dir7: uniform(vec4(0.0, 0.0, 0.0, 0.0))

        vertex: fn() {
            self.pos = self.geom.pos
            self.world = vec4(self.geom.pos.x, self.geom.pos.y, 0.0, 1.0)
            self.vertex_pos = vec4(self.geom.pos.x * 2.0 - 1.0, 1.0 - self.geom.pos.y * 2.0, 0.0, 1.0)
        }
        local_pos: fn(i: float) -> vec4f {
            if i < 0.5 { return self.u_local_pos0 }
            if i < 1.5 { return self.u_local_pos1 }
            if i < 2.5 { return self.u_local_pos2 }
            if i < 3.5 { return self.u_local_pos3 }
            if i < 4.5 { return self.u_local_pos4 }
            if i < 5.5 { return self.u_local_pos5 }
            if i < 6.5 { return self.u_local_pos6 }
            return self.u_local_pos7
        }
        local_color: fn(i: float) -> vec4f {
            if i < 0.5 { return self.u_local_color0 }
            if i < 1.5 { return self.u_local_color1 }
            if i < 2.5 { return self.u_local_color2 }
            if i < 3.5 { return self.u_local_color3 }
            if i < 4.5 { return self.u_local_color4 }
            if i < 5.5 { return self.u_local_color5 }
            if i < 6.5 { return self.u_local_color6 }
            return self.u_local_color7
        }
        local_dir: fn(i: float) -> vec4f {
            if i < 0.5 { return self.u_local_dir0 }
            if i < 1.5 { return self.u_local_dir1 }
            if i < 2.5 { return self.u_local_dir2 }
            if i < 3.5 { return self.u_local_dir3 }
            if i < 4.5 { return self.u_local_dir4 }
            if i < 5.5 { return self.u_local_dir5 }
            if i < 6.5 { return self.u_local_dir6 }
            return self.u_local_dir7
        }

        phase: fn(cosine: float) -> float {
            let g = clamp(self.u_scatter.x, 0.0, 0.9)
            let den = max(1.0 + g * g - 2.0 * g * clamp(cosine, -1.0, 1.0), 0.01)
            return (1.0 - g * g) / (12.5663706 * den * sqrt(den))
        }
        // Hardware 2x2 PCF at the medium point, with no surface-normal
        // offset (there is no surface). Outside the cascades is lit.
        sun_visibility: fn(p: vec3f) -> float {
            if self.csm_p.x < 0.5 { return 1.0 }
            for c in 0..4 {
                let ci = float(c)
                let q = self.csm_proj(ci, p)
                if self.csm_inside(q, 0.99) > 0.5 {
                    let z = q.z * self.csm_pick(self.csm_da, ci) + self.csm_pick(self.csm_db, ci)
                    return self.csm_tap(q.x * 0.5 + 0.5, 0.5 - q.y * 0.5, ci, z)
                }
            }
            return 1.0
        }
        local_light: fn(p: vec3f, ray: vec3f) -> vec3f {
            var light = vec3(0.0, 0.0, 0.0)
            if self.u_scatter.w > 0.0 {
                for i in 0..8 {
                    if float(i) < self.u_scatter.w {
                        let pr = self.local_pos(float(i))
                        let co = self.local_color(float(i))
                        let di = self.local_dir(float(i))
                        let delta = pr.xyz - p
                        let distance = max(length(delta), 0.0001)
                        let toward = delta / distance
                        let fade = max(1.0 - distance / max(pr.w, 0.0001), 0.0)
                        var cone = 1.0
                        if co.w > -1.0 {
                            let angle = dot(di.xyz, vec3(0.0, 0.0, 0.0) - toward)
                            cone = clamp((angle - co.w) / max(di.w - co.w, 0.0001), 0.0, 1.0)
                            cone = cone * cone * (3.0 - 2.0 * cone)
                        }
                        light = light + co.xyz * (fade * fade * cone * self.phase(dot(ray, toward)))
                    }
                }
            }
            return light
        }
        pixel: fn() -> vec4f {
            let uv = self.pos
            let raw = self.depth_tex.sample_nearest(uv).x
            let origin = self.ray_origin(uv)
            let ray = self.ray_dir(uv)
            let step = self.march_distance(uv, raw) / 24.0
            let jitter = fract(52.9829189 * fract(dot(floor(self.frag.xy) + self.u_scatter.z * vec2(5.588238, 5.588238), vec2(0.06711056, 0.00583715))))
            let sun_dir = self.light_dir / max(length(self.light_dir), 0.000001)
            let phase = self.phase(dot(ray, sun_dir))
            var trans = 1.0
            var scatter = vec3(0.0, 0.0, 0.0)
            for i in 0..24 {
                let p = origin + ray * ((float(i) + jitter) * step)
                var extinction = self.u_medium.x
                // Uniform density needs no height arithmetic: even an
                // extreme finite height cannot produce infinity * zero.
                if self.u_medium.z > 0.0 {
                    extinction = extinction * exp(clamp(0.0 - max(p.y - self.u_medium.y, 0.0) * self.u_medium.z, -80.0, 0.0))
                }
                let attenuation = exp(clamp(0.0 - extinction * step, -80.0, 0.0))
                var illumination = self.sun_sky * 0.25 + self.local_light(p, ray)
                if self.u_scatter.y > 0.0 {
                    illumination = illumination + self.sun_color * (self.sun_visibility(p) * phase * self.u_scatter.y)
                }
                // Exact constant-extinction integral of extinction * light
                // over the step, attenuated by all the preceding steps.
                scatter = scatter + self.u_albedo * illumination * (trans * (1.0 - attenuation))
                trans = trans * attenuation
            }
            return vec4(clamp(scatter, vec3(0.0, 0.0, 0.0), vec3(60000.0, 60000.0, 60000.0)), clamp(trans, 0.0, 1.0))
        }
        fragment: fn() { self.fb0 = self.pixel() }
    }

    mod.draw.DrawVolumeFogResolve = mod.std.set_type_default() do #(DrawVolumeFogResolve::script_shader(vm)){
        ..mod.draw.DrawQuad,
        ..mod.draw.VolumeFogCamera,
        color_format: @Rgba16F
        depth_clip: 0.0
        alpha_blend: false
        fb1: fragment_output(1, vec4f)
        march_tex: texture_2d(float)
        history_tex: texture_2d(float)
        history_depth: texture_2d(float)
        depth_tex: texture_2d(float)
        u_previous_clip: uniform(mat4x4f(1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0))
        u_previous_eye: uniform(vec4(0.0, 0.0, 0.0, 1.0))
        u_previous_axis: uniform(vec4(0.0, 0.0, 1.0, 0.0))
        u_previous_ref: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        u_history: uniform(0.0)
        vertex: fn() {
            self.pos = self.geom.pos
            self.world = vec4(self.geom.pos.x, self.geom.pos.y, 0.0, 1.0)
            self.vertex_pos = vec4(self.geom.pos.x * 2.0 - 1.0, 1.0 - self.geom.pos.y * 2.0, 0.0, 1.0)
        }
        fragment: fn() {
            let raw = self.depth_tex.sample_nearest(self.pos).x
            let p = self.ray_origin(self.pos) + self.ray_dir(self.pos) * self.march_distance(self.pos, raw)
            let depth = self.fog_scene_depth(p, self.u_eye, self.u_axis, self.u_ref)
            let background = self.background(raw)
            self.fb1 = vec4(depth, background, 0.0, 1.0)
            var value = self.march_tex.sample_nearest(self.pos)
            let old = self.u_previous_clip * vec4(p.x, p.y, p.z, 1.0)
            if self.u_history > 0.5 && old.w > 0.000001 {
                let uv = vec2(old.x / old.w * 0.5 + 0.5, 0.5 - old.y / old.w * 0.5)
                if uv.x >= 0.0 && uv.x <= 1.0 && uv.y >= 0.0 && uv.y <= 1.0 {
                    let previous = self.history_depth.sample_nearest(uv)
                    let expected = self.fog_scene_depth(p, self.u_previous_eye, self.u_previous_axis, self.u_previous_ref)
                    if abs(previous.y - background) < 0.5 && abs(previous.x - expected) <= max(expected, 0.001) * 0.1 {
                        // Nearest history keeps the fog and its depth from
                        // the same texel on a disocclusion edge.
                        value = mix(value, self.history_tex.sample_nearest(uv), 0.9)
                    }
                }
            }
            self.fb0 = value
        }
    }

    mod.draw.DrawVolumeFogApply = mod.std.set_type_default() do #(DrawVolumeFogApply::script_shader(vm)){
        ..mod.draw.DrawQuad,
        ..mod.draw.VolumeFogCamera,
        color_format: @Rgba16F
        depth_clip: 0.0
        alpha_blend: false
        scene_tex: texture_2d(float)
        fog_tex: texture_2d(float)
        fog_depth: texture_2d(float)
        depth_tex: texture_2d(float)
        vertex: fn() {
            self.pos = self.geom.pos
            self.world = vec4(self.geom.pos.x, self.geom.pos.y, 0.0, 1.0)
            self.vertex_pos = vec4(self.geom.pos.x * 2.0 - 1.0, 1.0 - self.geom.pos.y * 2.0, 0.0, 1.0)
        }
        pixel: fn() -> vec4f {
            let raw = self.depth_tex.sample_nearest(self.pos).x
            let p = self.ray_origin(self.pos) + self.ray_dir(self.pos) * self.march_distance(self.pos, raw)
            let depth = self.fog_scene_depth(p, self.u_eye, self.u_axis, self.u_ref)
            let background = self.background(raw)
            let size = self.fog_tex.size()
            let pixel = self.pos * size - vec2(0.5, 0.5)
            let base = floor(pixel)
            let f = fract(pixel)
            var sum = vec4(0.0, 0.0, 0.0, 0.0)
            var weight = 0.0
            var closest = self.fog_tex.sample_nearest(self.pos)
            var best = 1000000.0
            for i in 0..4 {
                let y = floor(float(i) * 0.5)
                let x = float(i) - y * 2.0
                let uv = (clamp(base + vec2(x, y), vec2(0.0, 0.0), size - vec2(1.0, 1.0)) + vec2(0.5, 0.5)) / size
                let d = self.fog_depth.sample_nearest(uv)
                let fog = self.fog_tex.sample_nearest(uv)
                let gap = abs(d.x - depth) / max(depth, 0.001) + abs(d.y - background)
                if gap < best { best = gap; closest = fog }
                let spatial = mix(1.0 - f.x, f.x, x) * mix(1.0 - f.y, f.y, y)
                let w = spatial * exp(clamp(0.0 - gap * 100.0, -80.0, 0.0))
                sum = sum + fog * w
                weight = weight + w
            }
            var fog = closest
            if weight > 0.000001 { fog = sum / weight }
            let scene = self.scene_tex.sample(self.pos)
            return vec4(scene.xyz * fog.w + fog.xyz, scene.w)
        }
        fragment: fn() { self.fb0 = self.pixel() }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawVolumeFogMarch { #[deref] pub draw_super: DrawQuad }
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawVolumeFogResolve { #[deref] pub draw_super: DrawQuad }
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawVolumeFogApply { #[deref] pub draw_super: DrawQuad }

/// CPU camera state carried with the history, in absolute world coordinates.
#[derive(Clone, Copy)]
pub struct VolumeFogCamera {
    pub general: crate::SsaoGeneral,
    pub eye: [f32; 4],
    pub axis: [f32; 4],
    pub reference: [f32; 4],
}
impl VolumeFogCamera {
    pub fn from_clip(clip: Mat4f) -> Self {
        let general = crate::SsaoGeneral::from_clip(clip);
        let terms = general.terms();
        Self { general, eye: terms.eye, axis: terms.axis, reference: terms.reference }
    }
}

/// Pure per-frame uniform computation; committing happens only after recording.
#[derive(Default)]
pub struct VolumeFogHistory { previous: Option<VolumeFogCamera>, frame: u32 }
pub struct VolumeFogUniforms {
    pub fog: VolumeFog,
    pub camera: VolumeFogCamera,
    pub previous: VolumeFogCamera,
    pub history: bool,
    pub frame: u32,
}
impl VolumeFogUniforms { pub fn active(&self) -> bool { self.fog.active() } }
impl VolumeFogHistory {
    pub fn uniforms(&self, fog: VolumeFog, camera: VolumeFogCamera) -> VolumeFogUniforms {
        VolumeFogUniforms { fog: fog.sanitized(), camera, previous: self.previous.unwrap_or(camera), history: self.previous.is_some(), frame: self.frame }
    }
    pub fn commit(&mut self, uniforms: &VolumeFogUniforms) {
        if uniforms.active() {
            self.previous = Some(uniforms.camera);
            self.frame = self.frame.wrapping_add(1);
        } else { *self = Self::default(); }
    }
}

#[derive(Clone, Copy, Default)]
struct FogLight { pos: [f32; 4], color: [f32; 4], dir: [f32; 4] }

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
    march: Stage, resolve: Stage, apply: Stage,
    raw: Texture, fog: [Texture; 2], depth: [Texture; 2], applied: Texture,
    draw_march: Box<DrawVolumeFogMarch>,
    draw_resolve: Box<DrawVolumeFogResolve>,
    draw_apply: Box<DrawVolumeFogApply>,
}

/// The self-contained pass chain. `ensure` is called only for enabled HDR frames.
#[derive(Default)]
pub struct VolumeFogPass {
    resources: Option<Resources>,
    history: VolumeFogHistory,
    size: DVec2,
    flip: usize,
    output_valid: bool,
    lights: [FogLight; 8],
    light_count: usize,
    overflow_logged: bool,
}

impl VolumeFogPass {
    pub fn ensure(&mut self, cx: &mut Cx) -> bool {
        if self.resources.is_some() { return true; }
        let Some((march, resolve, apply)) = cx.try_with_vm(|vm| (
            Box::new(DrawVolumeFogMarch::script_new_with_default(vm)),
            Box::new(DrawVolumeFogResolve::script_new_with_default(vm)),
            Box::new(DrawVolumeFogApply::script_new_with_default(vm)),
        )) else { return false; };
        let target = |cx: &mut Cx| Texture::new_with_format(cx, TextureFormat::RenderRGBAf16 { size: TextureSize::Auto, initial: true });
        self.resources = Some(Resources {
            march: Stage::new(cx, "volume fog march"), resolve: Stage::new(cx, "volume fog resolve"), apply: Stage::new(cx, "volume fog apply"),
            raw: target(cx), fog: [target(cx), target(cx)], depth: [target(cx), target(cx)], applied: target(cx),
            draw_march: march, draw_resolve: resolve, draw_apply: apply,
        });
        true
    }

    pub fn reset_history(&mut self) { self.history = VolumeFogHistory::default(); self.output_valid = false; }

    /// Withdraw before releasing: disabled passes must never repaint retained targets.
    pub fn disable(&mut self, cx: &mut Cx) {
        if self.resources.is_none() { return; }
        if let Some(r) = &self.resources {
            for s in [&r.march, &r.resolve, &r.apply] {
                s.pass.clear_color_textures(cx);
                s.pass.withdraw(cx);
            }
        }
        *self = Self::default();
    }

    pub fn begin_frame(&mut self) { self.output_valid = false; }

    pub fn output(&self) -> Option<&Texture> {
        self.resources.as_ref().filter(|_| self.output_valid).map(|r| &r.fog[self.flip])
    }
    pub fn applied(&self) -> Option<&Texture> {
        self.resources.as_ref().filter(|_| self.output_valid).map(|r| &r.applied)
    }
    pub fn march_draw_vars(&mut self) -> Option<&mut DrawVars> {
        self.resources.as_mut().map(|r| &mut r.draw_march.draw_super.draw_vars)
    }

    /// Select the closest eight flagged lights without a per-frame allocation.
    pub fn set_lights(&mut self, lights: &[crate::lightmap::LmLight], eye: Vec3f) {
        self.light_count = 0;
        let mut distances = [f32::INFINITY; 8];
        let mut count = 0;
        for l in lights.iter().filter(|l| l.volumetric && l.radius > 0.0) {
            let distance = (l.pos - eye).length();
            if !distance.is_finite() || !l.radius.is_finite() || !l.dir.x.is_finite() || !l.dir.y.is_finite() || !l.dir.z.is_finite() { continue; }
            count += 1;
            let Some(slot) = distances.iter().position(|&d| distance < d) else { continue; };
            for i in (slot + 1..8).rev() { distances[i] = distances[i - 1]; self.lights[i] = self.lights[i - 1]; }
            distances[slot] = distance;
            let (inner, outer) = l.cone.map_or((-1.0, -1.0), |(i, o)| (i.to_radians().cos(), o.to_radians().cos()));
            let dir = l.dir * (1.0 / l.dir.length().max(0.000001));
            let finite = |v: f32| if v.is_finite() { v.clamp(0.0, 60000.0) } else { 0.0 };
            self.lights[slot] = FogLight {
                pos: [l.pos.x, l.pos.y, l.pos.z, l.radius],
                color: [finite(l.color.x), finite(l.color.y), finite(l.color.z), outer],
                dir: [dir.x, dir.y, dir.z, inner],
            };
            self.light_count = (self.light_count + 1).min(8);
        }
        if count > 8 && !self.overflow_logged {
            log!("volume fog: more than eight volumetric lights; nearest eight scatter, remaining lights still illuminate surfaces");
            self.overflow_logged = true;
        }
    }

    /// Record all three passes; caller parents its scene producer under the
    /// returned march pass. `frame` matrices and lighting describe that scene.
    pub fn run(&mut self, cx: &mut Cx2d, frame: VolumeFogFrame<'_>) -> Option<DrawPassId> {
        if !frame.fog.active() { self.disable(cx.cx); return None; }
        if !self.ensure(cx.cx) { return None; }
        let pixels = frame.size * cx.current_dpi_factor();
        let pixels = dvec2(pixels.x.floor().max(1.0), pixels.y.floor().max(1.0));
        if self.size != pixels { self.reset_history(); self.size = pixels; }
        let half = dvec2((pixels.x * 0.5).floor().max(1.0), (pixels.y * 0.5).floor().max(1.0));
        let uniforms = self.history.uniforms(frame.fog, frame.camera);
        let previous = self.flip;
        self.flip ^= 1;
        let r = self.resources.as_mut().unwrap();
        r.march.pass.set_pass_parent(cx.cx, r.resolve.pass.draw_pass_id());
        r.resolve.pass.set_pass_parent(cx.cx, r.apply.pass.draw_pass_id());
        r.apply.pass.set_pass_parent(cx.cx, frame.parent);
        let depth_map = match cx.cx.gpu_backend() { GpuBackend::OpenGl | GpuBackend::WebGl => [2.0, -1.0], _ => [1.0, 0.0] };
        for dv in [&mut r.draw_march.draw_super.draw_vars, &mut r.draw_resolve.draw_super.draw_vars, &mut r.draw_apply.draw_super.draw_vars] {
            dv.set_uniform(cx.cx, live_id!(u_world_from_clip), &uniforms.camera.general.world_from_clip);
            dv.set_uniform(cx.cx, live_id!(u_eye), &uniforms.camera.eye);
            dv.set_uniform(cx.cx, live_id!(u_axis), &uniforms.camera.axis);
            dv.set_uniform(cx.cx, live_id!(u_ref), &uniforms.camera.reference);
            let f = uniforms.fog;
            dv.set_uniform(cx.cx, live_id!(u_medium), &[f.density, f.height, f.falloff, f.max_distance]);
            dv.set_uniform(cx.cx, live_id!(u_depth_map), &depth_map);
        }
        let dv = &mut r.draw_march.draw_super.draw_vars;
        dv.set_texture(0, frame.depth);
        let v3 = |v: Vec3f| [v.x, v.y, v.z];
        dv.set_uniform(cx.cx, live_id!(light_dir), &v3(frame.sun.dir));
        dv.set_uniform(cx.cx, live_id!(sun_color), &v3(frame.sun.color));
        dv.set_uniform(cx.cx, live_id!(sun_sky), &v3(frame.sun.sky));
        dv.set_uniform(cx.cx, live_id!(u_albedo), &uniforms.fog.color);
        dv.set_uniform(cx.cx, live_id!(u_scatter), &[uniforms.fog.anisotropy, uniforms.fog.shafts, (uniforms.frame % 4096) as f32, self.light_count as f32]);
        for (i, (pos, color, dir)) in [
            (live_id!(u_local_pos0), live_id!(u_local_color0), live_id!(u_local_dir0)),
            (live_id!(u_local_pos1), live_id!(u_local_color1), live_id!(u_local_dir1)),
            (live_id!(u_local_pos2), live_id!(u_local_color2), live_id!(u_local_dir2)),
            (live_id!(u_local_pos3), live_id!(u_local_color3), live_id!(u_local_dir3)),
            (live_id!(u_local_pos4), live_id!(u_local_color4), live_id!(u_local_dir4)),
            (live_id!(u_local_pos5), live_id!(u_local_color5), live_id!(u_local_dir5)),
            (live_id!(u_local_pos6), live_id!(u_local_color6), live_id!(u_local_dir6)),
            (live_id!(u_local_pos7), live_id!(u_local_color7), live_id!(u_local_dir7)),
        ].into_iter().enumerate() {
            let l = &self.lights[i];
            dv.set_uniform(cx.cx, pos, &l.pos);
            dv.set_uniform(cx.cx, color, &l.color);
            dv.set_uniform(cx.cx, dir, &l.dir);
        }
        r.march.record(cx, half, &[&r.raw], &mut r.draw_march.draw_super);
        let dv = &mut r.draw_resolve.draw_super.draw_vars;
        dv.set_texture(0, &r.raw);
        dv.set_texture(1, &r.fog[previous]);
        dv.set_texture(2, &r.depth[previous]);
        dv.set_texture(3, frame.depth);
        dv.set_uniform(cx.cx, live_id!(u_previous_clip), &uniforms.previous.general.clip_from_world);
        dv.set_uniform(cx.cx, live_id!(u_previous_eye), &uniforms.previous.eye);
        dv.set_uniform(cx.cx, live_id!(u_previous_axis), &uniforms.previous.axis);
        dv.set_uniform(cx.cx, live_id!(u_previous_ref), &uniforms.previous.reference);
        dv.set_uniform(cx.cx, live_id!(u_history), &[if uniforms.history { 1.0 } else { 0.0 }]);
        r.resolve.record(cx, half, &[&r.fog[self.flip], &r.depth[self.flip]], &mut r.draw_resolve.draw_super);
        let dv = &mut r.draw_apply.draw_super.draw_vars;
        dv.set_texture(0, frame.scene);
        dv.set_texture(1, &r.fog[self.flip]);
        dv.set_texture(2, &r.depth[self.flip]);
        dv.set_texture(3, frame.depth);
        r.apply.record(cx, pixels, &[&r.applied], &mut r.draw_apply.draw_super);
        self.history.commit(&uniforms);
        self.output_valid = true;
        Some(r.march.pass.draw_pass_id())
    }

    pub fn parent_apply(&self, cx: &mut Cx, parent: DrawPassId) {
        if let Some(r) = &self.resources { r.apply.pass.set_pass_parent(cx, parent); }
    }
}

pub struct VolumeFogFrame<'a> {
    pub fog: VolumeFog,
    pub camera: VolumeFogCamera,
    pub sun: crate::SunLight,
    pub depth: &'a Texture,
    pub scene: &'a Texture,
    pub size: DVec2,
    pub parent: DrawPassId,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_fog_and_previous_camera_uniforms() {
        let mut history = VolumeFogHistory::default();
        let first = VolumeFogCamera::from_clip(Mat4f::identity());
        let off = history.uniforms(VolumeFog::default(), first);
        assert!(!off.active(), "zero density must leave the chain off");
        history.commit(&off);
        let fog = VolumeFog { density: 0.03, ..Default::default() };
        let on = history.uniforms(fog, first);
        assert!(!on.history, "a disabled frame creates no history");
        history.commit(&on);
        let mut moved = Mat4f::identity();
        moved.v[12] = 2.0;
        let second = history.uniforms(fog, VolumeFogCamera::from_clip(moved));
        assert!(second.history);
        assert_eq!(second.previous.general.clip_from_world, first.general.clip_from_world, "reprojection uses the previous frame's view-projection");
        history.commit(&second);
        let third = history.uniforms(fog, first);
        assert_eq!(third.previous.general.clip_from_world, moved.v, "camera history advances after recording");
    }
}
