use crate::{
    cx_2d::*,
    draw_list_2d::ManyInstances,
    image_cache::ImageBuffer,
    makepad_platform::*,
    turtle::*,
};
use makepad_gltf::DecodedPrimitive;
use std::{f32::consts::PI, path::Path};

const PBR_FLOATS_PER_VERTEX: usize = 16;

pub type PbrMeshHandle = usize;

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw
    use mod.geom

    mod.draw.DrawPbr = mod.std.set_type_default() do #(DrawPbr::script_shader(vm)){
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.PbrVertex, geom.PbrGeom)
        base_color_texture: texture_2d(float)
        metallic_roughness_texture: texture_2d(float)
        normal_texture: texture_2d(float)
        occlusion_texture: texture_2d(float)
        emissive_texture: texture_2d(float)
        env_texture: texture_cube(float)
        model_c0: uniform(vec4(1.0, 0.0, 0.0, 0.0))
        model_c1: uniform(vec4(0.0, 1.0, 0.0, 0.0))
        model_c2: uniform(vec4(0.0, 0.0, 1.0, 0.0))
        model_c3: uniform(vec4(0.0, 0.0, 0.0, 1.0))
        view_c0: uniform(vec4(1.0, 0.0, 0.0, 0.0))
        view_c1: uniform(vec4(0.0, 1.0, 0.0, 0.0))
        view_c2: uniform(vec4(0.0, 0.0, 1.0, 0.0))
        view_c3: uniform(vec4(0.0, 0.0, 0.0, 1.0))
        proj_c0: uniform(vec4(1.0, 0.0, 0.0, 0.0))
        proj_c1: uniform(vec4(0.0, 1.0, 0.0, 0.0))
        proj_c2: uniform(vec4(0.0, 0.0, 1.0, 0.0))
        proj_c3: uniform(vec4(0.0, 0.0, 0.0, 1.0))
        clip_ndc: uniform(vec4(-1.0, -1.0, 1.0, 1.0))
        depth_range: uniform(vec2(0.0, 1.0))
        depth_forward_bias: uniform(float(0.0))
        u_base_color_factor: uniform(vec4(1.0, 1.0, 1.0, 1.0))
        u_metallic_factor: uniform(float(1.0))
        u_roughness_factor: uniform(float(1.0))
        u_emissive_factor: uniform(vec3(0.0, 0.0, 0.0))
        u_normal_scale: uniform(float(1.0))
        u_occlusion_strength: uniform(float(1.0))
        u_has_base_color_texture: uniform(float(0.0))
        u_has_metal_roughness_texture: uniform(float(0.0))
        u_has_normal_texture: uniform(float(0.0))
        u_has_occlusion_texture: uniform(float(0.0))
        u_has_emissive_texture: uniform(float(0.0))
        u_has_env_texture: uniform(float(0.0))
        u_light_dir: uniform(vec3(0.3, 0.7, 1.0))
        u_light_color: uniform(vec3(1.0, 1.0, 1.0))
        u_ambient: uniform(float(0.15))
        u_spec_strength: uniform(float(0.9))
        u_env_intensity: uniform(float(1.8))
        u_camera_pos: uniform(vec3(0.0, 0.0, 5.0))

        v_world: varying(vec3f)
        v_normal: varying(vec3f)
        v_tangent: varying(vec4f)
        v_uv: varying(vec2f)
        v_color: varying(vec4f)
        v_clip_ndc: varying(vec2f)

        vertex: fn() {
            let local_pos = vec4(self.geom.pos_nx.x, self.geom.pos_nx.y, self.geom.pos_nx.z, 1.0);
            let local_n = vec4(self.geom.pos_nx.w, self.geom.ny_nz_uv.x, self.geom.ny_nz_uv.y, 0.0);

            let model_pos =
                self.model_c0 * local_pos.x +
                self.model_c1 * local_pos.y +
                self.model_c2 * local_pos.z +
                self.model_c3 * local_pos.w;
            let model_n =
                self.model_c0 * local_n.x +
                self.model_c1 * local_n.y +
                self.model_c2 * local_n.z;
            let local_t = vec4(self.geom.tangent.x, self.geom.tangent.y, self.geom.tangent.z, 0.0);
            let model_t =
                self.model_c0 * local_t.x +
                self.model_c1 * local_t.y +
                self.model_c2 * local_t.z;

            self.v_world = vec3(model_pos.x, model_pos.y, model_pos.z);
            self.v_normal = vec3(model_n.x, model_n.y, model_n.z);
            self.v_tangent = vec4(model_t.x, model_t.y, model_t.z, self.geom.tangent.w);
            self.v_uv = vec2(self.geom.ny_nz_uv.z, self.geom.ny_nz_uv.w);
            self.v_color = self.geom.color;

            let world = vec4(model_pos.x, model_pos.y, model_pos.z + self.draw_call.zbias, 1.0);
            let view_pos =
                self.view_c0 * world.x +
                self.view_c1 * world.y +
                self.view_c2 * world.z +
                self.view_c3 * world.w;
            let clip_pos_base =
                self.proj_c0 * view_pos.x +
                self.proj_c1 * view_pos.y +
                self.proj_c2 * view_pos.z +
                self.proj_c3 * view_pos.w;
            let inv_w = 1.0 / max(abs(clip_pos_base.w), 0.000001);
            let ndc01 = (clip_pos_base.xy * inv_w) * 0.5 + vec2(0.5, 0.5);
            let mapped_ndc = mix(self.clip_ndc.xy, self.clip_ndc.zw, ndc01);
            let depth01 = clamp((clip_pos_base.z * inv_w) * 0.5 + 0.5, 0.0, 1.0);
            let depth_mapped = clamp(
                mix(self.depth_range.x, self.depth_range.y, depth01) - self.depth_forward_bias,
                0.0,
                1.0
            );
            let clip_pos = vec4(
                mapped_ndc.x * clip_pos_base.w,
                mapped_ndc.y * clip_pos_base.w,
                depth_mapped * clip_pos_base.w,
                clip_pos_base.w
            );
            self.v_clip_ndc = mapped_ndc;
            self.vertex_pos = clip_pos;
        }

        fragment: fn(){
            self.fb0 = self.pixel()
        }

        pixel: fn() {
            if self.v_clip_ndc.x < self.clip_ndc.x || self.v_clip_ndc.y < self.clip_ndc.y
                || self.v_clip_ndc.x > self.clip_ndc.z || self.v_clip_ndc.y > self.clip_ndc.w {
                return vec4(0.0, 0.0, 0.0, 0.0)
            }

            let base = self.u_base_color_factor * self.v_color;
            let tex_srgb = self.base_color_texture.sample_as_bgra(self.v_uv);
            let tex_linear = vec4(
                pow(max(tex_srgb.x, 0.0), 2.2),
                pow(max(tex_srgb.y, 0.0), 2.2),
                pow(max(tex_srgb.z, 0.0), 2.2),
                tex_srgb.w
            );
            let tex_mix = mix(
                vec4(1.0, 1.0, 1.0, 1.0),
                tex_linear,
                clamp(self.u_has_base_color_texture, 0.0, 1.0)
            );
            let albedo = base * tex_mix;
            let mr_tex = self.metallic_roughness_texture.sample_as_bgra(self.v_uv);
            let mr_mix = mix(vec4(1.0, 1.0, 1.0, 1.0), mr_tex, clamp(self.u_has_metal_roughness_texture, 0.0, 1.0));

            let n_geom = normalize(self.v_normal);
            let tangent_world = self.v_tangent.xyz;
            let tangent_len = length(tangent_world);
            let tangent_base = if tangent_len > 0.00001 {
                tangent_world / tangent_len
            } else {
                vec3(1.0, 0.0, 0.0)
            };
            let t_raw = tangent_base - n_geom * dot(n_geom, tangent_base);
            let t_len = length(t_raw);
            let up_axis = if abs(n_geom.y) > 0.99 { vec3(1.0, 0.0, 0.0) } else { vec3(0.0, 1.0, 0.0) };
            let t = if t_len > 0.00001 { t_raw / t_len } else { normalize(cross(up_axis, n_geom)) };
            let b = normalize(cross(n_geom, t)) * self.v_tangent.w;
            let n_tex_s = self.normal_texture.sample_as_bgra(self.v_uv);
            let n_tex = vec3(
                n_tex_s.x * 2.0 - 1.0,
                (n_tex_s.y * 2.0 - 1.0) * self.u_normal_scale,
                n_tex_s.z * 2.0 - 1.0
            );
            let n_tangent = normalize(t * n_tex.x + b * n_tex.y + n_geom * n_tex.z);
            let n = normalize(mix(n_geom, n_tangent, clamp(self.u_has_normal_texture, 0.0, 1.0)));

            let l = normalize(self.u_light_dir);
            let v = normalize(self.u_camera_pos - self.v_world);
            let h = normalize(l + v);
            let ndotl = max(dot(n, l), 0.0);
            let ndotv = max(dot(n, v), 0.0001);
            let ndoth = max(dot(n, h), 0.0001);
            let vdoth = max(dot(v, h), 0.0);

            let rough = clamp(self.u_roughness_factor * mr_mix.y, 0.045, 1.0);
            let metal = clamp(self.u_metallic_factor * mr_mix.z, 0.0, 1.0);

            let a = rough * rough;
            let a2 = a * a;
            let denom = ndoth * ndoth * (a2 - 1.0) + 1.0;
            let d = a2 / max(3.14159265 * denom * denom, 0.0001);

            let k0 = rough + 1.0;
            let k = (k0 * k0) / 8.0;
            let g_v = ndotv / max(ndotv * (1.0 - k) + k, 0.0001);
            let g_l = ndotl / max(ndotl * (1.0 - k) + k, 0.0001);
            let g = g_v * g_l;

            let f0 = mix(vec3(0.04, 0.04, 0.04), albedo.xyz, metal);
            let fresnel = pow(1.0 - vdoth, 5.0);
            let f = f0 + (vec3(1.0, 1.0, 1.0) - f0) * fresnel;

            let spec = (d * g) / max(4.0 * ndotv * ndotl, 0.0001);
            let specular = f * spec * self.u_spec_strength;

            let kd = (vec3(1.0, 1.0, 1.0) - f) * (1.0 - metal);
            let diffuse = kd * albedo.xyz * (1.0 / 3.14159265);

            let occlusion_tex = self.occlusion_texture.sample_as_bgra(self.v_uv);
            let occ_val = mix(1.0, occlusion_tex.x, clamp(self.u_occlusion_strength, 0.0, 1.0));
            let occlusion = mix(1.0, occ_val, clamp(self.u_has_occlusion_texture, 0.0, 1.0));

            let ndotv_env = clamp(dot(n, v), 0.0, 1.0);
            let refl = normalize(n * (2.0 * ndotv_env) - v);
            let refl_rough = normalize(mix(refl, n, rough * rough));
            let env_has = clamp(self.u_has_env_texture, 0.0, 1.0);

            let env_t_spec = clamp(refl_rough.y * 0.5 + 0.5, 0.0, 1.0);
            let env_t_diff = clamp(n.y * 0.5 + 0.5, 0.0, 1.0);
            let env_low = vec3(0.03, 0.035, 0.045);
            let env_high = vec3(0.36, 0.43, 0.5);
            let env_fallback_spec = mix(env_low, env_high, env_t_spec);
            let env_fallback_diff = mix(env_low, env_high, env_t_diff);

            let env_spec_tex = self.env_texture.sample_as_bgra(refl_rough).xyz;
            let env_diff_tex = self.env_texture.sample_as_bgra(n).xyz;
            let env_spec_color = mix(env_fallback_spec, env_spec_tex, env_has);
            let env_diff_color = mix(env_fallback_diff, env_diff_tex, env_has);

            let c0 = vec4(-1.0, -0.0275, -0.572, 0.022);
            let c1 = vec4(1.0, 0.0425, 1.04, -0.04);
            let r = c0 * rough + c1;
            let a004 = min(r.x * r.x, pow(2.0, -9.28 * ndotv)) * r.x + r.y;
            let env_brdf = vec2(-1.04, 1.04) * a004 + r.zw;
            let env_fresnel = f0 * env_brdf.x + vec3(env_brdf.y, env_brdf.y, env_brdf.y);

            let ibl_diffuse = kd * albedo.xyz * env_diff_color * self.u_env_intensity;
            let env_spec = env_spec_color * env_fresnel * self.u_spec_strength * self.u_env_intensity;

            let emissive_tex_srgb = self.emissive_texture.sample_as_bgra(self.v_uv);
            let emissive_tex = vec3(
                pow(max(emissive_tex_srgb.x, 0.0), 2.2),
                pow(max(emissive_tex_srgb.y, 0.0), 2.2),
                pow(max(emissive_tex_srgb.z, 0.0), 2.2)
            );
            let emissive_src = mix(
                vec3(1.0, 1.0, 1.0),
                emissive_tex,
                clamp(self.u_has_emissive_texture, 0.0, 1.0)
            );
            let emissive = self.u_emissive_factor * emissive_src;

            let lit = (diffuse + specular) * self.u_light_color * ndotl;
            let ambient = albedo.xyz * self.u_ambient;
            let indirect_diffuse = (ambient + ibl_diffuse) * occlusion;
            let indirect_spec = env_spec * mix(1.0, occlusion, 0.35);
            let color_linear = lit + indirect_diffuse + indirect_spec + emissive;

            let mapped = max(color_linear, vec3(0.0, 0.0, 0.0));
            let tone_num = mapped * (mapped * 2.51 + vec3(0.03, 0.03, 0.03));
            let tone_den = mapped * (mapped * 2.43 + vec3(0.59, 0.59, 0.59)) + vec3(0.14, 0.14, 0.14);
            let tone = tone_num / tone_den;

            let color = vec3(
                pow(max(tone.x, 0.0), 1.0 / 2.2),
                pow(max(tone.y, 0.0), 1.0 / 2.2),
                pow(max(tone.z, 0.0), 1.0 / 2.2)
            );
            return vec4(color.x, color.y, color.z, albedo.w)
        }
    }
}

