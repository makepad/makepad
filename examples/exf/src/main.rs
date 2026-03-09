pub use makepad_widgets;

use makepad_mb3d_render::exr_meta::{
    ViewerCameraMetadata, MB3D_CAMERA_ATTRIBUTE_NAME, MB3D_MIP_LEVEL_ATTRIBUTE_NAME,
};
use makepad_openexr::{read_headers_file, read_part_file, ExrPart, SampleBuffer};
use makepad_widgets::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

app_main!(App);

const DEFAULT_STYLE_MIX: f32 = 0.58;
const DEFAULT_CONTOUR_GAIN: f32 = 0.72;
const DEFAULT_GLOW_GAIN: f32 = 0.44;
const DEFAULT_HALO_WIDTH: f32 = 1.0;
const DEFAULT_HALO_FOG: f32 = 1.0;
const DEFAULT_LIGHT_GAIN: f32 = 1.8;
const DEFAULT_LIGHT_RADIUS: f32 = 1.5;
const DEFAULT_LIGHT_LIFT: f32 = 0.45;
const MIN_ZOOM: f32 = 0.5;
const MAX_ZOOM: f32 = 8.0;

#[derive(Clone, Copy)]
enum ChannelSlot {
    Named(&'static str),
    Constant(f32),
}

#[derive(Clone, Copy)]
struct PackSpec {
    label: &'static str,
    channels: [ChannelSlot; 4],
}

const PACK_SPECS: [PackSpec; 9] = [
    PackSpec {
        label: "surface",
        channels: [
            ChannelSlot::Named("R"),
            ChannelSlot::Named("G"),
            ChannelSlot::Named("B"),
            ChannelSlot::Named("ambient_occlusion.AO"),
        ],
    },
    PackSpec {
        label: "flow",
        channels: [
            ChannelSlot::Named("branches.Reciprocal"),
            ChannelSlot::Named("branches.Outer"),
            ChannelSlot::Named("gradient.Phase"),
            ChannelSlot::Named("roughness.Roughness"),
        ],
    },
    PackSpec {
        label: "metrics",
        channels: [
            ChannelSlot::Named("depth.Depth"),
            ChannelSlot::Named("estimator.DE"),
            ChannelSlot::Named("iterations.Iterations"),
            ChannelSlot::Named("march_steps.Steps"),
        ],
    },
    PackSpec {
        label: "folds",
        channels: [
            ChannelSlot::Named("folds.X"),
            ChannelSlot::Named("folds.Y"),
            ChannelSlot::Named("folds.Z"),
            ChannelSlot::Named("folds.Any"),
        ],
    },
    PackSpec {
        label: "normal",
        channels: [
            ChannelSlot::Named("normal.X"),
            ChannelSlot::Named("normal.Y"),
            ChannelSlot::Named("normal.Z"),
            ChannelSlot::Named("orbit.R2"),
        ],
    },
    PackSpec {
        label: "orbit",
        channels: [
            ChannelSlot::Named("orbit.X"),
            ChannelSlot::Named("orbit.Y"),
            ChannelSlot::Named("orbit.Z"),
            ChannelSlot::Named("orbit.W"),
        ],
    },
    PackSpec {
        label: "style",
        channels: [
            ChannelSlot::Named("sign_flips.X"),
            ChannelSlot::Named("sign_flips.Y"),
            ChannelSlot::Named("sign_flips.Z"),
            ChannelSlot::Constant(1.0),
        ],
    },
    PackSpec {
        label: "traps",
        channels: [
            ChannelSlot::Named("traps.X"),
            ChannelSlot::Named("traps.Y"),
            ChannelSlot::Named("traps.Z"),
            ChannelSlot::Named("traps.R"),
        ],
    },
    PackSpec {
        label: "uncertainty",
        channels: [
            ChannelSlot::Named("uncertainty.Clamp"),
            ChannelSlot::Named("uncertainty.Overshoot"),
            ChannelSlot::Named("uncertainty.RSFMin"),
            ChannelSlot::Named("uncertainty.Refine"),
        ],
    },
];

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    set_type_default() do #(DrawExf::script_shader(vm)){
        ..mod.draw.DrawQuad
        surface_tex: texture_2d(float)
        flow_tex: texture_2d(float)
        metrics_tex: texture_2d(float)
        folds_tex: texture_2d(float)
        normal_tex: texture_2d(float)
        orbit_tex: texture_2d(float)
        style_tex: texture_2d(float)
        traps_tex: texture_2d(float)
        uncertainty_tex: texture_2d(float)

        loaded: 0.0
        style_mix: 0.58
        contour_gain: 0.72
        glow_gain: 0.44
        halo_width: 1.0
        halo_fog: 1.0
        light_gain: 1.8
        light_radius: 1.5
        light_lift: 0.45
        time: 0.0
        debug_view: 0.0
        zoom: 1.0
        max_mip_level: 0.0
        pan: vec2(0.0, 0.0)
        image_size: vec2(1.0, 1.0)
        depth_min: 0.0
        depth_inv_range: 1.0
        camera_mid: vec3(0.0, 0.0, 0.0)
        camera_right_step: vec3(1.0, 0.0, 0.0)
        camera_up_step: vec3(0.0, 1.0, 0.0)
        camera_forward_dir: vec3(0.0, 0.0, 1.0)
        camera_fov_y: 45.0
        camera_z_start_delta: 0.0

        viewport_uv: fn() -> vec2 {
            let rect_aspect = self.rect_size.x / max(self.rect_size.y, 0.0001)
            let image_aspect = self.image_size.x / max(self.image_size.y, 0.0001)
            let mut p = self.pos - vec2(0.5, 0.5)

            if rect_aspect > image_aspect {
                p.x = p.x * rect_aspect / image_aspect
            } else {
                p.y = p.y * image_aspect / rect_aspect
            }

            return p / max(self.zoom, 0.001) + self.pan + vec2(0.5, 0.5)
        }

        auto_lod: fn() -> float {
            let fit_scale = min(
                self.rect_size.x / max(self.image_size.x, 1.0),
                self.rect_size.y / max(self.image_size.y, 1.0)
            )
            let pixel_scale = fit_scale * max(self.zoom, 0.001)
            let lod = if pixel_scale >= 1.0 {
                0.0
            } else {
                log2(1.0 / max(pixel_scale, 0.0001))
            }
            return clamp(lod, 0.0, self.max_mip_level)
        }

        selected_lod: fn() -> float {
            return floor(self.auto_lod() + 0.5)
        }

        camera_ray: fn(uv: vec2) -> vec3 {
            let aspect = self.image_size.x / max(self.image_size.y, 1.0)
            let tan_half_fov = tan(self.camera_fov_y * 0.017453292519943295 * 0.5)
            let plane = (vec2(0.5, 0.5) - uv) * vec2(aspect, 1.0) * (tan_half_fov * 2.0)
            return normalize(vec3(plane.x, plane.y, -1.0))
        }

        camera_position: fn(uv: vec2, raw_depth: float) -> vec3 {
            return self.camera_ray(uv) * raw_depth
        }

        backdrop: fn(grid_uv: vec2) -> vec3 {
            let grid = abs(fract(grid_uv) - 0.5)
            let line = (1.0 - clamp(min(grid.x, grid.y) * 18.0, 0.0, 1.0)) * 0.06
            let glow = 0.024 + 0.010 * clamp(1.0 - length(grid_uv - vec2(6.0, 6.0)) * 0.10, 0.0, 1.0)
            return vec3(0.04, 0.06, 0.08) + vec3(line + glow, line, line * 1.4)
        }

        rainbow_band: fn(phase: float) -> vec3 {
            return 0.5 + 0.5 * vec3(
                sin(phase),
                sin(phase + 2.0943951),
                sin(phase + 4.1887902)
            )
        }

        stone_palette: fn(phase: float) -> vec3 {
            let phase_a = 0.5 + 0.5 * sin(phase * 6.2831853)
            let phase_b = 0.5 + 0.5 * sin(phase * 6.2831853 + 2.2)
            let phase_c = 0.5 + 0.5 * sin(phase * 12.5663706 + 0.8)
            return vec3(0.04, 0.07, 0.12)
                .mix(vec3(0.16, 0.44, 0.88), phase_a * 0.76)
                .mix(vec3(0.30, 0.88, 1.10), phase_b * 0.28)
                .mix(vec3(0.86, 0.96, 1.08), phase_c * 0.12)
        }

