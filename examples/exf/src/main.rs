pub use makepad_widgets;

use makepad_openexr::{read_file, ExrPart, SampleBuffer};
use makepad_widgets::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

app_main!(App);

const DEFAULT_STYLE_MIX: f32 = 0.58;
const DEFAULT_CONTOUR_GAIN: f32 = 0.72;
const DEFAULT_GLOW_GAIN: f32 = 0.44;
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

const PACK_SPECS: [PackSpec; 10] = [
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
            ChannelSlot::Constant(1.0),
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
        label: "space",
        channels: [
            ChannelSlot::Named("orbit.R2"),
            ChannelSlot::Named("position.X"),
            ChannelSlot::Named("position.Y"),
            ChannelSlot::Named("position.Z"),
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
        space_tex: texture_2d(float)
        style_tex: texture_2d(float)
        traps_tex: texture_2d(float)
        uncertainty_tex: texture_2d(float)

        loaded: 0.0
        style_mix: 0.58
        contour_gain: 0.72
        glow_gain: 0.44
        zoom: 1.0
        pan: vec2(0.0, 0.0)
        image_size: vec2(1.0, 1.0)

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

        pixel: fn() {
            let uv = self.viewport_uv()
            let grid_uv = self.pos * vec2(12.0, 12.0)
            let inside =
                uv.x >= 0.0 &&
                uv.x <= 1.0 &&
                uv.y >= 0.0 &&
                uv.y <= 1.0 &&
                self.loaded > 0.5

            if !inside {
                return Pal.premul(vec4(self.backdrop(grid_uv), 1.0))
            }

            let surface = self.surface_tex.sample(uv)
            let flow = self.flow_tex.sample(uv)
            let metrics = self.metrics_tex.sample(uv)
            let folds = self.folds_tex.sample(uv)
            let normal_raw = self.normal_tex.sample(uv)
            let orbit = self.orbit_tex.sample(uv)
            let space = self.space_tex.sample(uv)
            let style = self.style_tex.sample(uv)
            let traps = self.traps_tex.sample(uv)
            let uncertainty = self.uncertainty_tex.sample(uv)

            let t = 0.0
            let texel = vec2(
                1.0 / max(self.image_size.x, 1.0),
                1.0 / max(self.image_size.y, 1.0)
            )
            let uv_px = clamp(uv + vec2(texel.x, 0.0), vec2(0.0, 0.0), vec2(1.0, 1.0))
            let uv_mx = clamp(uv - vec2(texel.x, 0.0), vec2(0.0, 0.0), vec2(1.0, 1.0))
            let uv_py = clamp(uv + vec2(0.0, texel.y), vec2(0.0, 0.0), vec2(1.0, 1.0))
            let uv_my = clamp(uv - vec2(0.0, texel.y), vec2(0.0, 0.0), vec2(1.0, 1.0))
            let halo_offset_a = texel * 8.0
            let halo_offset_b = texel * 26.0
            let uv_hax = clamp(uv + vec2(halo_offset_a.x, 0.0), vec2(0.0, 0.0), vec2(1.0, 1.0))
            let uv_hbx = clamp(uv - vec2(halo_offset_a.x, 0.0), vec2(0.0, 0.0), vec2(1.0, 1.0))
            let uv_hay = clamp(uv + vec2(0.0, halo_offset_a.y), vec2(0.0, 0.0), vec2(1.0, 1.0))
            let uv_hby = clamp(uv - vec2(0.0, halo_offset_a.y), vec2(0.0, 0.0), vec2(1.0, 1.0))
            let uv_hcx = clamp(uv + vec2(halo_offset_b.x, 0.0), vec2(0.0, 0.0), vec2(1.0, 1.0))
            let uv_hdx = clamp(uv - vec2(halo_offset_b.x, 0.0), vec2(0.0, 0.0), vec2(1.0, 1.0))
            let uv_hcy = clamp(uv + vec2(0.0, halo_offset_b.y), vec2(0.0, 0.0), vec2(1.0, 1.0))
            let uv_hdy = clamp(uv - vec2(0.0, halo_offset_b.y), vec2(0.0, 0.0), vec2(1.0, 1.0))
            let original_beauty = clamp(surface.xyz, vec3(0.0), vec3(1.0))
            let ao = clamp(surface.w, 0.0, 1.0)
            let normal = normalize(normal_raw.xyz * 2.0 - 1.0)
            let normal_px = normalize(self.normal_tex.sample(uv_px).xyz * 2.0 - 1.0)
            let normal_mx = normalize(self.normal_tex.sample(uv_mx).xyz * 2.0 - 1.0)
            let normal_py = normalize(self.normal_tex.sample(uv_py).xyz * 2.0 - 1.0)
            let normal_my = normalize(self.normal_tex.sample(uv_my).xyz * 2.0 - 1.0)
            let depth = clamp(metrics.x, 0.0, 1.0)
            let depth_px = clamp(self.metrics_tex.sample(uv_px).x, 0.0, 1.0)
            let depth_mx = clamp(self.metrics_tex.sample(uv_mx).x, 0.0, 1.0)
            let depth_py = clamp(self.metrics_tex.sample(uv_py).x, 0.0, 1.0)
            let depth_my = clamp(self.metrics_tex.sample(uv_my).x, 0.0, 1.0)
            let halo_depth_a =
                clamp(self.metrics_tex.sample(uv_hax).x, 0.0, 1.0)
                + clamp(self.metrics_tex.sample(uv_hbx).x, 0.0, 1.0)
                + clamp(self.metrics_tex.sample(uv_hay).x, 0.0, 1.0)
                + clamp(self.metrics_tex.sample(uv_hby).x, 0.0, 1.0)
            let halo_depth_b =
                clamp(self.metrics_tex.sample(uv_hcx).x, 0.0, 1.0)
                + clamp(self.metrics_tex.sample(uv_hdx).x, 0.0, 1.0)
                + clamp(self.metrics_tex.sample(uv_hcy).x, 0.0, 1.0)
                + clamp(self.metrics_tex.sample(uv_hdy).x, 0.0, 1.0)
            let estimator = clamp(metrics.y, 0.0, 1.0)
            let iterations = clamp(metrics.z, 0.0, 1.0)
            let march_steps = clamp(metrics.w, 0.0, 1.0)
            let roughness = clamp(flow.w, 0.0, 1.0)
            let pos = vec3(space.y, space.z, space.w)
            let far_mix = 1.0 - depth
            let halo_far_a = 1.0 - halo_depth_a * 0.25
            let halo_far_b = 1.0 - halo_depth_b * 0.25
            let halo_ao_a =
                clamp(self.surface_tex.sample(uv_hax).w, 0.0, 1.0)
                + clamp(self.surface_tex.sample(uv_hbx).w, 0.0, 1.0)
                + clamp(self.surface_tex.sample(uv_hay).w, 0.0, 1.0)
                + clamp(self.surface_tex.sample(uv_hby).w, 0.0, 1.0)
            let halo_ao_b =
                clamp(self.surface_tex.sample(uv_hcx).w, 0.0, 1.0)
                + clamp(self.surface_tex.sample(uv_hdx).w, 0.0, 1.0)
                + clamp(self.surface_tex.sample(uv_hcy).w, 0.0, 1.0)
                + clamp(self.surface_tex.sample(uv_hdy).w, 0.0, 1.0)
            let halo_open_a = halo_ao_a * 0.25
            let halo_open_b = halo_ao_b * 0.25
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
            let orbit_heat = clamp(length(orbit.xyz - vec3(0.5, 0.5, 0.5)) * 1.2 + orbit.w * 0.35 + space.x * 0.35, 0.0, 1.0)
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
            let light_a = normalize(vec3(-0.64, 0.36, 0.68))
            let light_b = normalize(vec3(0.22, -0.26, 0.80))
            let light_c = normalize(vec3(0.14, 0.84, 0.52))
            let view_dir = normalize(vec3(0.0, 0.0, 1.0))
            let half_a = normalize(light_a + view_dir)
            let half_b = normalize(light_b + view_dir)
            let half_c = normalize(light_c + view_dir)
            let ndotl_a = clamp(dot(normal, light_a), 0.0, 1.0)
            let ndotl_b = clamp(dot(normal, light_b), 0.0, 1.0)
            let ndotl_c = clamp(dot(normal, light_c), 0.0, 1.0)
            let ambient_color = vec3(0.03, 0.06, 0.10).mix(
                vec3(0.24, 0.40, 0.64),
                clamp(normal.y * 0.5 + 0.5, 0.0, 1.0)
            )
            let ambient = ambient_color * (0.22 + ao * 0.24 + far_mix * 0.06)
            let base_albedo = self.stone_palette(phase)
                .mix(vec3(0.78, 0.96, 1.08), fold_grid * 0.12 + beauty_ribbon * 0.08 + contour * 0.05)
                .mix(vec3(0.06, 0.12, 0.24), trap_energy * 0.10 + far_mix * 0.08)
            let diffuse =
                vec3(0.84, 0.92, 1.00) * (ndotl_a * 0.70)
                + vec3(0.32, 0.60, 0.94) * (ndotl_b * 0.30)
                + vec3(0.48, 0.86, 1.10) * (ndotl_c * rim * 0.12)
            let fresnel = pow(clamp(1.0 - dot(normal, view_dir), 0.0, 1.0), 4.0)
            let spec_power = 18.0 + (1.0 - roughness) * 110.0
            let spec_core = pow(clamp(dot(normal, half_a), 0.0, 1.0), spec_power)
            let spec_fill = pow(clamp(dot(normal, half_b), 0.0, 1.0), 12.0 + (1.0 - roughness) * 64.0)
            let spec_rim = pow(clamp(dot(normal, half_c), 0.0, 1.0), 10.0 + (1.0 - roughness) * 40.0)
            let rainbow_phase = phase * 6.2831853 + orbit.w * 7.0 + space.x * 4.0
            let rainbow_spec = self.rainbow_band(rainbow_phase)
            let spec_color = vec3(0.92, 0.98, 1.00).mix(
                rainbow_spec,
                clamp(trap_energy * 0.16 + self.style_mix * 0.08, 0.0, 1.0)
            )
            let spec_intensity = (0.20 + trap_energy * 0.46 + contour * 0.28 + uncertainty_band * 0.22)
                * (0.82 + self.glow_gain * 0.56)
            let animated_ribbon = 0.5 + 0.5 * sin(
                pos.x * 22.0
                - pos.y * 17.0
                + pos.z * 13.0
                + depth * 36.0
            )
            let animated_wave = 0.5 + 0.5 * sin(
                pos.x * 16.0
                - pos.z * 12.0
                + branch_bias * 9.0
                + traps.w * 7.0
            )
            let animated_chroma = self.rainbow_band(
                depth * 18.0
                + orbit.x * 9.0
                + orbit.y * 7.0
                + pos.z * 5.0
            )
            let glitter = self.feature_spark_mask(
                traps.x * 8.0 + traps.w * 6.0 + orbit.w * 2.0 + phase * 2.5,
                traps.y * 9.0 - traps.z * 7.0 + depth * 3.0 + space.x * 2.0,
                iterations * 8.0 + march_steps * 7.0 + sign_echo * 3.0 + branch_bias * 2.0,
                roughness,
                edge_glow
            )
            let glitter_color = self.rainbow_band(
                depth * 32.0
                + orbit_heat * 12.0
                + sign_echo * 8.0
            )
            let halo_trap_a =
                self.trap_strength(self.traps_tex.sample(uv_hax))
                + self.trap_strength(self.traps_tex.sample(uv_hbx))
                + self.trap_strength(self.traps_tex.sample(uv_hay))
                + self.trap_strength(self.traps_tex.sample(uv_hby))
            let halo_trap_b =
                self.trap_strength(self.traps_tex.sample(uv_hcx))
                + self.trap_strength(self.traps_tex.sample(uv_hdx))
                + self.trap_strength(self.traps_tex.sample(uv_hcy))
                + self.trap_strength(self.traps_tex.sample(uv_hdy))
            let broad_trap = halo_trap_a * 0.25
            let wide_trap = halo_trap_b * 0.25
            let highlight_ribbon = pow(
                clamp(1.0 - abs(beauty_ribbon - 0.5) * 2.0, 0.0, 1.0),
                3.0
            )
            let lit_surface =
                (base_albedo * (ambient + diffuse)
                + spec_color * (spec_core * spec_intensity + spec_fill * 0.12 + spec_rim * 0.08 + fresnel * 0.06)
                + vec3(rim * 0.06, rim * 0.04, rim * 0.03)
                + vec3(0.12, 0.08, 0.05) * (highlight_ribbon * 0.18 + contour * 0.03))
                * (0.42 + 0.28 * ao)
            let fog_amount = clamp(far_mix * (0.18 + contour * 0.10 + march_steps * 0.06), 0.0, 0.52)
            let base_beauty = lit_surface.mix(self.fog_palette(far_mix), fog_amount)
            let pretty_base =
                base_beauty
                .mix(
                    base_beauty * 1.12
                    + vec3(0.10, 0.18, 0.28) * (highlight_ribbon * 0.20 + spec_core * 0.10)
                    + vec3(contour * 0.03, contour * 0.04, contour * 0.05),
                    0.30 + self.style_mix * 0.08
                )
            let depth_band = pow(
                0.5 + 0.5 * sin(
                    far_mix * 28.0
                    + pos.z * 19.0
                    - pos.x * 8.0
                    + pos.y * 6.0
                ),
                6.0
            )
            let depth_ray = pow(
                clamp(
                    1.0 - abs(sin(pos.x * 10.0 + pos.y * 4.5 - far_mix * 10.0)),
                    0.0,
                    1.0
                ),
                10.0
            ) * (0.18 + 0.82 * far_mix)
            let trap_shimmer = pow(
                0.5 + 0.5 * sin(
                    traps.x * 13.0
                    + traps.y * 17.0
                    + traps.z * 19.0
                    + traps.w * 23.0
                    + contour * 9.0
                ),
                4.0
            )
            let depth_haze = vec3(0.10, 0.22, 0.42) * far_mix * (0.14 + depth_ray * 0.26 + depth_band * 0.14)
            let depth_sun = vec3(0.72, 0.96, 1.12) * (depth_band * 0.08 + highlight_ribbon * far_mix * 0.04)
            let trap_glow = vec3(0.18, 0.86, 1.08)
                .mix(vec3(0.56, 0.38, 1.08), trap_shimmer * 0.34 + pulse * 0.10)
                * trap_energy
                * (0.04 + trap_shimmer * 0.10 + contour * 0.04)
            let embedded_glitter = glitter * (0.08 + far_mix * 0.14 + trap_energy * 0.08 + highlight_ribbon * 0.06)
            let enhanced_base =
                pretty_base
                .mix(
                    pretty_base
                    + depth_haze
                    + depth_sun
                    + trap_glow
                    + vec3(0.04, 0.025, 0.015) * (depth_ray * 0.18 + contour * 0.04),
                    0.26 + far_mix * 0.10 + self.style_mix * 0.06
                )
            let base_luma = dot(pretty_base, vec3(0.2126, 0.7152, 0.0722))
            let cool_grade = vec3(0.02, 0.04, 0.10)
                .mix(vec3(0.10, 0.16, 0.40), clamp(base_luma * 1.2 + contour * 0.08, 0.0, 1.0))
                .mix(vec3(0.06, 0.44, 1.10), clamp(branch_bias * 0.38 + orbit_heat * 0.30, 0.0, 1.0))
                .mix(vec3(0.78, 0.94, 1.06), clamp(highlight_ribbon * 0.14 + spec_core * 0.06, 0.0, 1.0))
            let cathedral_base =
                pretty_base
                .mix(cool_grade * (0.20 + base_luma * 0.94), 0.46)
                * (0.54 + ao * 0.14)
            let nebula = vec3(0.08, 0.12, 0.34)
                .mix(vec3(0.12, 0.56, 1.10), clamp(branch_bias * 0.45 + orbit_heat * 0.18, 0.0, 1.0))
                * far_mix
                * (0.08 + depth_band * 0.22 + depth_ray * 0.16)
            let ember_core = pow(
                0.5 + 0.5 * sin(
                    traps.w * 34.0
                    + depth * 28.0
                    + pos.z * 16.0
                    + contour * 7.0
                ),
                18.0
            ) * (0.20 + 0.80 * trap_energy)
            let ember_sparks = clamp(embedded_glitter * 1.10 + ember_core * 0.28, 0.0, 1.5)
            let ember_color = vec3(0.64, 0.88, 1.14)
                .mix(vec3(0.96, 0.96, 1.00), clamp(ember_core * 0.40 + embedded_glitter * 0.15, 0.0, 1.0))
            let ice_rim = vec3(0.18, 0.84, 1.12)
                * (branch_bias * 0.16 + rim * 0.10 + sign_echo * 0.08)
                * (0.25 + 0.75 * pow(animated_wave, 4.0))
            let cavity_glow = clamp((1.0 - ao) * 0.65 + edge_glow * 0.55 + contour * 0.12, 0.0, 1.0)
            let lantern_band_a = pow(
                clamp(1.0 - abs(sin((traps.x * 7.0 + traps.w * 6.0 + orbit.w * 2.0 + phase * 2.0) * 6.2831853)), 0.0, 1.0),
                6.5
            )
            let lantern_band_b = pow(
                clamp(1.0 - abs(sin((traps.y * 8.0 - traps.z * 5.5 + iterations * 2.0 + depth * 3.0) * 6.2831853)), 0.0, 1.0),
                6.5
            )
            let lantern_halo = clamp(lantern_band_a * 0.55 + lantern_band_b * 0.55, 0.0, 1.0)
                * (0.20 + 0.80 * cavity_glow)
                * (0.20 + 0.80 * trap_energy)
            let lantern_core = lantern_band_a
                * lantern_band_b
                * (0.25 + 0.75 * cavity_glow)
                * (0.20 + 0.80 * trap_energy)
            let trap_halo = clamp(
                lantern_halo * 0.55
                + broad_trap * (0.34 + cavity_glow * 0.24)
                + wide_trap * (0.18 + halo_far_b * 0.18),
                0.0,
                1.5
            )
            let void_halo = clamp(
                halo_far_a * (0.30 + halo_open_a * 0.40)
                + halo_far_b * (0.20 + halo_open_b * 0.30)
                + edge_glow * 0.22,
                0.0,
                1.6
            )
            let uv_isolate_a = pow(
                clamp(1.0 - abs(sin((phase * 12.0 + branch_bias * 2.0 + orbit.w * 1.4) * 6.2831853)), 0.0, 1.0),
                10.0
            )
            let uv_isolate_b = pow(
                clamp(1.0 - abs(sin((phase * 7.0 - pos.z * 1.7 + iterations * 1.1 + traps.w * 1.8) * 6.2831853)), 0.0, 1.0),
                8.0
            )
            let uv_seed = uv_isolate_a
                * (0.20 + 0.80 * uv_isolate_b)
                * (0.18 + 0.82 * edge_glow)
                * (0.15 + 0.85 * trap_energy)
            let uv_core = pow(clamp((uv_seed - 0.12) * 3.2, 0.0, 1.0), 2.2)
                * (0.20 + 0.80 * cavity_glow)
            let uv_halo = pow(
                clamp(
                    uv_seed * 0.45
                    + lantern_halo * 0.18
                    + trap_halo * 0.12
                    + void_halo * 0.08
                    - 0.08,
                    0.0,
                    1.0
                ),
                1.6
            )
            let uv_gain = (uv_core * 3.8 + uv_halo * 1.2) * (0.46 + self.glow_gain * 0.42)
            let lantern_color = vec3(0.70, 0.94, 1.18)
                .mix(vec3(1.00, 1.00, 1.00), clamp(lantern_core * 0.80 + lantern_halo * 0.20, 0.0, 1.0))
            let uv_color = vec3(0.24, 0.46, 1.12)
                .mix(vec3(0.18, 0.92, 1.24), clamp(phase * 0.70 + halo_far_a * 0.20, 0.0, 1.0))
                .mix(vec3(0.94, 0.96, 1.08), clamp(trap_energy * 0.35 + lantern_core * 0.25, 0.0, 1.0))
            let cyan_halo = vec3(0.12, 0.78, 1.22)
                .mix(vec3(0.70, 1.04, 1.12), clamp(void_halo * 0.55 + halo_far_a * 0.25, 0.0, 1.0))
                * void_halo
                * (0.28 + halo_far_a * 0.44 + halo_far_b * 0.22)
            let amber_halo = vec3(0.38, 0.30, 1.04)
                .mix(vec3(0.88, 0.96, 1.06), clamp(trap_halo * 0.45 + lantern_core * 0.35, 0.0, 1.0))
                * trap_halo
                * (0.10 + cavity_glow * 0.18)
            let optical_glow = uv_color * (uv_gain * 0.12 + uv_halo * 0.30 + trap_halo * 0.08 + void_halo * 0.06)
            let optical_core = vec3(0.96, 0.98, 1.02) * uv_core * uv_gain * 0.07
            let original_luma = dot(original_beauty, vec3(0.2126, 0.7152, 0.0722))
            let original_key = pow(clamp(original_luma * 1.7 - 0.16, 0.0, 1.0), 1.5)
            let original_fill = original_beauty * (0.16 + original_luma * 0.38)
            let original_glow = original_beauty * (0.04 + original_key * (0.18 + self.glow_gain * 0.10))
            let vignette = clamp(1.06 - length(uv - vec2(0.5, 0.5)) * 1.62, 0.0, 1.0)
            let cavity = clamp((1.0 - ao) * 0.60 + contour * 0.26 + edge_glow * 0.12, 0.0, 1.0)
            let open_space = clamp(halo_open_a * 0.58 + halo_open_b * 0.42 + (1.0 - ao) * 0.16, 0.0, 1.0)
            let foreground_mask = clamp(
                pow(clamp(depth, 0.0, 1.0), 1.85)
                * (0.42 + cavity * 0.28 + edge_glow * 0.16 + rim * 0.10),
                0.0,
                1.0
            )
            let background_mask = clamp(
                pow(clamp(far_mix, 0.0, 1.0), 1.30)
                * (0.20 + open_space * 0.54 + halo_far_a * 0.18 + void_halo * 0.10),
                0.0,
                1.4
            )
            let midground_mask = clamp(
                1.0 - abs(depth - 0.48) * 2.6 + open_space * 0.08,
                0.0,
                1.0
            )
            let foreground_shadow = vec3(0.05, 0.10, 0.16) * foreground_mask * (0.94 + cavity * 0.24)
            let foreground_rim = vec3(0.18, 0.90, 1.14)
                * foreground_mask
                * edge_glow
                * (0.20 + spec_core * 0.10 + lantern_halo * 0.10)
            let background_pool = vec3(0.10, 0.50, 1.08)
                * background_mask
                * open_space
                * (0.28 + void_halo * 0.32 + halo_open_a * 0.18 + depth_ray * 0.08)
            let background_core = vec3(0.86, 0.98, 1.06)
                * background_mask
                * open_space
                * (0.04 + uv_core * 0.24 + lantern_core * 0.10 + depth_sun.x * 0.12)
            let mid_haze = vec3(0.08, 0.22, 0.40)
                * midground_mask
                * (0.04 + depth_band * 0.08 + halo_far_a * 0.06)

            let composite_look =
                cathedral_base * (0.78 - foreground_mask * 0.56) * vignette
                + enhanced_base * 0.02
                + nebula * 0.34
                + original_fill * (0.28 + background_mask * 0.10)
                + original_glow * (0.16 + open_space * 0.10)
                + background_pool
                + background_core
                + mid_haze
                + cyan_halo * 0.46
                + depth_haze * 0.18
                + depth_sun * 0.12
                + trap_glow * 0.08
                + amber_halo * 0.03
                + ice_rim * 0.42
                + optical_glow * 0.34
                + optical_core * 0.24
                + lantern_color * (lantern_halo * 0.10 + lantern_core * 0.48)
                + vec3(1.00, 1.00, 1.00) * lantern_core * 0.06
                + ember_color * ember_sparks * (0.01 + self.glow_gain * 0.02)
                + animated_chroma * pow(animated_ribbon, 4.0) * (0.003 + self.style_mix * 0.008)
                + spec_color * (spec_core * 0.12 + spec_rim * 0.04)
                + foreground_rim
                + vec3(contour * 0.016, contour * 0.024, contour * 0.040)
                - foreground_shadow
                - vec3(0.02, 0.03, 0.05) * cavity * 0.10

            let mut color = composite_look
            color = color.mix(color + vec3(trap_energy, contour * 0.25, uncertainty_band) * 0.08, self.glow_gain * 0.25)
            color = self.display_map(clamp(color, vec3(0.0), vec3(4.5)))
            color = clamp(color, vec3(0.0), vec3(1.0))
            return Pal.premul(vec4(color, 1.0))
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
                            file_title := Labelbold{text: "Loaded File"}
                            file_value := TextBox{text: "Scanning /tmp for mb3d EXR renders..."}
                            status_title := Labelbold{text: "Status"}
                            status_value := TextBox{text: "Waiting for draw pass"}

                            Hr{}
                            tuning_title := Labelbold{text: "Tuning"}
                            reload_latest_button := Button{text: "Reload Latest"}
                            reset_view_button := Button{text: "Reset View"}

                            Hr{}
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
    fn run(vm: &mut ScriptVm) -> Self {
        crate::makepad_widgets::script_mod(vm);
        App::from_script_mod(vm, self::script_mod)
    }

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

    fn queue_latest_render(&mut self, cx: &mut Cx) {
        if let Some(path) = find_latest_render_exr() {
            self.pending_exr_path = Some(path.clone());
            self.set_file_value(cx, &path.display().to_string());
            self.set_status_value(cx, "Queued latest render EXR");
        } else {
            self.pending_exr_path = None;
            self.set_file_value(cx, "No mb3d EXR renders found in /tmp");
            self.set_status_value(cx, "Nothing to load");
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

        match inner.load_exr_path(cx, &path) {
            Ok(summary) => {
                self.active_exr_path = Some(path.clone());
                self.set_file_value(cx, &path.display().to_string());
                self.set_status_value(
                    cx,
                    &format!(
                        "Loaded {}x{} EXR, {} channels, {} texture packs",
                        summary.width,
                        summary.height,
                        summary.channel_count,
                        PACK_SPECS.len()
                    ),
                );
            }
            Err(err) => {
                self.active_exr_path = None;
                self.set_file_value(cx, &path.display().to_string());
                self.set_status_value(cx, &format!("Load failed: {err}"));
            }
        }

        self.ui.redraw(cx);
        cx.redraw_all();
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    pending_exr_path: Option<PathBuf>,
    #[rust]
    active_exr_path: Option<PathBuf>,
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
            .label(cx, ids!(packs))
            .set_text(cx, &format!("GPU bank: {}", pack_schema_summary()));

        let args: Vec<String> = std::env::args().collect();
        if let Some(path) = resolve_startup_exr_path(&args) {
            self.pending_exr_path = Some(path.clone());
            self.set_file_value(cx, &path.display().to_string());
            self.set_status_value(cx, "Queued EXR for load");
        } else {
            self.set_file_value(cx, "No EXR path provided and no /tmp/mb3d*.exr render was found");
            self.set_status_value(cx, "Viewer is idle");
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
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
        if matches!(event, Event::Draw(_)) {
            self.try_load_exr(cx);
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
    zoom: f32,
    #[live]
    pan: Vec2f,
    #[live]
    image_size: Vec2f,
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
    drag_last_abs: Option<DVec2>,
    #[rust]
    textures: Vec<Texture>,
}

#[derive(Debug, Clone, Copy)]
struct LoadedExrSummary {
    width: usize,
    height: usize,
    channel_count: usize,
}

impl ExfViewport {
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
        self.draw_bg.style_mix = value.clamp(0.0, 1.0);
        self.area.redraw(cx);
    }

    fn set_contour_gain(&mut self, cx: &mut Cx, value: f32) {
        self.draw_bg.contour_gain = value.clamp(0.0, 2.0);
        self.area.redraw(cx);
    }

    fn set_glow_gain(&mut self, cx: &mut Cx, value: f32) {
        self.draw_bg.glow_gain = value.clamp(0.0, 2.0);
        self.area.redraw(cx);
    }

    fn reset_view(&mut self, cx: &mut Cx) {
        self.draw_bg.zoom = 1.0;
        self.draw_bg.pan = vec2(0.0, 0.0);
        self.area.redraw(cx);
    }

    fn load_exr_path(&mut self, cx: &mut Cx, path: &Path) -> Result<LoadedExrSummary, String> {
        let image = read_file(path).map_err(|err| err.to_string())?;
        let part = image
            .parts
            .into_iter()
            .max_by_key(|part| part.channels.len())
            .ok_or_else(|| "EXR has no readable parts".to_string())?;
        let summary = load_part_into_texture_bank(part, cx, &mut self.textures)?;
        self.draw_bg.loaded = 1.0;
        self.draw_bg.image_size = vec2(summary.width as f32, summary.height as f32);
        self.bind_textures();
        self.area.redraw(cx);
        Ok(summary)
    }
}

impl Widget for ExfViewport {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        match event.hits_with_capture_overload(cx, self.area, true) {
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                self.drag_last_abs = Some(fe.abs);
                cx.set_cursor(MouseCursor::Grabbing);
            }
            Hit::FingerMove(fe) => {
                if let Some(last_abs) = self.drag_last_abs {
                    let delta = fe.abs - last_abs;
                    let zoom = self.draw_bg.zoom.max(0.001);
                    self.draw_bg.pan.x -= delta.x as f32 / fe.rect.size.x.max(1.0) as f32 / zoom;
                    self.draw_bg.pan.y -= delta.y as f32 / fe.rect.size.y.max(1.0) as f32 / zoom;
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
                self.draw_bg.zoom = (self.draw_bg.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
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
        self.bind_textures();
        let rect = cx.walk_turtle(walk);
        self.draw_bg.draw_abs(cx, rect);
        self.area = self.draw_bg.area();
        DrawStep::done()
    }
}

fn load_part_into_texture_bank(
    part: ExrPart,
    cx: &mut Cx,
    textures: &mut Vec<Texture>,
) -> Result<LoadedExrSummary, String> {
    let width = part.width().map_err(|err| err.to_string())?;
    let height = part.height().map_err(|err| err.to_string())?;
    let channel_count = part.channels.len();
    let pixel_count = width
        .checked_mul(height)
        .ok_or_else(|| "EXR image dimensions overflow".to_string())?;

    let channel_map = collect_float_channels(part.channels, pixel_count)?;
    let planes = build_texture_planes(&channel_map, pixel_count)?;

    *textures = planes
        .into_iter()
        .map(|plane| {
            Texture::new_with_format(
                cx,
                TextureFormat::VecRGBAf32 {
                    width,
                    height,
                    data: Some(plane),
                    updated: TextureUpdated::Full,
                },
            )
        })
        .collect();

    Ok(LoadedExrSummary {
        width,
        height,
        channel_count,
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
    fn fixed_pack_schema_stays_at_ten_rgba_textures() {
        assert_eq!(PACK_SPECS.len(), 10);
        assert_eq!(PACK_SPECS[0].label, "surface");
        assert_eq!(PACK_SPECS[9].label, "uncertainty");
    }
}