#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawPbr {
    #[rust]
    pub many_instances: Option<ManyInstances>,
    #[rust]
    pub geometry: Option<Geometry>,
    #[rust]
    pub acc_verts: Vec<f32>,
    #[rust]
    pub acc_indices: Vec<u32>,
    #[rust]
    pub meshes: Vec<Geometry>,
    #[rust]
    default_env_texture: Option<Texture>,
    #[rust(Mat4f::identity())]
    pub cur_transform: Mat4f,
    #[rust(vec4(1.0, 1.0, 1.0, 1.0))]
    pub cur_color: Vec4f,
    #[deref]
    pub draw_vars: DrawVars,
    #[live]
    pub draw_clip: Vec4f,
    #[rust(vec4(1.0, 0.0, 0.0, 0.0))]
    pub model_c0: Vec4f,
    #[rust(vec4(0.0, 1.0, 0.0, 0.0))]
    pub model_c1: Vec4f,
    #[rust(vec4(0.0, 0.0, 1.0, 0.0))]
    pub model_c2: Vec4f,
    #[rust(vec4(0.0, 0.0, 0.0, 1.0))]
    pub model_c3: Vec4f,
    #[rust(vec4(1.0, 0.0, 0.0, 0.0))]
    pub view_c0: Vec4f,
    #[rust(vec4(0.0, 1.0, 0.0, 0.0))]
    pub view_c1: Vec4f,
    #[rust(vec4(0.0, 0.0, 1.0, 0.0))]
    pub view_c2: Vec4f,
    #[rust(vec4(0.0, 0.0, 0.0, 1.0))]
    pub view_c3: Vec4f,
    #[rust(vec4(1.0, 0.0, 0.0, 0.0))]
    pub proj_c0: Vec4f,
    #[rust(vec4(0.0, 1.0, 0.0, 0.0))]
    pub proj_c1: Vec4f,
    #[rust(vec4(0.0, 0.0, 1.0, 0.0))]
    pub proj_c2: Vec4f,
    #[rust(vec4(0.0, 0.0, 0.0, 1.0))]
    pub proj_c3: Vec4f,
    #[live(vec4(1.0, 1.0, 1.0, 1.0))]
    pub base_color_factor: Vec4f,
    #[live(1.0)]
    pub metallic_factor: f32,
    #[live(1.0)]
    pub roughness_factor: f32,
    #[live(vec3(0.0, 0.0, 0.0))]
    pub emissive_factor: Vec3f,
    #[live(1.0)]
    pub normal_scale: f32,
    #[live(1.0)]
    pub occlusion_strength: f32,
    #[live(0.0)]
    pub has_base_color_texture: f32,
    #[live(0.0)]
    pub has_metal_roughness_texture: f32,
    #[live(0.0)]
    pub has_normal_texture: f32,
    #[live(0.0)]
    pub has_occlusion_texture: f32,
    #[live(0.0)]
    pub has_emissive_texture: f32,
    #[live(0.0)]
    pub has_env_texture: f32,
    #[rust(vec4(-1.0, -1.0, 1.0, 1.0))]
    pub clip_ndc: Vec4f,
    #[rust(vec2(0.0, 1.0))]
    pub depth_range: Vec2f,
    /// Positive values move the 3D content forward in depth (towards 0.0).
    #[rust(0.0)]
    pub depth_forward_bias: f32,
    #[live(vec3(0.3, 0.7, 1.0))]
    pub light_dir: Vec3f,
    #[live(vec3(1.0, 1.0, 1.0))]
    pub light_color: Vec3f,
    #[live(0.15)]
    pub ambient: f32,
    #[live(128.0)]
    pub spec_power: f32,
    #[live(0.9)]
    pub spec_strength: f32,
    #[live(1.8)]
    pub env_intensity: f32,
    #[live(vec3(0.0, 0.0, 5.0))]
    pub camera_pos: Vec3f,
    #[live]
    pub pad1: f32,
}