        fog_palette: fn(far_mix: float) -> vec3 {
            return vec3(0.03, 0.06, 0.10)
                .mix(vec3(0.08, 0.18, 0.34), clamp(far_mix, 0.0, 1.0))
                .mix(vec3(0.44, 0.82, 1.04), clamp(far_mix * far_mix, 0.0, 1.0) * 0.82)
        }

        display_map: fn(color: vec3) -> vec3 {
            let lifted = color * 1.04
            let mapped = vec3(
                lifted.x / (1.0 + lifted.x * 0.72),
                lifted.y / (1.0 + lifted.y * 0.72),
                lifted.z / (1.0 + lifted.z * 0.72)
            )
            let contrasted = clamp(mapped - vec3(0.018, 0.016, 0.014), vec3(0.0), vec3(1.0))
            let luma = dot(contrasted, vec3(0.2126, 0.7152, 0.0722))
            let graded = contrasted + (contrasted - vec3(luma, luma, luma)) * 0.24
            return vec3(
                pow(clamp(graded.x, 0.0, 1.0), 0.98),
                pow(clamp(graded.y, 0.0, 1.0), 0.98),
                pow(clamp(graded.z, 0.0, 1.0), 0.98)
            )
        }

        feature_spark_mask: fn(feature_a: float, feature_b: float, feature_c: float, roughness: float, edge_glow: float) -> float {
            let stripe_a = pow(clamp(1.0 - abs(sin(feature_a * 6.2831853)), 0.0, 1.0), 8.0)
            let stripe_b = pow(clamp(1.0 - abs(sin(feature_b * 6.2831853)), 0.0, 1.0), 8.0)
            let stripe_c = pow(clamp(1.0 - abs(sin(feature_c * 6.2831853)), 0.0, 1.0), 6.0)
            return stripe_a
                * stripe_b
                * (0.30 + 0.70 * stripe_c)
                * (0.18 + 0.82 * edge_glow)
                * (0.20 + 0.80 * (1.0 - roughness))
        }

        trap_strength: fn(trap: vec4) -> float {
            return clamp(length(trap.xyz - vec3(0.5, 0.5, 0.5)) * 1.8 + trap.w, 0.0, 1.0)
        }

        raw_depth_value: fn(uv: vec2, lod: float) -> float {
            return max(
                self.metrics_tex.sample_lod(uv, clamp(lod, 0.0, self.max_mip_level)).x,
                0.0
            )
        }

        depth_value: fn(uv: vec2, lod: float) -> float {
            return clamp(
                (self.raw_depth_value(uv, lod) - self.depth_min) * self.depth_inv_range,
                0.0,
                1.0
            )
        }

        world_normal: fn(uv: vec2, lod: float) -> vec3 {
            let raw = self.normal_tex.sample_lod(uv, clamp(lod, 0.0, self.max_mip_level)).xyz
            let len_sq = dot(raw, raw)
            if len_sq <= 1e-8 {
                return vec3(0.0, 0.0, 1.0)
            }
            return raw / sqrt(len_sq)
        }

        camera_basis_normal: fn(world_normal: vec3) -> vec3 {
            let right_dir = normalize(self.camera_right_step)
            let up_dir = normalize(self.camera_up_step)
            let forward_dir = normalize(self.camera_forward_dir)
            let basis = vec3(
                dot(world_normal, right_dir),
                dot(world_normal, up_dir),
                dot(world_normal, forward_dir)
            )
            let len_sq = dot(basis, basis)
            if len_sq <= 1e-8 {
                return vec3(0.0, 0.0, 1.0)
            }
            return basis / sqrt(len_sq)
        }

        depth_gauss_mip: fn(uv: vec2, lod: float, radius: float) -> float {
            let center_lod = clamp(lod + max(radius, 0.0), 0.0, self.max_mip_level)
            let lower_lod = clamp(center_lod - 1.0, 0.0, self.max_mip_level)
            let upper_lod = clamp(center_lod + 1.0, 0.0, self.max_mip_level)
            let lower = self.depth_value(uv, lower_lod)
            let center = self.depth_value(uv, center_lod)
            let upper = self.depth_value(uv, upper_lod)
            return clamp((lower + center * 2.0 + upper) * 0.25, 0.0, 1.0)
        }

        reconstruct_position: fn(uv: vec2, raw_depth: float) -> vec3 {
            let image_width = max(self.image_size.x, 1.0)
            let image_height = max(self.image_size.y, 1.0)
            let sample_x = uv.x * image_width - 0.5
            let sample_y = uv.y * image_height - 0.5
            let half_w = image_width * 0.5
            let half_h = image_height * 0.5
            let fov_mul = self.camera_fov_y * 0.017453292519943295 / image_height
            let cafx = (half_w - sample_x) * fov_mul
            let cafy = (sample_y - half_h) * fov_mul
            let local_dir = normalize(vec3(-sin(cafx), sin(cafy), cos(cafx) * cos(cafy)))
            let right_dir = normalize(self.camera_right_step)
            let up_dir = normalize(self.camera_up_step)
            let forward_dir = normalize(self.camera_forward_dir)
            let ray_dir = normalize(
                right_dir * local_dir.x
                + up_dir * local_dir.y
                + forward_dir * local_dir.z
            )
            let ray_origin =
                self.camera_mid
                + forward_dir * self.camera_z_start_delta
                + self.camera_right_step * (sample_x - half_w)
                + self.camera_up_step * (sample_y - half_h)
            return ray_origin + ray_dir * raw_depth
        }

