use crate::{
    cx_2d::Cx2d, cx_draw::CxDraw, draw_list_2d::ManyInstances, geometry::geometry_gen::GeometryGen,
    image_cache::ImageBuffer, makepad_platform::*, turtle::*,
};
use makepad_math::DecodedPrimitive;
use std::{collections::HashMap, f32::consts::PI, path::Path};

const PBR_FLOATS_PER_VERTEX: usize = 16;

pub type PbrMeshHandle = usize;
type PbrMeshBuffers = (Vec<[f32; 3]>, Vec<[f32; 3]>, Vec<[f32; 2]>, Vec<u32>);

#[derive(Clone, Debug, Default)]
pub struct DrawPbrTextureSet {
    pub base_color: Option<Texture>,
    pub metallic_roughness: Option<Texture>,
    pub normal: Option<Texture>,
    pub occlusion: Option<Texture>,
    pub emissive: Option<Texture>,
    pub env: Option<Texture>,
    pub env_atlas: Option<Texture>,
    pub env_faces: Option<[Texture; 6]>,
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
    Cube {
        segments: u16,
    },
    Capsule {
        lat: u16,
        lon: u16,
        half_height_permille: u16,
    },
    Surface {
        seg_u: u16,
        seg_v: u16,
    },
    Sphere {
        lat: u16,
        lon: u16,
    },
    RoundedCube {
        segments: u16,
        corner_segments: u16,
        radius_permille: u16,
    },
}

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw
    use mod.geom

    mod.draw.DrawPbr = mod.std.set_type_default() do #(DrawPbr::script_shader(vm)){
        backface_culling: true
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
        env_atlas_texture: texture_2d(float)
        env_pos_x_texture: texture_2d(float)
        env_neg_x_texture: texture_2d(float)
        env_pos_y_texture: texture_2d(float)
        env_neg_y_texture: texture_2d(float)
        env_pos_z_texture: texture_2d(float)
        env_neg_z_texture: texture_2d(float)
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
        u_has_env_atlas_texture: uniform(float(0.0))
        u_has_env_face_textures: uniform(float(0.0))
        u_enable_occlusion: uniform(float(0.0))
        u_enable_emissive: uniform(float(0.0))
        u_enable_direct_light: uniform(float(0.0))
        u_enable_brdf: uniform(float(0.0))
        u_enable_direct_specular: uniform(float(0.0))
        u_enable_ibl: uniform(float(0.0))
        u_enable_env_specular: uniform(float(0.0))
        u_light_dir: uniform(vec3(0.3, 0.7, 1.0))
        u_light_color: uniform(vec3(1.0, 1.0, 1.0))
        u_ambient: uniform(float(0.15))
        u_spec_strength: uniform(float(0.9))
        u_env_intensity: uniform(float(1.8))

        v_world_clip: varying(vec4f)
        v_world: varying(vec3f)
        v_view_pos: varying(vec3f)
        v_normal: varying(vec3f)
        v_tangent: varying(vec4f)
        v_uv: varying(vec2f)
        v_color: varying(vec4f)

        get_vertex_displacement: fn(uv: vec2, local_pos: vec3) {
            return vec3(0.0, 0.0, 0.0)
        }

        active_camera_world_pos: fn() -> vec3f {
            let camera_world = self.draw_pass.camera_inv * vec4(0.0, 0.0, 0.0, 1.0);
            return vec3(
                camera_world.x / max(camera_world.w, 0.00001),
                camera_world.y / max(camera_world.w, 0.00001),
                camera_world.z / max(camera_world.w, 0.00001)
            )
        }

        world_with_model_matrix: fn(local_pos: vec4) {
            let model_view = self.draw_list.view_transform * self.model_matrix;
            return model_view * local_pos
        }

        world_with_model_direction: fn(local_dir: vec4) {
            let model_view = self.draw_list.view_transform * self.model_matrix;
            return model_view * vec4(local_dir.x, local_dir.y, local_dir.z, 0.0)
        }

        vertex: fn() {
            let local_uv = vec2(self.geom.ny_nz_uv.z, self.geom.ny_nz_uv.w);
            let local_pos_src = vec3(self.geom.pos_nx.x, self.geom.pos_nx.y, self.geom.pos_nx.z);
            let displacement = self.get_vertex_displacement(local_uv, local_pos_src);
            let local_scale = self.local_scale;
            let scaled_local_pos = vec3(
                (local_pos_src.x + displacement.x) * local_scale.x,
                (local_pos_src.y + displacement.y) * local_scale.y,
                (local_pos_src.z + displacement.z) * local_scale.z
            );
            let local_pos = vec4(
                scaled_local_pos.x,
                scaled_local_pos.y,
                scaled_local_pos.z,
                1.0
            );
            let safe_scale = vec3(
                max(abs(local_scale.x), 0.000001),
                max(abs(local_scale.y), 0.000001),
                max(abs(local_scale.z), 0.000001)
            );
            let local_n_scaled = normalize(vec3(
                self.geom.pos_nx.w / safe_scale.x,
                self.geom.ny_nz_uv.x / safe_scale.y,
                self.geom.ny_nz_uv.y / safe_scale.z
            ));
            let local_n = vec4(local_n_scaled.x, local_n_scaled.y, local_n_scaled.z, 0.0);
            let model_pos = self.world_with_model_matrix(local_pos);
            let model_n = self.world_with_model_direction(local_n);
            let local_t_scaled = normalize(vec3(
                self.geom.tangent.x / safe_scale.x,
                self.geom.tangent.y / safe_scale.y,
                self.geom.tangent.z / safe_scale.z
            ));
            let local_t = vec4(local_t_scaled.x, local_t_scaled.y, local_t_scaled.z, 0.0);
            let model_t = self.world_with_model_direction(local_t);

            self.v_world = vec3(model_pos.x, model_pos.y, model_pos.z);
            self.v_normal = vec3(model_n.x, model_n.y, model_n.z);
            self.v_tangent = vec4(model_t.x, model_t.y, model_t.z, self.geom.tangent.w);
            self.v_uv = local_uv;
            self.v_color = self.geom.color;

            let world = vec4(model_pos.x, model_pos.y, model_pos.z, 1.0);
            self.v_world_clip = world;
            let view_pos = self.draw_pass.camera_view * world;
            self.v_view_pos = vec3(view_pos.x, view_pos.y, view_pos.z);
            self.vertex_pos = self.draw_pass.camera_projection * view_pos;
        }

        pow5: fn(x: float) {
            let x2 = x * x;
            return x2 * x2 * x
        }

        tone_map_color: fn(color_linear: vec3f) -> vec3f {
            let mapped = max(color_linear, vec3(0.0, 0.0, 0.0));
            let tone_num = mapped * (mapped * 2.51 + vec3(0.03, 0.03, 0.03));
            let tone_den = mapped * (mapped * 2.43 + vec3(0.59, 0.59, 0.59)) + vec3(0.14, 0.14, 0.14);
            let tone = tone_num / tone_den;
            return vec3(
                pow(max(tone.x, 0.0), 1.0 / 2.2),
                pow(max(tone.y, 0.0), 1.0 / 2.2),
                pow(max(tone.z, 0.0), 1.0 / 2.2)
            )
        }

        get_base_color: fn(uv: vec2, vertex_color: vec4) {
            let base = self.u_base_color_factor * vertex_color;
            if self.u_has_base_color_texture <= 0.5 {
                return base
            }
            let tex_srgb = self.base_color_texture.sample_as_bgra(uv);
            let tex_linear = vec4(
                pow(max(tex_srgb.x, 0.0), 2.2),
                pow(max(tex_srgb.y, 0.0), 2.2),
                pow(max(tex_srgb.z, 0.0), 2.2),
                tex_srgb.w
            );
            return base * tex_linear
        }

        get_metal_roughness: fn(uv: vec2) {
            if self.u_has_metal_roughness_texture <= 0.5 {
                return vec2(
                    clamp(self.u_metallic_factor, 0.0, 1.0),
                    clamp(self.u_roughness_factor, 0.045, 1.0)
                )
            }
            let mr_tex = self.metallic_roughness_texture.sample_as_bgra(uv);
            return vec2(
                clamp(self.u_metallic_factor * mr_tex.z, 0.0, 1.0),
                clamp(self.u_roughness_factor * mr_tex.y, 0.045, 1.0)
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
            if self.u_enable_occlusion <= 0.5 {
                return 1.0
            }
            let occlusion_tex = self.occlusion_texture.sample_as_bgra(uv);
            return mix(1.0, occlusion_tex.x, clamp(self.u_occlusion_strength, 0.0, 1.0))
        }

        get_emissive: fn(uv: vec2) {
            if self.u_enable_emissive <= 0.5 {
                return vec3(0.0, 0.0, 0.0)
            }
            if self.u_has_emissive_texture <= 0.5 {
                return self.u_emissive_factor
            }
            let emissive_tex_srgb = self.emissive_texture.sample_as_bgra(uv);
            let emissive_tex = vec3(
                pow(max(emissive_tex_srgb.x, 0.0), 2.2),
                pow(max(emissive_tex_srgb.y, 0.0), 2.2),
                pow(max(emissive_tex_srgb.z, 0.0), 2.2)
            );
            return self.u_emissive_factor * emissive_tex
        }

        env_atlas_uv_from_dir: fn(dir: vec3f) -> vec2f {
            let ad = abs(dir);
            let axis = max(ad.x, max(ad.y, ad.z));
            let safe_axis = max(axis, 0.00001);

            if ad.x >= ad.y && ad.x >= ad.z {
                if dir.x >= 0.0 {
                    return (vec2(0.0, 0.0) + (vec2(-dir.z / safe_axis, -dir.y / safe_axis) * 0.5 + vec2(0.5, 0.5))) / vec2(3.0, 2.0)
                }
                return (vec2(1.0, 0.0) + (vec2(dir.z / safe_axis, -dir.y / safe_axis) * 0.5 + vec2(0.5, 0.5))) / vec2(3.0, 2.0)
            } else if ad.y >= ad.z {
                if dir.y >= 0.0 {
                    return (vec2(2.0, 0.0) + (vec2(dir.x / safe_axis, dir.z / safe_axis) * 0.5 + vec2(0.5, 0.5))) / vec2(3.0, 2.0)
                }
                return (vec2(0.0, 1.0) + (vec2(dir.x / safe_axis, -dir.z / safe_axis) * 0.5 + vec2(0.5, 0.5))) / vec2(3.0, 2.0)
            }
            if dir.z >= 0.0 {
                return (vec2(1.0, 1.0) + (vec2(dir.x / safe_axis, -dir.y / safe_axis) * 0.5 + vec2(0.5, 0.5))) / vec2(3.0, 2.0)
            }
            return (vec2(2.0, 1.0) + (vec2(-dir.x / safe_axis, -dir.y / safe_axis) * 0.5 + vec2(0.5, 0.5))) / vec2(3.0, 2.0)
        }

        sample_env_atlas: fn(dir: vec3f) -> vec3f {
            let uv = self.env_atlas_uv_from_dir(dir);
            return self.env_atlas_texture.sample_as_bgra(uv).xyz
        }

        sample_env_faces: fn(dir: vec3f) -> vec3f {
            let ad = abs(dir);
            let axis = max(ad.x, max(ad.y, ad.z));
            let safe_axis = max(axis, 0.00001);

            if ad.x >= ad.y && ad.x >= ad.z {
                if dir.x >= 0.0 {
                    let uv = vec2(-dir.z / safe_axis, -dir.y / safe_axis) * 0.5 + vec2(0.5, 0.5);
                    return self.env_pos_x_texture.sample_as_bgra(uv).xyz
                }
                let uv = vec2(dir.z / safe_axis, -dir.y / safe_axis) * 0.5 + vec2(0.5, 0.5);
                return self.env_neg_x_texture.sample_as_bgra(uv).xyz
            } else if ad.y >= ad.z {
                if dir.y >= 0.0 {
                    let uv = vec2(dir.x / safe_axis, dir.z / safe_axis) * 0.5 + vec2(0.5, 0.5);
                    return self.env_pos_y_texture.sample_as_bgra(uv).xyz
                }
                let uv = vec2(dir.x / safe_axis, -dir.z / safe_axis) * 0.5 + vec2(0.5, 0.5);
                return self.env_neg_y_texture.sample_as_bgra(uv).xyz
            }
            if dir.z >= 0.0 {
                let uv = vec2(dir.x / safe_axis, -dir.y / safe_axis) * 0.5 + vec2(0.5, 0.5);
                return self.env_pos_z_texture.sample_as_bgra(uv).xyz
            }
            let uv = vec2(-dir.x / safe_axis, -dir.y / safe_axis) * 0.5 + vec2(0.5, 0.5);
            return self.env_neg_z_texture.sample_as_bgra(uv).xyz
        }

        get_env_specular: fn(refl_dir: vec3) {
            let env_t_spec = clamp(refl_dir.y * 0.5 + 0.5, 0.0, 1.0);
            let env_low = vec3(0.03, 0.035, 0.045);
            let env_high = vec3(0.36, 0.43, 0.5);
            if self.u_has_env_face_textures > 0.5 {
                return self.sample_env_faces(refl_dir)
            }
            if self.u_has_env_texture > 0.5 {
                return self.env_texture.sample_as_bgra(refl_dir).xyz
            }
            if self.u_has_env_atlas_texture > 0.5 {
                return self.sample_env_atlas(refl_dir)
            }
            return mix(env_low, env_high, env_t_spec)
        }

        get_env_diffuse: fn(normal_dir: vec3) {
            let env_t_diff = clamp(normal_dir.y * 0.5 + 0.5, 0.0, 1.0);
            let env_low = vec3(0.03, 0.035, 0.045);
            let env_high = vec3(0.36, 0.43, 0.5);
            if self.u_has_env_face_textures > 0.5 {
                return self.sample_env_faces(normal_dir)
            }
            if self.u_has_env_texture > 0.5 {
                return self.env_texture.sample_as_bgra(normal_dir).xyz
            }
            if self.u_has_env_atlas_texture > 0.5 {
                return self.sample_env_atlas(normal_dir)
            }
            return mix(env_low, env_high, env_t_diff)
        }

        fragment: fn(){
            self.fb0 = depth_clip(self.v_world_clip, self.pixel(), self.depth_clip)
        }

        pixel: fn() {
            let uv = vec2(fract(self.v_uv.x), fract(self.v_uv.y));
            let albedo = self.get_base_color(uv, self.v_color);
            let occlusion = self.get_occlusion(uv);
            let emissive = self.get_emissive(uv);
            let ambient = albedo.xyz * self.u_ambient;
            if self.u_enable_brdf <= 0.5 {
                let color = self.tone_map_color(ambient * occlusion + emissive);
                return vec4(color.x, color.y, color.z, albedo.w)
            }

            let n_geom = normalize(self.v_normal);
            let n = if self.u_has_normal_texture > 0.5 {
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
                normalize(t * n_tex.x + b * n_tex.y + n_geom * n_tex.z)
            } else {
                n_geom
            };

            let l = self.u_light_dir;
            let v = normalize(self.active_camera_world_pos() - self.v_world);
            let h = normalize(l + v);
            let ndotl = if self.u_enable_direct_light > 0.5 {
                max(dot(n, l), 0.0)
            } else {
                0.0
            };
            let ndotv = max(dot(n, v), 0.0001);
            let vdoth = max(dot(v, h), 0.0);

            let mr = self.get_metal_roughness(uv);
            let metal = mr.x;
            let rough = mr.y;
            let f0 = mix(vec3(0.04, 0.04, 0.04), albedo.xyz, metal);
            let fresnel = self.pow5(1.0 - vdoth);
            let f = f0 + (vec3(1.0, 1.0, 1.0) - f0) * fresnel;
            let kd = (vec3(1.0, 1.0, 1.0) - f) * (1.0 - metal);
            let diffuse = kd * albedo.xyz * (1.0 / 3.14159265);

            let specular = if self.u_enable_direct_specular > 0.5 {
                let ndoth = max(dot(n, h), 0.0001);
                let a = rough * rough;
                let a2 = a * a;
                let denom = ndoth * ndoth * (a2 - 1.0) + 1.0;
                let d = a2 / max(3.14159265 * denom * denom, 0.0001);
                let k0 = rough + 1.0;
                let k = (k0 * k0) / 8.0;
                let g_v = ndotv / max(ndotv * (1.0 - k) + k, 0.0001);
                let g_l = ndotl / max(ndotl * (1.0 - k) + k, 0.0001);
                let g = g_v * g_l;
                let spec = (d * g) / max(4.0 * ndotv * ndotl, 0.0001);
                f * spec * self.u_spec_strength
            } else {
                vec3(0.0, 0.0, 0.0)
            };

            let lit = if self.u_enable_direct_light > 0.5 {
                (diffuse + specular) * self.u_light_color * ndotl
            } else {
                vec3(0.0, 0.0, 0.0)
            };

            let ibl_diffuse = if self.u_enable_ibl > 0.5 {
                let env_diff_color = self.get_env_diffuse(n);
                kd * albedo.xyz * env_diff_color * self.u_env_intensity
            } else {
                vec3(0.0, 0.0, 0.0)
            };

            let indirect_spec = if self.u_enable_env_specular > 0.5 {
                let ndotv_env = clamp(dot(n, v), 0.0, 1.0);
                let refl = normalize(n * (2.0 * ndotv_env) - v);
                let refl_rough = normalize(mix(refl, n, rough * rough));
                let env_spec_color = self.get_env_specular(refl_rough);
                let c0 = vec4(-1.0, -0.0275, -0.572, 0.022);
                let c1 = vec4(1.0, 0.0425, 1.04, -0.04);
                let r = c0 * rough + c1;
                let a004 = min(r.x * r.x, pow(2.0, -9.28 * ndotv)) * r.x + r.y;
                let env_brdf = vec2(-1.04, 1.04) * a004 + r.zw;
                let env_fresnel = f0 * env_brdf.x + vec3(env_brdf.y, env_brdf.y, env_brdf.y);
                let env_spec = env_spec_color * env_fresnel * self.u_spec_strength * self.u_env_intensity;
                env_spec * mix(1.0, occlusion, 0.35)
            } else {
                vec3(0.0, 0.0, 0.0)
            };

            let indirect_diffuse = (ambient + ibl_diffuse) * occlusion;
            let color = self.tone_map_color(lit + indirect_diffuse + indirect_spec + emissive);
            return vec4(color.x, color.y, color.z, albedo.w)
        }
    }

    mod.draw.DrawPbrRefractive = mod.std.set_type_default() do #(DrawPbrRefractive::script_shader(vm)){
        ..mod.draw.DrawPbr
        camera_texture: texture_video()
        source_size: vec2(1280.0, 960.0)
        camera_enabled: 0.0
        rotation_steps: 0.0
        camera_fov_y_degrees: 92.0
        camera_projection_scale: 1.0
        camera_exposure: 1.0
        camera_center_offset_uv: vec2(0.0, 0.0)
        camera_world_pos: vec3(0.0, 0.0, 0.0)
        camera_right: vec3(1.0, 0.0, 0.0)
        camera_up: vec3(0.0, 1.0, 0.0)
        camera_forward: vec3(0.0, 0.0, -1.0)
        object_center: vec3(0.0, 0.0, 0.0)
        object_right: vec3(1.0, 0.0, 0.0)
        object_up: vec3(0.0, 1.0, 0.0)
        object_forward: vec3(0.0, 0.0, -1.0)
        object_half_extents: vec3(0.085, 0.085, 0.085)
        object_corner_radius: 0.018
        transmission_focus_distance: 1.8

        active_eye_camera_world_pos: fn() -> vec3f {
            let camera_world = self.draw_pass.camera_inv * vec4(0.0, 0.0, 0.0, 1.0);
            return vec3(
                camera_world.x / max(camera_world.w, 0.00001),
                camera_world.y / max(camera_world.w, 0.00001),
                camera_world.z / max(camera_world.w, 0.00001)
            )
        }

        active_eye_camera_right: fn() -> vec3f {
            let camera_right = self.draw_pass.camera_inv * vec4(1.0, 0.0, 0.0, 0.0);
            return normalize(vec3(camera_right.x, camera_right.y, camera_right.z))
        }

        active_eye_camera_up: fn() -> vec3f {
            let camera_up = self.draw_pass.camera_inv * vec4(0.0, 1.0, 0.0, 0.0);
            return normalize(vec3(camera_up.x, camera_up.y, camera_up.z))
        }

        active_eye_camera_forward: fn() -> vec3f {
            let camera_forward = self.draw_pass.camera_inv * vec4(0.0, 0.0, -1.0, 0.0);
            return normalize(vec3(camera_forward.x, camera_forward.y, camera_forward.z))
        }

        project_camera_uv: fn(dir_world: vec3f) -> vec3f {
            let cam_right = self.active_eye_camera_right();
            let cam_up = self.active_eye_camera_up();
            let cam_forward = self.active_eye_camera_forward();
            let cam_x = dot(dir_world, cam_right);
            let cam_y = dot(dir_world, cam_up);
            let cam_z = dot(dir_world, cam_forward);

            let aspect = max(self.source_size.x / max(self.source_size.y, 1.0), 1.0);
            let tan_half_y_base = tan(self.camera_fov_y_degrees * 0.5 * 0.01745329251);
            let projection_scale = max(self.camera_projection_scale, 0.0001);
            let tan_half_y = tan_half_y_base * projection_scale;
            let tan_half_x = tan_half_y * aspect;
            let safe_z = max(cam_z, 0.0001);
            let uv = vec2(
                0.5 + cam_x / (2.0 * tan_half_x * safe_z),
                0.5 - cam_y / (2.0 * tan_half_y * safe_z)
            ) + self.camera_center_offset_uv;
            return vec3(uv.x, uv.y, cam_z)
        }

        sample_camera_rgb: fn(coord: vec2f) -> vec3f {
            if self.camera_enabled <= 0.5 {
                return vec3(0.0, 0.0, 0.0);
            }

            let coord_90 = vec2(1.0 - coord.y, coord.x);
            let coord_180 = vec2(1.0 - coord.x, 1.0 - coord.y);
            let coord_270 = vec2(coord.y, 1.0 - coord.x);
            let is_90 = step(0.5, self.rotation_steps) * step(self.rotation_steps, 1.5);
            let is_180 = step(1.5, self.rotation_steps) * step(self.rotation_steps, 2.5);
            let is_270 = step(2.5, self.rotation_steps);
            let is_0 = 1.0 - is_90 - is_180 - is_270;
            let sample_coord = coord * is_0 + coord_90 * is_90 + coord_180 * is_180 + coord_270 * is_270;
            let sample = self.camera_texture.sample_video(sample_coord).xyz;

            let y = (sample.y * 255.0 - 16.0) / 219.0;
            let u = (sample.x * 255.0 - 128.0) / 224.0;
            let v = (sample.z * 255.0 - 128.0) / 224.0;
            let r = y + 1.8556 * u;
            let g = y - 0.1873 * u - 0.4681 * v;
            let b = y + 1.5748 * v;
            let exposure = max(self.camera_exposure, 0.0);
            return vec3(
                clamp(r * exposure, 0.0, 1.0),
                clamp(g * exposure, 0.0, 1.0),
                clamp(b * exposure, 0.0, 1.0)
            )
        }

        world_to_object_point: fn(point_world: vec3f) -> vec3f {
            let rel = point_world - self.object_center;
            let object_right = normalize(self.object_right);
            let object_up = normalize(self.object_up);
            let object_forward = normalize(self.object_forward);
            return vec3(
                dot(rel, object_right),
                dot(rel, object_up),
                dot(rel, object_forward)
            )
        }

        world_to_object_dir: fn(dir_world: vec3f) -> vec3f {
            let object_right = normalize(self.object_right);
            let object_up = normalize(self.object_up);
            let object_forward = normalize(self.object_forward);
            return vec3(
                dot(dir_world, object_right),
                dot(dir_world, object_up),
                dot(dir_world, object_forward)
            )
        }

        object_to_world_point: fn(point_local: vec3f) -> vec3f {
            let object_right = normalize(self.object_right);
            let object_up = normalize(self.object_up);
            let object_forward = normalize(self.object_forward);
            return self.object_center
                + object_right * point_local.x
                + object_up * point_local.y
                + object_forward * point_local.z
        }

        object_to_world_dir: fn(dir_local: vec3f) -> vec3f {
            let object_right = normalize(self.object_right);
            let object_up = normalize(self.object_up);
            let object_forward = normalize(self.object_forward);
            return normalize(
                object_right * dir_local.x
                + object_up * dir_local.y
                + object_forward * dir_local.z
            )
        }

        rounded_box_sdf: fn(point_local: vec3f) -> float {
            let radius = min(
                self.object_corner_radius,
                min(self.object_half_extents.x, min(self.object_half_extents.y, self.object_half_extents.z))
            );
            let inner = max(
                self.object_half_extents - vec3(radius, radius, radius),
                vec3(0.0, 0.0, 0.0)
            );
            let q = abs(point_local) - inner;
            return length(max(q, vec3(0.0, 0.0, 0.0))) + min(max(q.x, max(q.y, q.z)), 0.0) - radius
        }

        rounded_box_normal_local: fn(point_local: vec3f) -> vec3f {
            let eps = 0.0015;
            let grad = vec3(
                self.rounded_box_sdf(point_local + vec3(eps, 0.0, 0.0))
                    - self.rounded_box_sdf(point_local - vec3(eps, 0.0, 0.0)),
                self.rounded_box_sdf(point_local + vec3(0.0, eps, 0.0))
                    - self.rounded_box_sdf(point_local - vec3(0.0, eps, 0.0)),
                self.rounded_box_sdf(point_local + vec3(0.0, 0.0, eps))
                    - self.rounded_box_sdf(point_local - vec3(0.0, 0.0, eps))
            );
            if length(grad) > 0.00001 {
                return normalize(grad)
            }
            return normalize(point_local)
        }

        ray_box_exit_t: fn(origin_local: vec3f, dir_local: vec3f) -> float {
            let huge = 100000.0;
            let tx = if abs(dir_local.x) > 0.00001 {
                let face_x = if dir_local.x > 0.0 { self.object_half_extents.x } else { -self.object_half_extents.x };
                let hit_t = (face_x - origin_local.x) / dir_local.x;
                if hit_t > 0.0 { hit_t } else { huge }
            } else {
                huge
            };
            let ty = if abs(dir_local.y) > 0.00001 {
                let face_y = if dir_local.y > 0.0 { self.object_half_extents.y } else { -self.object_half_extents.y };
                let hit_t = (face_y - origin_local.y) / dir_local.y;
                if hit_t > 0.0 { hit_t } else { huge }
            } else {
                huge
            };
            let tz = if abs(dir_local.z) > 0.00001 {
                let face_z = if dir_local.z > 0.0 { self.object_half_extents.z } else { -self.object_half_extents.z };
                let hit_t = (face_z - origin_local.z) / dir_local.z;
                if hit_t > 0.0 { hit_t } else { huge }
            } else {
                huge
            };
            return min(tx, min(ty, tz))
        }

        trace_rounded_box_exit_local: fn(origin_local: vec3f, dir_local: vec3f) -> vec3f {
            let t_box = max(self.ray_box_exit_t(origin_local, dir_local), 0.001);
            let mid_1 = 0.5 * t_box;
            let inside_1 = step(self.rounded_box_sdf(origin_local + dir_local * mid_1), 0.0);
            let low_1 = mix(0.0, mid_1, inside_1);
            let high_1 = mix(mid_1, t_box, inside_1);

            let mid_2 = 0.5 * (low_1 + high_1);
            let inside_2 = step(self.rounded_box_sdf(origin_local + dir_local * mid_2), 0.0);
            let low_2 = mix(low_1, mid_2, inside_2);
            let high_2 = mix(mid_2, high_1, inside_2);

            let mid_3 = 0.5 * (low_2 + high_2);
            let inside_3 = step(self.rounded_box_sdf(origin_local + dir_local * mid_3), 0.0);
            let low_3 = mix(low_2, mid_3, inside_3);
            let high_3 = mix(mid_3, high_2, inside_3);

            let mid_4 = 0.5 * (low_3 + high_3);
            let inside_4 = step(self.rounded_box_sdf(origin_local + dir_local * mid_4), 0.0);
            let low_4 = mix(low_3, mid_4, inside_4);
            let high_4 = mix(mid_4, high_3, inside_4);

            let mid_5 = 0.5 * (low_4 + high_4);
            let inside_5 = step(self.rounded_box_sdf(origin_local + dir_local * mid_5), 0.0);
            let low_5 = mix(low_4, mid_5, inside_5);
            let high_5 = mix(mid_5, high_4, inside_5);

            let mid_6 = 0.5 * (low_5 + high_5);
            let inside_6 = step(self.rounded_box_sdf(origin_local + dir_local * mid_6), 0.0);
            let low_6 = mix(low_5, mid_6, inside_6);
            let high_6 = mix(mid_6, high_5, inside_6);

            return origin_local + dir_local * (0.5 * (low_6 + high_6))
        }

        sample_transmission: fn(point_world: vec3f, dir_world: vec3f) -> vec3f {
            let focus_distance = max(self.transmission_focus_distance, 0.25);
            let camera_world_pos = self.active_eye_camera_world_pos();
            let projection =
                self.project_camera_uv(point_world + dir_world * focus_distance - camera_world_pos);
            let sample_uv = clamp(projection.xy, vec2(0.0, 0.0), vec2(1.0, 1.0));
            let visible = step(0.0, projection.z)
                * step(0.0, projection.x)
                * step(0.0, projection.y)
                * step(projection.x, 1.0)
                * step(projection.y, 1.0);
            let camera_color = self.sample_camera_rgb(sample_uv);
            let atlas_color = self.sample_env_atlas(dir_world);
            return mix(atlas_color, camera_color, visible * clamp(self.camera_enabled, 0.0, 1.0))
        }

        refracted_dir: fn(incident_dir: vec3f, normal_dir: vec3f, eta: float) -> vec3f {
            let incident = normalize(incident_dir);
            let ndoti = dot(normal_dir, incident);
            let k = 1.0 - eta * eta * (1.0 - ndoti * ndoti);
            if k > 0.0 {
                return normalize(eta * incident - (eta * ndoti + sqrt(k)) * normal_dir)
            }
            return normalize(incident - 2.0 * dot(incident, normal_dir) * normal_dir)
        }

        surface_wobble_local: fn(point_local: vec3f) -> vec3f {
            let sx = sin(point_local.y * 34.0 + point_local.z * 23.0)
                + 0.45 * sin(point_local.y * 71.0 - point_local.x * 29.0);
            let sy = sin(point_local.z * 28.0 + point_local.x * 27.0)
                + 0.40 * sin(point_local.z * 63.0 - point_local.y * 21.0);
            let sz = sin(point_local.x * 31.0 + point_local.y * 25.0)
                + 0.50 * sin(point_local.x * 67.0 - point_local.z * 33.0);
            return normalize(vec3(sx, sy, sz))
        }

        pixel: fn() {
            let env_enabled = self.u_enable_ibl > 0.5;
            let highlight_enabled = self.u_enable_direct_specular > 0.5;
            if !env_enabled && !highlight_enabled {
                return vec4(0.0, 0.0, 0.0, 1.0)
            }

            let uv = vec2(fract(self.v_uv.x), fract(self.v_uv.y));
            let rough = self.get_metal_roughness(uv).y;
            let n_geom = normalize(self.v_normal);
            let n_base = if self.u_has_normal_texture > 0.5 {
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
                normalize(t * n_tex.x + b * n_tex.y + n_geom * n_tex.z)
            } else {
                n_geom
            };
            let local_surface = self.world_to_object_point(self.v_world);
            let wobble_world = self.object_to_world_dir(self.surface_wobble_local(local_surface));
            let n = normalize(n_base + wobble_world * 0.14);
            let camera_world_pos = self.active_eye_camera_world_pos();
            let view_dir_world = normalize(camera_world_pos - self.v_world);
            let ndotv = clamp(dot(n, view_dir_world), 0.0, 1.0);
            let highlight = if highlight_enabled {
                let l = self.u_light_dir;
                let h = normalize(l + view_dir_world);
                let ndotl = max(dot(n, l), 0.0);
                self.u_light_color
                    * pow(max(dot(n, h), 0.0), mix(72.0, 180.0, 1.0 - rough))
                    * self.u_spec_strength
                    * 0.20
                    * ndotl
            } else {
                vec3(0.0, 0.0, 0.0)
            };

            let env_color = if env_enabled {
                let albedo = self.get_base_color(uv, self.v_color);
                let reflection_dir = normalize(n * (2.0 * ndotv) - view_dir_world);
                let reflection = self.get_env_specular(normalize(mix(reflection_dir, n, rough * rough)));

                let ior = 1.04;
                let inside_dir_world = self.refracted_dir(-view_dir_world, n, 1.0 / ior);
                let local_origin = self.world_to_object_point(self.v_world + inside_dir_world * 0.0035);
                let local_dir = normalize(self.world_to_object_dir(inside_dir_world));
                let exit_local = self.trace_rounded_box_exit_local(local_origin, local_dir);
                let exit_world = self.object_to_world_point(exit_local);
                let exit_normal_local = self.rounded_box_normal_local(exit_local);
                let exit_wobble_world = self.object_to_world_dir(self.surface_wobble_local(exit_local));
                let exit_normal_world = normalize(self.object_to_world_dir(exit_normal_local) + exit_wobble_world * 0.14);
                let exit_dir_world = self.refracted_dir(inside_dir_world, -exit_normal_world, ior);
                let exit_up_axis = if abs(exit_normal_world.y) > 0.99 { vec3(1.0, 0.0, 0.0) } else { vec3(0.0, 1.0, 0.0) };
                let exit_tangent = normalize(cross(exit_up_axis, exit_normal_world));
                let exit_bitangent = normalize(cross(exit_normal_world, exit_tangent));
                let chroma = (0.0032 + rough * 0.0055) * (0.28 + (1.0 - ndotv));
                let refr_r = normalize(exit_dir_world + exit_tangent * chroma + exit_bitangent * (0.30 * chroma));
                let refr_g = normalize(exit_dir_world);
                let refr_b = normalize(exit_dir_world - exit_tangent * chroma - exit_bitangent * (0.30 * chroma));
                let transmitted = vec3(
                    self.sample_transmission(exit_world, refr_r).x,
                    self.sample_transmission(exit_world, refr_g).y,
                    self.sample_transmission(exit_world, refr_b).z
                );

                let tint = mix(vec3(1.0, 1.0, 1.0), albedo.xyz, 0.58) * vec3(0.74, 0.88, 1.20);
                let trace_length = length(exit_world - self.v_world);
                let thickness = clamp(trace_length / max(self.object_half_extents.x * 2.0, 0.001), 0.0, 1.2)
                    + rough * 0.10
                    + (1.0 - ndotv) * 0.18;
                let absorption = mix(vec3(1.0, 1.0, 1.0), tint, clamp(thickness, 0.0, 1.0));
                let f0 = pow((1.12 - 1.0) / (1.12 + 1.0), 2.0);
                let fresnel = f0 + (1.0 - f0) * self.pow5(1.0 - ndotv);
                let transmitted_color = transmitted * absorption * self.u_env_intensity * 0.94;
                let reflection_color = reflection * self.u_env_intensity * (0.42 + 0.30 * self.u_spec_strength);
                let sheen_reflection = self.get_env_specular(normalize(mix(reflection_dir, n, 0.02)));
                let sheen_strength =
                    (0.028 + 0.028 * self.u_spec_strength)
                    + (0.14 + 0.10 * self.u_spec_strength) * pow(1.0 - ndotv, 4.0);
                mix(
                    transmitted_color,
                    reflection_color,
                    clamp(fresnel * 0.68 + rough * 0.06, 0.0, 0.82)
                ) + sheen_reflection * self.u_env_intensity * sheen_strength
            } else {
                vec3(0.0, 0.0, 0.0)
            };

            let color = self.tone_map_color(env_color + highlight);
            return vec4(color.x, color.y, color.z, 1.0)
        }
    }
}