impl DrawPbr {
    pub fn begin(&mut self) {
        self.acc_verts.clear();
        self.acc_indices.clear();
        self.set_transform(Mat4f::identity());
        self.cur_color = vec4(1.0, 1.0, 1.0, 1.0);
        self.base_color_factor = vec4(1.0, 1.0, 1.0, 1.0);
        self.metallic_factor = 1.0;
        self.roughness_factor = 1.0;
        self.emissive_factor = vec3(0.0, 0.0, 0.0);
        self.normal_scale = 1.0;
        self.occlusion_strength = 1.0;
        self.set_base_color_texture(None);
        self.set_metal_roughness_texture(None);
        self.set_normal_texture(None);
        self.set_occlusion_texture(None);
        self.set_emissive_texture(None);
        self.set_env_texture(None);
    }

    pub fn set_transform(&mut self, transform: Mat4f) {
        self.cur_transform = transform;
        self.model_c0 = vec4(transform.v[0], transform.v[1], transform.v[2], transform.v[3]);
        self.model_c1 = vec4(transform.v[4], transform.v[5], transform.v[6], transform.v[7]);
        self.model_c2 = vec4(transform.v[8], transform.v[9], transform.v[10], transform.v[11]);
        self.model_c3 = vec4(transform.v[12], transform.v[13], transform.v[14], transform.v[15]);
    }