        pixel: fn() {
            let uv = self.viewport_uv()
            let grid_uv = self.pos * vec2(12.0, 12.0)
            let lod = self.selected_lod()
            let mip_scale = pow(2.0, max(lod, 0.0))
            let inside =
                uv.x >= 0.0 &&
                uv.x <= 1.0 &&
                uv.y >= 0.0 &&
                uv.y <= 1.0 &&
                self.loaded > 0.5

            if !inside {
                return Pal.premul(vec4(self.backdrop(grid_uv), 1.0))
            }

            let surface = self.surface_tex.sample_lod(uv, lod)
            let flow = self.flow_tex.sample_lod(uv, lod)
            let metrics = self.metrics_tex.sample_lod(uv, lod)
            let folds = self.folds_tex.sample_lod(uv, lod)
            let normal_pack = self.normal_tex.sample_lod(uv, lod)
            let orbit = self.orbit_tex.sample_lod(uv, lod)
            let style = self.style_tex.sample_lod(uv, lod)
            let traps = self.traps_tex.sample_lod(uv, lod)
            let uncertainty = self.uncertainty_tex.sample_lod(uv, lod)

            let t = self.time * 0.35
            let texel = vec2(
                mip_scale / max(self.image_size.x, 1.0),
                mip_scale / max(self.image_size.y, 1.0)
            )
            let uv_px = clamp(uv + vec2(texel.x, 0.0), vec2(0.0, 0.0), vec2(1.0, 1.0))
            let uv_mx = clamp(uv - vec2(texel.x, 0.0), vec2(0.0, 0.0), vec2(1.0, 1.0))
            let uv_py = clamp(uv + vec2(0.0, texel.y), vec2(0.0, 0.0), vec2(1.0, 1.0))
            let uv_my = clamp(uv - vec2(0.0, texel.y), vec2(0.0, 0.0), vec2(1.0, 1.0))
            let ao = clamp(surface.w, 0.0, 1.0)
            let raw_normal = self.world_normal(uv, lod)
            let normal_px = self.world_normal(uv_px, lod)
            let normal_mx = self.world_normal(uv_mx, lod)
            let normal_py = self.world_normal(uv_py, lod)
            let normal_my = self.world_normal(uv_my, lod)
            if self.debug_view > 0.5 && self.debug_view < 1.5 {
                let normal_rgb = clamp(raw_normal * 0.5 + vec3(0.5, 0.5, 0.5), vec3(0.0), vec3(1.0))
                return Pal.premul(vec4(normal_rgb, 1.0))
            }
            let raw_depth = max(metrics.x, 0.0)
            let depth = clamp((raw_depth - self.depth_min) * self.depth_inv_range, 0.0, 1.0)
            let depth_px = self.depth_value(uv_px, lod)
            let depth_mx = self.depth_value(uv_mx, lod)
            let depth_py = self.depth_value(uv_py, lod)
            let depth_my = self.depth_value(uv_my, lod)
            let glow_radius_a = 0.35 + self.halo_width * 1.6
            let glow_radius_b = glow_radius_a + 0.75 + self.halo_width * 0.9
            let halo_depth_a = self.depth_gauss_mip(uv, lod, glow_radius_a)
            let halo_depth_b = self.depth_gauss_mip(uv, lod, glow_radius_b)
            let estimator = clamp(metrics.y, 0.0, 1.0)
            let iterations = clamp(metrics.z, 0.0, 1.0)
            let march_steps = clamp(metrics.w, 0.0, 1.0)
            let roughness = clamp(flow.w, 0.0, 1.0)
            let pos = self.reconstruct_position(uv, raw_depth)
            let view_pos = self.camera_position(uv, raw_depth)
            let view_dir = normalize(view_pos * -1.0)
            let normal = if dot(raw_normal, view_dir) < 0.0 {
                raw_normal * -1.0
            } else {
                raw_normal
            }
            let orbit_r2 = clamp(normal_pack.w, 0.0, 1.0)
            let far_mix = 1.0 - depth
            let halo_far_a = 1.0 - halo_depth_a
            let halo_far_b = 1.0 - halo_depth_b
            let halo_open_a = clamp((halo_depth_a - depth) * 2.8, 0.0, 1.0)
            let halo_open_b = clamp((halo_depth_b - depth) * 2.2, 0.0, 1.0)
            let halo_fog = self.halo_fog
            let normal_edge = (length(normal_px - normal_mx) + length(normal_py - normal_my)) * 0.34
            let depth_edge = (abs(depth_px - depth_mx) + abs(depth_py - depth_my)) * 3.2
            let edge_glow = clamp(normal_edge + depth_edge + far_mix * 0.04, 0.0, 1.0)
            let rim = pow(clamp(1.0 - normal.z, 0.0, 1.0), 2.0)
            let contour =
                clamp(
                    far_mix * 0.56
                    + estimator * 0.42
                    + iterations * 0.18
                    + march_steps * 0.14,
                    0.0,
                    1.0
                )
                * self.contour_gain
            let trap_energy = clamp(length(traps.xyz - vec3(0.5, 0.5, 0.5)) * 1.8 + traps.w, 0.0, 1.0)
            let uncertainty_band = clamp(uncertainty.z * 1.2 + uncertainty.x * 0.8 + uncertainty.y * 0.5, 0.0, 1.0)
            let fold_grid = clamp((folds.x + folds.y + folds.z + folds.w) * 0.35, 0.0, 1.0)
            let orbit_heat = clamp(length(orbit.xyz - vec3(0.5, 0.5, 0.5)) * 1.2 + orbit.w * 0.35 + orbit_r2 * 0.35, 0.0, 1.0)
            let branch_bias = clamp(flow.x * 0.7 + flow.y * 0.5 + flow.z * 0.25, 0.0, 1.0)
            let sign_echo = clamp((style.x + style.y + style.z) * 0.33, 0.0, 1.0)
            let phase = fract(flow.z)
            let pulse = 0.5 + 0.5 * sin(orbit.x * 8.0 + orbit.y * 5.0 + orbit.z * 3.0)
            let beauty_ribbon = 0.5 + 0.5 * sin(
                pos.x * 18.0
                - pos.y * 14.0
                + pos.z * 11.0
                + depth * 20.0
            )
            let particle_uv = vec2(0.50, 0.50)
                + vec2(sin(t * 0.73), cos(t * 0.51)) * vec2(0.08, 0.06)
            let particle_raw_depth = self.raw_depth_value(particle_uv, lod)
            let particle_depth = clamp((particle_raw_depth - self.depth_min) * self.depth_inv_range, 0.0, 1.0)
            let particle_screen = (particle_uv - uv)
                * vec2(self.image_size.x / max(self.image_size.y, 1.0), 1.0)
            let light_lift_world = self.light_lift * 0.18 / max(self.depth_inv_range, 0.000001)
            let particle_anchor = self.camera_position(particle_uv, particle_raw_depth)
            let particle_pos = particle_anchor - self.camera_ray(particle_uv) * light_lift_world
            let particle_delta = particle_pos - view_pos
            let particle_dist = max(length(particle_delta), 0.0001)
            let particle_flat_dir = normalize(vec3(
                particle_screen.x * 2.6,
                particle_screen.y * 2.6,
                -1.75
            ))
            let particle_point_dir = particle_delta / particle_dist
            let particle_dir = normalize(particle_flat_dir.mix(particle_point_dir, 0.58))
            let particle_half = normalize(particle_dir + view_dir)
            let particle_ndotl = clamp(dot(normal, particle_dir), 0.0, 1.0)
            let particle_wrap = clamp(dot(normal, particle_dir) * 0.5 + 0.5, 0.0, 1.0)
            let particle_spec = pow(
                clamp(dot(normal, particle_half), 0.0, 1.0),
                10.0 + (1.0 - roughness) * 42.0
            )
            let particle_gloss = pow(
                particle_wrap,
                8.0 + (1.0 - roughness) * 20.0
            )
            let particle_screen_dist = length(particle_screen)
            let particle_radius = self.light_radius * (1.0 + self.halo_width * 0.35)
            let particle_sprite = pow(
                clamp(1.0 - particle_screen_dist * (2.8 / particle_radius), 0.0, 1.0),
                3.6
            )
            let particle_core = pow(
                clamp(1.0 - particle_screen_dist * (5.2 / particle_radius), 0.0, 1.0),
                5.4
            )
            let particle_haze = pow(
                clamp(
                    1.0 - particle_screen_dist * (1.45 / (1.0 + self.halo_width * 1.8)),
                    0.0,
                    1.0
                ),
                2.2
            )
            let particle_depth_gate = clamp(1.0 - abs(depth - particle_depth) * 2.6, 0.0, 1.0)
            let particle_range_falloff =
                1.0
                / (
                    1.0
                    + particle_dist
                    * self.depth_inv_range
                    * (0.85 / max(self.light_radius, 0.25))
                )
            let particle_screen_falloff = particle_sprite * 0.35 + particle_core * 0.65
            let particle_attenuation =
                particle_screen_falloff
                * particle_range_falloff
                * (0.30 + particle_depth_gate * 0.70)
            let particle_color = vec3(0.16, 0.90, 1.26)
                .mix(vec3(1.10, 0.96, 0.54), 0.5 + 0.5 * sin(t * 0.41 + 0.8))
            let base_albedo = self.stone_palette(phase)
                .mix(vec3(0.78, 0.96, 1.08), fold_grid * 0.14 + orbit_r2 * 0.10)
                .mix(vec3(0.06, 0.12, 0.24), trap_energy * 0.12)
            let ao_term = 0.18 + ao * 0.82
            let particle_relight = particle_color
                * particle_attenuation
                * self.light_gain
                * (particle_wrap * 0.34 + particle_ndotl * 0.28)
            let particle_specular = particle_color.mix(vec3(1.0, 1.0, 1.0), 0.45)
                * max(particle_spec * 1.8, particle_gloss * 0.55)
                * particle_attenuation
                * self.light_gain
                * (7.5 + self.glow_gain * 3.2)
                * (0.35 + 0.65 * pow(clamp(1.0 - dot(normal, view_dir), 0.0, 1.0), 3.0))
            if self.debug_view > 1.5 && self.debug_view < 2.5 {
                let light_debug = vec3(
                    particle_attenuation,
                    particle_ndotl,
                    particle_wrap
                )
                return Pal.premul(vec4(clamp(light_debug, vec3(0.0), vec3(1.0)), 1.0))
            }
            if self.debug_view > 2.5 {
                let spec_debug = clamp(particle_specular * 6.0, vec3(0.0), vec3(1.0))
                return Pal.premul(vec4(spec_debug, 1.0))
            }
            let base_layer = base_albedo * ao_term
            let fog_shape = clamp(
                halo_open_a * 0.40
                + halo_open_b * 0.78
                + edge_glow * 0.18
                + contour * 0.08,
                0.0,
                1.0
            )
            let fog_depth_mix = clamp(halo_far_b * 0.72 + halo_far_a * 0.18 + far_mix * 0.10, 0.0, 1.0)
            let fog_color = self.fog_palette(fog_depth_mix)
                .mix(particle_color, 0.24 + 0.22 * particle_wrap)
            let base_fog_amount = clamp(
                far_mix * 0.08
                + halo_open_a * 0.06
                + halo_open_b * 0.10 * halo_fog,
                0.0,
                0.42
            )
            let halo_glow = fog_color
                * particle_haze
                * particle_range_falloff
                * (0.24 + particle_depth_gate * 0.76)
                * fog_shape
                * halo_fog
                * self.light_gain
                * (0.16 + self.glow_gain * 0.34)
            let mut color = base_layer.mix(fog_color, base_fog_amount)
                + particle_relight
                + particle_specular
                + halo_glow
            color = self.display_map(clamp(color, vec3(0.0), vec3(3.0)))
            return Pal.premul(vec4(clamp(color, vec3(0.0), vec3(1.0)), 1.0))
        }
    }

    mod.widgets.ExfViewportBase = #(ExfViewport::register_widget(vm))
    mod.widgets.ExfViewport = set_type_default() do mod.widgets.ExfViewportBase {
        width: Fill
        height: Fill
        draw_bg +: {
            loaded: 0.0
            style_mix: 0.58
            contour_gain: 0.72
            glow_gain: 0.44
            zoom: 1.0
            max_mip_level: 0.0
            pan: vec2(0.0, 0.0)
            image_size: vec2(1.0, 1.0)
        }
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(1680, 980)
                window.title: "EXF Viewer"
                pass.clear_color: vec4(0.03, 0.05, 0.07, 1.0)
                body +: {
                    root := View{
                        width: Fill
                        height: Fill
                        flow: Right
                        spacing: 0.0

                        rail := SolidView{
                            width: 330
                            height: Fill
                            flow: Down
                            spacing: 12.0
                            padding: Inset{left: 20.0, top: 20.0, right: 20.0, bottom: 20.0}
                            draw_bg +: {
                                color: #x0d1620
                            }

                            title := H2{text: "EXF Viewer"}
                            subtitle := P{
                                text: "Latest fractal EXR, reinterpreted through a shader bank."
                            }

                            Hr{}
                            tuning_title := Labelbold{text: "Tuning"}
                            reload_latest_button := Button{text: "Reload Latest"}
                            reset_view_button := Button{text: "Reset View"}
                            toggle_normal_debug_button := Button{text: "Cycle Debug View"}
                            style_slider := SliderRoundFlat{
                                text: "Style Mix"
                                min: 0.0
                                max: 1.0
                                step: 0.01
                                precision: 2
                            }
                            contour_slider := SliderRoundFlat{
                                text: "Contour"
                                min: 0.0
                                max: 2.0
                                step: 0.01
                                precision: 2
                            }
                            glow_slider := SliderRoundFlat{
                                text: "Glow"
                                min: 0.0
                                max: 2.0
                                step: 0.01
                                precision: 2
                            }
                            light_gain_slider := SliderRoundFlat{
                                text: "Light Gain"
                                min: 0.0
                                max: 3.0
                                step: 0.01
                                precision: 2
                            }
                            light_radius_slider := SliderRoundFlat{
                                text: "Light Radius"
                                min: 0.25
                                max: 4.0
                                step: 0.01
                                precision: 2
                            }
                            light_lift_slider := SliderRoundFlat{
                                text: "Light Lift"
                                min: 0.0
                                max: 2.5
                                step: 0.01
                                precision: 2
                            }
                            halo_width_slider := SliderRoundFlat{
                                text: "Halo Width"
                                min: 0.0
                                max: 4.0
                                step: 0.01
                                precision: 2
                            }
                            halo_fog_slider := SliderRoundFlat{
                                text: "Halo Fog"
                                min: 0.0
                                max: 3.0
                                step: 0.01
                                precision: 2
                            }

                            Hr{}
                            mip_title := Labelbold{text: "Mip Lookup"}
                            mip_value := TextBox{text: "Base only"}
                            view_mode_title := Labelbold{text: "View Mode"}
                            view_mode_value := TextBox{text: "Beauty"}
                            file_title := Labelbold{text: "Loaded File"}
                            file_value := TextBox{text: "Scanning /tmp for mb3d EXR renders..."}
                            status_title := Labelbold{text: "Status"}
                            status_value := TextBox{text: "Waiting for draw pass"}

                            Filler{}
                            hint := P{
                                text: "Default source: the newest /tmp/mb3d*.exr render, or pass --file=/path/to/file.exr."
                            }
                            packs := P{text: ""}
                        }

                        viewer_shell := SolidView{
                            width: Fill
                            height: Fill
                            padding: Inset{left: 16.0, top: 16.0, right: 16.0, bottom: 16.0}
                            draw_bg +: {
                                color: #x060b11
                            }
                            viewer := mod.widgets.ExfViewport{
                                width: Fill
                                height: Fill
                            }
                        }
                    }
                }
            }
        }
    }
}