#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawPbr {
    #[rust]
    pub many_instances: Option<ManyInstances>,
    #[rust]
    many_instances_mesh: Option<PbrMeshHandle>,
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
    #[rust]
    default_env_atlas_texture: Option<Texture>,
    #[rust(Mat4f::identity())]
    pub cur_transform: Mat4f,
    #[rust]
    pub transform_stack: Vec<Mat4f>,
    #[rust(vec4(1.0, 1.0, 1.0, 1.0))]
    pub cur_color: Vec4f,
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
    #[rust(0.0)]
    pub has_env_atlas_texture: f32,
    #[rust(0.0)]
    pub has_env_face_textures: f32,
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
    #[rust(0.0)]
    pub pad1: f32,
    #[deref]
    pub draw_vars: DrawVars,
    #[live]
    pub model_matrix: Mat4f,
    #[live(vec3(1.0, 1.0, 1.0))]
    pub local_scale: Vec3f,
    #[live]
    pub draw_clip: Vec4f,
    #[live(0.0)]
    pub depth_clip: f32,
}

#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawPbrRefractive {
    #[deref]
    pub draw_super: DrawPbr,
    #[live]
    pub source_size: Vec2f,
    #[live]
    pub camera_enabled: f32,
    #[live]
    pub rotation_steps: f32,
    #[live]
    pub camera_fov_y_degrees: f32,
    #[live]
    pub camera_projection_scale: f32,
    #[live]
    pub camera_exposure: f32,
    #[live]
    pub camera_center_offset_uv: Vec2f,
    #[live]
    pub camera_world_pos: Vec3f,
    #[live]
    pub camera_right: Vec3f,
    #[live]
    pub camera_up: Vec3f,
    #[live]
    pub camera_forward: Vec3f,
    #[live]
    pub object_center: Vec3f,
    #[live]
    pub object_right: Vec3f,
    #[live]
    pub object_up: Vec3f,
    #[live]
    pub object_forward: Vec3f,
    #[live]
    pub object_half_extents: Vec3f,
    #[live]
    pub object_corner_radius: f32,
    #[live]
    pub transmission_focus_distance: f32,
}