    pub fn set_view_projection(&mut self, view: Mat4f, projection: Mat4f) {
        self.view_c0 = vec4(view.v[0], view.v[1], view.v[2], view.v[3]);
        self.view_c1 = vec4(view.v[4], view.v[5], view.v[6], view.v[7]);
        self.view_c2 = vec4(view.v[8], view.v[9], view.v[10], view.v[11]);
        self.view_c3 = vec4(view.v[12], view.v[13], view.v[14], view.v[15]);

        self.proj_c0 = vec4(
            projection.v[0],
            projection.v[1],
            projection.v[2],
            projection.v[3],
        );
        self.proj_c1 = vec4(
            projection.v[4],
            projection.v[5],
            projection.v[6],
            projection.v[7],
        );
        self.proj_c2 = vec4(
            projection.v[8],
            projection.v[9],
            projection.v[10],
            projection.v[11],
        );
        self.proj_c3 = vec4(
            projection.v[12],
            projection.v[13],
            projection.v[14],
            projection.v[15],
        );
    }

    pub fn set_color(&mut self, color: Vec4f) {
        self.cur_color = color;
    }

    pub fn set_color_rgba(&mut self, r: f32, g: f32, b: f32, a: f32) {
        self.cur_color = vec4(r, g, b, a);
    }

    pub fn set_base_color_factor(&mut self, color: Vec4f) {
        self.base_color_factor = color;
    }

    pub fn set_base_color_factor_rgba(&mut self, r: f32, g: f32, b: f32, a: f32) {
        self.base_color_factor = vec4(r, g, b, a);
    }

    pub fn set_metal_roughness(&mut self, metallic: f32, roughness: f32) {
        self.metallic_factor = metallic;
        self.roughness_factor = roughness;
    }

    pub fn set_emissive_factor(&mut self, emissive: Vec3f) {
        self.emissive_factor = emissive;
    }

    pub fn set_normal_scale(&mut self, normal_scale: f32) {
        self.normal_scale = normal_scale;
    }

    pub fn set_occlusion_strength(&mut self, occlusion_strength: f32) {
        self.occlusion_strength = occlusion_strength;
    }

    pub fn set_base_color_texture(&mut self, texture: Option<Texture>) {
        self.has_base_color_texture = if texture.is_some() { 1.0 } else { 0.0 };
        self.draw_vars.texture_slots[0] = texture;
    }

    pub fn set_metal_roughness_texture(&mut self, texture: Option<Texture>) {
        self.has_metal_roughness_texture = if texture.is_some() { 1.0 } else { 0.0 };
        self.draw_vars.texture_slots[1] = texture;
    }

    pub fn set_normal_texture(&mut self, texture: Option<Texture>) {
        self.has_normal_texture = if texture.is_some() { 1.0 } else { 0.0 };
        self.draw_vars.texture_slots[2] = texture;
    }

    pub fn set_occlusion_texture(&mut self, texture: Option<Texture>) {
        self.has_occlusion_texture = if texture.is_some() { 1.0 } else { 0.0 };
        self.draw_vars.texture_slots[3] = texture;
    }

    pub fn set_emissive_texture(&mut self, texture: Option<Texture>) {
        self.has_emissive_texture = if texture.is_some() { 1.0 } else { 0.0 };
        self.draw_vars.texture_slots[4] = texture;
    }

    pub fn set_env_texture(&mut self, texture: Option<Texture>) {
        self.has_env_texture = if texture.is_some() { 1.0 } else { 0.0 };
        self.draw_vars.texture_slots[5] = texture;
    }

    pub fn set_clip_ndc(&mut self, clip_ndc: Vec4f) {
        self.clip_ndc = clip_ndc;
    }

    pub fn set_depth_range(&mut self, min_depth: f32, max_depth: f32) {
        self.depth_range = vec2(
            min_depth.min(max_depth).clamp(0.0, 1.0),
            max_depth.max(min_depth).clamp(0.0, 1.0),
        );
    }

    pub fn set_depth_forward_bias(&mut self, bias: f32) {
        self.depth_forward_bias = bias.clamp(0.0, 1.0);
    }