impl App {
    fn viewport_ref(&self, cx: &mut Cx) -> WidgetRef {
        let direct = self.ui.widget(cx, ids!(viewer));
        let flood = self.ui.widget_flood(cx, ids!(viewer));
        if flood.borrow::<ExfViewport>().is_some() {
            flood
        } else {
            direct
        }
    }

    fn set_file_value(&self, cx: &mut Cx, value: &str) {
        self.ui.label(cx, ids!(file_value)).set_text(cx, value);
    }

    fn set_status_value(&self, cx: &mut Cx, value: &str) {
        self.ui.label(cx, ids!(status_value)).set_text(cx, value);
    }

    fn set_mip_value(&self, cx: &mut Cx, value: &str) {
        self.ui.label(cx, ids!(mip_value)).set_text(cx, value);
    }

    fn set_view_mode_value(&self, cx: &mut Cx, value: &str) {
        self.ui.label(cx, ids!(view_mode_value)).set_text(cx, value);
    }

    fn queue_latest_render(&mut self, cx: &mut Cx) {
        if let Some(path) = find_latest_render_exr() {
            self.pending_exr_path = Some(path.clone());
            self.loaded_summary = None;
            self.available_mips.clear();
            self.displayed_mip_value.clear();
            self.set_file_value(cx, &path.display().to_string());
            self.set_status_value(cx, "Queued latest render EXR");
            self.set_mip_value(cx, "Scanning mip parts...");
        } else {
            self.pending_exr_path = None;
            self.set_file_value(cx, "No mb3d EXR renders found in /tmp");
            self.set_status_value(cx, "Nothing to load");
            self.set_mip_value(cx, "Base only");
        }
    }

    fn try_load_exr(&mut self, cx: &mut Cx) {
        let Some(path) = self.pending_exr_path.take() else {
            return;
        };

        let viewer = self.viewport_ref(cx);
        let Some(mut inner) = viewer.borrow_mut::<ExfViewport>() else {
            self.pending_exr_path = Some(path);
            self.ui.redraw(cx);
            return;
        };

        let source = match discover_viewer_source(&path) {
            Ok(source) => source,
            Err(err) => {
                self.set_file_value(cx, &path.display().to_string());
                self.set_status_value(cx, &format!("Load failed: {err}"));
                self.set_mip_value(cx, "Unavailable");
                self.ui.redraw(cx);
                cx.redraw_all();
                return;
            }
        };
        let load_result = inner.load_exr_path(cx, &path, &source.mip_parts, &source.camera);
        drop(inner);

        match load_result {
            Ok(summary) => {
                self.loaded_summary = Some(summary);
                self.available_mips = source.mip_parts;
                self.displayed_mip_value.clear();
                self.set_file_value(cx, &path.display().to_string());
                self.set_status_value(
                    cx,
                    &format!(
                        "Loaded {}x{} EXR, {} channels, {} texture packs, {} mip levels; auto mip lookup follows zoom",
                        summary.width,
                        summary.height,
                        summary.channel_count,
                        PACK_SPECS.len(),
                        summary.mip_count
                    ),
                );
            }
            Err(err) => {
                self.set_file_value(cx, &path.display().to_string());
                self.set_status_value(cx, &format!("Load failed: {err}"));
                self.set_mip_value(cx, "Unavailable");
            }
        }

        self.ui.redraw(cx);
        cx.redraw_all();
    }