impl DrawPbrRefractive {
    pub fn set_camera_texture(&mut self, texture: Option<Texture>) {
        self.draw_super.draw_vars.texture_slots[13] = texture;
    }
}

impl DrawPbr {
    fn emissive_enabled(&self) -> f32 {
        if self.emissive_factor.x.abs() > 0.000_01
            || self.emissive_factor.y.abs() > 0.000_01
            || self.emissive_factor.z.abs() > 0.000_01
        {
            1.0
        } else {
            0.0
        }
    }

    fn direct_light_enabled(&self) -> f32 {
        if self.light_color.length() > 0.000_01 {
            1.0
        } else {
            0.0
        }
    }

    fn ibl_enabled(&self) -> f32 {
        if self.env_intensity.abs() > 0.000_01 {
            1.0
        } else {
            0.0
        }
    }

    fn direct_specular_enabled(&self, direct_light_enabled: f32) -> f32 {
        if direct_light_enabled > 0.5 && self.spec_strength.abs() > 0.000_01 {
            1.0
        } else {
            0.0
        }
    }

    fn env_specular_enabled(&self, ibl_enabled: f32) -> f32 {
        if ibl_enabled > 0.5 && self.spec_strength.abs() > 0.000_01 {
            1.0
        } else {
            0.0
        }
    }