    fn apply_draw_uniforms(&mut self, cx: &mut Cx2d) {
        self.draw_vars.set_uniform(
            cx.cx,
            live_id!(model_c0),
            &[self.model_c0.x, self.model_c0.y, self.model_c0.z, self.model_c0.w],
        );
        self.draw_vars.set_uniform(
            cx.cx,
            live_id!(model_c1),
            &[self.model_c1.x, self.model_c1.y, self.model_c1.z, self.model_c1.w],
        );
        self.draw_vars.set_uniform(
            cx.cx,
            live_id!(model_c2),
            &[self.model_c2.x, self.model_c2.y, self.model_c2.z, self.model_c2.w],
        );
        self.draw_vars.set_uniform(
            cx.cx,
            live_id!(model_c3),
            &[self.model_c3.x, self.model_c3.y, self.model_c3.z, self.model_c3.w],
        );

        self.draw_vars.set_uniform(
            cx.cx,
            live_id!(view_c0),
            &[self.view_c0.x, self.view_c0.y, self.view_c0.z, self.view_c0.w],
        );
        self.draw_vars.set_uniform(
            cx.cx,
            live_id!(view_c1),
            &[self.view_c1.x, self.view_c1.y, self.view_c1.z, self.view_c1.w],
        );
        self.draw_vars.set_uniform(
            cx.cx,
            live_id!(view_c2),
            &[self.view_c2.x, self.view_c2.y, self.view_c2.z, self.view_c2.w],
        );
        self.draw_vars.set_uniform(
            cx.cx,
            live_id!(view_c3),
            &[self.view_c3.x, self.view_c3.y, self.view_c3.z, self.view_c3.w],
        );
        self.draw_vars.set_uniform(
            cx.cx,
            live_id!(proj_c0),
            &[self.proj_c0.x, self.proj_c0.y, self.proj_c0.z, self.proj_c0.w],
        );
        self.draw_vars.set_uniform(
            cx.cx,
            live_id!(proj_c1),
            &[self.proj_c1.x, self.proj_c1.y, self.proj_c1.z, self.proj_c1.w],
        );
        self.draw_vars.set_uniform(
            cx.cx,
            live_id!(proj_c2),
            &[self.proj_c2.x, self.proj_c2.y, self.proj_c2.z, self.proj_c2.w],
        );
        self.draw_vars.set_uniform(
            cx.cx,
            live_id!(proj_c3),
            &[self.proj_c3.x, self.proj_c3.y, self.proj_c3.z, self.proj_c3.w],
        );
        self.draw_vars.set_uniform(
            cx.cx,
            live_id!(clip_ndc),
            &[
                self.clip_ndc.x,
                self.clip_ndc.y,
                self.clip_ndc.z,
                self.clip_ndc.w,
            ],
        );
        self.draw_vars.set_uniform(
            cx.cx,
            live_id!(depth_range),
            &[self.depth_range.x, self.depth_range.y],
        );
        self.draw_vars.set_uniform(
            cx.cx,
            live_id!(depth_forward_bias),
            &[self.depth_forward_bias],
        );
        self.draw_vars.set_uniform(
            cx.cx,
            live_id!(u_base_color_factor),
            &[
                self.base_color_factor.x,
                self.base_color_factor.y,
                self.base_color_factor.z,
                self.base_color_factor.w,
            ],
        );
        self.draw_vars
            .set_uniform(cx.cx, live_id!(u_metallic_factor), &[self.metallic_factor]);
        self.draw_vars
            .set_uniform(cx.cx, live_id!(u_roughness_factor), &[self.roughness_factor]);
        self.draw_vars.set_uniform(
            cx.cx,
            live_id!(u_emissive_factor),
            &[
                self.emissive_factor.x,
                self.emissive_factor.y,
                self.emissive_factor.z,
            ],
        );
        self.draw_vars
            .set_uniform(cx.cx, live_id!(u_normal_scale), &[self.normal_scale]);
        self.draw_vars.set_uniform(
            cx.cx,
            live_id!(u_occlusion_strength),
            &[self.occlusion_strength],
        );
        self.draw_vars.set_uniform(
            cx.cx,
            live_id!(u_has_base_color_texture),
            &[self.has_base_color_texture],
        );
        self.draw_vars.set_uniform(
            cx.cx,
            live_id!(u_has_metal_roughness_texture),
            &[self.has_metal_roughness_texture],
        );
        self.draw_vars.set_uniform(
            cx.cx,
            live_id!(u_has_normal_texture),
            &[self.has_normal_texture],
        );
        self.draw_vars.set_uniform(
            cx.cx,
            live_id!(u_has_occlusion_texture),
            &[self.has_occlusion_texture],
        );
        self.draw_vars.set_uniform(
            cx.cx,
            live_id!(u_has_emissive_texture),
            &[self.has_emissive_texture],
        );
        self.draw_vars.set_uniform(
            cx.cx,
            live_id!(u_has_env_texture),
            &[self.has_env_texture],
        );
        self.draw_vars.set_uniform(
            cx.cx,
            live_id!(u_light_dir),
            &[self.light_dir.x, self.light_dir.y, self.light_dir.z],
        );
        self.draw_vars.set_uniform(
            cx.cx,
            live_id!(u_light_color),
            &[self.light_color.x, self.light_color.y, self.light_color.z],
        );
        self.draw_vars
            .set_uniform(cx.cx, live_id!(u_ambient), &[self.ambient]);
        self.draw_vars
            .set_uniform(cx.cx, live_id!(u_spec_strength), &[self.spec_strength]);
        self.draw_vars
            .set_uniform(cx.cx, live_id!(u_env_intensity), &[self.env_intensity]);
        self.draw_vars.set_uniform(
            cx.cx,
            live_id!(u_camera_pos),
            &[self.camera_pos.x, self.camera_pos.y, self.camera_pos.z],
        );
    }

    pub fn add_decoded_primitive(&mut self, primitive: &DecodedPrimitive) -> Result<(), String> {
        self.add_indexed_triangles(
            &primitive.positions,
            primitive.normals.as_deref(),
            primitive.tangents.as_deref(),
            primitive.texcoords0.as_deref(),
            None,
            &primitive.indices,
        )
    }