    fn update_auto_mip_display(&mut self, cx: &mut Cx) {
        let value = if self.available_mips.is_empty() {
            "Base only".to_string()
        } else {
            let viewer = self.viewport_ref(cx);
            let lod = {
                let Some(inner) = viewer.borrow::<ExfViewport>() else {
                    return;
                };
                inner.auto_mip_level(cx)
            };
            format_auto_mip_value(&self.available_mips, lod)
        };

        if value != self.displayed_mip_value {
            self.displayed_mip_value = value.clone();
            self.set_mip_value(cx, &value);
        }
    }

}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    pending_exr_path: Option<PathBuf>,
    #[rust]
    available_mips: Vec<ViewerMipPart>,
    #[rust]
    loaded_summary: Option<LoadedExrSummary>,
    #[rust]
    displayed_mip_value: String,
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        self.ui
            .slider(cx, ids!(style_slider))
            .set_value(cx, DEFAULT_STYLE_MIX as f64);
        self.ui
            .slider(cx, ids!(contour_slider))
            .set_value(cx, DEFAULT_CONTOUR_GAIN as f64);
        self.ui
            .slider(cx, ids!(glow_slider))
            .set_value(cx, DEFAULT_GLOW_GAIN as f64);
        self.ui
            .slider(cx, ids!(light_gain_slider))
            .set_value(cx, DEFAULT_LIGHT_GAIN as f64);
        self.ui
            .slider(cx, ids!(light_radius_slider))
            .set_value(cx, DEFAULT_LIGHT_RADIUS as f64);
        self.ui
            .slider(cx, ids!(light_lift_slider))
            .set_value(cx, DEFAULT_LIGHT_LIFT as f64);
        self.ui
            .slider(cx, ids!(halo_width_slider))
            .set_value(cx, DEFAULT_HALO_WIDTH as f64);
        self.ui
            .slider(cx, ids!(halo_fog_slider))
            .set_value(cx, DEFAULT_HALO_FOG as f64);
        self.ui
            .label(cx, ids!(packs))
            .set_text(cx, &format!("GPU bank: {}", pack_schema_summary()));
        self.displayed_mip_value = "Base only".to_string();
        self.set_mip_value(cx, "Base only");
        self.set_view_mode_value(cx, "Beauty");

        let args: Vec<String> = std::env::args().collect();
        if let Some(path) = resolve_startup_exr_path(&args) {
            self.pending_exr_path = Some(path.clone());
            self.loaded_summary = None;
            self.set_file_value(cx, &path.display().to_string());
            self.set_status_value(cx, "Queued EXR for load");
            self.set_mip_value(cx, "Scanning mip parts...");
        } else {
            self.set_file_value(cx, "No EXR path provided and no /tmp/mb3d*.exr render was found");
            self.set_status_value(cx, "Viewer is idle");
            self.set_mip_value(cx, "Base only");
        }
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.ui.button(cx, ids!(reload_latest_button)).clicked(actions) {
            self.queue_latest_render(cx);
        }
        if self.ui.button(cx, ids!(reset_view_button)).clicked(actions) {
            let viewer = self.viewport_ref(cx);
            if let Some(mut inner) = viewer.borrow_mut::<ExfViewport>() {
                inner.reset_view(cx);
            };
        }
        if self
            .ui
            .button(cx, ids!(toggle_normal_debug_button))
            .clicked(actions)
        {
            let viewer = self.viewport_ref(cx);
            if let Some(mut inner) = viewer.borrow_mut::<ExfViewport>() {
                inner.cycle_debug_view(cx);
                self.set_view_mode_value(cx, inner.view_mode_label());
            };
        }
        if let Some(value) = self.ui.slider(cx, ids!(style_slider)).slided(actions) {
            let viewer = self.viewport_ref(cx);
            if let Some(mut inner) = viewer.borrow_mut::<ExfViewport>() {
                inner.set_style_mix(cx, value as f32);
            };
        }
        if let Some(value) = self.ui.slider(cx, ids!(contour_slider)).slided(actions) {
            let viewer = self.viewport_ref(cx);
            if let Some(mut inner) = viewer.borrow_mut::<ExfViewport>() {
                inner.set_contour_gain(cx, value as f32);
            };
        }
        if let Some(value) = self.ui.slider(cx, ids!(glow_slider)).slided(actions) {
            let viewer = self.viewport_ref(cx);
            if let Some(mut inner) = viewer.borrow_mut::<ExfViewport>() {
                inner.set_glow_gain(cx, value as f32);
            };
        }
        if let Some(value) = self.ui.slider(cx, ids!(light_gain_slider)).slided(actions) {
            let viewer = self.viewport_ref(cx);
            if let Some(mut inner) = viewer.borrow_mut::<ExfViewport>() {
                inner.set_light_gain(cx, value as f32);
            };
        }
        if let Some(value) = self.ui.slider(cx, ids!(light_radius_slider)).slided(actions) {
            let viewer = self.viewport_ref(cx);
            if let Some(mut inner) = viewer.borrow_mut::<ExfViewport>() {
                inner.set_light_radius(cx, value as f32);
            };
        }
        if let Some(value) = self.ui.slider(cx, ids!(light_lift_slider)).slided(actions) {
            let viewer = self.viewport_ref(cx);
            if let Some(mut inner) = viewer.borrow_mut::<ExfViewport>() {
                inner.set_light_lift(cx, value as f32);
            };
        }
        if let Some(value) = self.ui.slider(cx, ids!(halo_width_slider)).slided(actions) {
            let viewer = self.viewport_ref(cx);
            if let Some(mut inner) = viewer.borrow_mut::<ExfViewport>() {
                inner.set_halo_width(cx, value as f32);
            };
        }
        if let Some(value) = self.ui.slider(cx, ids!(halo_fog_slider)).slided(actions) {
            let viewer = self.viewport_ref(cx);
            if let Some(mut inner) = viewer.borrow_mut::<ExfViewport>() {
                inner.set_halo_fog(cx, value as f32);
            };
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
        if matches!(event, Event::Draw(_)) {
            self.try_load_exr(cx);
            self.update_auto_mip_display(cx);
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawExf {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    loaded: f32,
    #[live]
    style_mix: f32,
    #[live]
    contour_gain: f32,
    #[live]
    glow_gain: f32,
    #[live]
    halo_width: f32,
    #[live]
    halo_fog: f32,
    #[live]
    light_gain: f32,
    #[live]
    light_radius: f32,
    #[live]
    light_lift: f32,
    #[live]
    time: f32,
    #[live]
    debug_view: f32,
    #[live]
    zoom: f32,
    #[live]
    max_mip_level: f32,
    #[live]
    pan: Vec2f,
    #[live]
    image_size: Vec2f,
    #[live]
    depth_min: f32,
    #[live]
    depth_inv_range: f32,
    #[live]
    camera_mid: Vec3f,
    #[live]
    camera_right_step: Vec3f,
    #[live]
    camera_up_step: Vec3f,
    #[live]
    camera_forward_dir: Vec3f,
    #[live]
    camera_fov_y: f32,
    #[live]
    camera_z_start_delta: f32,
}

#[derive(Script, ScriptHook, Widget)]
pub struct ExfViewport {
    #[uid]
    uid: WidgetUid,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[live]
    draw_bg: DrawExf,
    #[rust]
    area: Area,
    #[rust]
    next_frame: NextFrame,
    #[rust]
    drag_last_abs: Option<DVec2>,
    #[rust(false)]
    loaded: bool,
    #[rust(DEFAULT_STYLE_MIX)]
    style_mix: f32,
    #[rust(DEFAULT_CONTOUR_GAIN)]
    contour_gain: f32,
    #[rust(DEFAULT_GLOW_GAIN)]
    glow_gain: f32,
    #[rust(DEFAULT_HALO_WIDTH)]
    halo_width: f32,
    #[rust(DEFAULT_HALO_FOG)]
    halo_fog: f32,
    #[rust(DEFAULT_LIGHT_GAIN)]
    light_gain: f32,
    #[rust(DEFAULT_LIGHT_RADIUS)]
    light_radius: f32,
    #[rust(DEFAULT_LIGHT_LIFT)]
    light_lift: f32,
    #[rust(0)]
    debug_view: i32,
    #[rust(1.0)]
    zoom: f32,
    #[rust(0.0)]
    max_mip_level: f32,
    #[rust(vec2(0.0, 0.0))]
    pan: Vec2f,
    #[rust(vec2(1.0, 1.0))]
    image_size: Vec2f,
    #[rust(0.0)]
    depth_min: f32,
    #[rust(1.0)]
    depth_inv_range: f32,
    #[rust(vec3(0.0, 0.0, 0.0))]
    camera_mid: Vec3f,
    #[rust(vec3(1.0, 0.0, 0.0))]
    camera_right_step: Vec3f,
    #[rust(vec3(0.0, 1.0, 0.0))]
    camera_up_step: Vec3f,
    #[rust(vec3(0.0, 0.0, 1.0))]
    camera_forward_dir: Vec3f,
    #[rust(45.0)]
    camera_fov_y: f32,
    #[rust(0.0)]
    camera_z_start_delta: f32,
    #[rust]
    textures: Vec<Texture>,
}

#[derive(Debug, Clone, Copy)]
struct LoadedExrSummary {
    width: usize,
    height: usize,
    channel_count: usize,
    mip_count: usize,
    depth_min: f32,
    depth_max: f32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ViewerMipPart {
    part_index: usize,
    level: usize,
    width: usize,
    height: usize,
}

#[derive(Debug, Clone)]
struct ViewerExrSource {
    mip_parts: Vec<ViewerMipPart>,
    camera: ViewerCameraMetadata,
}

fn parse_raw_int_attribute(part: &ExrPart, name: &str) -> Option<i32> {
    part.other_attributes
        .iter()
        .find(|attribute| attribute.name == name && attribute.type_name == "int")
        .and_then(|attribute| {
            if attribute.value.len() == 4 {
                Some(i32::from_le_bytes(attribute.value.as_slice().try_into().ok()?))
            } else {
                None
            }
        })
}

fn parse_raw_string_attribute(part: &ExrPart, name: &str) -> Option<String> {
    part.other_attributes
        .iter()
        .find(|attribute| {
            attribute.name == name
                && (attribute.type_name == "string" || attribute.type_name == "text")
        })
        .and_then(|attribute| String::from_utf8(attribute.value.clone()).ok())
}

fn discover_viewer_source(path: &Path) -> Result<ViewerExrSource, String> {
    let image = read_headers_file(path).map_err(|err| err.to_string())?;
    if image.parts.is_empty() {
        return Err("EXR has no readable parts".to_string());
    }

    let camera = image
        .parts
        .iter()
        .find_map(|part| parse_raw_string_attribute(part, MB3D_CAMERA_ATTRIBUTE_NAME))
        .ok_or_else(|| {
            format!(
                "EXR is missing {MB3D_CAMERA_ATTRIBUTE_NAME} metadata; rerender with the current mb3d renderer"
            )
        })
        .and_then(|value| ViewerCameraMetadata::decode_string(&value))?;

    let mut mip_parts = image
        .parts
        .iter()
        .enumerate()
        .filter_map(|(part_index, part)| {
            let level = parse_raw_int_attribute(part, MB3D_MIP_LEVEL_ATTRIBUTE_NAME)?;
            Some((
                level,
                ViewerMipPart {
                    part_index,
                    level: level.max(0) as usize,
                    width: part.width().ok()?,
                    height: part.height().ok()?,
                },
            ))
        })
        .collect::<Vec<_>>();

    if mip_parts.is_empty() {
        let (part_index, part) = image
            .parts
            .iter()
            .enumerate()
            .max_by_key(|(_, part)| part.channels.len())
            .ok_or_else(|| "EXR has no readable parts".to_string())?;
        return Ok(ViewerExrSource {
            mip_parts: vec![ViewerMipPart {
                part_index,
                level: 0,
                width: part.width().map_err(|err| err.to_string())?,
                height: part.height().map_err(|err| err.to_string())?,
            }],
            camera,
        });
    }

    mip_parts.sort_by_key(|(level, _)| *level);
    Ok(ViewerExrSource {
        mip_parts: mip_parts.into_iter().map(|(_, part)| part).collect(),
        camera,
    })
}

fn format_auto_mip_value(parts: &[ViewerMipPart], lod: f32) -> String {
    if parts.is_empty() {
        return "Base only".to_string();
    }
    let max_slot = parts.len().saturating_sub(1) as f32;
    let auto_lod = lod.clamp(0.0, max_slot);
    let selected_slot = (auto_lod + 0.5).floor() as usize;
    let selected = &parts[selected_slot];
    format!(
        "Auto LOD {:.2} -> selected mip {} ({}x{})",
        auto_lod,
        selected.level + 1,
        selected.width,
        selected.height
    )
}

fn auto_mip_level_for_view(
    image_width: f32,
    image_height: f32,
    viewport_width: f32,
    viewport_height: f32,
    zoom: f32,
    max_mip_level: f32,
) -> f32 {
    let image_width = image_width.max(1.0);
    let image_height = image_height.max(1.0);
    let viewport_width = viewport_width.max(1.0);
    let viewport_height = viewport_height.max(1.0);
    let fit_scale = (viewport_width / image_width).min(viewport_height / image_height);
    let pixel_scale = fit_scale * zoom.max(0.001);
    let lod = if pixel_scale >= 1.0 {
        0.0
    } else {
        (1.0 / pixel_scale.max(0.0001)).log2()
    };
    lod.clamp(0.0, max_mip_level.max(0.0))
}

impl ExfViewport {
    fn sync_draw_state(&mut self) {
        self.draw_bg.loaded = if self.loaded { 1.0 } else { 0.0 };
        self.draw_bg.style_mix = self.style_mix;
        self.draw_bg.contour_gain = self.contour_gain;
        self.draw_bg.glow_gain = self.glow_gain;
        self.draw_bg.halo_width = self.halo_width;
        self.draw_bg.halo_fog = self.halo_fog;
        self.draw_bg.light_gain = self.light_gain;
        self.draw_bg.light_radius = self.light_radius;
        self.draw_bg.light_lift = self.light_lift;
        self.draw_bg.debug_view = self.debug_view as f32;
        self.draw_bg.zoom = self.zoom;
        self.draw_bg.max_mip_level = self.max_mip_level;
        self.draw_bg.pan = self.pan;
        self.draw_bg.image_size = self.image_size;
        self.draw_bg.depth_min = self.depth_min;
        self.draw_bg.depth_inv_range = self.depth_inv_range;
        self.draw_bg.camera_mid = self.camera_mid;
        self.draw_bg.camera_right_step = self.camera_right_step;
        self.draw_bg.camera_up_step = self.camera_up_step;
        self.draw_bg.camera_forward_dir = self.camera_forward_dir;
        self.draw_bg.camera_fov_y = self.camera_fov_y;
        self.draw_bg.camera_z_start_delta = self.camera_z_start_delta;
    }

    fn ensure_fallback_textures(&mut self, cx: &mut Cx) {
        if !self.textures.is_empty() {
            return;
        }

        self.textures = (0..PACK_SPECS.len())
            .map(|_| {
                Texture::new_with_format(
                    cx,
                    TextureFormat::VecRGBAf32 {
                        width: 1,
                        height: 1,
                        data: Some(vec![0.0, 0.0, 0.0, 1.0]),
                        updated: TextureUpdated::Full,
                    },
                )
            })
            .collect();
    }

    fn bind_textures(&mut self) {
        for (index, texture) in self.textures.iter().enumerate() {
            self.draw_bg.draw_vars.set_texture(index, texture);
        }
    }

    fn set_style_mix(&mut self, cx: &mut Cx, value: f32) {
        self.style_mix = value.clamp(0.0, 1.0);
        self.sync_draw_state();
        self.area.redraw(cx);
    }

    fn set_contour_gain(&mut self, cx: &mut Cx, value: f32) {
        self.contour_gain = value.clamp(0.0, 2.0);
        self.sync_draw_state();
        self.area.redraw(cx);
    }

    fn set_glow_gain(&mut self, cx: &mut Cx, value: f32) {
        self.glow_gain = value.clamp(0.0, 2.0);
        self.sync_draw_state();
        self.area.redraw(cx);
    }

    fn set_halo_width(&mut self, cx: &mut Cx, value: f32) {
        self.halo_width = value.clamp(0.0, 4.0);
        self.sync_draw_state();
        self.area.redraw(cx);
    }

    fn set_halo_fog(&mut self, cx: &mut Cx, value: f32) {
        self.halo_fog = value.clamp(0.0, 3.0);
        self.sync_draw_state();
        self.area.redraw(cx);
    }

    fn set_light_gain(&mut self, cx: &mut Cx, value: f32) {
        self.light_gain = value.clamp(0.0, 3.0);
        self.sync_draw_state();
        self.area.redraw(cx);
    }

    fn set_light_radius(&mut self, cx: &mut Cx, value: f32) {
        self.light_radius = value.clamp(0.25, 4.0);
        self.sync_draw_state();
        self.area.redraw(cx);
    }

    fn set_light_lift(&mut self, cx: &mut Cx, value: f32) {
        self.light_lift = value.clamp(0.0, 2.5);
        self.sync_draw_state();
        self.area.redraw(cx);
    }

    fn cycle_debug_view(&mut self, cx: &mut Cx) {
        self.debug_view = (self.debug_view + 1) % 4;
        self.sync_draw_state();
        self.area.redraw(cx);
    }

    fn view_mode_label(&self) -> &'static str {
        match self.debug_view {
            0 => "Beauty",
            1 => "Normal RGB",
            2 => "Particle Light",
            3 => "Particle Spec",
            _ => "Beauty",
        }
    }

    fn reset_view(&mut self, cx: &mut Cx) {
        self.zoom = 1.0;
        self.pan = vec2(0.0, 0.0);
        self.sync_draw_state();
        self.area.redraw(cx);
    }

    fn auto_mip_level(&self, cx: &Cx) -> f32 {
        if !self.loaded || !self.area.is_valid(cx) {
            return 0.0;
        }
        let rect = self.area.rect(cx);
        auto_mip_level_for_view(
            self.image_size.x,
            self.image_size.y,
            rect.size.x as f32,
            rect.size.y as f32,
            self.zoom,
            self.max_mip_level,
        )
    }

    fn load_exr_path(
        &mut self,
        cx: &mut Cx,
        path: &Path,
        mip_parts: &[ViewerMipPart],
        camera: &ViewerCameraMetadata,
    ) -> Result<LoadedExrSummary, String> {
        let summary = load_mip_chain_into_texture_bank(path, mip_parts, cx, &mut self.textures)?;
        self.loaded = true;
        self.image_size = vec2(summary.width as f32, summary.height as f32);
        self.max_mip_level = summary.mip_count.saturating_sub(1) as f32;
        self.depth_min = summary.depth_min;
        self.depth_inv_range = if summary.depth_max > summary.depth_min {
            1.0 / (summary.depth_max - summary.depth_min)
        } else {
            0.0
        };
        self.camera_mid = vec3(camera.mid.x as f32, camera.mid.y as f32, camera.mid.z as f32);
        self.camera_right_step = vec3(
            camera.right_step.x as f32,
            camera.right_step.y as f32,
            camera.right_step.z as f32,
        );
        self.camera_up_step = vec3(
            camera.up_step.x as f32,
            camera.up_step.y as f32,
            camera.up_step.z as f32,
        );
        self.camera_forward_dir = vec3(
            camera.forward_dir.x as f32,
            camera.forward_dir.y as f32,
            camera.forward_dir.z as f32,
        );
        self.camera_fov_y = camera.fov_y as f32;
        self.camera_z_start_delta = camera.z_start_delta as f32;
        self.sync_draw_state();
        self.bind_textures();
        self.area.redraw(cx);
        Ok(summary)
    }
}

impl Widget for ExfViewport {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if let Event::NextFrame(ne) = event {
            if ne.set.contains(&self.next_frame) {
                self.draw_bg.time = ne.time as f32;
                self.area.redraw(cx);
                self.next_frame = cx.new_next_frame();
            }
        }

        if matches!(event, Event::Startup) {
            self.next_frame = cx.new_next_frame();
        }

        match event.hits_with_capture_overload(cx, self.area, true) {
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                self.drag_last_abs = Some(fe.abs);
                cx.set_cursor(MouseCursor::Grabbing);
            }
            Hit::FingerMove(fe) => {
                if let Some(last_abs) = self.drag_last_abs {
                    let delta = fe.abs - last_abs;
                    let zoom = self.zoom.max(0.001);
                    self.pan.x -= delta.x as f32 / fe.rect.size.x.max(1.0) as f32 / zoom;
                    self.pan.y -= delta.y as f32 / fe.rect.size.y.max(1.0) as f32 / zoom;
                    self.sync_draw_state();
                    self.drag_last_abs = Some(fe.abs);
                    self.area.redraw(cx);
                }
            }
            Hit::FingerScroll(fs) => {
                let scroll = if fs.scroll.y.abs() > f64::EPSILON {
                    fs.scroll.y
                } else {
                    fs.scroll.x
                };
                let factor = if scroll > 0.0 { 1.12 } else { 1.0 / 1.12 };
                self.zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
                self.sync_draw_state();
                self.area.redraw(cx);
            }
            Hit::FingerUp(fe) => {
                if self.drag_last_abs.take().is_some() {
                    if fe.is_over {
                        cx.set_cursor(MouseCursor::Grab);
                    } else {
                        cx.set_cursor(MouseCursor::Default);
                    }
                }
            }
            Hit::FingerHoverIn(_) => {
                if self.drag_last_abs.is_some() {
                    cx.set_cursor(MouseCursor::Grabbing);
                } else {
                    cx.set_cursor(MouseCursor::Grab);
                }
            }
            Hit::FingerHoverOut(_) => {
                if self.drag_last_abs.is_none() {
                    cx.set_cursor(MouseCursor::Default);
                }
            }
            _ => {}
        }

    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let _ = self.layout;
        self.ensure_fallback_textures(cx);
        self.sync_draw_state();
        self.bind_textures();
        let rect = cx.walk_turtle(walk);
        self.draw_bg.draw_abs(cx, rect);
        self.area = self.draw_bg.area();
        DrawStep::done()
    }
}