    fn brdf_enabled(&self, direct_light_enabled: f32, ibl_enabled: f32) -> f32 {
        if direct_light_enabled > 0.5 || ibl_enabled > 0.5 {
            1.0
        } else {
            0.0
        }
    }

    fn occlusion_enabled(&self) -> f32 {
        if self.has_occlusion_texture > 0.5 && self.occlusion_strength > 0.000_01 {
            1.0
        } else {
            0.0
        }
    }

    pub fn begin(&mut self) {
        if self.many_instances.is_some() {
            debug_assert!(
                false,
                "DrawPbr::begin called while many-instance batch is active"
            );
        } else {
            self.many_instances_mesh = None;
        }
        self.acc_verts.clear();
        self.acc_indices.clear();
        self.set_transform(Mat4f::identity());
        self.set_local_scale(vec3(1.0, 1.0, 1.0));
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
        self.set_env_atlas_texture(None);
        self.set_env_face_textures(None);
    }

    pub fn set_transform(&mut self, transform: Mat4f) {
        self.cur_transform = transform;
        self.model_matrix = transform;
    }

    pub fn set_local_scale(&mut self, scale: Vec3f) {
        self.local_scale = scale;
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

    pub fn material_rgba(&mut self, r: f32, g: f32, b: f32, a: f32, metallic: f32, roughness: f32) {
        self.material(vec4(r, g, b, a), metallic, roughness);
    }

    /// p5-like alias for base color (PBR albedo factor).
    pub fn fill(&mut self, color: Vec4f) {
        self.set_base_color_factor(color);
    }

    pub fn fill_rgba(&mut self, r: f32, g: f32, b: f32, a: f32) {
        self.fill(vec4(r, g, b, a));
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

    pub fn set_env_atlas_texture(&mut self, texture: Option<Texture>) {
        self.has_env_atlas_texture = if texture.is_some() { 1.0 } else { 0.0 };
        self.draw_vars.texture_slots[6] = texture;
    }

    pub fn set_env_face_textures(&mut self, textures: Option<&[Texture; 6]>) {
        self.has_env_face_textures = if textures.is_some() { 1.0 } else { 0.0 };
        if let Some(textures) = textures {
            for (index, texture) in textures.iter().enumerate() {
                self.draw_vars.texture_slots[7 + index] = Some(texture.clone());
            }
        } else {
            for slot in 7..13 {
                self.draw_vars.texture_slots[slot] = None;
            }
        }
    }

    pub fn set_depth_write(&mut self, depth_write: bool) {
        self.draw_vars.options.depth_write = depth_write;
    }

    pub fn set_depth_clip(&mut self, depth_clip: f32) {
        self.depth_clip = depth_clip;
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
        self.set_env_atlas_texture(material.textures.env_atlas.clone());
        self.set_env_face_textures(material.textures.env_faces.as_ref());
    }

    fn apply_draw_uniforms(&mut self, cx: &mut CxDraw) {
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
        self.draw_vars.set_uniform(
            cx.cx,
            live_id!(u_roughness_factor),
            &[self.roughness_factor],
        );
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
        self.draw_vars
            .set_uniform(cx.cx, live_id!(u_has_env_texture), &[self.has_env_texture]);
        self.draw_vars.set_uniform(
            cx.cx,
            live_id!(u_has_env_atlas_texture),
            &[self.has_env_atlas_texture],
        );
        self.draw_vars.set_uniform(
            cx.cx,
            live_id!(u_has_env_face_textures),
            &[self.has_env_face_textures],
        );
        let enable_occlusion = self.occlusion_enabled();
        let enable_emissive = self.emissive_enabled();
        let enable_direct_light = self.direct_light_enabled();
        let enable_ibl = self.ibl_enabled();
        let enable_brdf = self.brdf_enabled(enable_direct_light, enable_ibl);
        let enable_direct_specular = self.direct_specular_enabled(enable_direct_light);
        let enable_env_specular = self.env_specular_enabled(enable_ibl);
        self.draw_vars
            .set_uniform(cx.cx, live_id!(u_enable_occlusion), &[enable_occlusion]);
        self.draw_vars
            .set_uniform(cx.cx, live_id!(u_enable_emissive), &[enable_emissive]);
        self.draw_vars.set_uniform(
            cx.cx,
            live_id!(u_enable_direct_light),
            &[enable_direct_light],
        );
        self.draw_vars
            .set_uniform(cx.cx, live_id!(u_enable_brdf), &[enable_brdf]);
        self.draw_vars.set_uniform(
            cx.cx,
            live_id!(u_enable_direct_specular),
            &[enable_direct_specular],
        );
        self.draw_vars
            .set_uniform(cx.cx, live_id!(u_enable_ibl), &[enable_ibl]);
        self.draw_vars.set_uniform(
            cx.cx,
            live_id!(u_enable_env_specular),
            &[enable_env_specular],
        );
        let light_dir = if self.light_dir.length() > 0.000_01 {
            self.light_dir.normalize()
        } else {
            vec3(0.0, 0.0, 1.0)
        };
        self.draw_vars.set_uniform(
            cx.cx,
            live_id!(u_light_dir),
            &[light_dir.x, light_dir.y, light_dir.z],
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
        let (verts, inds) =
            self.build_vertex_data(positions, normals, tangents, uvs, colors, indices)?;
        let base_index = (self.acc_verts.len() / PBR_FLOATS_PER_VERTEX) as u32;
        self.acc_verts.extend_from_slice(&verts);
        self.acc_indices
            .extend(inds.iter().map(|i| base_index + *i));
        Ok(())
    }

    /// Upload one mesh to GPU geometry and return a reusable mesh handle.
    /// Mesh data stays in local/object space and can be reused across draws.
    #[allow(clippy::too_many_arguments)]
    pub fn upload_indexed_triangles_mesh(
        &mut self,
        cx: &mut CxDraw,
        positions: &[[f32; 3]],
        normals: Option<&[[f32; 3]]>,
        tangents: Option<&[[f32; 4]]>,
        uvs: Option<&[[f32; 2]]>,
        colors: Option<&[[f32; 4]]>,
        indices: &[u32],
    ) -> Result<PbrMeshHandle, String> {
        let (verts, inds) =
            self.build_vertex_data(positions, normals, tangents, uvs, colors, indices)?;
        let geom = Geometry::new(cx.cx);
        geom.update(cx.cx, inds, verts);
        self.meshes.push(geom);
        Ok(self.meshes.len() - 1)
    }

    pub fn update_mesh_indices(
        &mut self,
        cx: &mut CxDraw,
        mesh: PbrMeshHandle,
        indices: Vec<u32>,
    ) -> Result<(), String> {
        let geom = self
            .meshes
            .get(mesh)
            .ok_or_else(|| format!("invalid mesh handle {mesh}"))?;
        geom.update_indices(cx.cx, indices);
        Ok(())
    }

    pub fn upload_decoded_primitive_mesh(
        &mut self,
        cx: &mut CxDraw,
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
        cx: &mut CxDraw,
        path: impl AsRef<Path>,
    ) -> Result<(), String> {
        let path = path.as_ref();
        let bytes = std::fs::read(path)
            .map_err(|err| format!("failed reading env map {}: {err}", path.display()))?;
        self.load_default_env_equirect_from_bytes(cx, &bytes, Some(path))
    }

    pub fn load_default_env_equirect_from_bytes(
        &mut self,
        cx: &mut CxDraw,
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

    pub fn default_env_texture(&mut self, cx: &mut CxDraw) -> Texture {
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

    pub fn default_env_atlas_texture(&mut self, cx: &mut CxDraw) -> Texture {
        if let Some(texture) = self.default_env_atlas_texture.clone() {
            return texture;
        }

        let face_size = 256usize;
        let data = Self::build_default_env_atlas_data(face_size);
        let texture = Texture::new_with_format(
            cx.cx,
            TextureFormat::VecBGRAu8_32 {
                width: face_size * 3,
                height: face_size * 2,
                data: Some(data),
                updated: TextureUpdated::Full,
            },
        );
        self.default_env_atlas_texture = Some(texture.clone());
        texture
    }

    fn default_env_face_dir(face: usize, x: usize, y: usize, size: usize) -> Vec3f {
        let u = ((x as f32 + 0.5) / size as f32) * 2.0 - 1.0;
        let v = ((y as f32 + 0.5) / size as f32) * 2.0 - 1.0;
        match face {
            0 => vec3(1.0, -v, -u),  // +X
            1 => vec3(-1.0, -v, u),  // -X
            2 => vec3(u, 1.0, v),    // +Y
            3 => vec3(u, -1.0, -v),  // -Y
            4 => vec3(u, -v, 1.0),   // +Z
            _ => vec3(-u, -v, -1.0), // -Z
        }
        .normalize()
    }

    fn default_env_color(dir: Vec3f) -> Vec3f {
        let sky_t = (dir.y * 0.5 + 0.5).clamp(0.0, 1.0);
        let ground = vec3(0.06, 0.065, 0.07);
        let sky = vec3(0.27, 0.36, 0.46);
        let mut color = ground + (sky - ground) * sky_t;

        let horizon = (1.0 - dir.y.abs()).powf(2.5) * 0.16;
        color += vec3(horizon * 0.95, horizon * 0.85, horizon * 0.7);

        let sun_dir = vec3(0.22, 0.72, 0.66).normalize();
        let sun_dot = dir.dot(sun_dir).max(0.0);
        let sun_core = sun_dot.powf(96.0) * 1.1;
        let sun_glow = sun_dot.powf(16.0) * 0.35;
        let sun = sun_core + sun_glow;
        color += vec3(sun * 1.0, sun * 0.96, sun * 0.88);
        color
    }

    fn build_default_env_cube_data(size: usize) -> Vec<u32> {
        let mut out = vec![0u32; size.saturating_mul(size).saturating_mul(6)];
        for face in 0..6usize {
            for y in 0..size {
                for x in 0..size {
                    let d = Self::default_env_face_dir(face, x, y, size);
                    let color = Self::default_env_color(d);

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

    fn build_default_env_atlas_data(face_size: usize) -> Vec<u32> {
        let atlas_width = face_size.saturating_mul(3);
        let atlas_height = face_size.saturating_mul(2);
        let mut out = vec![0u32; atlas_width.saturating_mul(atlas_height)];
        for face in 0..6usize {
            let (tile_x, tile_y) = match face {
                0 => (0usize, 0usize),
                1 => (1usize, 0usize),
                2 => (2usize, 0usize),
                3 => (0usize, 1usize),
                4 => (1usize, 1usize),
                _ => (2usize, 1usize),
            };
            for y in 0..face_size {
                for x in 0..face_size {
                    let dir = Self::default_env_face_dir(face, x, y, face_size);
                    let color = Self::default_env_color(dir);
                    let atlas_x = tile_x.saturating_mul(face_size).saturating_add(x);
                    let atlas_y = tile_y.saturating_mul(face_size).saturating_add(y);
                    let idx = atlas_y.saturating_mul(atlas_width).saturating_add(atlas_x);
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
                        0 => vec3(1.0, -v, -u),  // +X
                        1 => vec3(-1.0, -v, u),  // -X
                        2 => vec3(u, 1.0, v),    // +Y
                        3 => vec3(u, -1.0, -v),  // -Y
                        4 => vec3(u, -v, 1.0),   // +Z
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
        cx: &mut CxDraw,
        mesh: PbrMeshHandle,
        transform: Mat4f,
    ) -> Result<(), String> {
        let prev_model = self.model_matrix;
        self.model_matrix = transform;
        let result = self.draw_mesh(cx, mesh);
        self.model_matrix = prev_model;
        result
    }

    pub fn draw_mesh_with_transform_and_local_scale(
        &mut self,
        cx: &mut CxDraw,
        mesh: PbrMeshHandle,
        transform: Mat4f,
        local_scale: Vec3f,
    ) -> Result<(), String> {
        let prev_model = self.model_matrix;
        let prev_scale = self.local_scale;
        self.model_matrix = transform;
        self.local_scale = local_scale;
        let result = self.draw_mesh(cx, mesh);
        self.local_scale = prev_scale;
        self.model_matrix = prev_model;
        result
    }

    /// Draw a cube using the current material/shader state.
    /// Uses cached unit-cube meshes and applies size as a transform scale.
    pub fn draw_cube(
        &mut self,
        cx: &mut CxDraw,
        size: Vec3f,
        subdivisions: usize,
    ) -> Result<(), String> {
        let mesh = self.ensure_cube_mesh(cx, subdivisions)?;
        self.draw_mesh_with_transform_and_local_scale(cx, mesh, self.cur_transform, size)
    }

    pub fn draw_cube_with_material(
        &mut self,
        cx: &mut CxDraw,
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
        cx: &mut CxDraw,
        size: Vec2f,
        seg_u: usize,
        seg_v: usize,
    ) -> Result<(), String> {
        let mesh = self.ensure_surface_mesh(cx, seg_u, seg_v)?;
        self.draw_mesh_with_transform_and_local_scale(
            cx,
            mesh,
            self.cur_transform,
            vec3(size.x, 1.0, size.y),
        )
    }

    pub fn draw_surface_with_material(
        &mut self,
        cx: &mut CxDraw,
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
        cx: &mut CxDraw,
        radius: f32,
        subdivisions: usize,
    ) -> Result<(), String> {
        let lat = subdivisions.clamp(4, 96);
        let lon = (lat * 2).clamp(8, 192);
        let mesh = self.ensure_sphere_mesh(cx, lat, lon)?;
        let radius = radius.max(0.0001);
        self.draw_mesh_with_transform_and_local_scale(
            cx,
            mesh,
            self.cur_transform,
            vec3(radius, radius, radius),
        )
    }

    pub fn draw_sphere_with_material(
        &mut self,
        cx: &mut CxDraw,
        radius: f32,
        subdivisions: usize,
        material: &DrawPbrMaterialState,
    ) -> Result<(), String> {
        self.apply_material_state(material);
        self.draw_sphere(cx, radius, subdivisions)
    }

    /// Draw a capsule (pill) aligned with the local Y axis using current material/shader state.
    ///
    /// * `radius` - Capsule radius.
    /// * `half_height` - Half of the cylindrical middle section length, excluding the hemispherical caps.
    /// * `subdivisions` - Controls tessellation density for the hemispheres.
    pub fn draw_capsule(
        &mut self,
        cx: &mut CxDraw,
        radius: f32,
        half_height: f32,
        subdivisions: usize,
    ) -> Result<(), String> {
        let radius = radius.max(0.0001);
        let half_height = half_height.max(0.0);
        if half_height <= 0.0001 {
            return self.draw_sphere(cx, radius, subdivisions);
        }

        let lat = subdivisions.clamp(4, 96);
        let lon = (lat * 2).clamp(8, 192);
        let ratio = (half_height / radius).clamp(0.0, 64.0);
        let mesh = self.ensure_capsule_mesh(cx, lat, lon, ratio)?;
        self.draw_mesh_with_transform_and_local_scale(
            cx,
            mesh,
            self.cur_transform,
            vec3(radius, radius, radius),
        )
    }

    pub fn draw_capsule_with_material(
        &mut self,
        cx: &mut CxDraw,
        radius: f32,
        half_height: f32,
        subdivisions: usize,
        material: &DrawPbrMaterialState,
    ) -> Result<(), String> {
        self.apply_material_state(material);
        self.draw_capsule(cx, radius, half_height, subdivisions)
    }

    /// Draw a rounded cube (box with smooth rounded edges and corners).
    ///
    /// * `size` — half-extents along each axis (the full box spans ±size on each axis before rounding).
    /// * `radius` — corner radius. Clamped to half the smallest axis so the shape stays valid.
    /// * `subdivisions` — tessellation of the flat face quads (per-edge segment count).
    /// * `corner_segments` — tessellation of the rounded edges/corners (number of arc steps).
    pub fn draw_rounded_cube(
        &mut self,
        cx: &mut CxDraw,
        size: Vec3f,
        radius: f32,
        subdivisions: usize,
        corner_segments: usize,
    ) -> Result<(), String> {
        // Clamp radius to at most the half of the smallest axis
        let min_half = size.x.min(size.y).min(size.z);
        let clamped_radius = radius.max(0.0).min(min_half);
        // Express radius as a fraction of the half-extent (0..1000 permille for cache key)
        let frac = if min_half > 0.0001 {
            clamped_radius / min_half
        } else {
            0.0
        };
        let mesh = self.ensure_rounded_cube_mesh(cx, subdivisions, corner_segments, frac)?;
        self.draw_mesh_with_transform_and_local_scale(
            cx,
            mesh,
            self.cur_transform,
            vec3(size.x * 2.0, size.y * 2.0, size.z * 2.0),
        )
    }

    pub fn draw_rounded_cube_with_material(
        &mut self,
        cx: &mut CxDraw,
        size: Vec3f,
        radius: f32,
        subdivisions: usize,
        corner_segments: usize,
        material: &DrawPbrMaterialState,
    ) -> Result<(), String> {
        self.apply_material_state(material);
        self.draw_rounded_cube(cx, size, radius, subdivisions, corner_segments)
    }

    pub fn upload_uniform_rounded_cube_mesh(
        &mut self,
        cx: &mut CxDraw,
        half_extent: f32,
        radius: f32,
        subdivisions: usize,
        corner_segments: usize,
    ) -> Result<PbrMeshHandle, String> {
        let half_extent = half_extent.max(0.0001);
        let radius = radius.max(0.0).min(half_extent);
        let segments = subdivisions.clamp(1, 64);
        let cseg = corner_segments.clamp(1, 32);
        let (positions, normals, uvs, indices) =
            Self::build_rounded_cube_mesh(half_extent, radius, segments, cseg);
        self.upload_indexed_triangles_mesh(
            cx,
            &positions,
            Some(&normals),
            None,
            Some(&uvs),
            None,
            &indices,
        )
    }

    fn ensure_cube_mesh(
        &mut self,
        cx: &mut CxDraw,
        subdivisions: usize,
    ) -> Result<PbrMeshHandle, String> {
        let segments = subdivisions.clamp(1, 64) as u16;
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

    fn ensure_capsule_mesh(
        &mut self,
        cx: &mut CxDraw,
        lat: usize,
        lon: usize,
        half_height_ratio: f32,
    ) -> Result<PbrMeshHandle, String> {
        let lat = lat.clamp(4, 256) as u16;
        let lon = lon.clamp(8, 512) as u16;
        let half_height_permille = (half_height_ratio.clamp(0.0, 64.0) * 1000.0) as u16;
        let key = PbrPrimitiveMeshKey::Capsule {
            lat,
            lon,
            half_height_permille,
        };
        if let Some(handle) = self.primitive_mesh_cache.get(&key).copied() {
            return Ok(handle);
        }

        let (positions, normals, uvs, indices) =
            Self::build_capsule_mesh(lat as usize, lon as usize, half_height_ratio);
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
        cx: &mut CxDraw,
        seg_u: usize,
        seg_v: usize,
    ) -> Result<PbrMeshHandle, String> {
        let seg_u = seg_u.clamp(1, 256) as u16;
        let seg_v = seg_v.clamp(1, 256) as u16;
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
        cx: &mut CxDraw,
        lat: usize,
        lon: usize,
    ) -> Result<PbrMeshHandle, String> {
        let lat = lat.clamp(4, 256) as u16;
        let lon = lon.clamp(8, 512) as u16;
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

    fn ensure_rounded_cube_mesh(
        &mut self,
        cx: &mut CxDraw,
        subdivisions: usize,
        corner_segments: usize,
        radius_frac: f32,
    ) -> Result<PbrMeshHandle, String> {
        let segments = subdivisions.clamp(1, 64) as u16;
        let cseg = corner_segments.clamp(1, 32) as u16;
        let radius_permille = (radius_frac.clamp(0.0, 1.0) * 1000.0) as u16;
        let key = PbrPrimitiveMeshKey::RoundedCube {
            segments,
            corner_segments: cseg,
            radius_permille,
        };
        if let Some(handle) = self.primitive_mesh_cache.get(&key).copied() {
            return Ok(handle);
        }

        // Build a unit rounded cube (half_extent=0.5) with the radius as a fraction of 0.5
        let unit_radius = 0.5 * radius_frac.clamp(0.0, 1.0);
        let (positions, normals, uvs, indices) =
            Self::build_rounded_cube_mesh(0.5, unit_radius, segments as usize, cseg as usize);
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

    /// Build a rounded cube mesh centered at origin.
    ///
    /// The cube spans from -half_extent to +half_extent on each axis, with edges
    /// and corners rounded to `radius`. The flat faces are subdivided by `segments`,
    /// and the rounded parts use `corner_segments` arc steps.
    ///
    /// Geometry structure:
    /// - 6 flat face quads (inset by radius), each subdivided
    /// - 12 edge cylinder strips (quarter-cylinder arcs along each edge)
    /// - 8 corner sphere patches (octant of a sphere at each corner)
    fn build_rounded_cube_mesh(
        half_extent: f32,
        radius: f32,
        segments: usize,
        corner_segments: usize,
    ) -> PbrMeshBuffers {
        let segments = segments.max(1);
        let cs = corner_segments.max(1);
        let radius = radius.min(half_extent).max(0.0);
        let inner = half_extent - radius; // Half-extent of the inner (flat) box

        let face_vertex_count = 6 * (segments + 1) * (segments + 1);
        let edge_vertex_count = 12 * (segments + 1) * (cs + 1);
        let corner_vertex_count = 8 * (cs + 1) * (cs + 1);
        let total_vertex_count = face_vertex_count + edge_vertex_count + corner_vertex_count;

        let face_index_count = 6 * segments * segments * 6;
        let edge_index_count = 12 * segments * cs * 6;
        let corner_index_count = 8 * cs * cs * 6;
        let total_index_count = face_index_count + edge_index_count + corner_index_count;

        let mut positions = Vec::with_capacity(total_vertex_count);
        let mut normals = Vec::with_capacity(total_vertex_count);
        let mut uvs = Vec::with_capacity(total_vertex_count);
        let mut indices = Vec::with_capacity(total_index_count);

        // Macro to push a vertex inline (avoids borrow-checker issues with closures)
        macro_rules! push_vert {
            ($pos:expr, $nor:expr, $uv:expr) => {{
                positions.push($pos);
                normals.push($nor);
                uvs.push($uv);
            }};
        }

        // ── 1. SIX FLAT FACES ──────────────────────────────────────────────
        // Each face is a subdivided quad on the face plane, inset by `radius`
        // so it covers only the flat region (from -inner to +inner on the two
        // tangent axes, at ±half_extent on the normal axis).
        struct FaceDef {
            normal: [f32; 3],
            // Two tangent axes (u_axis, v_axis) forming a right-handed frame with normal
            u_axis: [f32; 3],
            v_axis: [f32; 3],
        }
        let faces = [
            FaceDef {
                normal: [0.0, 0.0, 1.0],
                u_axis: [1.0, 0.0, 0.0],
                v_axis: [0.0, 1.0, 0.0],
            }, // +Z
            FaceDef {
                normal: [0.0, 0.0, -1.0],
                u_axis: [-1.0, 0.0, 0.0],
                v_axis: [0.0, 1.0, 0.0],
            }, // -Z
            FaceDef {
                normal: [1.0, 0.0, 0.0],
                u_axis: [0.0, 0.0, -1.0],
                v_axis: [0.0, 1.0, 0.0],
            }, // +X
            FaceDef {
                normal: [-1.0, 0.0, 0.0],
                u_axis: [0.0, 0.0, 1.0],
                v_axis: [0.0, 1.0, 0.0],
            }, // -X
            FaceDef {
                normal: [0.0, 1.0, 0.0],
                u_axis: [1.0, 0.0, 0.0],
                v_axis: [0.0, 0.0, 1.0],
            }, // +Y
            FaceDef {
                normal: [0.0, -1.0, 0.0],
                u_axis: [1.0, 0.0, 0.0],
                v_axis: [0.0, 0.0, -1.0],
            }, // -Y
        ];

        for face in &faces {
            let n = face.normal;
            let u_ax = face.u_axis;
            let v_ax = face.v_axis;
            let base_idx = positions.len() as u32;

            for iy in 0..=segments {
                let v = iy as f32 / segments as f32;
                let fv = -inner + 2.0 * inner * v;
                for ix in 0..=segments {
                    let u = ix as f32 / segments as f32;
                    let fu = -inner + 2.0 * inner * u;

                    let pos = [
                        n[0] * half_extent + u_ax[0] * fu + v_ax[0] * fv,
                        n[1] * half_extent + u_ax[1] * fu + v_ax[1] * fv,
                        n[2] * half_extent + u_ax[2] * fu + v_ax[2] * fv,
                    ];
                    push_vert!(pos, n, [u, v]);
                }
            }

            let stride = (segments + 1) as u32;
            for iy in 0..segments as u32 {
                for ix in 0..segments as u32 {
                    let i0 = base_idx + iy * stride + ix;
                    let i1 = i0 + 1;
                    let i2 = i0 + stride;
                    let i3 = i2 + 1;
                    indices.extend_from_slice(&[i0, i2, i1, i1, i2, i3]);
                }
            }
        }

        // ── 2. TWELVE EDGES ────────────────────────────────────────────────
        // Each edge is a quarter-cylinder connecting two adjacent faces.
        // We parameterise along the edge (t) and around the arc (a).
        struct EdgeDef {
            // The edge runs from corner0 to corner1 (inner box corners)
            axis: [f32; 3],         // unit direction along the edge
            center_start: [f32; 3], // start point of the edge center-line (inner box)
            n0: [f32; 3],           // outward normal of face 0 at this edge
            n1: [f32; 3],           // outward normal of face 1 at this edge
        }

        // The 12 edges of a cube: 4 along each primary axis
        let edge_defs = [
            // Edges along X axis (y,z combinations)
            EdgeDef {
                axis: [1.0, 0.0, 0.0],
                center_start: [-inner, inner, inner],
                n0: [0.0, 1.0, 0.0],
                n1: [0.0, 0.0, 1.0],
            },
            EdgeDef {
                axis: [1.0, 0.0, 0.0],
                center_start: [-inner, -inner, inner],
                n0: [0.0, 0.0, 1.0],
                n1: [0.0, -1.0, 0.0],
            },
            EdgeDef {
                axis: [1.0, 0.0, 0.0],
                center_start: [-inner, -inner, -inner],
                n0: [0.0, -1.0, 0.0],
                n1: [0.0, 0.0, -1.0],
            },
            EdgeDef {
                axis: [1.0, 0.0, 0.0],
                center_start: [-inner, inner, -inner],
                n0: [0.0, 0.0, -1.0],
                n1: [0.0, 1.0, 0.0],
            },
            // Edges along Y axis (x,z combinations)
            EdgeDef {
                axis: [0.0, 1.0, 0.0],
                center_start: [inner, -inner, inner],
                n0: [1.0, 0.0, 0.0],
                n1: [0.0, 0.0, 1.0],
            },
            EdgeDef {
                axis: [0.0, 1.0, 0.0],
                center_start: [-inner, -inner, inner],
                n0: [0.0, 0.0, 1.0],
                n1: [-1.0, 0.0, 0.0],
            },
            EdgeDef {
                axis: [0.0, 1.0, 0.0],
                center_start: [-inner, -inner, -inner],
                n0: [-1.0, 0.0, 0.0],
                n1: [0.0, 0.0, -1.0],
            },
            EdgeDef {
                axis: [0.0, 1.0, 0.0],
                center_start: [inner, -inner, -inner],
                n0: [0.0, 0.0, -1.0],
                n1: [1.0, 0.0, 0.0],
            },
            // Edges along Z axis (x,y combinations)
            EdgeDef {
                axis: [0.0, 0.0, 1.0],
                center_start: [inner, inner, -inner],
                n0: [1.0, 0.0, 0.0],
                n1: [0.0, 1.0, 0.0],
            },
            EdgeDef {
                axis: [0.0, 0.0, 1.0],
                center_start: [-inner, inner, -inner],
                n0: [0.0, 1.0, 0.0],
                n1: [-1.0, 0.0, 0.0],
            },
            EdgeDef {
                axis: [0.0, 0.0, 1.0],
                center_start: [-inner, -inner, -inner],
                n0: [-1.0, 0.0, 0.0],
                n1: [0.0, -1.0, 0.0],
            },
            EdgeDef {
                axis: [0.0, 0.0, 1.0],
                center_start: [inner, -inner, -inner],
                n0: [0.0, -1.0, 0.0],
                n1: [1.0, 0.0, 0.0],
            },
        ];

        for edge in &edge_defs {
            let base_idx = positions.len() as u32;
            let edge_len = 2.0 * inner;

            for it in 0..=segments {
                let t = it as f32 / segments as f32;
                let along = t * edge_len;
                // Center point on the inner box edge
                let cx = edge.center_start[0] + edge.axis[0] * along;
                let cy = edge.center_start[1] + edge.axis[1] * along;
                let cz = edge.center_start[2] + edge.axis[2] * along;

                for ia in 0..=cs {
                    let a = ia as f32 / cs as f32;
                    let angle = a * PI * 0.5; // 0 to π/2
                    let cos_a = angle.cos();
                    let sin_a = angle.sin();

                    // Normal interpolates from n0 to n1 via rotation
                    let nx = edge.n0[0] * cos_a + edge.n1[0] * sin_a;
                    let ny = edge.n0[1] * cos_a + edge.n1[1] * sin_a;
                    let nz = edge.n0[2] * cos_a + edge.n1[2] * sin_a;

                    let pos = [cx + nx * radius, cy + ny * radius, cz + nz * radius];
                    let uv = [t, a];
                    push_vert!(pos, [nx, ny, nz], uv);
                }
            }

            let arc_stride = (cs + 1) as u32;
            for it in 0..segments as u32 {
                for ia in 0..cs as u32 {
                    let i0 = base_idx + it * arc_stride + ia;
                    let i1 = i0 + 1;
                    let i2 = i0 + arc_stride;
                    let i3 = i2 + 1;
                    indices.extend_from_slice(&[i0, i2, i1, i1, i2, i3]);
                }
            }
        }

        // ── 3. EIGHT CORNERS ───────────────────────────────────────────────
        // Each corner is a spherical triangle patch (octant of a sphere).
        // We use a simple latitude/longitude parameterization over the octant.
        let corner_signs: [[f32; 3]; 8] = [
            [1.0, 1.0, 1.0],
            [-1.0, 1.0, 1.0],
            [-1.0, -1.0, 1.0],
            [1.0, -1.0, 1.0],
            [1.0, 1.0, -1.0],
            [-1.0, 1.0, -1.0],
            [-1.0, -1.0, -1.0],
            [1.0, -1.0, -1.0],
        ];

        for signs in &corner_signs {
            let base_idx = positions.len() as u32;
            // Center of this corner's sphere
            let center = [signs[0] * inner, signs[1] * inner, signs[2] * inner];

            // We generate a patch from the three face normals meeting at this corner.
            // Use spherical interpolation: latitude from one axis, longitude sweeps the other two.
            // The octant goes from the primary axis toward the two secondary axes.
            // We parameterize using two angles: phi (0..π/2) and theta (0..π/2)
            for iy in 0..=cs {
                let v = iy as f32 / cs as f32;
                let phi = v * PI * 0.5; // 0 to π/2 (latitude from +Y toward XZ plane)
                let cos_phi = phi.cos();
                let sin_phi = phi.sin();

                for ix in 0..=cs {
                    let u = ix as f32 / cs as f32;
                    let theta = u * PI * 0.5; // 0 to π/2 (longitude from +X toward +Z)
                    let cos_theta = theta.cos();
                    let sin_theta = theta.sin();

                    // Spherical normal in the octant (+x, +y, +z)
                    let nx = sin_phi * cos_theta;
                    let ny = cos_phi;
                    let nz = sin_phi * sin_theta;

                    // Apply the sign to map to the correct octant
                    let snx = nx * signs[0];
                    let sny = ny * signs[1];
                    let snz = nz * signs[2];

                    let pos = [
                        center[0] + snx * radius,
                        center[1] + sny * radius,
                        center[2] + snz * radius,
                    ];
                    push_vert!(pos, [snx, sny, snz], [u, v]);
                }
            }

            let stride = (cs + 1) as u32;
            for iy in 0..cs as u32 {
                for ix in 0..cs as u32 {
                    let i0 = base_idx + iy * stride + ix;
                    let i1 = i0 + 1;
                    let i2 = i0 + stride;
                    let i3 = i2 + 1;
                    indices.extend_from_slice(&[i0, i2, i1, i1, i2, i3]);
                }
            }
        }

        Self::fix_triangle_winding_from_normals(&positions, &normals, &mut indices);

        (positions, normals, uvs, indices)
    }

    fn fix_triangle_winding_from_normals(
        positions: &[[f32; 3]],
        normals: &[[f32; 3]],
        indices: &mut [u32],
    ) {
        for triangle in indices.chunks_exact_mut(3) {
            let i0 = triangle[0] as usize;
            let i1 = triangle[1] as usize;
            let i2 = triangle[2] as usize;
            let (Some(&p0), Some(&p1), Some(&p2), Some(&n0), Some(&n1), Some(&n2)) = (
                positions.get(i0),
                positions.get(i1),
                positions.get(i2),
                normals.get(i0),
                normals.get(i1),
                normals.get(i2),
            ) else {
                continue;
            };
            let e1 = vec3f(p1[0] - p0[0], p1[1] - p0[1], p1[2] - p0[2]);
            let e2 = vec3f(p2[0] - p0[0], p2[1] - p0[1], p2[2] - p0[2]);
            let face_normal = Vec3f::cross(e1, e2);
            if face_normal.length() <= 1.0e-8 {
                continue;
            }

            let avg_normal = vec3f(
                n0[0] + n1[0] + n2[0],
                n0[1] + n1[1] + n2[1],
                n0[2] + n1[2] + n2[2],
            );
            if avg_normal.length() <= 1.0e-8 {
                continue;
            }

            if face_normal.dot(avg_normal) < 0.0 {
                triangle.swap(1, 2);
            }
        }
    }

    fn geometry_gen_to_pbr(gen: &GeometryGen) -> Result<PbrMeshBuffers, String> {
        if !gen.vertices.len().is_multiple_of(9) {
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

    fn build_surface_mesh(seg_u: usize, seg_v: usize) -> PbrMeshBuffers {
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

    fn build_uv_sphere_mesh(lat: usize, lon: usize) -> PbrMeshBuffers {
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

    fn build_capsule_mesh(lat: usize, lon: usize, half_height: f32) -> PbrMeshBuffers {
        let lat = lat.max(4);
        let lon = lon.max(8);
        let half_height = half_height.max(0.0);
        let cyl_segments = (half_height * lat as f32).ceil() as usize;
        let ring_count = (lat + 1) + cyl_segments.saturating_sub(1) + lat;
        let mut positions = Vec::with_capacity(ring_count * (lon + 1));
        let mut normals = Vec::with_capacity(ring_count * (lon + 1));
        let mut uvs = Vec::with_capacity(ring_count * (lon + 1));
        let mut indices = Vec::with_capacity(ring_count.saturating_sub(1) * lon * 6);
        let total_half_height = half_height + 1.0;

        let mut push_ring = |y: f32, ring_radius: f32, normal_y: f32, normal_radius: f32| {
            let v = 1.0 - ((y + total_half_height) / (2.0 * total_half_height));
            for x in 0..=lon {
                let u = x as f32 / lon as f32;
                let phi = u * 2.0 * PI;
                let sin_phi = phi.sin();
                let cos_phi = phi.cos();
                let px = ring_radius * cos_phi;
                let pz = ring_radius * sin_phi;
                let nx = normal_radius * cos_phi;
                let nz = normal_radius * sin_phi;

                positions.push([px, y, pz]);
                normals.push([nx, normal_y, nz]);
                uvs.push([u, v]);
            }
        };

        for y in 0..=lat {
            let v = y as f32 / lat as f32;
            let angle = -0.5 * PI + v * 0.5 * PI;
            push_ring(
                -half_height + angle.sin(),
                angle.cos(),
                angle.sin(),
                angle.cos(),
            );
        }

        if cyl_segments > 1 {
            for segment in 1..cyl_segments {
                let t = segment as f32 / cyl_segments as f32;
                push_ring(-half_height + t * (2.0 * half_height), 1.0, 0.0, 1.0);
            }
        }

        for y in 1..=lat {
            let v = y as f32 / lat as f32;
            let angle = v * 0.5 * PI;
            push_ring(
                half_height + angle.sin(),
                angle.cos(),
                angle.sin(),
                angle.cos(),
            );
        }

        let stride = lon + 1;
        let rows = positions.len() / stride;
        for y in 0..rows.saturating_sub(1) {
            for x in 0..lon {
                let i0 = (y * stride + x) as u32;
                let i1 = i0 + 1;
                let i2 = i0 + stride as u32;
                let i3 = i2 + 1;
                indices.extend_from_slice(&[i0, i2, i1, i1, i2, i3]);
            }
        }

        (positions, normals, uvs, indices)
    }

    pub fn draw_mesh(&mut self, cx: &mut CxDraw, mesh: PbrMeshHandle) -> Result<(), String> {
        if self.many_instances_mesh.is_some() {
            if self.many_instances_mesh != Some(mesh) {
                return Err(format!(
                    "DrawPbr many-instance batch active for mesh {:?}, requested {:?}",
                    self.many_instances_mesh, mesh
                ));
            }
            if let Some(instances) = self.many_instances.as_mut() {
                instances
                    .instances
                    .extend_from_slice(self.draw_vars.as_slice());
                return Ok(());
            }
            return Err("DrawPbr many-instance state invalid".to_string());
        }

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
            let new_area = cx.add_instance(&self.draw_vars);
            self.draw_vars.area = cx.update_area_refs(self.draw_vars.area, new_area);
        }
        Ok(())
    }

    pub fn begin_many_instances_for_mesh(
        &mut self,
        cx: &mut CxDraw,
        mesh: PbrMeshHandle,
    ) -> Result<(), String> {
        if self.many_instances.is_some() {
            if self.many_instances_mesh == Some(mesh) {
                return Ok(());
            }
            return Err(format!(
                "DrawPbr many-instance batch already active for mesh {:?}",
                self.many_instances_mesh
            ));
        }

        let geom = self
            .meshes
            .get(mesh)
            .ok_or_else(|| format!("invalid mesh handle {mesh}"))?;
        self.draw_vars.geometry_id = Some(geom.geometry_id());
        self.apply_draw_uniforms(cx);
        let Some(instances) = cx.begin_many_instances(&self.draw_vars) else {
            return Err("DrawPbr begin_many_instances failed".to_string());
        };
        self.many_instances = Some(instances);
        self.many_instances_mesh = Some(mesh);
        Ok(())
    }

    pub fn push_many_instance_with_transform(&mut self, transform: Mat4f) {
        let prev_model = self.model_matrix;
        self.model_matrix = transform;
        if let Some(instances) = self.many_instances.as_mut() {
            instances
                .instances
                .extend_from_slice(self.draw_vars.as_slice());
        } else {
            debug_assert!(
                false,
                "DrawPbr::push_many_instance_with_transform called without active batch"
            );
        }
        self.model_matrix = prev_model;
    }

    pub fn end_many_instances(&mut self, cx: &mut CxDraw) {
        if let Some(instances) = self.many_instances.take() {
            let new_area = cx.end_many_instances(instances);
            self.draw_vars.area = cx.update_area_refs(self.draw_vars.area, new_area);
        }
        self.many_instances_mesh = None;
    }

    pub fn end(&mut self, cx: &mut CxDraw) {
        self.flush(cx);
    }

    /// Submit currently accumulated geometry as one draw call and clear buffers.
    /// Useful when emitting one draw call per primitive/material.
    pub fn flush(&mut self, cx: &mut CxDraw) {
        if self.acc_verts.is_empty() || self.acc_indices.is_empty() {
            return;
        }
        let geom = self.geometry.get_or_insert_with(|| Geometry::new(cx.cx));
        geom.update_with_recycled_buffers(cx.cx, &mut self.acc_indices, &mut self.acc_verts);
        self.draw_vars.geometry_id = Some(geom.geometry_id());
        self.apply_draw_uniforms(cx);
        cx.new_draw_call(&self.draw_vars);
        if self.draw_vars.can_instance() {
            let new_area = cx.add_instance(&self.draw_vars);
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
            generated_tangents = Self::compute_tangent_frame(positions, normals, uvs, &out_indices);
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
                pos[0], pos[1], pos[2], src_n[0], src_n[1], src_n[2], uv[0], uv[1], color[0],
                color[1], color[2], color[3], src_t[0], src_t[1], src_t[2], src_t[3],
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
            let w = if bitangent.dot(tan2[i]) < 0.0 {
                -1.0
            } else {
                1.0
            };
            out[i] = [tangent.x, tangent.y, tangent.z, w];
        }
        out
    }
}