    pub fn add_indexed_triangles(
        &mut self,
        positions: &[[f32; 3]],
        normals: Option<&[[f32; 3]]>,
        tangents: Option<&[[f32; 4]]>,
        uvs: Option<&[[f32; 2]]>,
        colors: Option<&[[f32; 4]]>,
        indices: &[u32],
    ) -> Result<(), String> {
        if positions.is_empty() || indices.is_empty() {
            return Ok(());
        }
        let (verts, inds) = self.build_vertex_data(
            positions,
            normals,
            tangents,
            uvs,
            colors,
            indices,
        )?;
        let base_index = (self.acc_verts.len() / PBR_FLOATS_PER_VERTEX) as u32;
        self.acc_verts.extend_from_slice(&verts);
        self.acc_indices
            .extend(inds.iter().map(|i| base_index + *i));
        Ok(())
    }

    /// Upload one mesh to GPU geometry and return a reusable mesh handle.
    /// Mesh data stays in local/object space and can be reused across draws.
    pub fn upload_indexed_triangles_mesh(
        &mut self,
        cx: &mut Cx2d,
        positions: &[[f32; 3]],
        normals: Option<&[[f32; 3]]>,
        tangents: Option<&[[f32; 4]]>,
        uvs: Option<&[[f32; 2]]>,
        colors: Option<&[[f32; 4]]>,
        indices: &[u32],
    ) -> Result<PbrMeshHandle, String> {
        let (verts, inds) = self.build_vertex_data(positions, normals, tangents, uvs, colors, indices)?;
        let geom = Geometry::new(cx.cx.cx);
        geom.update(cx.cx.cx, inds, verts);
        self.meshes.push(geom);
        Ok(self.meshes.len() - 1)
    }

    pub fn upload_decoded_primitive_mesh(
        &mut self,
        cx: &mut Cx2d,
        primitive: &DecodedPrimitive,
    ) -> Result<PbrMeshHandle, String> {
        self.upload_indexed_triangles_mesh(
            cx,
            &primitive.positions,
            primitive.normals.as_deref(),
            primitive.tangents.as_deref(),
            primitive.texcoords0.as_deref(),
            None,
            &primitive.indices,
        )
    }

    pub fn clear_meshes(&mut self) {
        self.meshes.clear();
    }

    pub fn load_default_env_equirect_from_path(
        &mut self,
        cx: &mut Cx2d,
        path: impl AsRef<Path>,
    ) -> Result<(), String> {
        let path = path.as_ref();
        let bytes = std::fs::read(path)
            .map_err(|err| format!("failed reading env map {}: {err}", path.display()))?;
        self.load_default_env_equirect_from_bytes(cx, &bytes, Some(path))
    }

    pub fn load_default_env_equirect_from_bytes(
        &mut self,
        cx: &mut Cx2d,
        bytes: &[u8],
        path_hint: Option<&Path>,
    ) -> Result<(), String> {
        let image = Self::decode_env_equirect(bytes, path_hint)?;
        let size = 512usize;
        let data = Self::build_env_cube_from_equirect(&image, size);
        let texture = Texture::new_with_format(
            cx.cx,
            TextureFormat::VecCubeBGRAu8_32 {
                width: size,
                height: size,
                data: Some(data),
                updated: TextureUpdated::Full,
            },
        );
        self.default_env_texture = Some(texture);
        Ok(())
    }