fn load_mip_chain_into_texture_bank(
    path: &Path,
    mip_parts: &[ViewerMipPart],
    cx: &mut Cx,
    textures: &mut Vec<Texture>,
) -> Result<LoadedExrSummary, String> {
    if mip_parts.is_empty() {
        return Err("EXR has no readable mip parts".to_string());
    }

    let total_plane_len = mip_parts.iter().try_fold(0usize, |acc, part| {
        let pixels = part
            .width
            .checked_mul(part.height)
            .ok_or_else(|| "EXR image dimensions overflow".to_string())?;
        let rgba_len = pixels
            .checked_mul(4)
            .ok_or_else(|| "EXR mip plane size overflow".to_string())?;
        acc.checked_add(rgba_len)
            .ok_or_else(|| "EXR mip chain size overflow".to_string())
    })?;

    let mut mip_planes = (0..PACK_SPECS.len())
        .map(|_| Vec::with_capacity(total_plane_len))
        .collect::<Vec<_>>();

    let base = &mip_parts[0];
    let mut previous_dims = Some((base.width, base.height));
    let mut channel_count = 0usize;
    let mut depth_min = 0.0f32;
    let mut depth_max = 1.0f32;

    for (slot, mip_part) in mip_parts.iter().enumerate() {
        if slot > 0 {
            let (prev_width, prev_height) = previous_dims.expect("previous mip dimensions missing");
            let expected_width = (prev_width / 2).max(1);
            let expected_height = (prev_height / 2).max(1);
            if mip_part.width != expected_width || mip_part.height != expected_height {
                return Err(format!(
                    "mip {} had {}x{}, expected {}x{}",
                    mip_part.level,
                    mip_part.width,
                    mip_part.height,
                    expected_width,
                    expected_height
                ));
            }
        }

        let part = read_part_file(path, mip_part.part_index).map_err(|err| err.to_string())?;
        let width = part.width().map_err(|err| err.to_string())?;
        let height = part.height().map_err(|err| err.to_string())?;
        if width != mip_part.width || height != mip_part.height {
            return Err(format!(
                "mip {} header/data size mismatch: header {}x{}, data {}x{}",
                mip_part.level, mip_part.width, mip_part.height, width, height
            ));
        }

        let part_channel_count = part.channels.len();
        if slot == 0 {
            channel_count = part_channel_count;
        } else if part_channel_count != channel_count {
            return Err(format!(
                "mip {} had {} channels, expected {}",
                mip_part.level, part_channel_count, channel_count
            ));
        }

        let pixel_count = width
            .checked_mul(height)
            .ok_or_else(|| "EXR image dimensions overflow".to_string())?;
        let channel_map = collect_float_channels(part.channels, pixel_count)?;
        if slot == 0 {
            let depth_values = channel_map
                .get("depth.Depth")
                .ok_or_else(|| "EXR is missing channel depth.Depth".to_string())?;
            let mut min = f32::INFINITY;
            let mut max = f32::NEG_INFINITY;
            for &value in depth_values {
                if value.is_finite() {
                    min = min.min(value);
                    max = max.max(value);
                }
            }
            if min.is_finite() && max.is_finite() {
                depth_min = min;
                depth_max = max;
            }
        }
        let planes = build_texture_planes(&channel_map, pixel_count)?;

        for (dst, plane) in mip_planes.iter_mut().zip(planes.into_iter()) {
            dst.extend(plane);
        }
        previous_dims = Some((width, height));
    }

    let max_level = mip_parts.len().saturating_sub(1);
    *textures = mip_planes
        .into_iter()
        .map(|plane| {
            Texture::new_with_format(
                cx,
                TextureFormat::VecMipRGBAf32 {
                    width: base.width,
                    height: base.height,
                    data: Some(plane),
                    max_level: Some(max_level),
                    updated: TextureUpdated::Full,
                },
            )
        })
        .collect();

    Ok(LoadedExrSummary {
        width: base.width,
        height: base.height,
        channel_count,
        mip_count: mip_parts.len(),
        depth_min,
        depth_max,
    })
}

