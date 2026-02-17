use crate::{
    cx_2d::*,
    draw_list_2d::ManyInstances,
    geometry::geometry_gen::GeometryGen,
    image_cache::ImageBuffer,
    makepad_platform::*,
    turtle::*,
};
use makepad_gltf::DecodedPrimitive;
use std::{collections::HashMap, f32::consts::PI, path::Path};

const PBR_FLOATS_PER_VERTEX: usize = 16;

pub type PbrMeshHandle = usize;

#[derive(Clone, Debug, Default)]
pub struct DrawPbrTextureSet {
    pub base_color: Option<Texture>,
    pub metallic_roughness: Option<Texture>,
    pub normal: Option<Texture>,
    pub occlusion: Option<Texture>,
    pub emissive: Option<Texture>,
    pub env: Option<Texture>,
}

#[derive(Clone, Debug)]
pub struct DrawPbrMaterialState {
    pub base_color_factor: Vec4f,
    pub metallic_factor: f32,
    pub roughness_factor: f32,
    pub emissive_factor: Vec3f,
    pub normal_scale: f32,
    pub occlusion_strength: f32,
    pub textures: DrawPbrTextureSet,
}

impl Default for DrawPbrMaterialState {
    fn default() -> Self {
        Self {
            base_color_factor: vec4(1.0, 1.0, 1.0, 1.0),
            metallic_factor: 1.0,
            roughness_factor: 1.0,
            emissive_factor: vec3(0.0, 0.0, 0.0),
            normal_scale: 1.0,
            occlusion_strength: 1.0,
            textures: DrawPbrTextureSet::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum PbrPrimitiveMeshKey {
    Cube { segments: u16 },
    Surface { seg_u: u16, seg_v: u16 },
    Sphere { lat: u16, lon: u16 },
}

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
        view_matrix: uniform(mat4x4f(
            1.0, 0.0, 0.0, 0.0,
            0.0, 1.0, 0.0, 0.0,
            0.0, 0.0, 1.0, 0.0,
            0.0, 0.0, 0.0, 1.0
        ))
        projection_matrix: uniform(mat4x4f(
            1.0, 0.0, 0.0, 0.0,
            0.0, 1.0, 0.0, 0.0,
            0.0, 0.0, 1.0, 0.0,
            0.0, 0.0, 0.0, 1.0
        ))
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

        get_vertex_displacement: fn(uv: vec2, local_pos: vec3) {
            return vec3(0.0, 0.0, 0.0)
        }
        
        vertex: fn() {
            let local_uv = vec2(self.geom.ny_nz_uv.z, self.geom.ny_nz_uv.w);
            let local_pos_src = vec3(self.geom.pos_nx.x, self.geom.pos_nx.y, self.geom.pos_nx.z);
            let displacement = self.get_vertex_displacement(local_uv, local_pos_src);
            let local_pos = vec4(
                local_pos_src.x + displacement.x,
                local_pos_src.y + displacement.y,
                local_pos_src.z + displacement.z,
                1.0
            );
            let local_n = vec4(self.geom.pos_nx.w, self.geom.ny_nz_uv.x, self.geom.ny_nz_uv.y, 0.0);

            let model_pos = self.model_matrix * local_pos;
            let model_n = self.model_matrix * local_n;
            let local_t = vec4(self.geom.tangent.x, self.geom.tangent.y, self.geom.tangent.z, 0.0);
            let model_t = self.model_matrix * local_t;

            self.v_world = vec3(model_pos.x, model_pos.y, model_pos.z);
            self.v_normal = vec3(model_n.x, model_n.y, model_n.z);
            self.v_tangent = vec4(model_t.x, model_t.y, model_t.z, self.geom.tangent.w);
            self.v_uv = local_uv;
            self.v_color = self.geom.color;

            let world = vec4(model_pos.x, model_pos.y, model_pos.z + self.draw_call.zbias, 1.0);
            self.vertex_pos = self.projection_matrix * (self.view_matrix * world);
        }

        get_base_color: fn(uv: vec2, vertex_color: vec4) {
            let base = self.u_base_color_factor * vertex_color;
            let tex_srgb = self.base_color_texture.sample_as_bgra(uv);
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
            return base * tex_mix
        }

        get_metal_roughness: fn(uv: vec2) {
            let mr_tex = self.metallic_roughness_texture.sample_as_bgra(uv);
            let mr_mix = mix(
                vec4(1.0, 1.0, 1.0, 1.0),
                mr_tex,
                clamp(self.u_has_metal_roughness_texture, 0.0, 1.0)
            );
            return vec2(
                clamp(self.u_metallic_factor * mr_mix.z, 0.0, 1.0),
                clamp(self.u_roughness_factor * mr_mix.y, 0.045, 1.0)
            )
        }

        get_normal_tangent: fn(uv: vec2) {
            let n_tex_s = self.normal_texture.sample_as_bgra(uv);
            return vec3(
                n_tex_s.x * 2.0 - 1.0,
                (n_tex_s.y * 2.0 - 1.0) * self.u_normal_scale,
                n_tex_s.z * 2.0 - 1.0
            )
        }

        get_occlusion: fn(uv: vec2) {
            let occlusion_tex = self.occlusion_texture.sample_as_bgra(uv);
            let occ_val = mix(1.0, occlusion_tex.x, clamp(self.u_occlusion_strength, 0.0, 1.0));
            return mix(1.0, occ_val, clamp(self.u_has_occlusion_texture, 0.0, 1.0))
        }

        get_emissive: fn(uv: vec2) {
            let emissive_tex_srgb = self.emissive_texture.sample_as_bgra(uv);
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
            return self.u_emissive_factor * emissive_src
        }

        get_env_specular: fn(refl_dir: vec3) {
            let env_has = clamp(self.u_has_env_texture, 0.0, 1.0);
            let env_t_spec = clamp(refl_dir.y * 0.5 + 0.5, 0.0, 1.0);
            let env_low = vec3(0.03, 0.035, 0.045);
            let env_high = vec3(0.36, 0.43, 0.5);
            let env_fallback_spec = mix(env_low, env_high, env_t_spec);
            let env_spec_tex = self.env_texture.sample_as_bgra(refl_dir).xyz;
            return mix(env_fallback_spec, env_spec_tex, env_has)
        }

        get_env_diffuse: fn(normal_dir: vec3) {
            let env_has = clamp(self.u_has_env_texture, 0.0, 1.0);
            let env_t_diff = clamp(normal_dir.y * 0.5 + 0.5, 0.0, 1.0);
            let env_low = vec3(0.03, 0.035, 0.045);
            let env_high = vec3(0.36, 0.43, 0.5);
            let env_fallback_diff = mix(env_low, env_high, env_t_diff);
            let env_diff_tex = self.env_texture.sample_as_bgra(normal_dir).xyz;
            return mix(env_fallback_diff, env_diff_tex, env_has)
        }

        fragment: fn(){
            self.fb0 = self.pixel()
        }

        pixel: fn() {
            let uv = vec2(fract(self.v_uv.x), fract(self.v_uv.y));
            let albedo = self.get_base_color(uv, self.v_color);

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
            let n_tex = self.get_normal_tangent(uv);
            let n_tangent = normalize(t * n_tex.x + b * n_tex.y + n_geom * n_tex.z);
            let n = normalize(mix(n_geom, n_tangent, clamp(self.u_has_normal_texture, 0.0, 1.0)));

            let l = normalize(self.u_light_dir);
            let v = normalize(self.u_camera_pos - self.v_world);
            let h = normalize(l + v);
            let ndotl = max(dot(n, l), 0.0);
            let ndotv = max(dot(n, v), 0.0001);
            let ndoth = max(dot(n, h), 0.0001);
            let vdoth = max(dot(v, h), 0.0);

            let mr = self.get_metal_roughness(uv);
            let metal = mr.x;
            let rough = mr.y;

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

            let occlusion = self.get_occlusion(uv);

            let ndotv_env = clamp(dot(n, v), 0.0, 1.0);
            let refl = normalize(n * (2.0 * ndotv_env) - v);
            let refl_rough = normalize(mix(refl, n, rough * rough));

            let env_spec_color = self.get_env_specular(refl_rough);
            let env_diff_color = self.get_env_diffuse(n);

            let c0 = vec4(-1.0, -0.0275, -0.572, 0.022);
            let c1 = vec4(1.0, 0.0425, 1.04, -0.04);
            let r = c0 * rough + c1;
            let a004 = min(r.x * r.x, pow(2.0, -9.28 * ndotv)) * r.x + r.y;
            let env_brdf = vec2(-1.04, 1.04) * a004 + r.zw;
            let env_fresnel = f0 * env_brdf.x + vec3(env_brdf.y, env_brdf.y, env_brdf.y);

            let ibl_diffuse = kd * albedo.xyz * env_diff_color * self.u_env_intensity;
            let env_spec = env_spec_color * env_fresnel * self.u_spec_strength * self.u_env_intensity;
            let emissive = self.get_emissive(uv);

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
    primitive_mesh_cache: HashMap<PbrPrimitiveMeshKey, PbrMeshHandle>,
    #[rust]
    default_env_texture: Option<Texture>,
    #[rust(Mat4f::identity())]
    pub cur_transform: Mat4f,
    #[rust]
    pub transform_stack: Vec<Mat4f>,
    #[rust(vec4(1.0, 1.0, 1.0, 1.0))]
    pub cur_color: Vec4f,
    #[rust(Mat4f::identity())]
    pub view_matrix: Mat4f,
    #[rust(Mat4f::identity())]
    pub projection_matrix: Mat4f,
    #[rust(vec4(-1.0, -1.0, 1.0, 1.0))]
    pub clip_ndc: Vec4f,
    #[rust(vec2(0.0, 1.0))]
    pub depth_range: Vec2f,
    /// Positive values move the 3D content forward in depth (towards 0.0).
    #[rust(0.0)]
    pub depth_forward_bias: f32,
    #[rust(vec4(1.0, 1.0, 1.0, 1.0))]
    pub base_color_factor: Vec4f,
    #[rust(1.0)]
    pub metallic_factor: f32,
    #[rust(1.0)]
    pub roughness_factor: f32,
    #[rust(vec3(0.0, 0.0, 0.0))]
    pub emissive_factor: Vec3f,
    #[rust(1.0)]
    pub normal_scale: f32,
    #[rust(1.0)]
    pub occlusion_strength: f32,
    #[rust(0.0)]
    pub has_base_color_texture: f32,
    #[rust(0.0)]
    pub has_metal_roughness_texture: f32,
    #[rust(0.0)]
    pub has_normal_texture: f32,
    #[rust(0.0)]
    pub has_occlusion_texture: f32,
    #[rust(0.0)]
    pub has_emissive_texture: f32,
    #[rust(0.0)]
    pub has_env_texture: f32,
    #[rust(vec3(0.3, 0.7, 1.0))]
    pub light_dir: Vec3f,
    #[rust(vec3(1.0, 1.0, 1.0))]
    pub light_color: Vec3f,
    #[rust(0.15)]
    pub ambient: f32,
    #[rust(128.0)]
    pub spec_power: f32,
    #[rust(0.9)]
    pub spec_strength: f32,
    #[rust(1.8)]
    pub env_intensity: f32,
    #[rust(vec3(0.0, 0.0, 5.0))]
    pub camera_pos: Vec3f,
    #[rust(0.0)]
    pub pad1: f32,
    #[deref]
    pub draw_vars: DrawVars,
    #[live]
    pub model_matrix: Mat4f,
    #[live]
    pub draw_clip: Vec4f,
}

impl DrawPbr {
    pub fn begin(&mut self) {
        self.acc_verts.clear();
        self.acc_indices.clear();
        self.set_transform(Mat4f::identity());
        self.transform_stack.clear();
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
        self.model_matrix = transform;
    }

    /// Reset the p5-style model matrix stack state to identity.
    pub fn reset_matrix(&mut self) {
        self.set_transform(Mat4f::identity());
        self.transform_stack.clear();
    }

    /// Save current model matrix.
    pub fn push_matrix(&mut self) {
        self.transform_stack.push(self.cur_transform);
    }

    /// Restore last saved model matrix.
    pub fn pop_matrix(&mut self) {
        if let Some(transform) = self.transform_stack.pop() {
            self.set_transform(transform);
        } else {
            self.set_transform(Mat4f::identity());
        }
    }

    /// Post-multiply an additional transform onto the current model matrix.
    pub fn apply_transform(&mut self, transform: Mat4f) {
        self.set_transform(Mat4f::mul(&self.cur_transform, &transform));
    }

    pub fn translate_v(&mut self, offset: Vec3f) {
        self.apply_transform(Mat4f::translation(offset));
    }

    pub fn translate(&mut self, x: f32, y: f32, z: f32) {
        self.translate_v(vec3(x, y, z));
    }

    pub fn rotate_xyz(&mut self, x_rad: f32, y_rad: f32, z_rad: f32) {
        self.apply_transform(Mat4f::rotation(vec3(x_rad, y_rad, z_rad)));
    }

    pub fn rotate_x(&mut self, x_rad: f32) {
        self.rotate_xyz(x_rad, 0.0, 0.0);
    }

    pub fn rotate_y(&mut self, y_rad: f32) {
        self.rotate_xyz(0.0, y_rad, 0.0);
    }

    pub fn rotate_z(&mut self, z_rad: f32) {
        self.rotate_xyz(0.0, 0.0, z_rad);
    }

    pub fn scale(&mut self, uniform: f32) {
        self.apply_transform(Mat4f::scale(uniform));
    }

    pub fn scale_xyz(&mut self, x: f32, y: f32, z: f32) {
        self.apply_transform(Mat4f::nonuniform_scaled_translation(
            vec3(x, y, z),
            vec3(0.0, 0.0, 0.0),
        ));
    }

    /// p5-like material convenience: set base color + metallic/roughness.
    pub fn material(&mut self, base_color: Vec4f, metallic: f32, roughness: f32) {
        self.set_base_color_factor(base_color);
        self.set_metal_roughness(metallic, roughness);
    }

    pub fn material_rgba(
        &mut self,
        r: f32,
        g: f32,
        b: f32,
        a: f32,
        metallic: f32,
        roughness: f32,
    ) {
        self.material(vec4(r, g, b, a), metallic, roughness);
    }

    /// p5-like alias for base color (PBR albedo factor).
    pub fn fill(&mut self, color: Vec4f) {
        self.set_base_color_factor(color);
    }

    pub fn fill_rgba(&mut self, r: f32, g: f32, b: f32, a: f32) {
        self.fill(vec4(r, g, b, a));
    }

    pub fn set_view_projection(&mut self, view: Mat4f, projection: Mat4f) {
        self.view_matrix = view;
        self.projection_matrix = projection;
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

    pub fn set_camera_state(&mut self, view: Mat4f, projection: Mat4f, camera_pos: Vec3f) {
        self.view_matrix = view;
        self.projection_matrix = projection;
        self.camera_pos = camera_pos;
    }

    pub fn apply_material_state(&mut self, material: &DrawPbrMaterialState) {
        self.set_base_color_factor(material.base_color_factor);
        self.set_metal_roughness(material.metallic_factor, material.roughness_factor);
        self.set_emissive_factor(material.emissive_factor);
        self.set_normal_scale(material.normal_scale);
        self.set_occlusion_strength(material.occlusion_strength);
        self.set_base_color_texture(material.textures.base_color.clone());
        self.set_metal_roughness_texture(material.textures.metallic_roughness.clone());
        self.set_normal_texture(material.textures.normal.clone());
        self.set_occlusion_texture(material.textures.occlusion.clone());
        self.set_emissive_texture(material.textures.emissive.clone());
        self.set_env_texture(material.textures.env.clone());
    }

    fn apply_draw_uniforms(&mut self, cx: &mut Cx2d) {
        self.draw_vars.set_uniform(
            cx.cx,
            live_id!(view_matrix),
            &self.view_matrix.v,
        );
        self.draw_vars.set_uniform(
            cx.cx,
            live_id!(projection_matrix),
            &self.projection_matrix.v,
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
        self.primitive_mesh_cache.clear();
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

    pub fn draw_mesh_with_transform(
        &mut self,
        cx: &mut Cx2d,
        mesh: PbrMeshHandle,
        transform: Mat4f,
    ) -> Result<(), String> {
        let prev_model = self.model_matrix;
        self.model_matrix = transform;
        let result = self.draw_mesh(cx, mesh);
        self.model_matrix = prev_model;
        result
    }

    /// Draw a cube using the current material/shader state.
    /// Uses cached unit-cube meshes and applies size as a transform scale.
    pub fn draw_cube(
        &mut self,
        cx: &mut Cx2d,
        size: Vec3f,
        subdivisions: usize,
    ) -> Result<(), String> {
        let mesh = self.ensure_cube_mesh(cx, subdivisions)?;
        let scale =
            Mat4f::nonuniform_scaled_translation(size, vec3(0.0, 0.0, 0.0));
        let transform = Mat4f::mul(&self.cur_transform, &scale);
        self.draw_mesh_with_transform(cx, mesh, transform)
    }

    pub fn draw_cube_with_material(
        &mut self,
        cx: &mut Cx2d,
        size: Vec3f,
        subdivisions: usize,
        material: &DrawPbrMaterialState,
    ) -> Result<(), String> {
        self.apply_material_state(material);
        self.draw_cube(cx, size, subdivisions)
    }

    /// Draw an XZ surface patch (normal +Y) using current material/shader state.
    pub fn draw_surface(
        &mut self,
        cx: &mut Cx2d,
        size: Vec2f,
        seg_u: usize,
        seg_v: usize,
    ) -> Result<(), String> {
        let mesh = self.ensure_surface_mesh(cx, seg_u, seg_v)?;
        let scale = Mat4f::nonuniform_scaled_translation(
            vec3(size.x, 1.0, size.y),
            vec3(0.0, 0.0, 0.0),
        );
        let transform = Mat4f::mul(&self.cur_transform, &scale);
        self.draw_mesh_with_transform(cx, mesh, transform)
    }

    pub fn draw_surface_with_material(
        &mut self,
        cx: &mut Cx2d,
        size: Vec2f,
        seg_u: usize,
        seg_v: usize,
        material: &DrawPbrMaterialState,
    ) -> Result<(), String> {
        self.apply_material_state(material);
        self.draw_surface(cx, size, seg_u, seg_v)
    }

    /// Draw a UV sphere using current material/shader state.
    pub fn draw_sphere(
        &mut self,
        cx: &mut Cx2d,
        radius: f32,
        subdivisions: usize,
    ) -> Result<(), String> {
        let lat = subdivisions.max(4).min(96);
        let lon = (lat * 2).max(8).min(192);
        let mesh = self.ensure_sphere_mesh(cx, lat, lon)?;
        let scale = Mat4f::scaled_translation(
            radius.max(0.0001),
            vec3(0.0, 0.0, 0.0),
        );
        let transform = Mat4f::mul(&self.cur_transform, &scale);
        self.draw_mesh_with_transform(cx, mesh, transform)
    }

    pub fn draw_sphere_with_material(
        &mut self,
        cx: &mut Cx2d,
        radius: f32,
        subdivisions: usize,
        material: &DrawPbrMaterialState,
    ) -> Result<(), String> {
        self.apply_material_state(material);
        self.draw_sphere(cx, radius, subdivisions)
    }

    fn ensure_cube_mesh(
        &mut self,
        cx: &mut Cx2d,
        subdivisions: usize,
    ) -> Result<PbrMeshHandle, String> {
        let segments = subdivisions.max(1).min(64) as u16;
        let key = PbrPrimitiveMeshKey::Cube { segments };
        if let Some(handle) = self.primitive_mesh_cache.get(&key).copied() {
            return Ok(handle);
        }

        let gen = GeometryGen::from_cube_3d(
            1.0,
            1.0,
            1.0,
            segments as usize,
            segments as usize,
            segments as usize,
        );
        let (positions, normals, uvs, indices) = Self::geometry_gen_to_pbr(&gen)?;
        let handle = self.upload_indexed_triangles_mesh(
            cx,
            &positions,
            Some(&normals),
            None,
            Some(&uvs),
            None,
            &indices,
        )?;
        self.primitive_mesh_cache.insert(key, handle);
        Ok(handle)
    }

    fn ensure_surface_mesh(
        &mut self,
        cx: &mut Cx2d,
        seg_u: usize,
        seg_v: usize,
    ) -> Result<PbrMeshHandle, String> {
        let seg_u = seg_u.max(1).min(256) as u16;
        let seg_v = seg_v.max(1).min(256) as u16;
        let key = PbrPrimitiveMeshKey::Surface { seg_u, seg_v };
        if let Some(handle) = self.primitive_mesh_cache.get(&key).copied() {
            return Ok(handle);
        }

        let (positions, normals, uvs, indices) =
            Self::build_surface_mesh(seg_u as usize, seg_v as usize);
        let handle = self.upload_indexed_triangles_mesh(
            cx,
            &positions,
            Some(&normals),
            None,
            Some(&uvs),
            None,
            &indices,
        )?;
        self.primitive_mesh_cache.insert(key, handle);
        Ok(handle)
    }

    fn ensure_sphere_mesh(
        &mut self,
        cx: &mut Cx2d,
        lat: usize,
        lon: usize,
    ) -> Result<PbrMeshHandle, String> {
        let lat = lat.max(4).min(256) as u16;
        let lon = lon.max(8).min(512) as u16;
        let key = PbrPrimitiveMeshKey::Sphere { lat, lon };
        if let Some(handle) = self.primitive_mesh_cache.get(&key).copied() {
            return Ok(handle);
        }

        let (positions, normals, uvs, indices) =
            Self::build_uv_sphere_mesh(lat as usize, lon as usize);
        let handle = self.upload_indexed_triangles_mesh(
            cx,
            &positions,
            Some(&normals),
            None,
            Some(&uvs),
            None,
            &indices,
        )?;
        self.primitive_mesh_cache.insert(key, handle);
        Ok(handle)
    }

    fn geometry_gen_to_pbr(
        gen: &GeometryGen,
    ) -> Result<
        (
            Vec<[f32; 3]>,
            Vec<[f32; 3]>,
            Vec<[f32; 2]>,
            Vec<u32>,
        ),
        String,
    > {
        if gen.vertices.len() % 9 != 0 {
            return Err(format!(
                "expected GeometryGen vertex stride 9, got {} floats",
                gen.vertices.len()
            ));
        }
        let mut positions = Vec::with_capacity(gen.vertices.len() / 9);
        let mut normals = Vec::with_capacity(gen.vertices.len() / 9);
        let mut uvs = Vec::with_capacity(gen.vertices.len() / 9);

        for chunk in gen.vertices.chunks_exact(9) {
            positions.push([chunk[0], chunk[1], chunk[2]]);
            normals.push([chunk[4], chunk[5], chunk[6]]);
            uvs.push([chunk[7], chunk[8]]);
        }
        Ok((positions, normals, uvs, gen.indices.clone()))
    }

    fn build_surface_mesh(
        seg_u: usize,
        seg_v: usize,
    ) -> (
        Vec<[f32; 3]>,
        Vec<[f32; 3]>,
        Vec<[f32; 2]>,
        Vec<u32>,
    ) {
        let seg_u = seg_u.max(1);
        let seg_v = seg_v.max(1);
        let vert_count = (seg_u + 1) * (seg_v + 1);
        let mut positions = Vec::with_capacity(vert_count);
        let mut normals = Vec::with_capacity(vert_count);
        let mut uvs = Vec::with_capacity(vert_count);
        let mut indices = Vec::with_capacity(seg_u * seg_v * 6);

        for y in 0..=seg_v {
            let v = y as f32 / seg_v as f32;
            let pz = v - 0.5;
            for x in 0..=seg_u {
                let u = x as f32 / seg_u as f32;
                let px = u - 0.5;
                positions.push([px, 0.0, pz]);
                normals.push([0.0, 1.0, 0.0]);
                uvs.push([u, 1.0 - v]);
            }
        }

        let stride = seg_u + 1;
        for y in 0..seg_v {
            for x in 0..seg_u {
                let i0 = (y * stride + x) as u32;
                let i1 = i0 + 1;
                let i2 = i0 + stride as u32;
                let i3 = i2 + 1;
                indices.extend_from_slice(&[i0, i2, i1, i1, i2, i3]);
            }
        }

        (positions, normals, uvs, indices)
    }

    fn build_uv_sphere_mesh(
        lat: usize,
        lon: usize,
    ) -> (
        Vec<[f32; 3]>,
        Vec<[f32; 3]>,
        Vec<[f32; 2]>,
        Vec<u32>,
    ) {
        let lat = lat.max(4);
        let lon = lon.max(8);
        let mut positions = Vec::with_capacity((lat + 1) * (lon + 1));
        let mut normals = Vec::with_capacity((lat + 1) * (lon + 1));
        let mut uvs = Vec::with_capacity((lat + 1) * (lon + 1));
        let mut indices = Vec::with_capacity(lat * lon * 6);

        for y in 0..=lat {
            let v = y as f32 / lat as f32;
            let theta = v * PI;
            let sin_theta = theta.sin();
            let cos_theta = theta.cos();

            for x in 0..=lon {
                let u = x as f32 / lon as f32;
                let phi = u * 2.0 * PI;
                let sin_phi = phi.sin();
                let cos_phi = phi.cos();
                let px = sin_theta * cos_phi;
                let py = cos_theta;
                let pz = sin_theta * sin_phi;

                positions.push([px, py, pz]);
                normals.push([px, py, pz]);
                uvs.push([u, 1.0 - v]);
            }
        }

        let stride = lon + 1;
        for y in 0..lat {
            for x in 0..lon {
                let i0 = (y * stride + x) as u32;
                let i1 = i0 + 1;
                let i2 = i0 + stride as u32;
                let i3 = i2 + 1;

                if y != 0 {
                    indices.extend_from_slice(&[i0, i2, i1]);
                }
                if y != lat - 1 {
                    indices.extend_from_slice(&[i1, i2, i3]);
                }
            }
        }

        (positions, normals, uvs, indices)
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