    fn decode_env_equirect(bytes: &[u8], path_hint: Option<&Path>) -> Result<ImageBuffer, String> {
        let ext = path_hint
            .and_then(|path| path.extension())
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.to_ascii_lowercase());
        match ext.as_deref() {
            Some("jpg") | Some("jpeg") => {
                ImageBuffer::from_jpg(bytes).map_err(|err| format!("jpg decode failed: {err}"))
            }
            Some("png") => {
                ImageBuffer::from_png(bytes).map_err(|err| format!("png decode failed: {err}"))
            }
            _ => {
                if bytes.starts_with(&[0xFF, 0xD8]) {
                    ImageBuffer::from_jpg(bytes).map_err(|err| format!("jpg decode failed: {err}"))
                } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
                    ImageBuffer::from_png(bytes).map_err(|err| format!("png decode failed: {err}"))
                } else {
                    let source = path_hint
                        .map(|path| path.display().to_string())
                        .unwrap_or_else(|| "<memory>".to_string());
                    Err(format!("unsupported env map format for {source}"))
                }
            }
        }
    }

    pub fn default_env_texture(&mut self, cx: &mut Cx2d) -> Texture {
        if let Some(texture) = self.default_env_texture.clone() {
            return texture;
        }

        let size = 256usize;
        let data = Self::build_default_env_cube_data(size);
        let texture = Texture::new_with_format(
            cx.cx,
            TextureFormat::VecCubeBGRAu8_32 {
                width: size,
                height: size,
                data: Some(data),
                updated: TextureUpdated::Full,
            },
        );
        self.default_env_texture = Some(texture.clone());
        texture
    }

    fn build_default_env_cube_data(size: usize) -> Vec<u32> {
        let mut out = vec![0u32; size.saturating_mul(size).saturating_mul(6)];
        for face in 0..6usize {
            for y in 0..size {
                for x in 0..size {
                    let u = ((x as f32 + 0.5) / size as f32) * 2.0 - 1.0;
                    let v = ((y as f32 + 0.5) / size as f32) * 2.0 - 1.0;
                    let d = match face {
                        0 => vec3(1.0, -v, -u),   // +X
                        1 => vec3(-1.0, -v, u),   // -X
                        2 => vec3(u, 1.0, v),     // +Y
                        3 => vec3(u, -1.0, -v),   // -Y
                        4 => vec3(u, -v, 1.0),    // +Z
                        _ => vec3(-u, -v, -1.0), // -Z
                    }
                    .normalize();

                    let sky_t = (d.y * 0.5 + 0.5).clamp(0.0, 1.0);
                    let ground = vec3(0.06, 0.065, 0.07);
                    let sky = vec3(0.27, 0.36, 0.46);
                    let mut color = ground + (sky - ground) * sky_t;

                    let horizon = (1.0 - d.y.abs()).powf(2.5) * 0.16;
                    color += vec3(horizon * 0.95, horizon * 0.85, horizon * 0.7);

                    let sun_dir = vec3(0.22, 0.72, 0.66).normalize();
                    let sun_dot = d.dot(sun_dir).max(0.0);
                    let sun_core = sun_dot.powf(96.0) * 1.1;
                    let sun_glow = sun_dot.powf(16.0) * 0.35;
                    let sun = sun_core + sun_glow;
                    color += vec3(sun * 1.0, sun * 0.96, sun * 0.88);

                    let idx = face
                        .saturating_mul(size)
                        .saturating_mul(size)
                        .saturating_add(y.saturating_mul(size))
                        .saturating_add(x);
                    out[idx] = Self::pack_bgra_u32(color.x, color.y, color.z, 1.0);
                }
            }
        }
        out
    }

    fn build_env_cube_from_equirect(image: &ImageBuffer, size: usize) -> Vec<u32> {
        let mut out = vec![0u32; size.saturating_mul(size).saturating_mul(6)];
        for face in 0..6usize {
            for y in 0..size {
                for x in 0..size {
                    let u = ((x as f32 + 0.5) / size as f32) * 2.0 - 1.0;
                    let v = ((y as f32 + 0.5) / size as f32) * 2.0 - 1.0;
                    let d = match face {
                        0 => vec3(1.0, -v, -u),   // +X
                        1 => vec3(-1.0, -v, u),   // -X
                        2 => vec3(u, 1.0, v),     // +Y
                        3 => vec3(u, -1.0, -v),   // -Y
                        4 => vec3(u, -v, 1.0),    // +Z
                        _ => vec3(-u, -v, -1.0), // -Z
                    }
                    .normalize();

                    let mut lon = d.z.atan2(d.x);
                    if lon < -PI {
                        lon += 2.0 * PI;
                    } else if lon > PI {
                        lon -= 2.0 * PI;
                    }
                    let lat = d.y.clamp(-1.0, 1.0).asin();
                    let uv = vec2(0.5 + lon / (2.0 * PI), 0.5 - lat / PI);
                    let color = Self::sample_equirect_linear(image, uv.x, uv.y);
                    let idx = face
                        .saturating_mul(size)
                        .saturating_mul(size)
                        .saturating_add(y.saturating_mul(size))
                        .saturating_add(x);
                    out[idx] = Self::pack_bgra_u32(color.x, color.y, color.z, 1.0);
                }
            }
        }
        out
    }

    fn sample_equirect_linear(image: &ImageBuffer, u: f32, v: f32) -> Vec3f {
        let width = image.width.max(1);
        let height = image.height.max(1);
        let uf = u.rem_euclid(1.0);
        let vf = v.clamp(0.0, 1.0);

        let x = uf * (width as f32 - 1.0);
        let y = vf * (height as f32 - 1.0);
        let x0 = x.floor() as usize;
        let y0 = y.floor() as usize;
        let x1 = (x0 + 1) % width;
        let y1 = (y0 + 1).min(height - 1);
        let tx = x - x0 as f32;
        let ty = y - y0 as f32;

        let c00 = Self::decode_pixel_linear(image.data[y0 * width + x0]);
        let c10 = Self::decode_pixel_linear(image.data[y0 * width + x1]);
        let c01 = Self::decode_pixel_linear(image.data[y1 * width + x0]);
        let c11 = Self::decode_pixel_linear(image.data[y1 * width + x1]);

        let cx0 = c00 + (c10 - c00) * tx;
        let cx1 = c01 + (c11 - c01) * tx;
        cx0 + (cx1 - cx0) * ty
    }

    fn decode_pixel_linear(packed: u32) -> Vec3f {
        let r = ((packed >> 16) & 0xff) as f32 / 255.0;
        let g = ((packed >> 8) & 0xff) as f32 / 255.0;
        let b = (packed & 0xff) as f32 / 255.0;
        vec3(r.powf(2.2), g.powf(2.2), b.powf(2.2))
    }

    fn pack_bgra_u32(r: f32, g: f32, b: f32, a: f32) -> u32 {
        let r = (r.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
        let g = (g.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
        let b = (b.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
        let a = (a.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
        (a << 24) | (r << 16) | (g << 8) | b
    }

    pub fn draw_mesh(&mut self, cx: &mut Cx2d, mesh: PbrMeshHandle) -> Result<(), String> {
        let geom = self
            .meshes
            .get(mesh)
            .ok_or_else(|| format!("invalid mesh handle {mesh}"))?;
        self.draw_vars.geometry_id = Some(geom.geometry_id());
        self.apply_draw_uniforms(cx);
        if cx.new_draw_call(&self.draw_vars).is_none() {
            return Err("DrawPbr draw call failed (shader not initialized)".to_string());
        }
        if self.draw_vars.can_instance() {
            let new_area = cx.add_aligned_instance(&self.draw_vars);
            self.draw_vars.area = cx.update_area_refs(self.draw_vars.area, new_area);
        }
        Ok(())
    }

    pub fn end(&mut self, cx: &mut Cx2d) {
        self.flush(cx);
    }

    /// Submit currently accumulated geometry as one draw call and clear buffers.
    /// Useful when emitting one draw call per primitive/material.
    pub fn flush(&mut self, cx: &mut Cx2d) {
        if self.acc_verts.is_empty() || self.acc_indices.is_empty() {
            return;
        }
        let verts = std::mem::take(&mut self.acc_verts);
        let indices = std::mem::take(&mut self.acc_indices);
        let geom = self.geometry.get_or_insert_with(|| Geometry::new(cx.cx.cx));
        geom.update(cx.cx.cx, indices, verts);
        self.draw_vars.geometry_id = Some(geom.geometry_id());
        self.apply_draw_uniforms(cx);
        cx.new_draw_call(&self.draw_vars);
        if self.draw_vars.can_instance() {
            let new_area = cx.add_aligned_instance(&self.draw_vars);
            self.draw_vars.area = cx.update_area_refs(self.draw_vars.area, new_area);
        }
    }

    /// Convenience: walk_turtle, begin, call draw_fn, end
    pub fn draw_walk(
        &mut self,
        cx: &mut Cx2d,
        walk: Walk,
        draw_fn: impl FnOnce(&mut Self, Rect),
    ) -> Rect {
        let rect = cx.walk_turtle(walk);
        self.begin();
        draw_fn(self, rect);
        self.end(cx);
        rect
    }

    fn build_vertex_data(
        &self,
        positions: &[[f32; 3]],
        normals: Option<&[[f32; 3]]>,
        tangents: Option<&[[f32; 4]]>,
        uvs: Option<&[[f32; 2]]>,
        colors: Option<&[[f32; 4]]>,
        indices: &[u32],
    ) -> Result<(Vec<f32>, Vec<u32>), String> {
        if let Some(normals) = normals {
            if normals.len() != positions.len() {
                return Err(format!(
                    "normal count {} does not match position count {}",
                    normals.len(),
                    positions.len()
                ));
            }
        }
        if let Some(uvs) = uvs {
            if uvs.len() != positions.len() {
                return Err(format!(
                    "uv count {} does not match position count {}",
                    uvs.len(),
                    positions.len()
                ));
            }
        }
        if let Some(tangents) = tangents {
            if tangents.len() != positions.len() {
                return Err(format!(
                    "tangent count {} does not match position count {}",
                    tangents.len(),
                    positions.len()
                ));
            }
        }
        if let Some(colors) = colors {
            if colors.len() != positions.len() {
                return Err(format!(
                    "color count {} does not match position count {}",
                    colors.len(),
                    positions.len()
                ));
            }
        }

        let mut out_indices = Vec::with_capacity(indices.len());
        for &index in indices {
            if index as usize >= positions.len() {
                return Err(format!(
                    "index {} is out of bounds for {} positions",
                    index,
                    positions.len()
                ));
            }
            out_indices.push(index);
        }

        let generated_tangents;
        let tangent_data = if let Some(tangents) = tangents {
            tangents
        } else {
            generated_tangents =
                Self::compute_tangent_frame(positions, normals, uvs, &out_indices);
            generated_tangents.as_slice()
        };

        let mut out_verts = Vec::with_capacity(positions.len() * PBR_FLOATS_PER_VERTEX);
        for (i, pos) in positions.iter().enumerate() {
            let src_n = normals
                .and_then(|n| n.get(i))
                .copied()
                .unwrap_or([0.0, 0.0, 1.0]);
            let src_t = tangent_data.get(i).copied().unwrap_or([1.0, 0.0, 0.0, 1.0]);

            let uv = uvs.and_then(|t| t.get(i)).copied().unwrap_or([0.0, 0.0]);
            let color = colors.and_then(|c| c.get(i)).copied().unwrap_or([
                self.cur_color.x,
                self.cur_color.y,
                self.cur_color.z,
                self.cur_color.w,
            ]);

            out_verts.extend_from_slice(&[
                pos[0],
                pos[1],
                pos[2],
                src_n[0],
                src_n[1],
                src_n[2],
                uv[0],
                uv[1],
                color[0],
                color[1],
                color[2],
                color[3],
                src_t[0],
                src_t[1],
                src_t[2],
                src_t[3],
            ]);
        }
        Ok((out_verts, out_indices))
    }

    fn compute_tangent_frame(
        positions: &[[f32; 3]],
        normals: Option<&[[f32; 3]]>,
        uvs: Option<&[[f32; 2]]>,
        indices: &[u32],
    ) -> Vec<[f32; 4]> {
        let count = positions.len();
        if count == 0 {
            return Vec::new();
        }
        let Some(uvs) = uvs else {
            return vec![[1.0, 0.0, 0.0, 1.0]; count];
        };

        let mut tan1 = vec![vec3(0.0, 0.0, 0.0); count];
        let mut tan2 = vec![vec3(0.0, 0.0, 0.0); count];

        for tri in indices.chunks_exact(3) {
            let i0 = tri[0] as usize;
            let i1 = tri[1] as usize;
            let i2 = tri[2] as usize;

            let p0 = vec3(positions[i0][0], positions[i0][1], positions[i0][2]);
            let p1 = vec3(positions[i1][0], positions[i1][1], positions[i1][2]);
            let p2 = vec3(positions[i2][0], positions[i2][1], positions[i2][2]);

            let uv0 = vec2(uvs[i0][0], uvs[i0][1]);
            let uv1 = vec2(uvs[i1][0], uvs[i1][1]);
            let uv2 = vec2(uvs[i2][0], uvs[i2][1]);

            let e1 = p1 - p0;
            let e2 = p2 - p0;
            let duv1 = uv1 - uv0;
            let duv2 = uv2 - uv0;

            let denom = duv1.x * duv2.y - duv2.x * duv1.y;
            if denom.abs() < 0.000_000_1 {
                continue;
            }
            let inv = 1.0 / denom;
            let sdir = (e1 * duv2.y - e2 * duv1.y) * inv;
            let tdir = (e2 * duv1.x - e1 * duv2.x) * inv;

            tan1[i0] += sdir;
            tan1[i1] += sdir;
            tan1[i2] += sdir;
            tan2[i0] += tdir;
            tan2[i1] += tdir;
            tan2[i2] += tdir;
        }

        let mut out = vec![[1.0, 0.0, 0.0, 1.0]; count];
        for i in 0..count {
            let n = normals
                .and_then(|all| all.get(i))
                .map(|n| vec3(n[0], n[1], n[2]).normalize())
                .unwrap_or(vec3(0.0, 0.0, 1.0));
            let t = tan1[i];

            let t_ortho = t - n * n.dot(t);
            let tangent = if t_ortho.length() > 0.000_000_1 {
                t_ortho.normalize()
            } else {
                let up = if n.y.abs() > 0.99 {
                    vec3(1.0, 0.0, 0.0)
                } else {
                    vec3(0.0, 1.0, 0.0)
                };
                Vec3f::cross(n, up).normalize()
            };
            let bitangent = Vec3f::cross(n, tangent);
            let w = if bitangent.dot(tan2[i]) < 0.0 { -1.0 } else { 1.0 };
            out[i] = [tangent.x, tangent.y, tangent.z, w];
        }
        out
    }
}