fn collect_float_channels(
    channels: Vec<makepad_openexr::ExrChannel>,
    pixel_count: usize,
) -> Result<HashMap<String, Vec<f32>>, String> {
    let mut out = HashMap::with_capacity(channels.len());
    for channel in channels {
        let values = match channel.samples {
            SampleBuffer::Float(values) => values,
            SampleBuffer::Half(values) => values.into_iter().map(|value| value.to_f32()).collect(),
            SampleBuffer::Uint(values) => values.into_iter().map(|value| value as f32).collect(),
        };
        if values.len() != pixel_count {
            return Err(format!(
                "channel {} had {} samples, expected {}",
                channel.name,
                values.len(),
                pixel_count
            ));
        }
        out.insert(channel.name, values);
    }
    Ok(out)
}

fn build_texture_planes(
    channel_map: &HashMap<String, Vec<f32>>,
    pixel_count: usize,
) -> Result<Vec<Vec<f32>>, String> {
    let mut planes = Vec::with_capacity(PACK_SPECS.len());

    for spec in PACK_SPECS {
        let mut packed = vec![0.0; pixel_count * 4];
        for (lane, slot) in spec.channels.iter().enumerate() {
            match slot {
                ChannelSlot::Named(name) => {
                    let values = channel_map
                        .get(*name)
                        .ok_or_else(|| format!("EXR is missing channel {name}"))?;
                    for (index, value) in values.iter().enumerate() {
                        packed[index * 4 + lane] = *value;
                    }
                }
                ChannelSlot::Constant(value) => {
                    for chunk in packed[lane..].iter_mut().step_by(4) {
                        *chunk = *value;
                    }
                }
            }
        }
        planes.push(packed);
    }

    Ok(planes)
}

fn resolve_startup_exr_path(args: &[String]) -> Option<PathBuf> {
    find_explicit_exr_path_arg(args).or_else(find_latest_render_exr)
}

fn find_explicit_exr_path_arg(args: &[String]) -> Option<PathBuf> {
    let mut iter = args.iter().skip(1);
    while let Some(arg) = iter.next() {
        if let Some(path) = arg.strip_prefix("--file=") {
            return Some(PathBuf::from(path));
        }
        if let Some(path) = arg.strip_prefix("--exr=") {
            return Some(PathBuf::from(path));
        }
        if arg == "--file" || arg == "--exr" {
            if let Some(path) = iter.next() {
                return Some(PathBuf::from(path));
            }
            continue;
        }
        if arg.starts_with('-') {
            continue;
        }
        if arg.to_ascii_lowercase().ends_with(".exr") {
            return Some(PathBuf::from(arg));
        }
    }
    None
}

fn find_latest_render_exr() -> Option<PathBuf> {
    find_latest_exr_matching(Path::new("/tmp"), |path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("mb3d") && name.ends_with(".exr"))
    })
}

fn find_latest_exr_matching(
    dir: &Path,
    matches: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return None;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !matches(&path) {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        let Ok(modified) = metadata.modified() else {
            continue;
        };
        candidates.push(RenderExrCandidate { path, modified });
    }

    choose_latest_exr_path(candidates)
}

#[derive(Clone)]
struct RenderExrCandidate {
    path: PathBuf,
    modified: SystemTime,
}

fn choose_latest_exr_path(mut candidates: Vec<RenderExrCandidate>) -> Option<PathBuf> {
    candidates.sort_by(|left, right| {
        left.modified
            .cmp(&right.modified)
            .then_with(|| left.path.cmp(&right.path))
    });
    candidates.pop().map(|candidate| candidate.path)
}

fn pack_schema_summary() -> String {
    PACK_SPECS
        .iter()
        .map(|spec| spec.label)
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn picks_latest_exr_candidate() {
        let latest = choose_latest_exr_path(vec![
            RenderExrCandidate {
                path: PathBuf::from("/tmp/a.exr"),
                modified: UNIX_EPOCH + Duration::from_secs(10),
            },
            RenderExrCandidate {
                path: PathBuf::from("/tmp/b.exr"),
                modified: UNIX_EPOCH + Duration::from_secs(20),
            },
            RenderExrCandidate {
                path: PathBuf::from("/tmp/c.exr"),
                modified: UNIX_EPOCH + Duration::from_secs(15),
            },
        ]);
        assert_eq!(latest, Some(PathBuf::from("/tmp/b.exr")));
    }

    #[test]
    fn finds_explicit_path_from_args() {
        let args = vec![
            "makepad-example-exf".to_string(),
            "--file=/tmp/test.exr".to_string(),
        ];
        assert_eq!(
            find_explicit_exr_path_arg(&args),
            Some(PathBuf::from("/tmp/test.exr"))
        );
    }

    #[test]
    fn auto_mip_level_matches_fit_scale() {
        let lod = auto_mip_level_for_view(7680.0, 4320.0, 640.0, 360.0, 1.0, 5.0);
        assert!((lod - 3.5849626).abs() < 0.01, "lod was {lod}");
    }

    #[test]
    fn auto_mip_level_clamps_when_zoomed_in() {
        let lod = auto_mip_level_for_view(1920.0, 1080.0, 1920.0, 1080.0, 4.0, 4.0);
        assert_eq!(lod, 0.0);
    }

    #[test]
    fn fixed_pack_schema_stays_at_nine_rgba_textures() {
        assert_eq!(PACK_SPECS.len(), 9);
        assert_eq!(PACK_SPECS[0].label, "surface");
        assert_eq!(PACK_SPECS[8].label, "uncertainty");
    }
}
